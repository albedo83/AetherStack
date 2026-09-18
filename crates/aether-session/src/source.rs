use std::error::Error;
use std::fmt::{Display, Formatter};
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use aether_fits::{
    HeaderReadOptions, ImageReadError, PrimaryImageReader, Severity, ValidationMode,
};
use aether_metadata::normalize_header;

use crate::{
    ClassificationPolicy, FingerprintError, ManifestFile, ManifestValidationError, classify_frame,
    fingerprint_reader,
};

/// Error raised while analyzing one FITS source for a session manifest.
#[derive(Debug)]
pub enum SourceAnalysisError {
    /// The FITS header or primary-image descriptor could not be read.
    Image(ImageReadError),
    /// Strict mode rejected one or more retained conformance diagnostics.
    HeaderRejected {
        /// Number of error-severity diagnostics in the header report.
        error_count: usize,
    },
    /// The source stream could not be rewound before fingerprinting.
    Rewind(std::io::Error),
    /// Full-source fingerprinting failed.
    Fingerprint(FingerprintError),
    /// The source could not be parsed again after fingerprinting.
    Verification(ImageReadError),
    /// Header or image-layout metadata changed during analysis.
    SourceChanged,
    /// The analyzed values violate a manifest invariant.
    Manifest(ManifestValidationError),
}

impl Display for SourceAnalysisError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Image(error) => write!(formatter, "cannot analyze FITS source: {error}"),
            Self::HeaderRejected { error_count } => write!(
                formatter,
                "strict FITS validation rejected {error_count} error diagnostics"
            ),
            Self::Rewind(error) => write!(formatter, "cannot rewind FITS source: {error}"),
            Self::Fingerprint(error) => Display::fmt(error, formatter),
            Self::Verification(error) => {
                write!(
                    formatter,
                    "cannot verify FITS source after hashing: {error}"
                )
            }
            Self::SourceChanged => {
                formatter.write_str("FITS source changed while it was being analyzed")
            }
            Self::Manifest(error) => Display::fmt(error, formatter),
        }
    }
}

impl Error for SourceAnalysisError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Image(error) => Some(error),
            Self::Rewind(error) => Some(error),
            Self::Fingerprint(error) => Some(error),
            Self::Verification(error) => Some(error),
            Self::Manifest(error) => Some(error),
            Self::HeaderRejected { .. } | Self::SourceChanged => None,
        }
    }
}

