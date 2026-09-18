use std::collections::HashMap;
use std::error::Error;
use std::fmt::{Display, Formatter};

use aether_fits::ValidationMode;
use aether_metadata::{BayerPattern, CameraModel, FrameType};
use sha2::{Digest, Sha256};

use crate::{
    ClassificationPolicy, GroupingKeyError, ManifestFile, ManifestGroup, ManifestValidationError,
    SessionManifest, StrictGroupingKey,
};

/// Error raised while constructing a grouped session manifest.
#[derive(Debug)]
pub enum ManifestGenerationError {
    /// Canonical metadata could not form an exact grouping key.
    Grouping(GroupingKeyError),
    /// Generated content violates a manifest invariant.
    Manifest(ManifestValidationError),
}

impl Display for ManifestGenerationError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Grouping(error) => write!(formatter, "cannot group manifest source: {error}"),
            Self::Manifest(error) => Display::fmt(error, formatter),
        }
    }
}

impl Error for ManifestGenerationError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Grouping(error) => Some(error),
            Self::Manifest(error) => Some(error),
        }
    }
}

/// Generates deterministic exact groups from analyzed FITS source records.
///
/// A source is grouped automatically only when its frame classification is
/// resolved and its [`StrictGroupingKey`] has no missing fields. Unresolved or
/// incomplete sources remain in [`SessionManifest::files`] but are absent from
/// every group. Callers may later construct an explicit [`ManifestGroup`] with
/// accepted missing fields and a rationale.
///
/// Group identifiers are the lowercase SHA-256 of a versioned, unambiguous
/// binary encoding of the exact key. The manifest constructor then canonicalizes
/// file and group order, so filesystem discovery order does not affect output.
///
/// # Errors
///
/// Returns a typed error when finite grouping values cannot be constructed or
/// generated records violate the manifest schema.
pub fn generate_manifest(
    fits_validation_mode: ValidationMode,
    classification_policy: ClassificationPolicy,
    files: Vec<ManifestFile>,
) -> Result<SessionManifest, ManifestGenerationError> {
    let mut grouped_paths: HashMap<StrictGroupingKey, Vec<String>> = HashMap::new();

    for file in &files {
        let Some(resolution) = file.resolution() else {
            continue;
        };
        let key = StrictGroupingKey::from_metadata(
            resolution.frame_type().clone(),
            file.metadata(),
            file.axes(),
        )
        .map_err(ManifestGenerationError::Grouping)?;
        if !key.missing_fields().is_empty() {
            continue;
        }
        grouped_paths
            .entry(key)
            .or_default()
            .push(file.relative_path().to_owned());
    }

    let mut groups = Vec::new();
    for (key, paths) in grouped_paths {
        let id = stable_group_id(&key);
        let group = ManifestGroup::new(id, key, paths, Vec::new(), None)
            .map_err(ManifestGenerationError::Manifest)?;
        groups.push(group);
    }

    SessionManifest::with_validation_mode(
        fits_validation_mode,
        classification_policy,
        files,
        groups,
    )
    .map_err(ManifestGenerationError::Manifest)
}

fn stable_group_id(key: &StrictGroupingKey) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"aetherstack-grouping-key-v1\0");
    hash_frame_type(&mut hasher, key.frame_type());
    hash_optional_camera(&mut hasher, key.camera());
    hasher.update([key.axes().len() as u8]);
    for axis in key.axes() {
        hasher.update(axis.to_be_bytes());
    }
    hash_optional_f64(&mut hasher, key.exposure_seconds());
    hash_optional_f64(&mut hasher, key.sensor_temperature_c());
    hash_optional_f64(&mut hasher, key.set_temperature_c());
    hash_optional_f64(&mut hasher, key.gain());
    hash_optional_f64(&mut hasher, key.offset());
    match key.binning() {
        Some(binning) => {
            hasher.update([1]);
            hasher.update(binning.x.to_be_bytes());
            hasher.update(binning.y.to_be_bytes());
        }
        None => hasher.update([0]),
    }
    hash_optional_string(&mut hasher, key.filter());
    hash_optional_bayer(&mut hasher, key.bayer_pattern());
    crate::fingerprint::encode_lower_hex(&hasher.finalize())
}

fn hash_frame_type(hasher: &mut Sha256, frame_type: &FrameType) {
    match frame_type {
        FrameType::Bias => hasher.update([0]),
        FrameType::Dark => hasher.update([1]),
        FrameType::Flat => hasher.update([2]),
        FrameType::Light => hasher.update([3]),
        FrameType::Other(value) => {
            hasher.update([4]);
            hash_string(hasher, value);
        }
    }
}

fn hash_optional_camera(hasher: &mut Sha256, camera: Option<&CameraModel>) {
    match camera {
        None => hasher.update([0]),
        Some(CameraModel::ZwoAsi294McPro) => hasher.update([1, 0]),
        Some(CameraModel::TouptekAtr585C) => hasher.update([1, 1]),
        Some(CameraModel::Other(value)) => {
            hasher.update([1, 2]);
            hash_string(hasher, value);
        }
    }
}

fn hash_optional_f64(hasher: &mut Sha256, value: Option<f64>) {
    match value {
        Some(value) => {
            hasher.update([1]);
            hasher.update(value.to_bits().to_be_bytes());
        }
        None => hasher.update([0]),
    }
}

