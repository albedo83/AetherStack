use aether_review::FrameId;
use sha2::{Digest, Sha256};

use crate::{
    COMMON_LANCZOS3_FOOTPRINT_ALGORITHM_ID, CommonFootprintError, CommonFootprintReport,
    MAX_COMMON_FOOTPRINT_FRAMES, ProjectiveRegistrationFootprint, ProjectiveTransform,
    RegistrationPlanError, derive_common_lanczos3_projective_footprint,
};

/// Stable identifier for immutable projective multi-frame geometry.
pub const PROJECTIVE_REGISTRATION_PLAN_ALGORITHM_ID: &str = "registration-projective-plan-v1";
const PROJECTIVE_PLAN_DIGEST_DOMAIN: &[u8] = b"aether-registration-projective-plan-v1\0";

/// One reviewed Light and its projective transform into reference coordinates.
#[derive(Clone, Debug, PartialEq)]
pub struct ProjectivePlannedRegistrationFrame {
    frame_id: FrameId,
    source_width: usize,
    source_height: usize,
    source_to_reference: ProjectiveTransform,
}

impl ProjectivePlannedRegistrationFrame {
    /// Creates one immutable projective plan input.
    #[must_use]
    pub const fn new(
        frame_id: FrameId,
        source_width: usize,
        source_height: usize,
        source_to_reference: ProjectiveTransform,
    ) -> Self {
        Self {
            frame_id,
            source_width,
            source_height,
            source_to_reference,
        }
    }

    /// Stable reviewed-frame identity.
    #[must_use]
    pub const fn frame_id(&self) -> &FrameId {
        &self.frame_id
    }

    /// Source width in pixels.
    #[must_use]
    pub const fn source_width(&self) -> usize {
        self.source_width
    }

    /// Source height in pixels.
    #[must_use]
    pub const fn source_height(&self) -> usize {
        self.source_height
    }

    /// Accepted source-to-reference homography in physical source pixels.
    #[must_use]
    pub const fn source_to_reference(&self) -> ProjectiveTransform {
        self.source_to_reference
    }
}

/// Canonical projective multi-Light geometry and exact common crop.
#[derive(Clone, Debug, PartialEq)]
pub struct ProjectiveRegistrationPlan {
    reference_frame_id: FrameId,
    reference_width: usize,
    reference_height: usize,
    frames: Vec<ProjectivePlannedRegistrationFrame>,
    common_footprint: CommonFootprintReport,
    plan_sha256: String,
}

impl ProjectiveRegistrationPlan {
    /// Validates and canonicalizes a complete reviewed projective set.
    ///
    /// Frames are sorted by stable identity. The reference must occur exactly
    /// once with declared dimensions and the canonical identity homography.
    /// Every coefficient and the derived exact crop enter the portable digest.
    pub fn new(
        reference_frame_id: FrameId,
        reference_width: usize,
        reference_height: usize,
        mut frames: Vec<ProjectivePlannedRegistrationFrame>,
    ) -> Result<Self, RegistrationPlanError> {
        if frames.len() < 2 {
            return Err(RegistrationPlanError::NotEnoughFrames {
                actual: frames.len(),
            });
        }
        if frames.len() > MAX_COMMON_FOOTPRINT_FRAMES {
            return Err(RegistrationPlanError::TooManyFrames {
                maximum: MAX_COMMON_FOOTPRINT_FRAMES,
                actual: frames.len(),
            });
        }
        frames.sort_by(|left, right| left.frame_id.cmp(&right.frame_id));
        if let Some(duplicate) = frames
            .windows(2)
            .find(|pair| pair[0].frame_id == pair[1].frame_id)
        {
            return Err(RegistrationPlanError::DuplicateFrame(
                duplicate[0].frame_id.clone(),
            ));
        }

        let reference = frames
            .iter()
            .find(|frame| frame.frame_id == reference_frame_id)
            .ok_or(RegistrationPlanError::ReferenceMissing)?;
        if reference.source_width != reference_width
            || reference.source_height != reference_height
            || reference.source_to_reference != ProjectiveTransform::IDENTITY
        {
            return Err(RegistrationPlanError::ReferenceGeometryMismatch);
        }

        let mut footprints = Vec::new();
        footprints.try_reserve_exact(frames.len()).map_err(|_| {
            RegistrationPlanError::Footprint(CommonFootprintError::AllocationFailed)
        })?;
        for frame in &frames {
            footprints.push(
                ProjectiveRegistrationFootprint::new(
                    frame.source_width,
                    frame.source_height,
                    frame.source_to_reference,
                )
                .map_err(RegistrationPlanError::Footprint)?,
            );
        }
        let common_footprint = derive_common_lanczos3_projective_footprint(
            reference_width,
            reference_height,
            &footprints,
        )
        .map_err(RegistrationPlanError::Footprint)?;
        if common_footprint.crop().is_none() {
            return Err(RegistrationPlanError::NoCommonCrop);
        }
        let plan_sha256 = projective_plan_sha256(
            &reference_frame_id,
            reference_width,
            reference_height,
            &frames,
            common_footprint,
        )?;

        Ok(Self {
            reference_frame_id,
            reference_width,
            reference_height,
            frames,
            common_footprint,
            plan_sha256,
        })
    }

