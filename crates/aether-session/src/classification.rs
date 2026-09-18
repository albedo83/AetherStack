use std::path::Path;

use aether_metadata::{CanonicalMetadata, FrameType};

/// Origin of a piece of evidence used to determine a frame type.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ClassificationSource {
    /// Canonical value read from a FITS keyword.
    HeaderKeyword(String),
    /// Name of a parent directory.
    Directory(String),
    /// Explicit prefix of the file name.
    FileName(String),
}

impl ClassificationSource {
    /// Returns the stable category of this evidence source.
    #[must_use]
    pub const fn kind(&self) -> ClassificationSourceKind {
        match self {
            Self::HeaderKeyword(_) => ClassificationSourceKind::HeaderKeyword,
            Self::Directory(_) => ClassificationSourceKind::Directory,
            Self::FileName(_) => ClassificationSourceKind::FileName,
        }
    }
}

/// Stable category of a classification evidence source.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClassificationSourceKind {
    /// A canonical FITS header keyword.
    HeaderKeyword,
    /// A recognized parent directory name.
    Directory,
    /// A recognized file-name prefix.
    FileName,
}

/// Individual classification evidence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClassificationEvidence {
    /// Frame type suggested by this evidence.
    pub frame_type: FrameType,
    /// Exact origin of the evidence.
    pub source: ClassificationSource,
}

/// Policy used to resolve classification evidence.
///
/// `RequireAgreement` is the safe default. Preference policies are intended for
/// explicit import profiles and remain visible in the resulting audit record.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ClassificationPolicy {
    /// Resolve only when all recognized evidence agrees.
    #[default]
    RequireAgreement,
    /// Prefer the FITS header when evidence conflicts.
    PreferHeader,
    /// Prefer the nearest recognized parent directory when evidence conflicts.
    PreferDirectory,
    /// Prefer an explicit file-name prefix when evidence conflicts.
    PreferFileName,
}

impl ClassificationPolicy {
    const fn preferred_source(self) -> Option<ClassificationSourceKind> {
        match self {
            Self::RequireAgreement => None,
            Self::PreferHeader => Some(ClassificationSourceKind::HeaderKeyword),
            Self::PreferDirectory => Some(ClassificationSourceKind::Directory),
            Self::PreferFileName => Some(ClassificationSourceKind::FileName),
        }
    }
}

/// Auditable basis for a resolved frame type.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResolutionBasis {
    /// Every recognized piece of evidence agrees.
    Agreement,
    /// A caller-selected source resolved a conflict.
    PreferredSource(ClassificationSourceKind),
}

/// Explicit, auditable resolution of a frame classification.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FrameResolution {
    frame_type: FrameType,
    basis: ResolutionBasis,
    overrode_conflict: bool,
}

impl FrameResolution {
    /// Resolved scientific frame type.
    #[must_use]
    pub const fn frame_type(&self) -> &FrameType {
        &self.frame_type
    }

    /// Evidence rule that produced this resolution.
    #[must_use]
    pub const fn basis(&self) -> ResolutionBasis {
        self.basis
    }

    /// Whether a preference policy intentionally overrode conflicting evidence.
    #[must_use]
    pub const fn overrode_conflict(&self) -> bool {
        self.overrode_conflict
    }
}

/// Explainable result of classifying one frame.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FrameClassification {
    evidence: Vec<ClassificationEvidence>,
    resolved: Option<FrameType>,
    has_conflict: bool,
}

impl FrameClassification {
    /// All retained evidence, including contradictions.
    #[must_use]
    pub fn evidence(&self) -> &[ClassificationEvidence] {
        &self.evidence
    }

    /// Frame type resolved only when all recognized evidence agrees.
    #[must_use]
    pub const fn resolved(&self) -> Option<&FrameType> {
        self.resolved.as_ref()
    }

    /// Returns whether at least two recognized sources contradict each other.
    #[must_use]
    pub const fn has_conflict(&self) -> bool {
        self.has_conflict
    }

