//! Bounded execution contracts for scientific pipeline stages.
//!
//! It defines cancellation, progress, and memory-accounting behavior shared by
//! executors and exposes the first deliberately narrow strict CPU pipeline.

mod cancellation;
mod defect;
mod demosaic;
mod drizzle;
mod drizzle_spool;
mod light_plan;
mod linear_defect;
mod local_normalization;
mod master_plan;
mod memory;
mod pipeline;
mod progress;
mod registered_stack;
mod registration;

pub use cancellation::{CancellationToken, Cancelled};
pub use defect::{
    DefectAnalysisError, DefectAnalysisResult, DefectCorrectionPipelineError,
    DefectCorrectionPipelineResult, DefectFitsReference, DefectMemoryEstimate,
    DefectMemoryEstimateError, DefectParameterSealError, DefectReferenceKind,
    DefectReferenceParameters, STRICT_DEFECT_CORRECTED_ALGORITHM_ID,
    STRICT_DEFECT_CORRECTION_STAGE_ID, STRICT_DEFECT_MAP_ALGORITHM_ID,
    StrictDefectCorrectionRequest, analyze_defect_references, estimate_defect_memory,
    run_strict_defect_correction, run_strict_defect_correction_with_progress,
    strict_defect_parameters_sha256,
};
pub use demosaic::{
    DemosaicPipelineError, StrictDemosaicRequest, StrictDemosaicResult,
    run_strict_demosaic_pipeline,
};
pub use drizzle::{
    DRIZZLE_SCIENCE_ALGORITHM_ID, DRIZZLE_SUPPORT_ALGORITHM_ID, DRIZZLE_WEIGHT_ALGORITHM_ID,
    DrizzleBandPlan, DrizzleBandPlanError, DrizzleFitsAccumulationError, DrizzleFitsFrame,
    DrizzleFitsStackError, DrizzleFitsTileEvidence, DrizzleFitsWindowEvidence,
    DrizzleOutputExecutionError, DrizzleOutputExecutionResult, DrizzlePlannedBand,
    DrizzleProductDestinations, DrizzleProductKind, DrizzleProductProvenance,
    DrizzlePublicationError, DrizzlePublicationResult, DrizzleSpoolError,
    DrizzleTileExecutionError, DrizzleTileExecutionResult, DrizzleTileMemoryEstimate,
    StrictDrizzleSource, accumulate_fits_cfa_frames, accumulate_fits_cfa_tile,
    estimate_drizzle_fits_tile_memory, plan_drizzle_fits_bands, publish_drizzle_products,
    run_drizzle_fits_output, run_drizzle_fits_tile, run_strict_drizzle_output,
    run_strict_drizzle_output_with_progress, strict_drizzle_parameters_sha256,
    strict_drizzle_plan_sha256,
};
pub use light_plan::{
    CalibratedLightFrameExecutionResult, CalibratedLightPlanExecutionResult,
    CalibratedLightPlanProgressEvent, DemosaicedLightFrameExecutionResult,
    DemosaicedLightPlanExecutionResult, DemosaicedLightPlanProgressEvent, LightPlanExecutionError,
    LightPlanExecutionRequest, LightPlanExecutionResult, LightPlanProgressEvent,
    LightProductExecutionResult, run_calibrated_light_plan, run_demosaiced_light_plan,
    run_light_plan,
};
pub use linear_defect::{
    LinearDefectMemoryEstimate, LinearDefectMemoryEstimateError, LinearDefectParameterSealError,
    LinearDefectPipelineError, LinearDefectPipelineResult,
    STRICT_LINEAR_DEFECT_CORRECTED_ALGORITHM_ID, STRICT_LINEAR_DEFECT_MAP_ALGORITHM_ID,
    STRICT_LINEAR_DEFECT_STAGE_ID, StrictLinearDefectCorrectionRequest,
    estimate_linear_defect_memory, run_strict_linear_defect_correction,
    run_strict_linear_defect_correction_with_progress, strict_linear_defect_parameters_sha256,
};
pub use local_normalization::{
    LocalNormalizationCellDiagnostic, LocalNormalizationControlPoint,
    LocalNormalizationMemoryEstimate, LocalNormalizationPipelineError, LocalNormalizationRequest,
    LocalNormalizationRequestError, LocalNormalizationResult, estimate_local_normalization_memory,
    estimate_local_normalization_peak_bytes, run_local_normalization,
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
    BALANCED_PSF_WEIGHT_ALGORITHM_ID, GENERALIZED_ESD_CLIPPED_MEAN_ALGORITHM_ID,
    GENERALIZED_ESD_REJECTION_MAP_ALGORITHM_ID, GeneralizedEsdParameters,
    LINEAR_FIT_CLIPPED_MEAN_ALGORITHM_ID, LINEAR_FIT_REJECTION_MAP_ALGORITHM_ID,
    LargeScaleRejectionParameters, LargeScaleTailParameters, LinearFitClipParameters,
    PERCENTILE_REJECTION_MAP_ALGORITHM_ID, PercentileClipParameters, QualityWeightMetrics,
    REGISTERED_CROP_MEAN_ALGORITHM_ID, REGISTERED_GENERALIZED_ESD_MEAN_ALGORITHM_ID,
    REGISTERED_LINEAR_FIT_CLIPPED_MEAN_ALGORITHM_ID, REGISTERED_MEDIAN_ALGORITHM_ID,
    REGISTERED_PERCENTILE_CLIPPED_MEAN_ALGORITHM_ID, REGISTERED_SIGMA_CLIPPED_MEAN_ALGORITHM_ID,
    REGISTERED_SPATIAL_ESD_MEAN_ALGORITHM_ID, REGISTERED_WEIGHTED_MEAN_ALGORITHM_ID,
    REGISTERED_WINSORIZED_SIGMA_CLIPPED_MEAN_ALGORITHM_ID, RegisteredFrameQuality,
    RegisteredFrameWeight, RegisteredRejectionMapOutput, RegisteredRejectionMapSummary,
    RegisteredSourceDispositionSummary, RegisteredSpatialEsdParameterError,
    RegisteredSpatialEsdParameters, RegisteredStackError, RegisteredStackEstimator,
    RegisteredStackRequest, RegisteredStackResult, RegisteredStackSource, RegisteredWeightSet,
    SIGMA_CLIPPED_MEAN_ALGORITHM_ID, SIGMA_REJECTION_MAP_ALGORITHM_ID,
    SPATIAL_ESD_REJECTION_MAP_ALGORITHM_ID, SigmaClipParameters, SourceDispositionCounts,
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