    /// Versioned projective plan construction policy.
    #[must_use]
    pub const fn algorithm_id(&self) -> &'static str {
        PROJECTIVE_REGISTRATION_PLAN_ALGORITHM_ID
    }

    /// Stable identity of the frame defining output coordinates.
    #[must_use]
    pub const fn reference_frame_id(&self) -> &FrameId {
        &self.reference_frame_id
    }

    /// Output reference width in physical pixels.
    #[must_use]
    pub const fn reference_width(&self) -> usize {
        self.reference_width
    }

    /// Output reference height in physical pixels.
    #[must_use]
    pub const fn reference_height(&self) -> usize {
        self.reference_height
    }

    /// Canonically identity-sorted reviewed Lights.
    #[must_use]
    pub fn frames(&self) -> &[ProjectivePlannedRegistrationFrame] {
        &self.frames
    }

    /// Exact all-frame projective Lanczos support and largest valid rectangle.
    #[must_use]
    pub const fn common_footprint(&self) -> CommonFootprintReport {
        self.common_footprint
    }

    /// Portable digest over identities, geometry, dimensions, and crop.
    #[must_use]
    pub fn plan_sha256(&self) -> &str {
        &self.plan_sha256
    }
}

fn projective_plan_sha256(
    reference_frame_id: &FrameId,
    reference_width: usize,
    reference_height: usize,
    frames: &[ProjectivePlannedRegistrationFrame],
    common_footprint: CommonFootprintReport,
) -> Result<String, RegistrationPlanError> {
    let mut hasher = Sha256::new();
    hasher.update(PROJECTIVE_PLAN_DIGEST_DOMAIN);
    update_string(&mut hasher, PROJECTIVE_REGISTRATION_PLAN_ALGORITHM_ID)?;
    update_string(&mut hasher, COMMON_LANCZOS3_FOOTPRINT_ALGORITHM_ID)?;
    update_string(&mut hasher, reference_frame_id.as_str())?;
    update_usize(&mut hasher, reference_width)?;
    update_usize(&mut hasher, reference_height)?;
    update_usize(&mut hasher, frames.len())?;
    for frame in frames {
        update_string(&mut hasher, frame.frame_id.as_str())?;
        update_usize(&mut hasher, frame.source_width)?;
        update_usize(&mut hasher, frame.source_height)?;
        for coefficient in frame.source_to_reference.coefficients().iter().flatten() {
            hasher.update(coefficient.to_bits().to_be_bytes());
        }
    }
    update_usize(&mut hasher, common_footprint.covered_pixels())?;
    let crop = common_footprint
        .crop()
        .ok_or(RegistrationPlanError::NoCommonCrop)?;
    update_usize(&mut hasher, crop.x())?;
    update_usize(&mut hasher, crop.y())?;
    update_usize(&mut hasher, crop.width())?;
    update_usize(&mut hasher, crop.height())?;
    Ok(lower_hex(&hasher.finalize()))
}

fn update_string(hasher: &mut Sha256, value: &str) -> Result<(), RegistrationPlanError> {
    update_usize(hasher, value.len())?;
    hasher.update(value.as_bytes());
    Ok(())
}

fn update_usize(hasher: &mut Sha256, value: usize) -> Result<(), RegistrationPlanError> {
    let portable = u64::try_from(value).map_err(|_| RegistrationPlanError::EncodingOverflow)?;
    hasher.update(portable.to_be_bytes());
    Ok(())
}

