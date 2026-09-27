use std::error::Error;
use std::fmt::{Display, Formatter};

use aether_review::FrameId;

use crate::{
    AffineTransform, CommonFootprintError, CommonFootprintReport, MAX_COMMON_FOOTPRINT_FRAMES,
    RegistrationFootprint, derive_common_lanczos3_footprint,
};

/// Stable identifier for the geometry-only multi-frame plan contract.
pub const REGISTRATION_PLAN_ALGORITHM_ID: &str = "registration-plan-v1";

/// One reviewed Light and its accepted transform into reference coordinates.
#[derive(Clone, Debug, PartialEq)]
pub struct PlannedRegistrationFrame {
    frame_id: FrameId,
    source_width: usize,
    source_height: usize,
    source_to_reference: AffineTransform,
}

impl PlannedRegistrationFrame {
    /// Creates one immutable plan input.
    ///
    /// Dimension and transform validity is rechecked when the complete plan is
    /// built, so a frame cannot bypass the common-footprint contract.
    #[must_use]
    pub const fn new(
        frame_id: FrameId,
        source_width: usize,
        source_height: usize,
        source_to_reference: AffineTransform,
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

    /// Accepted transform mapping this source into reference coordinates.
    #[must_use]
    pub const fn source_to_reference(&self) -> AffineTransform {
        self.source_to_reference
    }
}

/// Canonical multi-Light registration geometry and its exact common crop.
#[derive(Clone, Debug, PartialEq)]
pub struct RegistrationPlan {
    reference_frame_id: FrameId,
    reference_width: usize,
    reference_height: usize,
    frames: Vec<PlannedRegistrationFrame>,
    common_footprint: CommonFootprintReport,
}

impl RegistrationPlan {
    /// Validates and canonicalizes a complete accepted registration set.
    ///
    /// Frames are sorted by stable identity so discovery order cannot change
    /// the plan. The declared reference must occur exactly once, have the
    /// declared canvas dimensions, and use the exact identity transform. A
    /// plan without a non-empty all-frame Lanczos-3 crop is rejected.
    pub fn new(
        reference_frame_id: FrameId,
        reference_width: usize,
        reference_height: usize,
        mut frames: Vec<PlannedRegistrationFrame>,
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
            || reference.source_to_reference != AffineTransform::IDENTITY
        {
            return Err(RegistrationPlanError::ReferenceGeometryMismatch);
        }

        let mut footprints = Vec::new();
        footprints.try_reserve_exact(frames.len()).map_err(|_| {
            RegistrationPlanError::Footprint(CommonFootprintError::AllocationFailed)
        })?;
        for frame in &frames {
            footprints.push(
                RegistrationFootprint::new(
                    frame.source_width,
                    frame.source_height,
                    frame.source_to_reference,
                )
                .map_err(RegistrationPlanError::Footprint)?,
            );
        }
        let common_footprint =
            derive_common_lanczos3_footprint(reference_width, reference_height, &footprints)
                .map_err(RegistrationPlanError::Footprint)?;
        if common_footprint.crop().is_none() {
            return Err(RegistrationPlanError::NoCommonCrop);
        }

        Ok(Self {
            reference_frame_id,
            reference_width,
            reference_height,
            frames,
            common_footprint,
        })
    }

    /// Versioned plan construction policy.
    #[must_use]
    pub const fn algorithm_id(&self) -> &'static str {
        REGISTRATION_PLAN_ALGORITHM_ID
    }

    /// Stable identity of the frame defining output coordinates.
    #[must_use]
    pub const fn reference_frame_id(&self) -> &FrameId {
        &self.reference_frame_id
    }

    /// Output reference width in pixels.
    #[must_use]
    pub const fn reference_width(&self) -> usize {
        self.reference_width
    }

    /// Output reference height in pixels.
    #[must_use]
    pub const fn reference_height(&self) -> usize {
        self.reference_height
    }

    /// Canonically identity-sorted reviewed Lights.
    #[must_use]
    pub fn frames(&self) -> &[PlannedRegistrationFrame] {
        &self.frames
    }

    /// Exact all-frame Lanczos-3 support and largest valid rectangle.
    #[must_use]
    pub const fn common_footprint(&self) -> CommonFootprintReport {
        self.common_footprint
    }
}

/// Failure to create a complete immutable registration plan.
#[derive(Clone, Debug, PartialEq)]
pub enum RegistrationPlanError {
    /// Registration requires a reference and at least one distinct source.
    NotEnoughFrames {
        /// Number of submitted frames.
        actual: usize,
    },
    /// Frame count exceeds the shared common-footprint bound.
    TooManyFrames {
        /// Maximum supported frame count.
        maximum: usize,
        /// Number of submitted frames.
        actual: usize,
    },
    /// The same reviewed identity occurs more than once.
    DuplicateFrame(FrameId),
    /// The declared reference identity is absent.
    ReferenceMissing,
    /// Reference dimensions or transform disagree with its declared role.
    ReferenceGeometryMismatch,
    /// At least one footprint or the bounded intersection is invalid.
    Footprint(CommonFootprintError),
    /// Accepted transforms have no rectangular common Lanczos support.
    NoCommonCrop,
}

