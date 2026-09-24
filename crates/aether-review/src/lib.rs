//! Deterministic interaction model for frame review and Blink comparison.
//!
//! This crate is deliberately independent of a particular desktop toolkit. It
//! owns the state transitions that must remain identical in a command-line
//! client, the future Tauri interface, and tests: explicit review decisions,
//! transactional preview and undo, stable table sorting, and exact Blink frame
//! identity. Scientific pixel decoding and rendering live in other crates.

mod blink;
mod review;

pub use blink::{
    BlinkController, BlinkError, BlinkPlaybackMode, ChannelPresentation, DISPLAY_TRANSFORM_VERSION,
    DisplayLock, DisplayTransform, MAX_BLINK_FRAMES, Orientation, PlaybackState, Rotation,
    StepDirection, StretchSource, TransferFunction, ViewInterpretation, Viewport,
};
pub use review::{
    DecisionChange, DecisionDelta, FrameId, FrameMetrics, FrameSpec, MAX_BATCH_CHANGES,
    MAX_LABEL_BYTES, MAX_NOTE_BYTES, MAX_REVIEW_FRAMES, MAX_SOURCE_PATH_BYTES, MAX_UNDO_DEPTH,
    ManualDecision, ManualRejectionReason, MissingPlacement, ReviewBook, ReviewError,
    ReviewPreview, ReviewState, SortDirection, SortField, SortSpec,
};
