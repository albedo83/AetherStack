//! Bounded execution contracts for scientific pipeline stages.
//!
//! It defines cancellation, progress, and memory-accounting behavior shared by
//! executors and exposes the first deliberately narrow strict CPU pipeline.

mod cancellation;
mod light_plan;
mod master_plan;
mod memory;
mod pipeline;
mod progress;

pub use cancellation::{CancellationToken, Cancelled};
pub use light_plan::{
    CalibratedLightFrameExecutionResult, CalibratedLightPlanExecutionResult,
    CalibratedLightPlanProgressEvent, LightPlanExecutionError, LightPlanExecutionRequest,
    LightPlanExecutionResult, LightPlanProgressEvent, LightProductExecutionResult,
    run_calibrated_light_plan, run_light_plan,
};
pub use master_plan::{
    MasterPlanExecutionError, MasterPlanExecutionRequest, MasterPlanExecutionResult,
    MasterPlanProgressEvent, MasterProductExecutionResult, run_master_plan,
};
pub use memory::{MemoryBudget, MemoryBudgetError, MemoryReservation};
pub use pipeline::{
    PipelineInput, PipelineSource, STRICT_CALIBRATED_LIGHT_ALGORITHM_ID,
    STRICT_FLAT_MASTER_ALGORITHM_ID, STRICT_MEAN_ALGORITHM_ID, StrictCalibrationRequest,
    StrictFlatMasterRequest, StrictFlatMasterResult, StrictMasterRequest, StrictPipelineError,
    StrictPipelineRequest, StrictPipelineResult, run_strict_calibration_pipeline,
    run_strict_flat_master_pipeline, run_strict_master_pipeline, run_strict_pipeline,
};
pub use progress::{
    MAX_PROGRESS_CODE_BYTES, MAX_STAGE_ID_BYTES, ProgressEvent, ProgressEventError,
    ProgressSequence, ProgressState, StageId, StageIdError,
};