fn lower_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    output
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use super::*;

    type TestResult = Result<(), Box<dyn Error>>;

    fn id(byte: u8) -> Result<FrameId, aether_review::ReviewError> {
        FrameId::new(format!("{byte:02x}").repeat(32))
    }

    fn frame(
        byte: u8,
        transform: ProjectiveTransform,
    ) -> Result<ProjectivePlannedRegistrationFrame, Box<dyn Error>> {
        Ok(ProjectivePlannedRegistrationFrame::new(
            id(byte)?,
            31,
            29,
            transform,
        ))
    }

    #[test]
    fn canonicalizes_order_and_binds_projective_geometry() -> TestResult {
        let transform = ProjectiveTransform::new([
            [0.999, -0.012, 0.37],
            [0.012, 0.999, -0.28],
            [8.0e-5, -5.0e-5, 1.0],
        ])?;
        let forward = ProjectiveRegistrationPlan::new(
            id(1)?,
            31,
            29,
            vec![
                frame(2, transform)?,
                frame(1, ProjectiveTransform::IDENTITY)?,
            ],
        )?;
        let reverse = ProjectiveRegistrationPlan::new(
            id(1)?,
            31,
            29,
            vec![
                frame(1, ProjectiveTransform::IDENTITY)?,
                frame(2, transform)?,
            ],
        )?;

        assert_eq!(forward, reverse);
        assert_eq!(
            forward.algorithm_id(),
            PROJECTIVE_REGISTRATION_PLAN_ALGORITHM_ID
        );
        assert_eq!(forward.reference_frame_id(), &id(1)?);
        assert_eq!(
            (forward.reference_width(), forward.reference_height()),
            (31, 29)
        );
        assert_eq!(forward.frames()[0].frame_id(), &id(1)?);
        assert_eq!(forward.frames()[1].source_to_reference(), transform);
        assert_eq!(forward.plan_sha256().len(), 64);
        assert!(forward.common_footprint().crop().is_some());
        Ok(())
    }

    #[test]
    fn digest_changes_with_one_perspective_coefficient() -> TestResult {
        let first =
            ProjectiveTransform::new([[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [1.0e-5, 0.0, 1.0]])?;
        let second =
            ProjectiveTransform::new([[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [2.0e-5, 0.0, 1.0]])?;
        let make = |transform| {
            ProjectiveRegistrationPlan::new(
                id(1)?,
                31,
                29,
                vec![
                    frame(1, ProjectiveTransform::IDENTITY)?,
                    frame(2, transform)?,
                ],
            )
            .map_err(Box::<dyn Error>::from)
        };

        assert_ne!(make(first)?.plan_sha256(), make(second)?.plan_sha256());
        Ok(())
    }

    #[test]
    fn rejects_reference_mismatch_duplicates_and_disjoint_support() -> TestResult {
        assert!(matches!(
            ProjectiveRegistrationPlan::new(
                id(1)?,
                31,
                29,
                vec![
                    frame(1, ProjectiveTransform::IDENTITY)?,
                    frame(1, ProjectiveTransform::IDENTITY)?
                ],
            ),
            Err(RegistrationPlanError::DuplicateFrame(_))
        ));
        assert!(matches!(
            ProjectiveRegistrationPlan::new(
                id(1)?,
                31,
                29,
                vec![
                    frame(
                        1,
                        ProjectiveTransform::from_affine(crate::AffineTransform::new(
                            1.0, 0.0, 0.0, 1.0, 1.0, 0.0
                        )?)?
                    )?,
                    frame(2, ProjectiveTransform::IDENTITY)?
                ],
            ),
            Err(RegistrationPlanError::ReferenceGeometryMismatch)
        ));
        let disjoint = ProjectiveTransform::from_affine(crate::AffineTransform::new(
            1.0, 0.0, 0.0, 1.0, 100.0, 0.0,
        )?)?;
        assert!(matches!(
            ProjectiveRegistrationPlan::new(
                id(1)?,
                31,
                29,
                vec![
                    frame(1, ProjectiveTransform::IDENTITY)?,
                    frame(2, disjoint)?
                ],
            ),
            Err(RegistrationPlanError::NoCommonCrop)
        ));
        Ok(())
    }
}
