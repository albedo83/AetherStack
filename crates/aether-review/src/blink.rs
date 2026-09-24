use std::error::Error;
use std::fmt::{Display, Formatter};

use serde::Serialize;

use crate::FrameId;

/// Version of the resolved display-transform schema.
pub const DISPLAY_TRANSFORM_VERSION: u32 = 1;
/// Maximum number of frames in one interactive Blink comparison set.
pub const MAX_BLINK_FRAMES: usize = 100_000;
const MAX_ALGORITHM_ID_BYTES: usize = 128;
const MAX_VIEWPORT_ZOOM: f64 = 1_048_576.0;

/// Transfer function used only to render a scientific image for inspection.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferFunction {
    /// Direct normalized linear mapping.
    Linear,
    /// Midtone transfer controlled by the display transform's midtone value.
    Midtones,
    /// Inverse-hyperbolic-sine stretch with explicit positive softness.
    Asinh {
        /// Positive softness in normalized display space.
        softness: f64,
    },
}

/// Fully resolved and versioned display transform shared by all Blink frames.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct DisplayTransform {
    version: u32,
    black_point: f64,
    white_point: f64,
    midtone: f64,
    transfer_function: TransferFunction,
}

impl DisplayTransform {
    /// Creates a finite display transform with a non-empty input interval.
    ///
    /// # Errors
    ///
    /// Returns a typed error unless black and white points are finite and
    /// strictly ordered, midtone is strictly inside `(0, 1)`, and any transfer
    /// parameter is finite and positive.
    pub fn new(
        black_point: f64,
        white_point: f64,
        midtone: f64,
        transfer_function: TransferFunction,
    ) -> Result<Self, BlinkError> {
        if !black_point.is_finite() || !white_point.is_finite() || black_point >= white_point {
            return Err(BlinkError::InvalidDisplayRange);
        }
        if !midtone.is_finite() || !(0.0..1.0).contains(&midtone) {
            return Err(BlinkError::InvalidMidtone);
        }
        if matches!(
            transfer_function,
            TransferFunction::Asinh { softness } if !softness.is_finite() || softness <= 0.0
        ) {
            return Err(BlinkError::InvalidTransferFunction);
        }
        Ok(Self {
            version: DISPLAY_TRANSFORM_VERSION,
            black_point: canonical_zero(black_point),
            white_point: canonical_zero(white_point),
            midtone,
            transfer_function,
        })
    }

    /// Display-transform schema version.
    #[must_use]
    pub const fn version(self) -> u32 {
        self.version
    }

    /// Input value mapped to display black.
    #[must_use]
    pub const fn black_point(self) -> f64 {
        self.black_point
    }

    /// Input value mapped to display white.
    #[must_use]
    pub const fn white_point(self) -> f64 {
        self.white_point
    }

    /// Normalized midtone control.
    #[must_use]
    pub const fn midtone(self) -> f64 {
        self.midtone
    }

    /// Selected display-only transfer function.
    #[must_use]
    pub const fn transfer_function(self) -> TransferFunction {
        self.transfer_function
    }
}

/// Origin of the single transform locked across a Blink comparison.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "mode")]
pub enum StretchSource {
    /// Values were resolved directly from explicit user controls.
    Manual,
    /// One automatic transform was resolved from the whole comparison set.
    SharedAutomatic {
        /// Stable versioned automatic-stretch algorithm identifier.
        algorithm_id: String,
        /// Number of frames contributing to the shared estimate.
        supporting_frames: usize,
    },
}

impl StretchSource {
    /// Creates a validated shared automatic-transform origin.
    ///
    /// # Errors
    ///
    /// Returns a typed error for a non-portable algorithm identifier or zero
    /// supporting frames.
    pub fn shared_automatic(
        algorithm_id: impl Into<String>,
        supporting_frames: usize,
    ) -> Result<Self, BlinkError> {
        let algorithm_id = algorithm_id.into();
        if !valid_algorithm_id(&algorithm_id) {
            return Err(BlinkError::InvalidAlgorithmId);
        }
        if supporting_frames == 0 {
            return Err(BlinkError::ZeroStretchSupport);
        }
        Ok(Self::SharedAutomatic {
            algorithm_id,
            supporting_frames,
        })
    }
}