fn hash_optional_string(hasher: &mut Sha256, value: Option<&str>) {
    match value {
        Some(value) => {
            hasher.update([1]);
            hash_string(hasher, value);
        }
        None => hasher.update([0]),
    }
}

fn hash_optional_bayer(hasher: &mut Sha256, pattern: Option<&BayerPattern>) {
    match pattern {
        None => hasher.update([0]),
        Some(BayerPattern::Rggb) => hasher.update([1, 0]),
        Some(BayerPattern::Bggr) => hasher.update([1, 1]),
        Some(BayerPattern::Grbg) => hasher.update([1, 2]),
        Some(BayerPattern::Gbrg) => hasher.update([1, 3]),
        Some(BayerPattern::Other(value)) => {
            hasher.update([1, 4]);
            hash_string(hasher, value);
        }
    }
}

fn hash_string(hasher: &mut Sha256, value: &str) {
    hasher.update((value.len() as u64).to_be_bytes());
    hasher.update(value.as_bytes());
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;
    use std::path::Path;

    use aether_metadata::{
        BayerPattern, Binning, CameraModel, CanonicalMetadata, CanonicalValue, Confidence,
        FrameType,
    };

    use super::*;
    use crate::{SourceFingerprint, classify_frame};

    type TestResult<T = ()> = Result<T, Box<dyn StdError>>;

    fn value<T>(value: T, keyword: &str) -> CanonicalValue<T> {
        CanonicalValue::new(value, keyword, Confidence::Exact)
    }

    fn metadata(frame_type: FrameType, complete: bool) -> CanonicalMetadata {
        CanonicalMetadata {
            camera: complete.then(|| value(CameraModel::TouptekAtr585C, "INSTRUME")),
            frame_type: Some(value(frame_type, "IMAGETYP")),
            exposure_seconds: complete.then(|| value(60.0, "EXPTIME")),
            sensor_temperature_c: complete.then(|| value(-10.0, "CCD-TEMP")),
            set_temperature_c: complete.then(|| value(-10.0, "SET-TEMP")),
            gain: complete.then(|| value(120.0, "GAIN")),
            offset: complete.then(|| value(512.0, "OFFSET")),
            binning: complete.then(|| value(Binning { x: 1, y: 1 }, "XBINNING+YBINNING")),
            filter: complete.then(|| value("SYNTHETIC".to_owned(), "FILTER")),
            bayer_pattern: complete.then(|| value(BayerPattern::Rggb, "BAYERPAT")),
            issues: Vec::new(),
        }
    }

    fn file(path: &str, digest_digit: char, complete: bool) -> TestResult<ManifestFile> {
        let metadata = metadata(FrameType::Light, complete);
        let classification = classify_frame(Path::new(path), &metadata);
        Ok(ManifestFile::from_analysis(
            path,
            SourceFingerprint::new(5_760, digest_digit.to_string().repeat(64))?,
            vec![4, 3],
            metadata,
            Vec::new(),
            classification,
            ClassificationPolicy::RequireAgreement,
        )?)
    }

    #[test]
    fn groups_complete_equal_keys_and_leaves_incomplete_sources_unassigned() -> TestResult {
        let first = file("lights/frame_0002.fits", 'b', true)?;
        let second = file("lights/frame_0001.fits", 'a', true)?;
        let incomplete = file("lights/frame_0003.fits", 'c', false)?;

        let manifest = generate_manifest(
            ValidationMode::Strict,
            ClassificationPolicy::RequireAgreement,
            vec![first, incomplete, second],
        )?;

        assert_eq!(manifest.files().len(), 3);
        assert_eq!(manifest.groups().len(), 1);
        assert_eq!(manifest.groups()[0].id().len(), 64);
        assert_eq!(
            manifest.groups()[0].id(),
            "01b9785cbd3c4b867951adb3ccf5d892225cbfc8782079a915e9ff5df7385997"
        );
        assert_eq!(
            manifest.groups()[0].files(),
            &["lights/frame_0001.fits", "lights/frame_0002.fits"]
        );
        assert!(
            manifest.groups()[0]
                .files()
                .iter()
                .all(|path| path != "lights/frame_0003.fits")
        );
        Ok(())
    }

    #[test]
    fn discovery_order_does_not_change_manifest_json() -> TestResult {
        let first = file("lights/frame_0001.fits", 'a', true)?;
        let second = file("lights/frame_0002.fits", 'b', true)?;
        let forward = generate_manifest(
            ValidationMode::Strict,
            ClassificationPolicy::RequireAgreement,
            vec![first.clone(), second.clone()],
        )?;
        let reverse = generate_manifest(
            ValidationMode::Strict,
            ClassificationPolicy::RequireAgreement,
            vec![second, first],
        )?;

        assert_eq!(forward.to_json_pretty()?, reverse.to_json_pretty()?);
        Ok(())
    }

    #[test]
    fn duplicate_sources_remain_a_manifest_error() -> TestResult {
        let source = file("lights/frame.fits", 'a', true)?;
        assert!(matches!(
            generate_manifest(
                ValidationMode::Strict,
                ClassificationPolicy::RequireAgreement,
                vec![source.clone(), source],
            ),
            Err(ManifestGenerationError::Manifest(error))
                if error.code() == crate::ManifestValidationCode::DuplicateFilePath
        ));
        Ok(())
    }
}