/// Analyzes and fingerprints one seekable FITS source without loading pixels.
///
/// The source is parsed for its primary header and image layout, rewound, then
/// read sequentially to compute its immutable fingerprint. The bounded header
/// parse is repeated afterward and must match exactly, detecting observable
/// mutation during analysis. Classification uses only the supplied portable
/// relative path and canonical metadata.
///
/// # Errors
///
/// Returns a typed error for structural FITS failure, strict conformance
/// rejection, rewind or fingerprint I/O failure, or a manifest invariant
/// violation.
pub fn analyze_fits_source<R: Read + Seek>(
    relative_path: impl Into<String>,
    reader: R,
    header_options: HeaderReadOptions,
    fits_validation_mode: ValidationMode,
    classification_policy: ClassificationPolicy,
) -> Result<ManifestFile, SourceAnalysisError> {
    let relative_path = relative_path.into();
    let image_reader =
        PrimaryImageReader::open(reader, header_options).map_err(SourceAnalysisError::Image)?;
    let error_count = image_reader
        .report()
        .diagnostics()
        .iter()
        .filter(|diagnostic| diagnostic.severity() == Severity::Error)
        .count();
    if !image_reader.report().is_accepted(fits_validation_mode) {
        return Err(SourceAnalysisError::HeaderRejected { error_count });
    }

    let original_report = image_reader.report().clone();
    let original_descriptor = image_reader.descriptor().clone();

    let mut reader = image_reader.into_inner();
    reader
        .seek(SeekFrom::Start(0))
        .map_err(SourceAnalysisError::Rewind)?;
    let fingerprint = fingerprint_reader(&mut reader).map_err(SourceAnalysisError::Fingerprint)?;
    reader
        .seek(SeekFrom::Start(0))
        .map_err(SourceAnalysisError::Rewind)?;
    let verified = PrimaryImageReader::open(reader, header_options)
        .map_err(SourceAnalysisError::Verification)?;
    if verified.report() != &original_report || verified.descriptor() != &original_descriptor {
        return Err(SourceAnalysisError::SourceChanged);
    }

    let axes = original_descriptor.axes().to_vec();
    let metadata = normalize_header(original_report.header());
    let diagnostics = original_report.diagnostics().to_vec();
    let classification = classify_frame(Path::new(&relative_path), &metadata);

    ManifestFile::from_analysis(
        relative_path,
        fingerprint,
        axes,
        metadata,
        diagnostics,
        classification,
        classification_policy,
    )
    .map_err(SourceAnalysisError::Manifest)
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Seek};

    use aether_fits::{BLOCK_SIZE, CARD_SIZE, DiagnosticCode};
    use aether_metadata::{CameraModel, CanonicalValue, FrameType};
    use sha2::{Digest, Sha256};

    use super::*;
    use crate::{ManifestValidationCode, SessionManifest};

    struct RewindFails {
        inner: Cursor<Vec<u8>>,
    }

    impl Read for RewindFails {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            self.inner.read(buffer)
        }
    }

    impl Seek for RewindFails {
        fn seek(&mut self, _position: SeekFrom) -> std::io::Result<u64> {
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "rewind denied",
            ))
        }
    }

    struct ReadFailsAfterRewind {
        inner: Cursor<Vec<u8>>,
        rewound: bool,
    }

    enum SecondPassMutation {
        Header,
        Truncate,
    }

    struct MutatesAfterHash {
        inner: Cursor<Vec<u8>>,
        rewind_count: usize,
        mutation: SecondPassMutation,
    }

    impl Read for MutatesAfterHash {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            self.inner.read(buffer)
        }
    }

    impl Seek for MutatesAfterHash {
        fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
            if position == SeekFrom::Start(0) {
                self.rewind_count += 1;
                if self.rewind_count == 2 {
                    match self.mutation {
                        SecondPassMutation::Header => {
                            let instrument_value_start = 5 * CARD_SIZE + 11;
                            if let Some(byte) = self.inner.get_mut().get_mut(instrument_value_start)
                            {
                                *byte = b'X';
                            }
                        }
                        SecondPassMutation::Truncate => self.inner.get_mut().clear(),
                    }
                }
            }
            self.inner.seek(position)
        }
    }

    impl Read for ReadFailsAfterRewind {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            if self.rewound {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "fingerprint read denied",
                ));
            }
            self.inner.read(buffer)
        }
    }

    impl Seek for ReadFailsAfterRewind {
        fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
            let offset = self.inner.seek(position)?;
            self.rewound = true;
            Ok(offset)
        }
    }

    fn fixed_card(keyword: &str, value: &str) -> String {
        format!("{keyword:<8}= {value:>20}")
    }

    fn fits_source(left_align_mandatory_values: bool) -> Vec<u8> {
        let mandatory = |keyword: &str, value: &str| {
            if left_align_mandatory_values {
                format!("{keyword:<8}= {value}")
            } else {
                fixed_card(keyword, value)
            }
        };
        let cards = [
            mandatory("SIMPLE", "T"),
            mandatory("BITPIX", "16"),
            mandatory("NAXIS", "2"),
            mandatory("NAXIS1", "2"),
            mandatory("NAXIS2", "2"),
            "INSTRUME= 'ATR585C'".to_owned(),
            "IMAGETYP= 'Light'".to_owned(),
            "BAYERPAT= 'RGGB'".to_owned(),
            fixed_card("EXPTIME", "12.5"),
            fixed_card("CCD-TEMP", "-7.0"),
            fixed_card("SET-TEMP", "-10.0"),
            fixed_card("GAIN", "42"),
            fixed_card("OFFSET", "7"),
            fixed_card("XBINNING", "1"),
            fixed_card("YBINNING", "1"),
            "FILTER  = 'SYNTHETIC'".to_owned(),
            "END".to_owned(),
        ];
        let mut bytes = vec![b' '; BLOCK_SIZE];
        for (index, card) in cards.iter().enumerate() {
            let start = index * CARD_SIZE;
            let end = start + CARD_SIZE;
            if let Some(destination) = bytes.get_mut(start..end) {
                let source = card.as_bytes();
                let length = source.len().min(CARD_SIZE);
                destination[..length].copy_from_slice(&source[..length]);
            }
        }
        bytes.extend([0_u8; 8]);
        bytes.resize(bytes.len().next_multiple_of(BLOCK_SIZE), 0);
        bytes
    }

    #[test]
    fn analyzes_and_hashes_a_conformant_source() {
        let input = fits_source(false);
        let expected_digest = Sha256::digest(&input);
        let result = analyze_fits_source(
            "lights/frame.fits",
            Cursor::new(input.clone()),
            HeaderReadOptions::default(),
            ValidationMode::Strict,
            ClassificationPolicy::RequireAgreement,
        );
        assert!(result.is_ok());
        let Some(file) = result.ok() else {
            return;
        };
        let mut expected_hex = String::new();
        for byte in expected_digest {
            expected_hex.push_str(&format!("{byte:02x}"));
        }

        assert_eq!(file.fingerprint().byte_length(), input.len() as u64);
        assert_eq!(file.fingerprint().sha256(), expected_hex);
        assert!(file.fits_diagnostics().is_empty());
        assert_eq!(file.axes(), &[2, 2]);
        assert_eq!(
            file.metadata().camera.as_ref().map(CanonicalValue::value),
            Some(&CameraModel::TouptekAtr585C)
        );
        assert_eq!(
            file.resolution().map(|resolution| resolution.frame_type()),
            Some(&FrameType::Light)
        );
    }

    #[test]
    fn strict_mode_rejects_but_tolerant_mode_retains_diagnostics() {
        let input = fits_source(true);
        let strict = analyze_fits_source(
            "lights/frame.fits",
            Cursor::new(input.clone()),
            HeaderReadOptions::default(),
            ValidationMode::Strict,
            ClassificationPolicy::RequireAgreement,
        );
        assert!(matches!(
            strict,
            Err(SourceAnalysisError::HeaderRejected { error_count }) if error_count == 5
        ));

        let tolerant = analyze_fits_source(
            "lights/frame.fits",
            Cursor::new(input),
            HeaderReadOptions::default(),
            ValidationMode::Tolerant,
            ClassificationPolicy::RequireAgreement,
        );
        assert!(tolerant.is_ok());
        let Some(file) = tolerant.ok() else {
            return;
        };
        assert_eq!(file.fits_diagnostics().len(), 5);
        assert!(
            file.fits_diagnostics()
                .iter()
                .all(|diagnostic| { diagnostic.code() == DiagnosticCode::NonStandardFixedValue })
        );

        assert!(matches!(
            SessionManifest::new(
                ClassificationPolicy::RequireAgreement,
                vec![file.clone()],
                Vec::new(),
            ),
            Err(error) if error.code() == ManifestValidationCode::FitsDiagnosticsRejected
        ));
        let tolerant_manifest = SessionManifest::with_validation_mode(
            ValidationMode::Tolerant,
            ClassificationPolicy::RequireAgreement,
            vec![file],
            Vec::new(),
        );
        assert!(tolerant_manifest.is_ok());
        let Some(tolerant_manifest) = tolerant_manifest.ok() else {
            return;
        };
        let encoded = tolerant_manifest.to_json_pretty();
        assert!(encoded.is_ok());
        let Some(encoded) = encoded.ok() else {
            return;
        };
        let decoded = SessionManifest::from_json_slice(&encoded);
        assert!(decoded.is_ok());
        let Some(decoded) = decoded.ok() else {
            return;
        };
        assert_eq!(decoded.fits_validation_mode(), ValidationMode::Tolerant);
        assert_eq!(decoded.files()[0].fits_diagnostics().len(), 5);
    }

    #[test]
    fn reports_structural_rewind_fingerprint_and_manifest_failures() {
        let structural = analyze_fits_source(
            "lights/frame.fits",
            Cursor::new(Vec::<u8>::new()),
            HeaderReadOptions::default(),
            ValidationMode::Strict,
            ClassificationPolicy::RequireAgreement,
        );
        assert!(matches!(structural, Err(SourceAnalysisError::Image(_))));

        let rewind = analyze_fits_source(
            "lights/frame.fits",
            RewindFails {
                inner: Cursor::new(fits_source(false)),
            },
            HeaderReadOptions::default(),
            ValidationMode::Strict,
            ClassificationPolicy::RequireAgreement,
        );
        assert!(matches!(
            rewind,
            Err(SourceAnalysisError::Rewind(error))
                if error.kind() == std::io::ErrorKind::PermissionDenied
        ));

        let fingerprint = analyze_fits_source(
            "lights/frame.fits",
            ReadFailsAfterRewind {
                inner: Cursor::new(fits_source(false)),
                rewound: false,
            },
            HeaderReadOptions::default(),
            ValidationMode::Strict,
            ClassificationPolicy::RequireAgreement,
        );
        assert!(matches!(
            fingerprint,
            Err(SourceAnalysisError::Fingerprint(FingerprintError::Io(error)))
                if error.kind() == std::io::ErrorKind::PermissionDenied
        ));

        let manifest = analyze_fits_source(
            "../frame.fits",
            Cursor::new(fits_source(false)),
            HeaderReadOptions::default(),
            ValidationMode::Strict,
            ClassificationPolicy::RequireAgreement,
        );
        assert!(matches!(
            manifest,
            Err(SourceAnalysisError::Manifest(error))
                if error.code() == ManifestValidationCode::InvalidRelativePath
        ));
    }

    #[test]
    fn detects_source_mutation_during_analysis() {
        let changed_header = analyze_fits_source(
            "lights/frame.fits",
            MutatesAfterHash {
                inner: Cursor::new(fits_source(false)),
                rewind_count: 0,
                mutation: SecondPassMutation::Header,
            },
            HeaderReadOptions::default(),
            ValidationMode::Strict,
            ClassificationPolicy::RequireAgreement,
        );
        assert!(matches!(
            changed_header,
            Err(SourceAnalysisError::SourceChanged)
        ));

        let truncated = analyze_fits_source(
            "lights/frame.fits",
            MutatesAfterHash {
                inner: Cursor::new(fits_source(false)),
                rewind_count: 0,
                mutation: SecondPassMutation::Truncate,
            },
            HeaderReadOptions::default(),
            ValidationMode::Strict,
            ClassificationPolicy::RequireAgreement,
        );
        assert!(matches!(
            truncated,
            Err(SourceAnalysisError::Verification(_))
        ));
    }
}