/// Shared normalized viewport retained while Blink switches frames.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Viewport {
    center_x: f64,
    center_y: f64,
    zoom: f64,
}

impl Viewport {
    /// Creates a normalized viewport.
    ///
    /// # Errors
    ///
    /// Returns a typed error unless both center coordinates are finite and in
    /// `[0, 1]`, and zoom is finite, positive, and bounded.
    pub fn new(center_x: f64, center_y: f64, zoom: f64) -> Result<Self, BlinkError> {
        if !center_x.is_finite()
            || !center_y.is_finite()
            || !(0.0..=1.0).contains(&center_x)
            || !(0.0..=1.0).contains(&center_y)
        {
            return Err(BlinkError::InvalidViewportCenter);
        }
        if !zoom.is_finite() || zoom <= 0.0 || zoom > MAX_VIEWPORT_ZOOM {
            return Err(BlinkError::InvalidViewportZoom);
        }
        Ok(Self {
            center_x: canonical_zero(center_x),
            center_y: canonical_zero(center_y),
            zoom,
        })
    }

    /// Normalized horizontal center.
    #[must_use]
    pub const fn center_x(self) -> f64 {
        self.center_x
    }

    /// Normalized vertical center.
    #[must_use]
    pub const fn center_y(self) -> f64 {
        self.center_y
    }

    /// Shared zoom scale.
    #[must_use]
    pub const fn zoom(self) -> f64 {
        self.zoom
    }
}

/// Right-angle image rotation used by the display layer.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Rotation {
    /// No rotation.
    Degrees0,
    /// Clockwise quarter turn.
    Degrees90,
    /// Half turn.
    Degrees180,
    /// Clockwise three-quarter turn.
    Degrees270,
}

/// Shared orientation applied to every displayed Blink frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct Orientation {
    rotation: Rotation,
    mirror_horizontal: bool,
    mirror_vertical: bool,
}

impl Orientation {
    /// Creates an explicit shared display orientation.
    #[must_use]
    pub const fn new(rotation: Rotation, mirror_horizontal: bool, mirror_vertical: bool) -> Self {
        Self {
            rotation,
            mirror_horizontal,
            mirror_vertical,
        }
    }

    /// Right-angle rotation.
    #[must_use]
    pub const fn rotation(self) -> Rotation {
        self.rotation
    }

    /// Whether horizontal mirroring is enabled.
    #[must_use]
    pub const fn mirror_horizontal(self) -> bool {
        self.mirror_horizontal
    }

    /// Whether vertical mirroring is enabled.
    #[must_use]
    pub const fn mirror_vertical(self) -> bool {
        self.mirror_vertical
    }
}

/// Channel mapping selected for display.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelPresentation {
    /// A raw single-plane CFA mosaic.
    CfaMosaic,
    /// A monochrome or derived luminance plane.
    Luminance,
    /// Red channel only.
    Red,
    /// Green channel only.
    Green,
    /// Blue channel only.
    Blue,
    /// Three-channel color composition.
    Rgb,
}

/// Scientific interpretation named beside the viewer.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ViewInterpretation {
    /// Original raw color-filter-array samples.
    RawCfa,
    /// Display-only demosaiced representation.
    DebayeredPreview,
    /// Monochrome source or derived plane.
    Monochrome,
    /// Integrated processing product.
    IntegratedProduct,
}

/// Display state locked across every frame in one Blink controller.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DisplayLock {
    transform: DisplayTransform,
    stretch_source: StretchSource,
    viewport: Viewport,
    orientation: Orientation,
    channels: ChannelPresentation,
    interpretation: ViewInterpretation,
}

