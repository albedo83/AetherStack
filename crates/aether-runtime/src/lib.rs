//! Bounded execution contracts for scientific pipeline stages.
//!
//! It defines cancellation, progress, and memory-accounting behavior shared by
//! executors and exposes the first deliberately narrow strict CPU pipeline.

mod cancellation;
mod demosaic;
mod light_plan;
mod local_normalization;
mod master_plan;
mod memory;
mod pipeline;
mod progress;
mod registered_stack;
mod registration;

pub use cancellation::{CancellationToken, Cancelled};
pub use demosaic::{
    DemosaicPipelineError, StrictDemosaicRequest, StrictDemosaicResult,
    run_strict_demosaic_pipeline,
};
pub use light_plan::{
    CalibratedLightFrameExecutionResult, CalibratedLightPlanExecutionResult,
    CalibratedLightPlanProgressEvent, DemosaicedLightFrameExecutionResult,
    DemosaicedLightPlanExecutionResult, DemosaicedLightPlanProgressEvent, LightPlanExecutionError,
    LightPlanExecutionRequest, LightPlanExecutionResult, LightPlanProgressEvent,
    LightProductExecutionResult, run_calibrated_light_plan, run_demosaiced_light_plan,
    run_light_plan,
};
pub use local_normalization::{
    LocalNormalizationCellDiagnostic, LocalNormalizationControlPoint,
    LocalNormalizationPipelineError, LocalNormalizationRequest, LocalNormalizationRequestError,
    LocalNormalizationResult, estimate_local_normalization_peak_bytes, run_local_normalization,
    run_local_normalization_with_progress,
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
pub use registered_stack::{
    BALANCED_PSF_WEIGHT_ALGORITHM_ID, PERCENTILE_REJECTION_MAP_ALGORITHM_ID,
    PercentileClipParameters, QualityWeightMetrics, REGISTERED_CROP_MEAN_ALGORITHM_ID,
    REGISTERED_MEDIAN_ALGORITHM_ID, REGISTERED_PERCENTILE_CLIPPED_MEAN_ALGORITHM_ID,
    REGISTERED_SIGMA_CLIPPED_MEAN_ALGORITHM_ID, REGISTERED_WEIGHTED_MEAN_ALGORITHM_ID,
    REGISTERED_WINSORIZED_SIGMA_CLIPPED_MEAN_ALGORITHM_ID, RegisteredFrameQuality,
    RegisteredFrameWeight, RegisteredRejectionMapOutput, RegisteredRejectionMapSummary,
    RegisteredStackError, RegisteredStackEstimator, RegisteredStackRequest, RegisteredStackResult,
    RegisteredStackSource, RegisteredWeightSet, SIGMA_CLIPPED_MEAN_ALGORITHM_ID,
    SIGMA_REJECTION_MAP_ALGORITHM_ID, SigmaClipParameters,
    WINSORIZED_SIGMA_CLIPPED_MEAN_ALGORITHM_ID, WINSORIZED_SIGMA_REJECTION_MAP_ALGORITHM_ID,
    run_registered_stack,
};
pub use registration::{
    ProjectiveRegistrationPlanExecutionRequest, RegisteredFrameExecutionResult,
    RegistrationPipelineError, RegistrationPlanExecutionError, RegistrationPlanExecutionRequest,
    RegistrationPlanExecutionResult, RegistrationPlanProgressEvent, RegistrationPlanSource,
    StrictRegistrationRequest, StrictRegistrationResult, run_projective_registration_plan,
    run_registration_plan, run_strict_registration_pipeline,
};
