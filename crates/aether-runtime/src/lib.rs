//! Bounded execution contracts for scientific pipeline stages.
//!
//! This crate deliberately contains no scheduler or processing algorithm. It
//! defines the cancellation, progress, and memory-accounting behavior that all
//! later executors and stages must share.

mod cancellation;
mod memory;
mod progress;

pub use cancellation::{CancellationToken, Cancelled};
pub use memory::{MemoryBudget, MemoryBudgetError, MemoryReservation};
pub use progress::{
    MAX_PROGRESS_CODE_BYTES, MAX_STAGE_ID_BYTES, ProgressEvent, ProgressEventError,
    ProgressSequence, ProgressState, StageId, StageIdError,
};