impl DisplayLock {
    /// Combines all display-only state that must remain fixed during comparison.
    ///
    /// # Errors
    ///
    /// Returns a typed error if a caller-constructed automatic stretch source is
    /// invalid or its support disagrees with the Blink set at controller creation.
    pub fn new(
        transform: DisplayTransform,
        stretch_source: StretchSource,
        viewport: Viewport,
        orientation: Orientation,
        channels: ChannelPresentation,
        interpretation: ViewInterpretation,
    ) -> Result<Self, BlinkError> {
        validate_stretch_source(&stretch_source)?;
        validate_channel_interpretation(channels, interpretation)?;
        Ok(Self {
            transform,
            stretch_source,
            viewport,
            orientation,
            channels,
            interpretation,
        })
    }

    /// Resolved shared transform.
    #[must_use]
    pub const fn transform(&self) -> DisplayTransform {
        self.transform
    }

    /// Manual or shared-automatic transform origin.
    #[must_use]
    pub const fn stretch_source(&self) -> &StretchSource {
        &self.stretch_source
    }

    /// Shared viewport.
    #[must_use]
    pub const fn viewport(&self) -> Viewport {
        self.viewport
    }

    /// Shared display orientation.
    #[must_use]
    pub const fn orientation(&self) -> Orientation {
        self.orientation
    }

    /// Shared channel mapping.
    #[must_use]
    pub const fn channels(&self) -> ChannelPresentation {
        self.channels
    }

    /// Named scientific interpretation.
    #[must_use]
    pub const fn interpretation(&self) -> ViewInterpretation {
        self.interpretation
    }
}

/// Direction of a manual Blink step and internal bounce traversal.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StepDirection {
    /// Move toward the next comparison frame.
    Forward,
    /// Move toward the previous comparison frame.
    Backward,
}

/// Automatic Blink traversal mode.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BlinkPlaybackMode {
    /// Advance and wrap from the final frame to the first.
    LoopForward,
    /// Move backward and wrap from the first frame to the final frame.
    LoopBackward,
    /// Reverse direction at both ends without repeating an endpoint.
    Bounce,
}

/// Whether timer-driven Blink stepping is active.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PlaybackState {
    /// Timer steps are disabled; manual steps remain available.
    Paused,
    /// Timer steps may request the next preview.
    Playing,
}

#[derive(Clone, Copy, Debug)]
struct PendingStep {
    index: usize,
    bounce_direction_after_commit: StepDirection,
}

/// Exact-identity state machine for interactive Blink comparison.
///
/// A step first creates a pending frame request. The visible identity does not
/// change until [`BlinkController::commit_ready`] receives the exact requested
/// identity, preventing a slow or stale preview from being mislabeled.
#[derive(Debug)]
pub struct BlinkController {
    frames: Vec<FrameId>,
    current_index: usize,
    pending: Option<PendingStep>,
    playback_mode: BlinkPlaybackMode,
    playback_state: PlaybackState,
    bounce_direction: StepDirection,
    display_lock: DisplayLock,
}

