//! Bounded execution contracts for scientific pipeline stages.
//!
//! It defines cancellation, progress, and memory-accounting behavior shared by
//! executors and exposes the first deliberately narrow strict CPU pipeline.

mod cancellation;
mod memory;
mod pipeline;
mod progress;

pub use cancellation::{CancellationToken, Cancelled};
pub use memory::{MemoryBudget, MemoryBudgetError, MemoryReservation};
pub use pipeline::{
    PipelineInput, PipelineSource, STRICT_MEAN_ALGORITHM_ID, StrictMasterRequest,
    StrictPipelineError, StrictPipelineRequest, StrictPipelineResult, run_strict_master_pipeline,
    run_strict_pipeline,
};
pub use progress::{
    MAX_PROGRESS_CODE_BYTES, MAX_STAGE_ID_BYTES, ProgressEvent, ProgressEventError,
    ProgressSequence, ProgressState, StageId, StageIdError,
};