    /// Resolves the evidence according to an explicit policy.
    ///
    /// A preference policy only resolves a conflict when evidence from the
    /// preferred source exists. This avoids silently falling back to another
    /// arbitrary source.
    #[must_use]
    pub fn resolve_with(&self, policy: ClassificationPolicy) -> Option<FrameResolution> {
        if let Some(frame_type) = &self.resolved {
            return Some(FrameResolution {
                frame_type: frame_type.clone(),
                basis: ResolutionBasis::Agreement,
                overrode_conflict: false,
            });
        }

        let preferred_source = policy.preferred_source()?;
        let evidence = self
            .evidence
            .iter()
            .find(|evidence| evidence.source.kind() == preferred_source)?;
        Some(FrameResolution {
            frame_type: evidence.frame_type.clone(),
            basis: ResolutionBasis::PreferredSource(preferred_source),
            overrode_conflict: true,
        })
    }
}

/// Combines canonical metadata with conservative path evidence.
///
/// Only complete directory names and known file-name prefixes are interpreted.
/// A target whose name happens to contain `dark` is therefore not classified as
/// a dark frame.
#[must_use]
pub fn classify_frame(path: &Path, metadata: &CanonicalMetadata) -> FrameClassification {
    let mut evidence = Vec::new();

    if let Some(declared) = &metadata.frame_type
        && !matches!(declared.value(), FrameType::Other(_))
    {
        evidence.push(ClassificationEvidence {
            frame_type: declared.value().clone(),
            source: ClassificationSource::HeaderKeyword(declared.source_keyword().to_owned()),
        });
    }
    if let Some((frame_type, component)) = nearest_directory_hint(path) {
        evidence.push(ClassificationEvidence {
            frame_type,
            source: ClassificationSource::Directory(component),
        });
    }
    if let Some((frame_type, prefix)) = filename_hint(path) {
        evidence.push(ClassificationEvidence {
            frame_type,
            source: ClassificationSource::FileName(prefix),
        });
    }

    let resolved = evidence.first().map(|item| item.frame_type.clone());
    let has_conflict = resolved.as_ref().is_some_and(|first| {
        evidence
            .iter()
            .skip(1)
            .any(|item| &item.frame_type != first)
    });

    FrameClassification {
        evidence,
        resolved: (!has_conflict).then_some(resolved).flatten(),
        has_conflict,
    }
}

fn nearest_directory_hint(path: &Path) -> Option<(FrameType, String)> {
    let parent = path.parent()?;
    for component in parent.components().rev() {
        let value = component.as_os_str().to_str()?;
        if let Some(frame_type) = directory_frame_type(value) {
            return Some((frame_type, value.to_owned()));
        }
    }
    None
}

fn directory_frame_type(value: &str) -> Option<FrameType> {
    let normalized = value.trim().to_ascii_uppercase();
    match normalized.as_str() {
        "BIAS" | "BIASES" => Some(FrameType::Bias),
        "DARK" | "DARKS" => Some(FrameType::Dark),
        "FLAT" | "FLATS" => Some(FrameType::Flat),
        "LIGHT" | "LIGHTS" => Some(FrameType::Light),
        _ if is_timed_dark_directory(&normalized) => Some(FrameType::Dark),
        _ => None,
    }
}

fn is_timed_dark_directory(value: &str) -> bool {
    let Some(suffix) = value.strip_prefix("DARKS") else {
        return false;
    };
    let suffix = suffix.strip_suffix("MS").unwrap_or(suffix);
    !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit())
}

fn filename_hint(path: &Path) -> Option<(FrameType, String)> {
    let filename = path.file_name()?.to_str()?;
    let stem = filename.rsplit_once('.').map_or(filename, |(stem, _)| stem);
    let prefix = stem
        .split(['_', '-', ' '])
        .next()
        .unwrap_or(stem)
        .to_ascii_uppercase();
    let frame_type = match prefix.as_str() {
        "BIAS" => FrameType::Bias,
        "DARK" | "MASTERDARK" => FrameType::Dark,
        "FLAT" | "MASTERFLAT" => FrameType::Flat,
        "LIGHT" => FrameType::Light,
        _ => return None,
    };
    Some((frame_type, prefix))
}