impl BlinkController {
    /// Creates a paused controller at the first frame in the comparison set.
    ///
    /// # Errors
    ///
    /// Returns a typed error for an empty, duplicate, or oversized set, a shared
    /// stretch whose support differs from set length, or allocation failure.
    pub fn new(
        frames: Vec<FrameId>,
        display_lock: DisplayLock,
        playback_mode: BlinkPlaybackMode,
    ) -> Result<Self, BlinkError> {
        if frames.is_empty() {
            return Err(BlinkError::EmptyFrameSet);
        }
        if frames.len() > MAX_BLINK_FRAMES {
            return Err(BlinkError::TooManyFrames {
                maximum: MAX_BLINK_FRAMES,
            });
        }
        let mut identities = try_vec(frames.len())?;
        identities.extend(frames.iter().cloned());
        identities.sort_unstable();
        if identities.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(BlinkError::DuplicateFrame);
        }
        validate_display_support(&display_lock, frames.len())?;
        Ok(Self {
            frames,
            current_index: 0,
            pending: None,
            playback_mode,
            playback_state: PlaybackState::Paused,
            bounce_direction: StepDirection::Forward,
            display_lock,
        })
    }

    /// Number of frames in the immutable comparison set.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.frames.len()
    }

    /// Whether the comparison set contains no frames.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    /// Identity currently visible and safe for an accept/reject action.
    #[must_use]
    pub fn current_frame(&self) -> &FrameId {
        &self.frames[self.current_index]
    }

    /// One-based visible position and total count for accessible announcements.
    #[must_use]
    pub const fn position(&self) -> (usize, usize) {
        (self.current_index + 1, self.frames.len())
    }

    /// Identity requested from the preview renderer but not yet visible.
    #[must_use]
    pub fn pending_frame(&self) -> Option<&FrameId> {
        self.pending.map(|pending| &self.frames[pending.index])
    }

    /// Current automatic traversal mode.
    #[must_use]
    pub const fn playback_mode(&self) -> BlinkPlaybackMode {
        self.playback_mode
    }

    /// Current playback state.
    #[must_use]
    pub const fn playback_state(&self) -> PlaybackState {
        self.playback_state
    }

    /// Display state shared by every comparison frame.
    #[must_use]
    pub const fn display_lock(&self) -> &DisplayLock {
        &self.display_lock
    }

    /// Starts or pauses timer-driven playback.
    ///
    /// Pausing cancels an unresolved timer request so it cannot become visible
    /// after the user has stopped playback.
    pub fn set_playback_state(&mut self, state: PlaybackState) {
        self.playback_state = state;
        if state == PlaybackState::Paused {
            self.pending = None;
        }
    }

    /// Changes the automatic traversal mode while no preview is pending.
    ///
    /// # Errors
    ///
    /// Returns [`BlinkError::PreviewPending`] if the old-mode request has not
    /// been committed or cancelled.
    pub fn set_playback_mode(&mut self, mode: BlinkPlaybackMode) -> Result<(), BlinkError> {
        self.require_no_pending()?;
        self.playback_mode = mode;
        self.bounce_direction = StepDirection::Forward;
        Ok(())
    }

    /// Replaces the shared viewport/transform/orientation lock.
    ///
    /// # Errors
    ///
    /// Returns a typed error while a preview is pending or if automatic-stretch
    /// support disagrees with the immutable comparison set.
    pub fn set_display_lock(&mut self, display_lock: DisplayLock) -> Result<(), BlinkError> {
        self.require_no_pending()?;
        validate_display_support(&display_lock, self.frames.len())?;
        self.display_lock = display_lock;
        Ok(())
    }

    /// Requests a manual wrapped step without changing visible identity.
    ///
    /// # Errors
    ///
    /// Returns a typed error for a single-frame set or an unresolved request.
    pub fn request_step(&mut self, direction: StepDirection) -> Result<&FrameId, BlinkError> {
        self.require_no_pending()?;
        if self.frames.len() < 2 {
            return Err(BlinkError::SingleFrameSet);
        }
        let target = wrapped_index(self.current_index, self.frames.len(), direction);
        self.pending = Some(PendingStep {
            index: target,
            bounce_direction_after_commit: self.bounce_direction,
        });
        Ok(&self.frames[target])
    }

    /// Requests the next frame defined by active playback mode.
    ///
    /// # Errors
    ///
    /// Returns a typed error while paused, for a single-frame set, or when a
    /// prior preview request remains unresolved.
    pub fn request_playback_step(&mut self) -> Result<&FrameId, BlinkError> {
        if self.playback_state != PlaybackState::Playing {
            return Err(BlinkError::PlaybackPaused);
        }
        self.require_no_pending()?;
        if self.frames.len() < 2 {
            return Err(BlinkError::SingleFrameSet);
        }
        let (target, direction_after_commit) = match self.playback_mode {
            BlinkPlaybackMode::LoopForward => (
                wrapped_index(
                    self.current_index,
                    self.frames.len(),
                    StepDirection::Forward,
                ),
                self.bounce_direction,
            ),
            BlinkPlaybackMode::LoopBackward => (
                wrapped_index(
                    self.current_index,
                    self.frames.len(),
                    StepDirection::Backward,
                ),
                self.bounce_direction,
            ),
            BlinkPlaybackMode::Bounce => {
                bounce_target(self.current_index, self.frames.len(), self.bounce_direction)
            }
        };
        self.pending = Some(PendingStep {
            index: target,
            bounce_direction_after_commit: direction_after_commit,
        });
        Ok(&self.frames[target])
    }

    /// Commits a decoded preview only when its identity exactly matches pending.
    ///
    /// A mismatch leaves both current and pending identities unchanged so a late
    /// asynchronous response cannot corrupt visible state.
    ///
    /// # Errors
    ///
    /// Returns a typed error if no request exists or the supplied identity is not
    /// the exact pending frame.
    pub fn commit_ready(&mut self, frame_id: &FrameId) -> Result<(), BlinkError> {
        let pending = self.pending.ok_or(BlinkError::NoPreviewPending)?;
        let expected = &self.frames[pending.index];
        if expected != frame_id {
            return Err(BlinkError::PreviewIdentityMismatch {
                expected: expected.clone(),
                received: frame_id.clone(),
            });
        }
        self.current_index = pending.index;
        self.bounce_direction = pending.bounce_direction_after_commit;
        self.pending = None;
        Ok(())
    }

    /// Cancels an unresolved preview request without changing visible identity.
    pub fn cancel_pending(&mut self) {
        self.pending = None;
    }

    fn require_no_pending(&self) -> Result<(), BlinkError> {
        if self.pending.is_some() {
            Err(BlinkError::PreviewPending)
        } else {
            Ok(())
        }
    }
}