impl Display for RegistrationPlanError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotEnoughFrames { actual } => write!(
                formatter,
                "registration plan requires at least two frames, received {actual}"
            ),
            Self::TooManyFrames { maximum, actual } => write!(
                formatter,
                "registration plan accepts at most {maximum} frames, received {actual}"
            ),
            Self::DuplicateFrame(frame_id) => {
                write!(formatter, "registration plan repeats frame {frame_id:?}")
            }
            Self::ReferenceMissing => {
                formatter.write_str("registration plan reference frame is missing")
            }
            Self::ReferenceGeometryMismatch => formatter.write_str(
                "registration plan reference must match the output canvas and identity transform",
            ),
            Self::Footprint(error) => write!(formatter, "invalid registration footprint: {error}"),
            Self::NoCommonCrop => formatter.write_str(
                "registration plan transforms do not share a rectangular Lanczos footprint",
            ),
        }
    }
}

impl Error for RegistrationPlanError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Footprint(error) => Some(error),
            Self::NotEnoughFrames { .. }
            | Self::TooManyFrames { .. }
            | Self::DuplicateFrame(_)
            | Self::ReferenceMissing
            | Self::ReferenceGeometryMismatch
            | Self::NoCommonCrop => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn Error>>;

    fn id(digit: char) -> Result<FrameId, aether_review::ReviewError> {
        FrameId::new(digit.to_string().repeat(64))
    }

    fn frame(
        digit: char,
        transform: AffineTransform,
    ) -> Result<PlannedRegistrationFrame, aether_review::ReviewError> {
        Ok(PlannedRegistrationFrame::new(id(digit)?, 12, 10, transform))
    }

    #[test]
    fn canonicalizes_input_order_and_derives_one_common_crop() -> TestResult {
        let shifted = AffineTransform::new(1.0, 0.0, 0.0, 1.0, 2.0, 1.0)?;
        let first = RegistrationPlan::new(
            id('a')?,
            12,
            10,
            vec![frame('b', shifted)?, frame('a', AffineTransform::IDENTITY)?],
        )?;
        let second = RegistrationPlan::new(
            id('a')?,
            12,
            10,
            vec![frame('a', AffineTransform::IDENTITY)?, frame('b', shifted)?],
        )?;

        assert_eq!(first, second);
        assert_eq!(first.algorithm_id(), REGISTRATION_PLAN_ALGORITHM_ID);
        assert_eq!(first.frames()[0].frame_id(), &id('a')?);
        assert_eq!(first.frames()[1].frame_id(), &id('b')?);
        let crop = first.common_footprint().crop().ok_or("crop missing")?;
        assert_eq!(
            (crop.x(), crop.y(), crop.width(), crop.height()),
            (2, 1, 10, 9)
        );
        Ok(())
    }

    #[test]
    fn rejects_duplicate_missing_and_non_identity_references() -> TestResult {
        let identity = frame('a', AffineTransform::IDENTITY)?;
        assert!(matches!(
            RegistrationPlan::new(id('a')?, 12, 10, vec![identity.clone()]),
            Err(RegistrationPlanError::NotEnoughFrames { actual: 1 })
        ));
        assert!(matches!(
            RegistrationPlan::new(id('a')?, 12, 10, vec![identity.clone(), identity]),
            Err(RegistrationPlanError::DuplicateFrame(_))
        ));
        assert_eq!(
            RegistrationPlan::new(
                id('c')?,
                12,
                10,
                vec![
                    frame('a', AffineTransform::IDENTITY)?,
                    frame('b', AffineTransform::IDENTITY)?,
                ],
            ),
            Err(RegistrationPlanError::ReferenceMissing)
        );
        let translated = AffineTransform::new(1.0, 0.0, 0.0, 1.0, 1.0, 0.0)?;
        assert_eq!(
            RegistrationPlan::new(
                id('a')?,
                12,
                10,
                vec![
                    frame('a', translated)?,
                    frame('b', AffineTransform::IDENTITY)?
                ],
            ),
            Err(RegistrationPlanError::ReferenceGeometryMismatch)
        );
        Ok(())
    }

    #[test]
    fn refuses_a_plan_without_common_support() -> TestResult {
        let disjoint = AffineTransform::new(1.0, 0.0, 0.0, 1.0, 100.0, 0.0)?;
        let result = RegistrationPlan::new(
            id('a')?,
            12,
            10,
            vec![
                frame('a', AffineTransform::IDENTITY)?,
                frame('b', disjoint)?,
            ],
        );

        assert_eq!(result, Err(RegistrationPlanError::NoCommonCrop));
        Ok(())
    }
}