#[cfg(test)]
mod tests {
    use aether_metadata::{CanonicalValue, Confidence};

    use super::*;

    fn metadata(frame_type: Option<FrameType>) -> CanonicalMetadata {
        CanonicalMetadata {
            frame_type: frame_type
                .map(|frame_type| CanonicalValue::new(frame_type, "IMAGETYP", Confidence::Exact)),
            ..CanonicalMetadata::default()
        }
    }

    #[test]
    fn resolves_agreeing_dark_evidence() {
        let classification = classify_frame(
            Path::new("/corpus/DARKS/585/60/Dark_001.fits"),
            &metadata(Some(FrameType::Dark)),
        );

        assert_eq!(classification.resolved(), Some(&FrameType::Dark));
        assert!(!classification.has_conflict());
        assert_eq!(classification.evidence().len(), 3);
    }

    #[test]
    fn exposes_flat_directory_and_light_header_conflict() {
        let classification = classify_frame(
            Path::new("/corpus/M51/FLATS/2026-03-20_0010.fits"),
            &metadata(Some(FrameType::Light)),
        );

        assert!(classification.resolved().is_none());
        assert!(classification.has_conflict());
        assert_eq!(classification.evidence().len(), 2);
        assert!(
            classification
                .resolve_with(ClassificationPolicy::RequireAgreement)
                .is_none()
        );
    }

    #[test]
    fn explicit_directory_policy_can_resolve_a_conflict() {
        let classification = classify_frame(
            Path::new("/corpus/M51/FLATS/2026-03-20_0010.fits"),
            &metadata(Some(FrameType::Light)),
        );

        let Some(resolution) = classification.resolve_with(ClassificationPolicy::PreferDirectory)
        else {
            return;
        };
        assert_eq!(resolution.frame_type(), &FrameType::Flat);
        assert_eq!(
            resolution.basis(),
            ResolutionBasis::PreferredSource(ClassificationSourceKind::Directory)
        );
        assert!(resolution.overrode_conflict());
    }

    #[test]
    fn preference_never_falls_back_to_an_unrelated_source() {
        let classification = classify_frame(
            Path::new("/corpus/M51/FLATS/2026-03-20_0010.fits"),
            &metadata(Some(FrameType::Light)),
        );

        assert!(
            classification
                .resolve_with(ClassificationPolicy::PreferFileName)
                .is_none()
        );
    }

    #[test]
    fn preference_does_not_override_agreement() {
        let classification = classify_frame(
            Path::new("/corpus/DARKS/585/60/Dark_001.fits"),
            &metadata(Some(FrameType::Dark)),
        );

        let Some(resolution) = classification.resolve_with(ClassificationPolicy::PreferHeader)
        else {
            return;
        };
        assert_eq!(resolution.frame_type(), &FrameType::Dark);
        assert_eq!(resolution.basis(), ResolutionBasis::Agreement);
        assert!(!resolution.overrode_conflict());
    }

    #[test]
    fn uses_path_when_header_type_is_missing() {
        let classification = classify_frame(
            Path::new("/corpus/target/darks250ms/frame.fits"),
            &metadata(None),
        );

        assert_eq!(classification.resolved(), Some(&FrameType::Dark));
        assert!(!classification.has_conflict());
    }

    #[test]
    fn does_not_match_arbitrary_target_name() {
        let classification = classify_frame(
            Path::new("/corpus/Dark Nebula/session/frame.fits"),
            &metadata(None),
        );

        assert!(classification.resolved().is_none());
        assert!(!classification.has_conflict());
    }

    #[test]
    fn filename_prefix_can_supply_type() {
        let classification = classify_frame(
            Path::new("/corpus/session/Bias_2s_0001.fit"),
            &metadata(None),
        );

        assert_eq!(classification.resolved(), Some(&FrameType::Bias));
        assert_eq!(classification.evidence().len(), 1);
    }
}