/// Failure to create or advance a safe Blink comparison.
#[derive(Clone, Debug, PartialEq)]
pub enum BlinkError {
    /// Black and white points were non-finite or not strictly ordered.
    InvalidDisplayRange,
    /// Midtone was non-finite or outside the open unit interval.
    InvalidMidtone,
    /// A transfer-function parameter was non-finite or non-positive.
    InvalidTransferFunction,
    /// Automatic stretch algorithm identifier was not portable.
    InvalidAlgorithmId,
    /// Shared automatic stretch had no supporting frames.
    ZeroStretchSupport,
    /// Shared automatic stretch support did not match the comparison set.
    StretchSupportMismatch {
        /// Blink comparison-set size.
        frames: usize,
        /// Declared automatic-stretch support.
        supporting_frames: usize,
    },
    /// Normalized viewport center was non-finite or outside the unit square.
    InvalidViewportCenter,
    /// Viewport zoom was non-finite, non-positive, or unreasonably large.
    InvalidViewportZoom,
    /// Channel mapping contradicts the named scientific interpretation.
    IncompatibleChannelInterpretation,
    /// Blink requires at least one frame.
    EmptyFrameSet,
    /// Comparison set exceeded its explicit bound.
    TooManyFrames {
        /// Maximum supported comparison-set size.
        maximum: usize,
    },
    /// Comparison set contained the same stable identity more than once.
    DuplicateFrame,
    /// A single frame cannot produce a Blink step.
    SingleFrameSet,
    /// Timer-driven stepping was requested while playback was paused.
    PlaybackPaused,
    /// Another preview request must be committed or cancelled first.
    PreviewPending,
    /// A preview was committed without a pending request.
    NoPreviewPending,
    /// A renderer response did not match the exact requested frame.
    PreviewIdentityMismatch {
        /// Identity the controller requested.
        expected: FrameId,
        /// Identity returned by the renderer.
        received: FrameId,
    },
    /// A bounded allocation could not be reserved.
    AllocationFailed {
        /// Number of elements requested.
        elements: usize,
    },
}

impl Display for BlinkError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidDisplayRange => {
                formatter.write_str("display black and white points must be finite and ordered")
            }
            Self::InvalidMidtone => formatter
                .write_str("display midtone must be finite and strictly between zero and one"),
            Self::InvalidTransferFunction => {
                formatter.write_str("display transfer-function parameters are invalid")
            }
            Self::InvalidAlgorithmId => formatter
                .write_str("automatic stretch algorithm identifier must be portable and versioned"),
            Self::ZeroStretchSupport => {
                formatter.write_str("shared automatic stretch requires supporting frames")
            }
            Self::StretchSupportMismatch {
                frames,
                supporting_frames,
            } => write!(
                formatter,
                "shared stretch used {supporting_frames} frames but Blink contains {frames}"
            ),
            Self::InvalidViewportCenter => {
                formatter.write_str("viewport center must be inside the normalized unit square")
            }
            Self::InvalidViewportZoom => formatter
                .write_str("viewport zoom must be finite, positive, and within its safety bound"),
            Self::IncompatibleChannelInterpretation => formatter
                .write_str("display channel mapping contradicts the named image interpretation"),
            Self::EmptyFrameSet => formatter.write_str("Blink set must contain at least one frame"),
            Self::TooManyFrames { maximum } => {
                write!(formatter, "Blink set exceeds the maximum {maximum} frames")
            }
            Self::DuplicateFrame => formatter.write_str("Blink set contains a duplicate frame"),
            Self::SingleFrameSet => {
                formatter.write_str("Blink stepping requires at least two frames")
            }
            Self::PlaybackPaused => formatter.write_str("Blink playback is paused"),
            Self::PreviewPending => {
                formatter.write_str("a Blink preview request is already pending")
            }
            Self::NoPreviewPending => formatter.write_str("no Blink preview request is pending"),
            Self::PreviewIdentityMismatch { expected, received } => write!(
                formatter,
                "preview identity {} does not match requested frame {}",
                received.as_str(),
                expected.as_str()
            ),
            Self::AllocationFailed { elements } => {
                write!(
                    formatter,
                    "cannot reserve storage for {elements} Blink frames"
                )
            }
        }
    }
}

impl Error for BlinkError {}

fn validate_stretch_source(source: &StretchSource) -> Result<(), BlinkError> {
    if let StretchSource::SharedAutomatic {
        algorithm_id,
        supporting_frames,
    } = source
    {
        if !valid_algorithm_id(algorithm_id) {
            return Err(BlinkError::InvalidAlgorithmId);
        }
        if *supporting_frames == 0 {
            return Err(BlinkError::ZeroStretchSupport);
        }
    }
    Ok(())
}

fn validate_display_support(display: &DisplayLock, frames: usize) -> Result<(), BlinkError> {
    if let StretchSource::SharedAutomatic {
        supporting_frames, ..
    } = display.stretch_source()
        && *supporting_frames != frames
    {
        return Err(BlinkError::StretchSupportMismatch {
            frames,
            supporting_frames: *supporting_frames,
        });
    }
    Ok(())
}

fn validate_channel_interpretation(
    channels: ChannelPresentation,
    interpretation: ViewInterpretation,
) -> Result<(), BlinkError> {
    let compatible = match interpretation {
        ViewInterpretation::RawCfa => channels == ChannelPresentation::CfaMosaic,
        ViewInterpretation::DebayeredPreview => matches!(
            channels,
            ChannelPresentation::Luminance
                | ChannelPresentation::Red
                | ChannelPresentation::Green
                | ChannelPresentation::Blue
                | ChannelPresentation::Rgb
        ),
        ViewInterpretation::Monochrome => channels == ChannelPresentation::Luminance,
        ViewInterpretation::IntegratedProduct => channels != ChannelPresentation::CfaMosaic,
    };
    if compatible {
        Ok(())
    } else {
        Err(BlinkError::IncompatibleChannelInterpretation)
    }
}

fn valid_algorithm_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_ALGORITHM_ID_BYTES
        && value.bytes().enumerate().all(|(index, byte)| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || (index > 0 && matches!(byte, b'-' | b'_' | b'.'))
        })
}

fn wrapped_index(current: usize, length: usize, direction: StepDirection) -> usize {
    match direction {
        StepDirection::Forward => (current + 1) % length,
        StepDirection::Backward => current.checked_sub(1).unwrap_or(length - 1),
    }
}

fn bounce_target(
    current: usize,
    length: usize,
    direction: StepDirection,
) -> (usize, StepDirection) {
    match direction {
        StepDirection::Forward if current + 1 == length => (current - 1, StepDirection::Backward),
        StepDirection::Backward if current == 0 => (1, StepDirection::Forward),
        StepDirection::Forward => (current + 1, StepDirection::Forward),
        StepDirection::Backward => (current - 1, StepDirection::Backward),
    }
}

fn canonical_zero(value: f64) -> f64 {
    if value == 0.0 { 0.0 } else { value }
}

fn try_vec<T>(elements: usize) -> Result<Vec<T>, BlinkError> {
    let mut output = Vec::new();
    output
        .try_reserve_exact(elements)
        .map_err(|_| BlinkError::AllocationFailed { elements })?;
    Ok(output)
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use super::*;

    type TestResult<T = ()> = Result<T, Box<dyn StdError>>;

    fn id(digit: char) -> TestResult<FrameId> {
        Ok(FrameId::new(digit.to_string().repeat(64))?)
    }

    fn display(frames: Option<usize>) -> TestResult<DisplayLock> {
        let stretch = match frames {
            Some(frames) => StretchSource::shared_automatic("shared-mad-v1", frames)?,
            None => StretchSource::Manual,
        };
        Ok(DisplayLock::new(
            DisplayTransform::new(100.0, 2_000.0, 0.25, TransferFunction::Midtones)?,
            stretch,
            Viewport::new(0.5, 0.5, 1.0)?,
            Orientation::new(Rotation::Degrees0, false, false),
            ChannelPresentation::CfaMosaic,
            ViewInterpretation::RawCfa,
        )?)
    }

    fn controller(mode: BlinkPlaybackMode) -> TestResult<BlinkController> {
        Ok(BlinkController::new(
            vec![id('a')?, id('b')?, id('c')?],
            display(None)?,
            mode,
        )?)
    }

    #[test]
    fn validates_display_transform_viewport_and_interpretation() -> TestResult {
        assert_eq!(
            DisplayTransform::new(1.0, 1.0, 0.5, TransferFunction::Linear),
            Err(BlinkError::InvalidDisplayRange)
        );
        assert_eq!(
            DisplayTransform::new(0.0, 1.0, 1.0, TransferFunction::Linear),
            Err(BlinkError::InvalidMidtone)
        );
        assert_eq!(
            DisplayTransform::new(0.0, 1.0, 0.5, TransferFunction::Asinh { softness: 0.0 },),
            Err(BlinkError::InvalidTransferFunction)
        );
        assert_eq!(
            Viewport::new(-0.1, 0.5, 1.0),
            Err(BlinkError::InvalidViewportCenter)
        );
        assert_eq!(
            Viewport::new(0.5, 0.5, 0.0),
            Err(BlinkError::InvalidViewportZoom)
        );
        assert_eq!(
            DisplayLock::new(
                DisplayTransform::new(0.0, 1.0, 0.5, TransferFunction::Linear)?,
                StretchSource::Manual,
                Viewport::new(0.5, 0.5, 1.0)?,
                Orientation::new(Rotation::Degrees0, false, false),
                ChannelPresentation::Rgb,
                ViewInterpretation::RawCfa,
            ),
            Err(BlinkError::IncompatibleChannelInterpretation)
        );
        Ok(())
    }

    #[test]
    fn validates_frame_set_and_shared_stretch_support() -> TestResult {
        assert!(matches!(
            BlinkController::new(Vec::new(), display(None)?, BlinkPlaybackMode::LoopForward),
            Err(BlinkError::EmptyFrameSet)
        ));
        assert!(matches!(
            BlinkController::new(
                vec![id('a')?, id('a')?],
                display(None)?,
                BlinkPlaybackMode::LoopForward,
            ),
            Err(BlinkError::DuplicateFrame)
        ));
        assert!(matches!(
            BlinkController::new(
                vec![id('a')?, id('b')?],
                display(Some(1))?,
                BlinkPlaybackMode::LoopForward,
            ),
            Err(BlinkError::StretchSupportMismatch {
                frames: 2,
                supporting_frames: 1,
            })
        ));
        Ok(())
    }

    #[test]
    fn pending_preview_never_changes_visible_identity() -> TestResult {
        let mut blink = controller(BlinkPlaybackMode::LoopForward)?;
        let requested = blink.request_step(StepDirection::Forward)?.clone();

        assert_eq!(requested, id('b')?);
        assert_eq!(blink.current_frame(), &id('a')?);
        assert_eq!(blink.pending_frame(), Some(&id('b')?));
        blink.commit_ready(&requested)?;
        assert_eq!(blink.current_frame(), &id('b')?);
        assert_eq!(blink.pending_frame(), None);
        Ok(())
    }

    #[test]
    fn stale_renderer_response_cannot_replace_current_frame() -> TestResult {
        let mut blink = controller(BlinkPlaybackMode::LoopForward)?;
        blink.request_step(StepDirection::Forward)?;
        assert!(matches!(
            blink.commit_ready(&id('c')?),
            Err(BlinkError::PreviewIdentityMismatch { .. })
        ));
        assert_eq!(blink.current_frame(), &id('a')?);
        assert_eq!(blink.pending_frame(), Some(&id('b')?));
        Ok(())
    }

    #[test]
    fn manual_steps_wrap_in_both_directions() -> TestResult {
        let mut blink = controller(BlinkPlaybackMode::LoopForward)?;
        let previous = blink.request_step(StepDirection::Backward)?.clone();
        blink.commit_ready(&previous)?;
        assert_eq!(blink.current_frame(), &id('c')?);
        let next = blink.request_step(StepDirection::Forward)?.clone();
        blink.commit_ready(&next)?;
        assert_eq!(blink.current_frame(), &id('a')?);
        Ok(())
    }

    #[test]
    fn bounce_reverses_without_repeating_endpoints() -> TestResult {
        let mut blink = controller(BlinkPlaybackMode::Bounce)?;
        blink.set_playback_state(PlaybackState::Playing);
        let mut visited = Vec::new();
        for _ in 0..5 {
            let requested = blink.request_playback_step()?.clone();
            blink.commit_ready(&requested)?;
            visited.push(blink.current_frame().clone());
        }
        assert_eq!(
            visited,
            vec![id('b')?, id('c')?, id('b')?, id('a')?, id('b')?]
        );
        Ok(())
    }

    #[test]
    fn pause_cancels_pending_timer_response() -> TestResult {
        let mut blink = controller(BlinkPlaybackMode::LoopForward)?;
        blink.set_playback_state(PlaybackState::Playing);
        let requested = blink.request_playback_step()?.clone();
        blink.set_playback_state(PlaybackState::Paused);

        assert_eq!(blink.pending_frame(), None);
        assert_eq!(blink.current_frame(), &id('a')?);
        assert_eq!(
            blink.commit_ready(&requested),
            Err(BlinkError::NoPreviewPending)
        );
        assert_eq!(
            blink.request_playback_step(),
            Err(BlinkError::PlaybackPaused)
        );
        Ok(())
    }

    #[test]
    fn transform_and_mode_cannot_change_under_pending_preview() -> TestResult {
        let mut blink = controller(BlinkPlaybackMode::LoopForward)?;
        blink.request_step(StepDirection::Forward)?;
        assert_eq!(
            blink.set_playback_mode(BlinkPlaybackMode::Bounce),
            Err(BlinkError::PreviewPending)
        );
        assert_eq!(
            blink.set_display_lock(display(None)?),
            Err(BlinkError::PreviewPending)
        );
        blink.cancel_pending();
        blink.set_playback_mode(BlinkPlaybackMode::Bounce)?;
        blink.set_display_lock(display(Some(3))?)?;
        assert_eq!(blink.playback_mode(), BlinkPlaybackMode::Bounce);
        Ok(())
    }

    #[test]
    fn single_frame_reports_position_but_cannot_step() -> TestResult {
        let mut blink = BlinkController::new(
            vec![id('a')?],
            display(None)?,
            BlinkPlaybackMode::LoopForward,
        )?;
        assert_eq!(blink.position(), (1, 1));
        assert_eq!(
            blink.request_step(StepDirection::Forward),
            Err(BlinkError::SingleFrameSet)
        );
        Ok(())
    }
}
