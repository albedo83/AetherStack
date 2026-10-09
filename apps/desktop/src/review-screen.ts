import type {
  DefectCorrectionSettings,
  LinearDefectSettings,
  LightCalibrationProduct,
  LightMasterAssociation,
  MasterPlanSettings,
  MasterProductPlan,
} from "./calibration-bridge.ts";
import type {
  DrizzleExecutionSettings,
  RegisteredStackIntegrationSettings,
  RegisteredStackReportProductInspection,
  RegisteredStackSourceVerification,
} from "./registration-bridge.ts";
import { buildQualityWeightPreflight } from "./quality-weight.ts";
import {
  localNormalizationMarkup,
  mountLocalNormalizationPanel,
} from "./local-normalization-screen.ts";
import type {
  FrameSelectionFrameResult,
  FrameSelectionMetric,
  FrameSelectionPlan,
  FrameSelectionRule,
} from "./selection-bridge.ts";
import type {
  ReviewActions,
  ReviewFrame,
  ReviewRejectionReason,
  ReviewState,
  ReviewViewModel,
  SortDirection,
  SortField,
} from "./model.ts";
import { defectReportVerdict } from "./defect-batch.ts";
import {
  renderWorkflowCoach,
  renderWorkflowOverview,
  workflowCoachMarkup,
  workflowOverviewMarkup,
} from "./workflow-overview.ts";
import { mountUiPreferences, uiPreferencesMarkup } from "./ui-preferences.ts";

export interface ReviewScreen {
  readonly update: (model: ReviewViewModel) => void;
  readonly setSessionDropState: (
    state: "hidden" | "ready" | "blocked",
    message?: string,
  ) => void;
  readonly destroy: () => void;
}

const rejectionReasons = [
  ["blur", "Blur"],
  ["trailing", "Trailing"],
  ["cloud", "Cloud / transparency"],
  ["intrusive_trail", "Satellite or aircraft trail"],
  ["gradient", "Gradient"],
  ["framing", "Framing"],
  ["saturation", "Saturation"],
] as const;

/**
 * Mounts the review workspace as a pure presenter. Scientific decisions,
 * sorting, and Blink advancement are requested through callbacks and must be
 * confirmed by a subsequent model update from the Rust backend.
 */
export function mountReviewScreen(
  root: HTMLElement,
  initialModel: ReviewViewModel,
  actions: ReviewActions,
): ReviewScreen {
  root.innerHTML = shellMarkup();

  const elements = {
    workspaceNavigation: requiredAll<HTMLElement>(root, "[data-workspace]"),
    diagnosticsButton: required<HTMLButtonElement>(
      root,
      '[data-action="open-diagnostics"]',
    ),
    diagnosticsDialog: required<HTMLElement>(root, "[data-diagnostics-dialog]"),
    diagnosticsState: required<HTMLElement>(root, "[data-diagnostics-state]"),
    diagnosticsFiles: required<HTMLElement>(root, "[data-diagnostics-files]"),
    diagnosticsBytes: required<HTMLElement>(root, "[data-diagnostics-bytes]"),
    diagnosticsScanTime: required<HTMLElement>(
      root,
      "[data-diagnostics-scan-time]",
    ),
    diagnosticsWorkers: required<HTMLElement>(
      root,
      "[data-diagnostics-workers]",
    ),
    diagnosticsFrames: required<HTMLElement>(root, "[data-diagnostics-frames]"),
    diagnosticsConflicts: required<HTMLElement>(
      root,
      "[data-diagnostics-conflicts]",
    ),
    diagnosticsFailures: required<HTMLElement>(
      root,
      "[data-diagnostics-failures]",
    ),
    diagnosticsUnassigned: required<HTMLElement>(
      root,
      "[data-diagnostics-unassigned]",
    ),
    diagnosticsRestored: required<HTMLElement>(
      root,
      "[data-diagnostics-restored]",
    ),
    diagnosticsMissing: required<HTMLElement>(
      root,
      "[data-diagnostics-missing]",
    ),
    diagnosticsRejected: required<HTMLElement>(
      root,
      "[data-diagnostics-rejected]",
    ),
    diagnosticsItems: required<HTMLOListElement>(
      root,
      "[data-diagnostics-items]",
    ),
    diagnosticsEmpty: required<HTMLElement>(root, "[data-diagnostics-empty]"),
    diagnosticsOmitted: required<HTMLElement>(
      root,
      "[data-diagnostics-omitted]",
    ),
    diagnosticsSearch: required<HTMLInputElement>(
      root,
      "[data-diagnostics-search]",
    ),
    diagnosticsFilters: requiredAll<HTMLButtonElement>(
      root,
      "[data-diagnostics-filter]",
    ),
    diagnosticsEvidenceSummary: required<HTMLElement>(
      root,
      "[data-diagnostics-evidence-summary]",
    ),
    diagnosticsExport: required<HTMLButtonElement>(
      root,
      '[data-action="export-diagnostics"]',
    ),
    diagnosticsExportStatus: required<HTMLElement>(
      root,
      "[data-diagnostics-export-status]",
    ),
    diagnosticsInspect: required<HTMLButtonElement>(
      root,
      '[data-action="inspect-diagnostics-report"]',
    ),
    diagnosticsInspectionStatus: required<HTMLElement>(
      root,
      "[data-diagnostics-inspection-status]",
    ),
    diagnosticsMaintenance: required<HTMLButtonElement>(
      root,
      '[data-action="preview-cache-maintenance"]',
    ),
    diagnosticsMaintenanceStatus: required<HTMLElement>(
      root,
      "[data-diagnostics-maintenance-status]",
    ),
    diagnosticsMaintenanceFacts: required<HTMLElement>(
      root,
      "[data-diagnostics-maintenance-facts]",
    ),
    diagnosticsMaintenanceRemove: required<HTMLButtonElement>(
      root,
      '[data-action="open-cache-maintenance-confirmation"]',
    ),
    cacheMaintenanceDialog: required<HTMLElement>(
      root,
      "[data-cache-maintenance-dialog]",
    ),
    cacheMaintenanceSummary: required<HTMLElement>(
      root,
      "[data-cache-maintenance-summary]",
    ),
    cacheMaintenanceSeal: required<HTMLElement>(
      root,
      "[data-cache-maintenance-seal]",
    ),
    framesWorkspace: required<HTMLElement>(root, "[data-frames-workspace]"),
    calibrationWorkspace: required<HTMLElement>(
      root,
      "[data-calibration-workspace]",
    ),
    registrationWorkspace: required<HTMLElement>(
      root,
      "[data-registration-workspace]",
    ),
    normalizationWorkspace: required<HTMLElement>(
      root,
      "[data-normalization-workspace]",
    ),
    runWorkspace: required<HTMLElement>(root, "[data-run-workspace]"),
    resultsWorkspace: required<HTMLElement>(root, "[data-results-workspace]"),
    settingsWorkspace: required<HTMLElement>(root, "[data-settings-workspace]"),
    workflowCoach: required<HTMLElement>(root, "[data-workflow-coach]"),
    registrationReference: required<HTMLSelectElement>(
      root,
      "[data-registration-reference]",
    ),
    registrationSource: required<HTMLSelectElement>(
      root,
      "[data-registration-source]",
    ),
    registrationGeometryModels: requiredAll<HTMLButtonElement>(
      root,
      "[data-registration-geometry-model]",
    ),
    analyzeRegistration: required<HTMLButtonElement>(
      root,
      '[data-action="analyze-registration"]',
    ),
    executeRegistration: required<HTMLButtonElement>(
      root,
      '[data-action="execute-registration"]',
    ),
    cancelRegistration: required<HTMLButtonElement>(
      root,
      '[data-action="cancel-registration"]',
    ),
    drizzle: required<HTMLElement>(root, "[data-drizzle]"),
    executeDrizzle: required<HTMLButtonElement>(
      root,
      '[data-action="execute-drizzle"]',
    ),
    cancelDrizzle: required<HTMLButtonElement>(
      root,
      '[data-action="cancel-drizzle"]',
    ),
    drizzleScaleButtons: requiredAll<HTMLButtonElement>(
      root,
      "[data-drizzle-scale]",
    ),
    drizzleWeightingButtons: requiredAll<HTMLButtonElement>(
      root,
      "[data-drizzle-weighting]",
    ),
    drizzleWeightStatus: required<HTMLElement>(
      root,
      "[data-drizzle-weight-status]",
    ),
    drizzleDropShrink: required<HTMLInputElement>(
      root,
      "[data-drizzle-drop-shrink]",
    ),
    drizzleDropShrinkValue: required<HTMLOutputElement>(
      root,
      "[data-drizzle-drop-shrink-value]",
    ),
    drizzleMaximumContributions: required<HTMLInputElement>(
      root,
      "[data-drizzle-maximum-contributions]",
    ),
    drizzleMaximumContributionsValue: required<HTMLOutputElement>(
      root,
      "[data-drizzle-maximum-contributions-value]",
    ),
    drizzleMaximumBandHeight: required<HTMLInputElement>(
      root,
      "[data-drizzle-maximum-band-height]",
    ),
    drizzleMaximumBandHeightValue: required<HTMLOutputElement>(
      root,
      "[data-drizzle-maximum-band-height-value]",
    ),
    drizzleMessage: required<HTMLElement>(root, "[data-drizzle-message]"),
    drizzleProgress: required<HTMLProgressElement>(
      root,
      "[data-drizzle-progress]",
    ),
    drizzleOutput: required<HTMLElement>(root, "[data-drizzle-output]"),
    drizzleProducts: requiredAll<HTMLButtonElement>(
      root,
      '[data-action="select-drizzle-product"]',
    ),
    drizzlePreviewImage: required<HTMLImageElement>(
      root,
      "[data-drizzle-preview-image]",
    ),
    drizzlePreviewPlaceholder: required<HTMLElement>(
      root,
      "[data-drizzle-preview-placeholder]",
    ),
    inspectDrizzleStatistics: required<HTMLButtonElement>(
      root,
      '[data-action="inspect-drizzle-statistics"]',
    ),
    drizzlePixelX: required<HTMLInputElement>(root, "[data-drizzle-pixel-x]"),
    drizzlePixelY: required<HTMLInputElement>(root, "[data-drizzle-pixel-y]"),
    inspectDrizzlePixel: required<HTMLButtonElement>(
      root,
      '[data-action="inspect-drizzle-pixel"]',
    ),
    drizzlePixelReadout: required<HTMLElement>(
      root,
      "[data-drizzle-pixel-readout]",
    ),
    registrationStatus: required<HTMLElement>(
      root,
      "[data-registration-status]",
    ),
    registrationRms: required<HTMLElement>(root, "[data-registration-rms]"),
    registrationInliers: required<HTMLElement>(
      root,
      "[data-registration-inliers]",
    ),
    registrationCoverage: required<HTMLElement>(
      root,
      "[data-registration-coverage]",
    ),
    registrationRotation: required<HTMLElement>(
      root,
      "[data-registration-rotation]",
    ),
    registrationScale: required<HTMLElement>(root, "[data-registration-scale]"),
    registrationReflection: required<HTMLElement>(
      root,
      "[data-registration-reflection]",
    ),
    registrationSupport: required<HTMLElement>(
      root,
      "[data-registration-support]",
    ),
    registrationCropText: required<HTMLElement>(
      root,
      "[data-registration-crop-text]",
    ),
    registrationCrop: required<HTMLElement>(root, "[data-registration-crop]"),
    registrationMatrix: required<HTMLElement>(
      root,
      "[data-registration-matrix]",
    ),
    registrationRejections: required<HTMLElement>(
      root,
      "[data-registration-rejections]",
    ),
    registrationPlanProgress: required<HTMLElement>(
      root,
      "[data-registration-plan-progress]",
    ),
    registrationPlanFrames: required<HTMLOListElement>(
      root,
      "[data-registration-plan-frames]",
    ),
    registrationPlanDigest: required<HTMLElement>(
      root,
      "[data-registration-plan-digest]",
    ),
    registrationExecution: required<HTMLElement>(
      root,
      "[data-registration-execution]",
    ),
    registrationExecutionMessage: required<HTMLElement>(
      root,
      "[data-registration-execution-message]",
    ),
    registrationExecutionProgress: required<HTMLProgressElement>(
      root,
      "[data-registration-execution-progress]",
    ),
    registrationExecutionOutput: required<HTMLElement>(
      root,
      "[data-registration-execution-output]",
    ),
    executeRegisteredStack: required<HTMLButtonElement>(
      root,
      '[data-action="execute-registered-stack"]',
    ),
    cancelRegisteredStack: required<HTMLButtonElement>(
      root,
      '[data-action="cancel-registered-stack"]',
    ),
    registeredStack: required<HTMLElement>(root, "[data-registered-stack]"),
    registeredStackMessage: required<HTMLElement>(
      root,
      "[data-registered-stack-message]",
    ),
    registeredStackProgress: required<HTMLProgressElement>(
      root,
      "[data-registered-stack-progress]",
    ),
    registeredStackOutput: required<HTMLElement>(
      root,
      "[data-registered-stack-output]",
    ),
    registeredStackSourceRejections: required<HTMLElement>(
      root,
      "[data-registered-stack-source-rejections]",
    ),
    registeredStackSourceRejectionSummary: required<HTMLElement>(
      root,
      "[data-registered-stack-source-rejection-summary]",
    ),
    registeredStackSourceRejectionRows: required<HTMLElement>(
      root,
      "[data-registered-stack-source-rejection-rows]",
    ),
    registeredStackSpatialPromotions: required<HTMLElement>(
      root,
      "[data-registered-stack-spatial-promotions]",
    ),
    registeredStackSpatialPromotionTotal: required<HTMLElement>(
      root,
      "[data-registered-stack-spatial-promotion-total]",
    ),
    registeredStackSpatialPromotionBreakdown: required<HTMLElement>(
      root,
      "[data-registered-stack-spatial-promotion-breakdown]",
    ),
    registeredStackReport: required<HTMLElement>(
      root,
      "[data-registered-stack-report]",
    ),
    registeredStackReportPath: required<HTMLElement>(
      root,
      "[data-registered-stack-report-path]",
    ),
    registeredStackReportDigest: required<HTMLElement>(
      root,
      "[data-registered-stack-report-digest]",
    ),
    inspectRegisteredStackReport: required<HTMLButtonElement>(
      root,
      '[data-action="inspect-registered-stack-report"]',
    ),
    registeredStackReportSummary: required<HTMLElement>(
      root,
      "[data-registered-stack-report-summary]",
    ),
    registeredStackReportProducts: required<HTMLElement>(
      root,
      "[data-registered-stack-report-products]",
    ),
    registeredStackReportSources: required<HTMLElement>(
      root,
      "[data-registered-stack-report-sources]",
    ),
    registeredStackSourceSummary: required<HTMLElement>(
      root,
      "[data-registered-stack-source-summary]",
    ),
    registeredStackSourceSearch: required<HTMLInputElement>(
      root,
      "[data-registered-stack-source-search]",
    ),
    clearRegisteredStackSourceSearch: required<HTMLButtonElement>(
      root,
      '[data-action="clear-source-evidence-search"]',
    ),
    showMoreRegisteredStackSources: required<HTMLButtonElement>(
      root,
      '[data-action="show-more-source-evidence"]',
    ),
    registeredStackSourceFilters: requiredAll<HTMLButtonElement>(
      root,
      "[data-source-evidence-filter]",
    ),
    verifyRegisteredStackSources: required<HTMLButtonElement>(
      root,
      '[data-action="verify-registered-stack-sources"]',
    ),
    cancelRegisteredStackSourceVerification: required<HTMLButtonElement>(
      root,
      '[data-action="cancel-registered-stack-source-verification"]',
    ),
    registeredStackSourceProgress: required<HTMLProgressElement>(
      root,
      "[data-registered-stack-source-progress]",
    ),
    registeredStackSourceVerification: required<HTMLElement>(
      root,
      "[data-registered-stack-source-verification]",
    ),
    returnToActiveStack: required<HTMLButtonElement>(
      root,
      '[data-action="return-to-active-stack"]',
    ),
    openRegisteredStackReport: required<HTMLButtonElement>(
      root,
      '[data-action="open-registered-stack-report"]',
    ),
    registeredStackModes: requiredAll<HTMLButtonElement>(
      root,
      '[data-action="select-registered-stack-mode"]',
    ),
    registeredStackAutomaticPlan: required<HTMLElement>(
      root,
      "[data-registered-stack-automatic-plan]",
    ),
    registeredStackAutomaticState: required<HTMLElement>(
      root,
      "[data-registered-stack-automatic-state]",
    ),
    registeredStackAutomaticTier: required<HTMLElement>(
      root,
      "[data-registered-stack-automatic-tier]",
    ),
    registeredStackAutomaticEstimator: required<HTMLElement>(
      root,
      "[data-registered-stack-automatic-estimator]",
    ),
    registeredStackAutomaticRationale: required<HTMLElement>(
      root,
      "[data-registered-stack-automatic-rationale]",
    ),
    registeredStackAutomaticSeal: required<HTMLElement>(
      root,
      "[data-registered-stack-automatic-seal]",
    ),
    registeredStackAdvanced: required<HTMLDetailsElement>(
      root,
      ".registered-stack__advanced",
    ),
    registeredStackEstimator: required<HTMLSelectElement>(
      root,
      "[data-registered-stack-estimator]",
    ),
    registeredStackLowFraction: required<HTMLInputElement>(
      root,
      "[data-registered-stack-low-fraction]",
    ),
    registeredStackHighFraction: required<HTMLInputElement>(
      root,
      "[data-registered-stack-high-fraction]",
    ),
    registeredStackLowSigma: required<HTMLInputElement>(
      root,
      "[data-registered-stack-low-sigma]",
    ),
    registeredStackHighSigma: required<HTMLInputElement>(
      root,
      "[data-registered-stack-high-sigma]",
    ),
    registeredStackEsdOutlierFraction: required<HTMLInputElement>(
      root,
      "[data-registered-stack-esd-outlier-fraction]",
    ),
    registeredStackEsdSignificance: required<HTMLInputElement>(
      root,
      "[data-registered-stack-esd-significance]",
    ),
    registeredStackMaximumIterations: required<HTMLInputElement>(
      root,
      "[data-registered-stack-maximum-iterations]",
    ),
    registeredStackLargeScale: required<HTMLElement>(
      root,
      "[data-registered-stack-large-scale]",
    ),
    registeredStackLargeScaleLowEnabled: required<HTMLInputElement>(
      root,
      "[data-registered-stack-large-scale-low-enabled]",
    ),
    registeredStackLargeScaleHighEnabled: required<HTMLInputElement>(
      root,
      "[data-registered-stack-large-scale-high-enabled]",
    ),
    registeredStackLargeScaleLowLayers: required<HTMLInputElement>(
      root,
      "[data-registered-stack-large-scale-low-layers]",
    ),
    registeredStackLargeScaleHighLayers: required<HTMLInputElement>(
      root,
      "[data-registered-stack-large-scale-high-layers]",
    ),
    registeredStackLargeScaleLowGrowth: required<HTMLInputElement>(
      root,
      "[data-registered-stack-large-scale-low-growth]",
    ),
    registeredStackLargeScaleHighGrowth: required<HTMLInputElement>(
      root,
      "[data-registered-stack-large-scale-high-growth]",
    ),
    registeredStackMinimumRetained: required<HTMLInputElement>(
      root,
      "[data-registered-stack-minimum-retained]",
    ),
    registeredStackRejectionMaps: required<HTMLInputElement>(
      root,
      "[data-registered-stack-rejection-maps]",
    ),
    registeredStackSupportMap: required<HTMLInputElement>(
      root,
      "[data-registered-stack-support-map]",
    ),
    registeredStackEstimatorLabel: required<HTMLElement>(
      root,
      "[data-registered-stack-estimator-label]",
    ),
    registeredStackWeightPreflight: required<HTMLElement>(
      root,
      "[data-registered-stack-weight-preflight]",
    ),
    registeredStackWeightStatus: required<HTMLElement>(
      root,
      "[data-registered-stack-weight-status]",
    ),
    registeredStackWeightSeal: required<HTMLElement>(
      root,
      "[data-registered-stack-weight-seal]",
    ),
    registeredStackWeightAlgorithm: required<HTMLElement>(
      root,
      "[data-registered-stack-weight-algorithm]",
    ),
    registeredStackWeightDigest: required<HTMLElement>(
      root,
      "[data-registered-stack-weight-digest]",
    ),
    registeredStackWeightReference: required<HTMLSelectElement>(
      root,
      "[data-registered-stack-weight-reference]",
    ),
    registeredStackWeightRows: required<HTMLTableSectionElement>(
      root,
      "[data-registered-stack-weight-rows]",
    ),
    registeredStackProducts: requiredAll<HTMLButtonElement>(
      root,
      '[data-action="select-registered-stack-product"]',
    ),
    registeredStackPreviewImage: required<HTMLImageElement>(
      root,
      "[data-registered-stack-preview-image]",
    ),
    registeredStackScienceImage: required<HTMLImageElement>(
      root,
      "[data-registered-stack-science-image]",
    ),
    registeredStackOverlayControl: required<HTMLElement>(
      root,
      "[data-registered-stack-overlay-control]",
    ),
    registeredStackOverlayOpacity: required<HTMLInputElement>(
      root,
      "[data-registered-stack-overlay-opacity]",
    ),
    registeredStackOverlayValue: required<HTMLOutputElement>(
      root,
      "[data-registered-stack-overlay-value]",
    ),
    registeredStackHistogram: required<HTMLElement>(
      root,
      "[data-registered-stack-histogram]",
    ),
    registeredStackHistogramSummary: required<HTMLElement>(
      root,
      "[data-registered-stack-histogram-summary]",
    ),
    registeredStackHistogramBins: required<HTMLElement>(
      root,
      "[data-registered-stack-histogram-bins]",
    ),
    registeredStackPixelReadout: required<HTMLElement>(
      root,
      "[data-registered-stack-pixel-readout]",
    ),
    registeredStackPixelX: required<HTMLInputElement>(
      root,
      "[data-registered-stack-pixel-x]",
    ),
    registeredStackPixelY: required<HTMLInputElement>(
      root,
      "[data-registered-stack-pixel-y]",
    ),
    inspectRegisteredStackPixel: required<HTMLButtonElement>(
      root,
      '[data-action="inspect-registered-stack-coordinate"]',
    ),
    registeredStackPreviewPlaceholder: required<HTMLElement>(
      root,
      "[data-registered-stack-preview-placeholder]",
    ),
    registeredFrame: required<HTMLSelectElement>(
      root,
      "[data-registered-frame]",
    ),
    registeredPreviewImage: required<HTMLImageElement>(
      root,
      "[data-registered-preview-image]",
    ),
    registeredPreviewPlaceholder: required<HTMLElement>(
      root,
      "[data-registered-preview-placeholder]",
    ),
    registeredPreviewMessage: required<HTMLElement>(
      root,
      "[data-registered-preview-message]",
    ),
    registeredPreviewPosition: required<HTMLElement>(
      root,
      "[data-registered-preview-position]",
    ),
    registeredPreviewStretch: required<HTMLElement>(
      root,
      "[data-registered-preview-stretch]",
    ),
    registeredPreviewPlay: required<HTMLButtonElement>(
      root,
      '[data-action="toggle-registered-play"]',
    ),
    calibrationStatus: required<HTMLElement>(root, "[data-calibration-status]"),
    calibrationProducts: required<HTMLElement>(
      root,
      "[data-calibration-products]",
    ),
    calibrationDigest: required<HTMLElement>(root, "[data-calibration-digest]"),
    pedestalPolicy: required<HTMLSelectElement>(root, "[data-pedestal-policy]"),
    exposureTolerance: required<HTMLInputElement>(
      root,
      "[data-exposure-tolerance]",
    ),
    temperatureTolerance: required<HTMLInputElement>(
      root,
      "[data-temperature-tolerance]",
    ),
    lightTemperatureTolerance: required<HTMLInputElement>(
      root,
      "[data-light-temperature-tolerance]",
    ),
    lightAssociations: required<HTMLElement>(root, "[data-light-associations]"),
    lightDigest: required<HTMLElement>(root, "[data-light-digest]"),
    lightOutputMode: required<HTMLSelectElement>(
      root,
      "[data-light-output-mode]",
    ),
    refreshMasterPlan: required<HTMLButtonElement>(
      root,
      '[data-action="refresh-master-plan"]',
    ),
    executeMasterPlan: required<HTMLButtonElement>(
      root,
      '[data-action="execute-master-plan"]',
    ),
    cancelMasterPlan: required<HTMLButtonElement>(
      root,
      '[data-action="cancel-master-plan"]',
    ),
    executeLightPlan: required<HTMLButtonElement>(
      root,
      '[data-action="execute-light-plan"]',
    ),
    cancelLightPlan: required<HTMLButtonElement>(
      root,
      '[data-action="cancel-light-plan"]',
    ),
    masterExecution: required<HTMLElement>(root, "[data-master-execution]"),
    masterExecutionMessage: required<HTMLElement>(
      root,
      "[data-master-execution-message]",
    ),
    masterExecutionProgress: required<HTMLProgressElement>(
      root,
      "[data-master-execution-progress]",
    ),
    masterExecutionOutput: required<HTMLElement>(
      root,
      "[data-master-execution-output]",
    ),
    lightExecution: required<HTMLElement>(root, "[data-light-execution]"),
    lightExecutionMessage: required<HTMLElement>(
      root,
      "[data-light-execution-message]",
    ),
    lightExecutionProgress: required<HTMLProgressElement>(
      root,
      "[data-light-execution-progress]",
    ),
    lightExecutionOutput: required<HTMLElement>(
      root,
      "[data-light-execution-output]",
    ),
    lightExecutionHeading: required<HTMLElement>(
      root,
      "[data-light-execution-heading]",
    ),
    defectCorrection: required<HTMLElement>(root, "[data-defect-correction]"),
    defectCorrectionMessage: required<HTMLElement>(
      root,
      "[data-defect-correction-message]",
    ),
    defectCorrectionProgress: required<HTMLProgressElement>(
      root,
      "[data-defect-correction-progress]",
    ),
    defectCorrectionOutput: required<HTMLElement>(
      root,
      "[data-defect-correction-output]",
    ),
    defectCorrectionEvidence: required<HTMLElement>(
      root,
      "[data-defect-correction-evidence]",
    ),
    defectBatchReport: required<HTMLElement>(
      root,
      "[data-defect-batch-report]",
    ),
    defectReportInspection: required<HTMLElement>(
      root,
      "[data-defect-report-inspection]",
    ),
    executeDefectCorrection: required<HTMLButtonElement>(
      root,
      '[data-action="execute-defect-correction"]',
    ),
    executeAllDefectCorrections: required<HTMLButtonElement>(
      root,
      '[data-action="execute-all-defect-corrections"]',
    ),
    exportDefectBatchReport: required<HTMLButtonElement>(
      root,
      '[data-action="export-defect-batch-report"]',
    ),
    resumeDefectBatch: required<HTMLButtonElement>(
      root,
      '[data-action="resume-defect-batch"]',
    ),
    inspectDefectBatchReport: required<HTMLButtonElement>(
      root,
      '[data-action="inspect-defect-batch-report"]',
    ),
    cancelDefectCorrection: required<HTMLButtonElement>(
      root,
      '[data-action="cancel-defect-correction"]',
    ),
    defectStrideButtons: Array.from(
      root.querySelectorAll<HTMLButtonElement>(
        '[data-action="select-defect-stride"]',
      ),
    ),
    defectDetectionRadius: required<HTMLInputElement>(
      root,
      "[data-defect-detection-radius]",
    ),
    defectDetectionMinimumNeighbours: required<HTMLInputElement>(
      root,
      "[data-defect-detection-minimum-neighbours]",
    ),
    defectDarkHotSigma: required<HTMLInputElement>(
      root,
      "[data-defect-dark-hot-sigma]",
    ),
    defectDarkColdSigma: required<HTMLInputElement>(
      root,
      "[data-defect-dark-cold-sigma]",
    ),
    defectDarkFloor: required<HTMLInputElement>(
      root,
      "[data-defect-dark-floor]",
    ),
    defectFlatHotSigma: required<HTMLInputElement>(
      root,
      "[data-defect-flat-hot-sigma]",
    ),
    defectFlatColdSigma: required<HTMLInputElement>(
      root,
      "[data-defect-flat-cold-sigma]",
    ),
    defectFlatFloor: required<HTMLInputElement>(
      root,
      "[data-defect-flat-floor]",
    ),
    defectCorrectionRadius: required<HTMLInputElement>(
      root,
      "[data-defect-correction-radius]",
    ),
    defectCorrectionMinimumNeighbours: required<HTMLInputElement>(
      root,
      "[data-defect-correction-minimum-neighbours]",
    ),
    linearDefect: required<HTMLElement>(root, "[data-linear-defect]"),
    linearDefectMessage: required<HTMLElement>(
      root,
      "[data-linear-defect-message]",
    ),
    linearDefectProgress: required<HTMLProgressElement>(
      root,
      "[data-linear-defect-progress]",
    ),
    linearDefectEvidence: required<HTMLElement>(
      root,
      "[data-linear-defect-evidence]",
    ),
    linearDefectOutput: required<HTMLElement>(
      root,
      "[data-linear-defect-output]",
    ),
    linearDefectAxisButtons: Array.from(
      root.querySelectorAll<HTMLButtonElement>(
        '[data-action="select-linear-defect-axis"]',
      ),
    ),
    linearDefectInputs: Array.from(
      root.querySelectorAll<HTMLInputElement>("[data-linear-defect-setting]"),
    ),
    executeLinearDefectCorrection: required<HTMLButtonElement>(
      root,
      '[data-action="execute-linear-defect-correction"]',
    ),
    cancelLinearDefectCorrection: required<HTMLButtonElement>(
      root,
      '[data-action="cancel-linear-defect-correction"]',
    ),
    defectPreviewButtons: Array.from(
      root.querySelectorAll<HTMLButtonElement>(
        '[data-action="select-defect-preview"]',
      ),
    ),
    defectPreviewImage: required<HTMLImageElement>(
      root,
      "[data-defect-preview-image]",
    ),
    defectPreviewPlaceholder: required<HTMLElement>(
      root,
      "[data-defect-preview-placeholder]",
    ),
    defectPreviewMessage: required<HTMLElement>(
      root,
      "[data-defect-preview-message]",
    ),
    sessionName: required<HTMLElement>(root, "[data-session-name]"),
    sessionStatus: required<HTMLElement>(root, "[data-session-status]"),
    sessionStatusLabel: required<HTMLElement>(
      root,
      "[data-session-status-label]",
    ),
    sessionImportProgress: required<HTMLProgressElement>(
      root,
      "[data-session-import-progress]",
    ),
    importSession: required<HTMLButtonElement>(root, "[data-import-session]"),
    reviewPlan: required<HTMLButtonElement>(
      root,
      '[data-action="open-selection-panel"]',
    ),
    undo: required<HTMLButtonElement>(root, '[data-action="undo"]'),
    roleTabs: required<HTMLElement>(root, "[data-role-tabs]"),
    roleHeading: required<HTMLElement>(root, "[data-role-heading]"),
    lightFrameView: required<HTMLElement>(root, "[data-light-frame-view]"),
    rawLightView: required<HTMLButtonElement>(
      root,
      '[data-action="select-light-frame-view"][data-light-frame-view-value="raw"]',
    ),
    calibratedLightView: required<HTMLButtonElement>(
      root,
      '[data-action="select-light-frame-view"][data-light-frame-view-value="calibrated"]',
    ),
    frameTable: required<HTMLTableElement>(root, "[data-frame-table]"),
    frameFilter: required<HTMLInputElement>(root, "[data-frame-filter]"),
    clearFrameFilter: required<HTMLButtonElement>(
      root,
      '[data-action="clear-frame-filter"]',
    ),
    tableBody: required<HTMLTableSectionElement>(root, "[data-frame-rows]"),
    selectionCount: required<HTMLElement>(root, "[data-selection-count]"),
    framePosition: required<HTMLElement>(root, "[data-frame-position]"),
    selectedLabel: required<HTMLElement>(root, "[data-selected-label]"),
    preview: required<HTMLElement>(root, "[data-preview]"),
    previewImage: required<HTMLImageElement>(root, "[data-preview-image]"),
    previewPlaceholder: required<HTMLElement>(
      root,
      "[data-preview-placeholder]",
    ),
    previewBadges: required<HTMLElement>(root, "[data-preview-badges]"),
    previewTitle: required<HTMLElement>(root, "[data-preview-title]"),
    previewDescription: required<HTMLElement>(
      root,
      "[data-preview-description]",
    ),
    fitPreview: required<HTMLButtonElement>(root, '[data-action="viewer-fit"]'),
    actualPreview: required<HTMLButtonElement>(
      root,
      '[data-action="viewer-actual"]',
    ),
    statisticsButton: required<HTMLButtonElement>(
      root,
      '[data-action="open-statistics"]',
    ),
    qualityButton: required<HTMLButtonElement>(
      root,
      '[data-action="measure-quality"]',
    ),
    qualityBatchButton: required<HTMLButtonElement>(
      root,
      '[data-action="measure-all-quality"]',
    ),
    qualityBadge: required<HTMLElement>(root, "[data-quality-badge]"),
    selectionPanel: required<HTMLDetailsElement>(
      root,
      "[data-selection-panel]",
    ),
    selectionRules: required<HTMLElement>(root, "[data-selection-rules]"),
    selectionAddRule: required<HTMLButtonElement>(
      root,
      '[data-action="add-selection-rule"]',
    ),
    selectionPreview: required<HTMLButtonElement>(
      root,
      '[data-action="preview-frame-selection"]',
    ),
    selectionApply: required<HTMLButtonElement>(
      root,
      '[data-action="open-selection-confirmation"]',
    ),
    selectionStatus: required<HTMLElement>(root, "[data-selection-status]"),
    selectionRetained: required<HTMLElement>(root, "[data-selection-retained]"),
    selectionRejected: required<HTMLElement>(root, "[data-selection-rejected]"),
    selectionDigest: required<HTMLElement>(root, "[data-selection-digest]"),
    selectionEvidence: required<HTMLElement>(root, "[data-selection-evidence]"),
    selectionEvidenceFrame: required<HTMLElement>(
      root,
      "[data-selection-evidence-frame]",
    ),
    selectionEvidenceList: required<HTMLOListElement>(
      root,
      "[data-selection-evidence-list]",
    ),
    selectionConfirmation: required<HTMLElement>(
      root,
      "[data-selection-confirmation]",
    ),
    selectionConfirmationCount: required<HTMLElement>(
      root,
      "[data-selection-confirmation-count]",
    ),
    cfaBadge: required<HTMLElement>(root, "[data-cfa-badge]"),
    state: required<HTMLElement>(root, "[data-review-state]"),
    signalToNoise: required<HTMLElement>(root, "[data-metric-signal-to-noise]"),
    fwhm: required<HTMLElement>(root, "[data-metric-fwhm]"),
    eccentricity: required<HTMLElement>(root, "[data-metric-eccentricity]"),
    stars: required<HTMLElement>(root, "[data-metric-stars]"),
    background: required<HTMLElement>(root, "[data-metric-background]"),
    noise: required<HTMLElement>(root, "[data-metric-noise]"),
    stretch: required<HTMLElement>(root, "[data-stretch-label]"),
    play: required<HTMLButtonElement>(root, '[data-action="toggle-play"]'),
    stepButtons: requiredAll<HTMLButtonElement>(root, "[data-step-action]"),
    decisionButtons: requiredAll<HTMLButtonElement>(
      root,
      "[data-decision-action]",
    ),
    clearDecision: required<HTMLButtonElement>(
      root,
      '[data-action="clear-decision"]',
    ),
    accept: required<HTMLButtonElement>(root, '[data-action="accept"]'),
    decisionControls: required<HTMLElement>(root, "[data-decision-controls]"),
    rejectDialog: required<HTMLElement>(root, "[data-reject-dialog]"),
    rejectFrame: required<HTMLElement>(root, "[data-reject-frame]"),
    statisticsDialog: required<HTMLElement>(root, "[data-statistics-dialog]"),
    statisticsFrame: required<HTMLElement>(root, "[data-statistics-frame]"),
    statisticsStatus: required<HTMLElement>(root, "[data-statistics-status]"),
    statisticsContent: required<HTMLElement>(root, "[data-statistics-content]"),
    statisticsAlgorithm: required<HTMLElement>(
      root,
      "[data-statistics-algorithm]",
    ),
    statisticsAxes: required<HTMLElement>(root, "[data-statistics-axes]"),
    statisticsFormat: required<HTMLElement>(root, "[data-statistics-format]"),
    statisticsHeader: required<HTMLElement>(root, "[data-statistics-header]"),
    statisticsUsable: required<HTMLElement>(root, "[data-statistics-usable]"),
    statisticsExcluded: required<HTMLElement>(
      root,
      "[data-statistics-excluded]",
    ),
    statisticsMinimum: required<HTMLElement>(root, "[data-statistics-minimum]"),
    statisticsMaximum: required<HTMLElement>(root, "[data-statistics-maximum]"),
    statisticsMean: required<HTMLElement>(root, "[data-statistics-mean]"),
    statisticsDeviation: required<HTMLElement>(
      root,
      "[data-statistics-deviation]",
    ),
    statisticsSampleDeviation: required<HTMLElement>(
      root,
      "[data-statistics-sample-deviation]",
    ),
    liveRegion: required<HTMLElement>(root, "[data-live-region]"),
  };
  const localNormalizationPanel = mountLocalNormalizationPanel(
    elements.normalizationWorkspace,
    actions,
  );
  const uiPreferences = mountUiPreferences(elements.settingsWorkspace);

  let model = initialModel;
  let pendingRejectFrameId: string | null = null;
  const sortDirections = new Map<SortField, SortDirection>();
  let sourceEvidenceFilter: SourceEvidenceFilter = "all";
  let sourceEvidenceQuery = "";
  let sourceEvidenceLimit = SOURCE_EVIDENCE_PAGE_SIZE;
  let sourceEvidenceReportSha256: string | null = null;
  let diagnosticsFilter: DiagnosticsEvidenceFilter = "all";
  let diagnosticsQuery = "";
  let frameQuery = "";

  const onClick = (event: MouseEvent): void => {
    const target = event.target instanceof Element ? event.target : null;
    const actionElement = target?.closest<HTMLElement>("[data-action]");
    if (!actionElement) return;
    const action = actionElement.dataset.action;

    if (action === "inspect-registered-stack-report") {
      actions.onInspectRegisteredStackReport();
      return;
    }

    if (action === "open-registered-stack-report") {
      actions.onOpenRegisteredStackReport();
      return;
    }
    if (action === "return-to-active-stack") {
      actions.onReturnToActiveStack();
      return;
    }
    if (action === "verify-registered-stack-sources") {
      actions.onVerifyRegisteredStackSources();
      return;
    }
    if (action === "cancel-registered-stack-source-verification") {
      actions.onCancelRegisteredStackSourceVerification();
      return;
    }
    if (action === "filter-source-evidence") {
      const filter = actionElement.dataset.sourceEvidenceFilter;
      if (isSourceEvidenceFilter(filter)) {
        sourceEvidenceFilter = filter;
        sourceEvidenceLimit = SOURCE_EVIDENCE_PAGE_SIZE;
        renderRegistration(
          elements,
          model,
          sourceEvidenceFilter,
          sourceEvidenceQuery,
          sourceEvidenceLimit,
        );
      }
      return;
    }
    if (action === "clear-source-evidence-search") {
      sourceEvidenceQuery = "";
      sourceEvidenceLimit = SOURCE_EVIDENCE_PAGE_SIZE;
      renderRegistration(
        elements,
        model,
        sourceEvidenceFilter,
        sourceEvidenceQuery,
        sourceEvidenceLimit,
      );
      elements.registeredStackSourceSearch.focus();
      return;
    }
    if (action === "show-more-source-evidence") {
      sourceEvidenceLimit = Math.min(
        Number.MAX_SAFE_INTEGER,
        sourceEvidenceLimit + SOURCE_EVIDENCE_PAGE_SIZE,
      );
      renderRegistration(
        elements,
        model,
        sourceEvidenceFilter,
        sourceEvidenceQuery,
        sourceEvidenceLimit,
      );
      return;
    }

    if (action === "inspect-registered-stack-coordinate") {
      const x = elements.registeredStackPixelX.valueAsNumber;
      const y = elements.registeredStackPixelY.valueAsNumber;
      if (Number.isInteger(x) && Number.isInteger(y)) {
        actions.onInspectRegisteredStackPixel(x, y);
      }
      return;
    }

    if (action === "inspect-registered-stack-pixel") {
      const result = model.registration.stack.result;
      if (!result || model.registration.stack.previewState !== "ready") return;
      const bounds = actionElement.getBoundingClientRect();
      const imageAspect = result.width / result.height;
      const viewportAspect = bounds.width / bounds.height;
      const displayedWidth =
        viewportAspect > imageAspect
          ? bounds.height * imageAspect
          : bounds.width;
      const displayedHeight =
        viewportAspect > imageAspect
          ? bounds.height
          : bounds.width / imageAspect;
      const left = bounds.left + (bounds.width - displayedWidth) / 2;
      const top = bounds.top + (bounds.height - displayedHeight) / 2;
      const localX = event.clientX - left;
      const localY = event.clientY - top;
      if (
        localX < 0 ||
        localY < 0 ||
        localX >= displayedWidth ||
        localY >= displayedHeight
      ) {
        return;
      }
      actions.onInspectRegisteredStackPixel(
        Math.min(
          result.width - 1,
          Math.floor((localX / displayedWidth) * result.width),
        ),
        Math.min(
          result.height - 1,
          Math.floor((localY / displayedHeight) * result.height),
        ),
      );
      return;
    }

    if (action === "step-number") {
      const stepper = actionElement.closest<HTMLElement>("[data-stepper]");
      const input = stepper?.querySelector<HTMLInputElement>(
        'input[type="number"]',
      );
      if (!input || input.disabled) return;
      if (actionElement.dataset.stepDirection === "down") {
        input.stepDown();
      } else {
        input.stepUp();
      }
      input.dispatchEvent(new Event("change", { bubbles: true }));
      input.focus();
      return;
    }

    if (action === "select-workspace") {
      const workspace = actionElement.dataset.workspace;
      if (
        workspace === "frames" ||
        workspace === "calibration" ||
        workspace === "registration" ||
        workspace === "normalization" ||
        workspace === "run" ||
        workspace === "results" ||
        workspace === "settings"
      ) {
        actions.onSelectWorkspace(workspace);
      }
      return;
    }
    if (action === "reveal-artifact") {
      const path = actionElement.dataset.artifactPath;
      if (!path) return;
      const kind = actionElement.dataset.artifactKind ?? "artifact";
      const status = required<HTMLElement>(
        root,
        "[data-results-reveal-status]",
      );
      actionElement.setAttribute("aria-busy", "true");
      status.textContent = `Locating ${kind}…`;
      void actions
        .onRevealArtifact(path)
        .then(() => {
          status.textContent = `${kind} revealed in the system file manager.`;
        })
        .catch(() => {
          status.textContent = `Could not reveal ${kind}. The published path remains available above.`;
        })
        .finally(() => {
          actionElement.removeAttribute("aria-busy");
        });
      return;
    }
    if (action === "clear-frame-filter") {
      frameQuery = "";
      elements.frameFilter.value = "";
      renderRows(elements.tableBody, model, frameQuery);
      updateFrameListSummary(elements, model, frameQuery);
      elements.frameFilter.focus();
      return;
    }
    if (action === "open-selection-panel") {
      actions.onSelectWorkspace("frames");
      actions.onSelectRole("light");
      elements.selectionPanel.open = true;
      queueMicrotask(() => {
        elements.selectionPanel.scrollIntoView?.({ block: "nearest" });
        elements.selectionPanel.querySelector<HTMLElement>("summary")?.focus();
      });
      return;
    }
    if (action === "refresh-master-plan") {
      actions.onRefreshMasterPlan();
      return;
    }
    if (action === "analyze-registration") {
      actions.onAnalyzeRegistration();
      return;
    }
    if (action === "select-registration-geometry") {
      const geometryModel = actionElement.dataset.registrationGeometryModel;
      if (geometryModel === "affine" || geometryModel === "projective") {
        actions.onSelectRegistrationGeometryModel(geometryModel);
      }
      return;
    }
    if (action === "execute-registration") {
      actions.onExecuteRegistration();
      return;
    }
    if (action === "cancel-registration") {
      actions.onCancelRegistration();
      return;
    }
    if (action === "execute-drizzle") {
      actions.onExecuteDrizzle();
      return;
    }
    if (action === "cancel-drizzle") {
      actions.onCancelDrizzle();
      return;
    }
    if (action === "select-drizzle-scale") {
      const scale = Number(actionElement.dataset.drizzleScale);
      if (Number.isSafeInteger(scale) && scale >= 1 && scale <= 3) {
        actions.onUpdateDrizzleSettings({
          ...model.registration.drizzle.settings,
          scale,
        });
      }
      return;
    }
    if (action === "select-drizzle-product") {
      const product = actionElement.dataset.drizzleProduct;
      if (
        product === "science" ||
        product === "weight" ||
        product === "support"
      ) {
        actions.onSelectDrizzleProduct(product);
      }
      return;
    }
    if (action === "select-drizzle-weighting") {
      const weighting = actionElement.dataset.drizzleWeighting;
      if (weighting === "uniform" || weighting === "balanced_psf") {
        actions.onSelectDrizzleWeighting(weighting);
      }
      return;
    }
    if (action === "inspect-drizzle-statistics") {
      actions.onInspectDrizzleStatistics();
      return;
    }
    if (action === "inspect-drizzle-pixel") {
      actions.onInspectDrizzlePixel(
        elements.drizzlePixelX.valueAsNumber,
        elements.drizzlePixelY.valueAsNumber,
      );
      return;
    }
    if (action === "execute-registered-stack") {
      actions.onExecuteRegisteredStack();
      return;
    }
    if (action === "select-registered-stack-mode") {
      const mode = actionElement.dataset.stackMode;
      if (mode === "manual" || mode === "automatic") {
        actions.onSetRegisteredStackMode(mode);
      }
      return;
    }
    if (action === "cancel-registered-stack") {
      actions.onCancelRegisteredStack();
      return;
    }
    if (action === "select-registered-stack-product") {
      const product = actionElement.dataset.stackProduct;
      if (
        product === "science" ||
        product === "rejection_low" ||
        product === "rejection_high" ||
        product === "support"
      ) {
        actions.onSelectRegisteredStackProduct(product);
      }
      return;
    }
    if (action === "toggle-registered-play") {
      actions.onSetRegisteredPlaying(!model.registration.resultReview.playing);
      return;
    }
    if (action === "previous-registered") {
      actions.onStepRegisteredFrame("backward");
      return;
    }
    if (action === "next-registered") {
      actions.onStepRegisteredFrame("forward");
      return;
    }
    if (action === "execute-master-plan") {
      actions.onExecuteMasterPlan();
      return;
    }
    if (action === "cancel-master-plan") {
      actions.onCancelMasterPlan();
      return;
    }
    if (action === "execute-light-plan") {
      actions.onExecuteLightPlan();
      return;
    }
    if (action === "cancel-light-plan") {
      actions.onCancelLightPlan();
      return;
    }
    if (action === "execute-defect-correction") {
      actions.onExecuteDefectCorrection();
      return;
    }
    if (action === "execute-all-defect-corrections") {
      actions.onExecuteAllDefectCorrections();
      return;
    }
    if (action === "export-defect-batch-report") {
      actions.onExportDefectBatchReport();
      return;
    }
    if (action === "resume-defect-batch") {
      actions.onResumeDefectBatch();
      return;
    }
    if (action === "inspect-defect-batch-report") {
      actions.onInspectDefectBatchReport();
      return;
    }
    if (action === "cancel-defect-correction") {
      actions.onCancelDefectCorrection();
      return;
    }
    if (action === "execute-linear-defect-correction") {
      actions.onExecuteLinearDefectCorrection();
      return;
    }
    if (action === "cancel-linear-defect-correction") {
      actions.onCancelLinearDefectCorrection();
      return;
    }
    if (action === "select-linear-defect-axis") {
      const axis = actionElement.dataset.linearDefectAxis;
      if (axis === "rows" || axis === "columns") {
        actions.onUpdateLinearDefectSettings({
          ...model.calibration.defectCorrection.linear.settings,
          axis,
        });
      }
      return;
    }
    if (action === "select-defect-stride") {
      const stride = Number(actionElement.dataset.defectStride);
      if (stride === 1 || stride === 2) {
        const settings = model.calibration.defectCorrection.settings;
        actions.onUpdateDefectCorrectionSettings({
          ...settings,
          darkDetection: { ...settings.darkDetection, stride },
          flatDetection: { ...settings.flatDetection, stride },
          correctionStride: stride,
        });
      }
      return;
    }
    if (action === "select-defect-preview") {
      const view = actionElement.dataset.defectPreview;
      if (view === "before" || view === "after" || view === "map") {
        actions.onSelectDefectPreview(view);
      }
      return;
    }

    if (action === "select-role") {
      const role = actionElement.dataset.role;
      if (isFrameRole(role)) actions.onSelectRole(role);
      return;
    }
    if (action === "select-light-frame-view") {
      const view = actionElement.dataset.lightFrameViewValue;
      if (view === "raw" || view === "calibrated") {
        actions.onSelectLightFrameView(view);
      }
      return;
    }
    if (action === "import-session") {
      actions.onImportSession();
      return;
    }
    if (action === "select-frame") {
      const frameId = actionElement.dataset.frameId;
      if (frameId) actions.onSelectFrame(frameId);
      return;
    }
    if (action === "sort") {
      const field = actionElement.dataset.sortField as SortField | undefined;
      if (!field) return;
      const previous = sortDirections.get(field) ?? "descending";
      const direction: SortDirection =
        previous === "ascending" ? "descending" : "ascending";
      sortDirections.set(field, direction);
      actionElement.setAttribute(
        "aria-label",
        `Sort ${humanize(field)} ${direction}`,
      );
      actions.onSort(field, direction);
      return;
    }
    if (action === "accept") {
      const frame = selectedFrame(model);
      if (frame) actions.onSetDecision(frame.id, "accepted", null);
      return;
    }
    if (action === "clear-decision") {
      const frame = selectedFrame(model);
      if (frame) actions.onClearDecision(frame.id);
      return;
    }
    if (action === "open-reject") {
      const frame = selectedFrame(model);
      if (frame) openRejectDialog(frame);
      return;
    }
    if (action === "cancel-reject") {
      closeRejectDialog();
      return;
    }
    if (action === "reject-reason") {
      const reason = actionElement.dataset.reason;
      if (pendingRejectFrameId && isReviewRejectionReason(reason)) {
        actions.onSetDecision(pendingRejectFrameId, "rejected", reason);
        closeRejectDialog();
      }
      return;
    }
    if (action === "undo") {
      actions.onUndo();
      return;
    }
    if (action === "toggle-play") {
      actions.onSetPlaying(!model.playing);
      return;
    }
    if (action === "previous") {
      actions.onRequestStep("backward");
      return;
    }
    if (action === "next") {
      actions.onRequestStep("forward");
      return;
    }
    if (action === "open-diagnostics") {
      elements.diagnosticsDialog.hidden = false;
      queueMicrotask(() => {
        required<HTMLButtonElement>(
          elements.diagnosticsDialog,
          '[data-action="close-diagnostics"]',
        ).focus();
      });
      return;
    }
    if (action === "close-diagnostics") {
      elements.diagnosticsDialog.hidden = true;
      queueMicrotask(() => elements.diagnosticsButton.focus());
      return;
    }
    if (action === "export-diagnostics") {
      actions.onExportDiagnostics();
      return;
    }
    if (action === "inspect-diagnostics-report") {
      actions.onInspectDiagnosticsReport();
      return;
    }
    if (action === "preview-cache-maintenance") {
      actions.onPreviewQualityCacheMaintenance();
      return;
    }
    if (action === "open-cache-maintenance-confirmation") {
      if (
        model.sessionDiagnostics.maintenanceState === "ready" &&
        model.sessionDiagnostics.maintenanceEligible > 0
      ) {
        elements.cacheMaintenanceDialog.hidden = false;
        queueMicrotask(() => {
          required<HTMLButtonElement>(
            elements.cacheMaintenanceDialog,
            '[data-action="cancel-cache-maintenance"]',
          ).focus();
        });
      }
      return;
    }
    if (action === "cancel-cache-maintenance") {
      elements.cacheMaintenanceDialog.hidden = true;
      queueMicrotask(() => elements.diagnosticsMaintenanceRemove.focus());
      return;
    }
    if (action === "confirm-cache-maintenance") {
      elements.cacheMaintenanceDialog.hidden = true;
      actions.onApplyQualityCacheMaintenance();
      return;
    }
    if (action === "filter-diagnostics-evidence") {
      const filter = actionElement.dataset.diagnosticsFilter;
      if (isDiagnosticsEvidenceFilter(filter)) {
        diagnosticsFilter = filter;
        renderSessionDiagnostics(
          elements,
          model,
          diagnosticsFilter,
          diagnosticsQuery,
        );
      }
      return;
    }
    if (action === "viewer-fit") {
      actions.onSetViewerScale("fit");
      return;
    }
    if (action === "viewer-actual") {
      actions.onSetViewerScale("actual");
      return;
    }
    if (action === "open-statistics") {
      const frame = selectedFrame(model);
      if (frame?.sourcePath) {
        actions.onOpenStatistics(frame.id);
        queueMicrotask(() => {
          required<HTMLButtonElement>(
            elements.statisticsDialog,
            '[data-action="close-statistics"]',
          ).focus();
        });
      }
      return;
    }
    if (action === "measure-quality") {
      const frame = selectedFrame(model);
      if (frame) actions.onMeasureQuality(frame.id);
      return;
    }
    if (action === "measure-all-quality") {
      actions.onMeasureAllQuality();
      return;
    }
    if (action === "preview-frame-selection") {
      actions.onPreviewFrameSelection();
      return;
    }
    if (action === "open-selection-confirmation") {
      const undecided = automaticSelectionChangeCount(model);
      if (model.frameSelection.state !== "ready" || undecided === 0) return;
      elements.selectionConfirmationCount.textContent = String(undecided);
      elements.selectionConfirmation.hidden = false;
      queueMicrotask(() => {
        required<HTMLButtonElement>(
          elements.selectionConfirmation,
          '[data-action="confirm-frame-selection"]',
        ).focus();
      });
      return;
    }
    if (action === "cancel-frame-selection") {
      closeSelectionConfirmation();
      return;
    }
    if (action === "confirm-frame-selection") {
      elements.selectionConfirmation.hidden = true;
      actions.onApplyFrameSelection();
      return;
    }
    if (action === "add-selection-rule") {
      const rules = readFrameSelectionRules(
        selectionRuleRows(elements.selectionRules),
      );
      if (!rules) return;
      const used = new Set(rules.map((rule) => rule.metric));
      const metric = selectionMetricOptions.find(
        ([candidate]) => !used.has(candidate),
      )?.[0];
      if (!metric) return;
      actions.onUpdateFrameSelectionRules([
        ...rules,
        defaultSelectionRule(metric),
      ]);
      return;
    }
    if (action === "remove-selection-rule") {
      const rules = readFrameSelectionRules(
        selectionRuleRows(elements.selectionRules),
      );
      const index = Number(actionElement.dataset.ruleIndex);
      if (!rules || rules.length <= 1 || !Number.isSafeInteger(index)) return;
      actions.onUpdateFrameSelectionRules(
        rules.filter((_, ruleIndex) => ruleIndex !== index),
      );
      return;
    }
    if (action === "close-statistics") {
      actions.onCloseStatistics();
      queueMicrotask(() => elements.statisticsButton.focus());
    }
  };

  const onChange = (event: Event): void => {
    const target = event.target;
    if (
      target instanceof Element &&
      target.matches("[data-selection-control]")
    ) {
      const row = target.closest<HTMLElement>("[data-selection-rule]");
      if (row && target.matches("[data-selection-metric]")) {
        configureSelectionThreshold(row);
      }
      const rows = selectionRuleRows(elements.selectionRules);
      configureAvailableSelectionMetrics(rows);
      const rules = readFrameSelectionRules(rows);
      if (rules) actions.onUpdateFrameSelectionRules(rules);
      return;
    }
    if (target === elements.registrationReference) {
      actions.onSelectRegistrationReference(
        elements.registrationReference.value,
      );
      return;
    }
    if (target === elements.registrationSource) {
      actions.onSelectRegistrationSource(elements.registrationSource.value);
      return;
    }
    if (target === elements.registeredFrame) {
      actions.onSelectRegisteredFrame(elements.registeredFrame.value);
      return;
    }
    if (
      target === elements.registeredStackEstimator ||
      target === elements.registeredStackLowFraction ||
      target === elements.registeredStackHighFraction ||
      target === elements.registeredStackLowSigma ||
      target === elements.registeredStackHighSigma ||
      target === elements.registeredStackEsdOutlierFraction ||
      target === elements.registeredStackEsdSignificance ||
      target === elements.registeredStackMaximumIterations ||
      target === elements.registeredStackLargeScaleLowEnabled ||
      target === elements.registeredStackLargeScaleHighEnabled ||
      target === elements.registeredStackLargeScaleLowLayers ||
      target === elements.registeredStackLargeScaleHighLayers ||
      target === elements.registeredStackLargeScaleLowGrowth ||
      target === elements.registeredStackLargeScaleHighGrowth ||
      target === elements.registeredStackMinimumRetained ||
      target === elements.registeredStackRejectionMaps ||
      target === elements.registeredStackSupportMap ||
      target === elements.registeredStackWeightReference
    ) {
      if (
        target === elements.registeredStackEstimator &&
        elements.registeredStackEstimator.value === "linear_fit_clipped"
      ) {
        elements.registeredStackLowSigma.value = "5";
        elements.registeredStackHighSigma.value = "3.5";
        elements.registeredStackMinimumRetained.value = "3";
      }
      if (
        target === elements.registeredStackEstimator &&
        elements.registeredStackEstimator.value === "generalized_esd"
      ) {
        elements.registeredStackEsdOutlierFraction.value = "0.3";
        elements.registeredStackEsdSignificance.value = "0.05";
        elements.registeredStackMinimumRetained.value = "3";
      }
      const settings = registeredStackSettings(elements);
      if (settings) actions.onUpdateRegisteredStackSettings(settings);
      return;
    }
    if (target === elements.lightOutputMode) {
      const mode = elements.lightOutputMode.value;
      if (mode === "calibrated_frames" || mode === "integrated") {
        actions.onUpdateLightOutputMode(mode);
      }
      return;
    }
    if (
      target === elements.defectDetectionRadius ||
      target === elements.defectDetectionMinimumNeighbours ||
      target === elements.defectDarkHotSigma ||
      target === elements.defectDarkColdSigma ||
      target === elements.defectDarkFloor ||
      target === elements.defectFlatHotSigma ||
      target === elements.defectFlatColdSigma ||
      target === elements.defectFlatFloor ||
      target === elements.defectCorrectionRadius ||
      target === elements.defectCorrectionMinimumNeighbours
    ) {
      const settings = defectCorrectionSettings(elements, model);
      if (settings) actions.onUpdateDefectCorrectionSettings(settings);
      return;
    }
    if (elements.linearDefectInputs.includes(target as HTMLInputElement)) {
      const settings = linearDefectSettings(elements, model);
      if (settings) actions.onUpdateLinearDefectSettings(settings);
      return;
    }
    if (
      target !== elements.pedestalPolicy &&
      target !== elements.exposureTolerance &&
      target !== elements.temperatureTolerance &&
      target !== elements.lightTemperatureTolerance
    ) {
      return;
    }
    const settings = calibrationSettings(elements);
    if (settings) actions.onUpdateCalibrationSettings(settings);
  };

  const onInput = (event: Event): void => {
    if (event.target === elements.frameFilter) {
      frameQuery = elements.frameFilter.value;
      renderRows(elements.tableBody, model, frameQuery);
      updateFrameListSummary(elements, model, frameQuery);
      return;
    }
    if (
      event.target === elements.drizzleDropShrink ||
      event.target === elements.drizzleMaximumContributions ||
      event.target === elements.drizzleMaximumBandHeight
    ) {
      const settings = drizzleSettings(elements, model);
      if (settings) actions.onUpdateDrizzleSettings(settings);
      return;
    }
    if (event.target === elements.diagnosticsSearch) {
      diagnosticsQuery = elements.diagnosticsSearch.value;
      renderSessionDiagnostics(
        elements,
        model,
        diagnosticsFilter,
        diagnosticsQuery,
      );
      return;
    }
    if (event.target === elements.registeredStackSourceSearch) {
      sourceEvidenceQuery = elements.registeredStackSourceSearch.value;
      sourceEvidenceLimit = SOURCE_EVIDENCE_PAGE_SIZE;
      renderRegistration(
        elements,
        model,
        sourceEvidenceFilter,
        sourceEvidenceQuery,
        sourceEvidenceLimit,
      );
      return;
    }
    if (event.target === elements.registeredStackOverlayOpacity) {
      const opacity =
        elements.registeredStackOverlayOpacity.valueAsNumber / 100;
      if (Number.isFinite(opacity)) {
        actions.onSetRegisteredStackOverlayOpacity(opacity);
      }
    }
  };

  const onKeyDown = (event: KeyboardEvent): void => {
    if (event.key === "Escape" && !elements.cacheMaintenanceDialog.hidden) {
      elements.cacheMaintenanceDialog.hidden = true;
      queueMicrotask(() => elements.diagnosticsMaintenanceRemove.focus());
      return;
    }
    if (event.key === "Escape" && !elements.diagnosticsDialog.hidden) {
      elements.diagnosticsDialog.hidden = true;
      queueMicrotask(() => elements.diagnosticsButton.focus());
      return;
    }
    if (event.key === "Escape" && !elements.selectionConfirmation.hidden) {
      closeSelectionConfirmation();
      return;
    }
    if (event.key === "Escape" && !elements.rejectDialog.hidden) {
      closeRejectDialog();
      return;
    }
    if (event.key === "Escape" && !elements.statisticsDialog.hidden) {
      actions.onCloseStatistics();
      queueMicrotask(() => elements.statisticsButton.focus());
      return;
    }
    const target = event.target instanceof Element ? event.target : null;
    if (
      !elements.rejectDialog.hidden ||
      !elements.cacheMaintenanceDialog.hidden ||
      !elements.diagnosticsDialog.hidden ||
      !elements.statisticsDialog.hidden ||
      !elements.selectionConfirmation.hidden
    ) {
      return;
    }
    if (!event.repeat && !isTextEntryTarget(target)) {
      const frame = selectedFrame(model);
      const reviewAvailable = reviewCommandsAvailable(model);
      const key = event.key.toLocaleLowerCase("en-US");
      if (
        key === "z" &&
        (event.metaKey || event.ctrlKey) &&
        !event.altKey &&
        !event.shiftKey &&
        reviewAvailable &&
        model.canUndo
      ) {
        event.preventDefault();
        actions.onUndo();
        return;
      }
      if (!event.metaKey && !event.ctrlKey && !event.altKey && frame) {
        if (key === "a" && reviewAvailable && frame.state !== "accepted") {
          event.preventDefault();
          actions.onSetDecision(frame.id, "accepted", null);
          return;
        }
        if (key === "r" && reviewAvailable) {
          event.preventDefault();
          openRejectDialog(frame);
          return;
        }
        if (key === "c" && reviewAvailable && frame.state !== "undecided") {
          event.preventDefault();
          actions.onClearDecision(frame.id);
          return;
        }
      }
    }
    const row = target?.closest<HTMLElement>('[role="row"][data-frame-id]');
    if (row && (event.key === "ArrowDown" || event.key === "ArrowUp")) {
      event.preventDefault();
      const currentIndex = model.frames.findIndex(
        (frame) => frame.id === row.dataset.frameId,
      );
      if (currentIndex < 0) return;
      const delta = event.key === "ArrowDown" ? 1 : -1;
      const nextIndex = Math.min(
        model.frames.length - 1,
        Math.max(0, currentIndex + delta),
      );
      const next = model.frames[nextIndex];
      if (next) {
        actions.onSelectFrame(next.id);
        queueMicrotask(() => {
          root
            .querySelector<HTMLElement>(`[data-frame-id="${next.id}"]`)
            ?.focus();
        });
      }
      return;
    }

    const tab = target?.closest<HTMLElement>('[role="tab"][data-role]');
    if (!tab || (event.key !== "ArrowRight" && event.key !== "ArrowLeft")) {
      return;
    }
    event.preventDefault();
    const currentIndex = model.roles.findIndex(
      (role) => role.role === tab.dataset.role,
    );
    if (currentIndex < 0 || model.roles.length === 0) return;
    const delta = event.key === "ArrowRight" ? 1 : -1;
    const nextIndex =
      (currentIndex + delta + model.roles.length) % model.roles.length;
    const next = model.roles[nextIndex];
    if (next) {
      actions.onSelectRole(next.role);
      queueMicrotask(() => {
        root.querySelector<HTMLElement>(`[data-role="${next.role}"]`)?.focus();
      });
    }
  };

  const openRejectDialog = (frame: ReviewFrame): void => {
    pendingRejectFrameId = frame.id;
    elements.rejectFrame.textContent = frame.label;
    elements.rejectDialog.hidden = false;
    required<HTMLButtonElement>(
      elements.rejectDialog,
      '[data-action="reject-reason"]',
    ).focus();
  };

  const closeRejectDialog = (): void => {
    pendingRejectFrameId = null;
    elements.rejectDialog.hidden = true;
    required<HTMLButtonElement>(root, '[data-action="open-reject"]').focus();
  };

  const closeSelectionConfirmation = (): void => {
    elements.selectionConfirmation.hidden = true;
    queueMicrotask(() => elements.selectionApply.focus());
  };

  const update = (nextModel: ReviewViewModel): void => {
    model = nextModel;
    elements.sessionName.textContent = model.sessionName;
    elements.sessionStatus.dataset.tone = model.sessionStatus.tone;
    elements.sessionStatusLabel.textContent = model.sessionStatus.label;
    const importing = model.sessionStatus.tone === "busy";
    const importProgress = model.sessionImportProgress;
    elements.sessionImportProgress.hidden = !importing;
    if (
      importing &&
      importProgress?.stage === "analyzing" &&
      importProgress.total !== null
    ) {
      elements.sessionImportProgress.max = Math.max(importProgress.total, 1);
      elements.sessionImportProgress.value = importProgress.completed;
      elements.sessionImportProgress.setAttribute(
        "aria-valuetext",
        `${importProgress.completed} of ${importProgress.total} FITS sources analyzed`,
      );
    } else {
      elements.sessionImportProgress.removeAttribute("value");
      elements.sessionImportProgress.removeAttribute("aria-valuetext");
    }
    const cancellingImport =
      importing && model.sessionStatus.label === "Cancelling FITS import";
    const masterBusy =
      model.calibration.execution.state === "running" ||
      model.calibration.execution.state === "cancelling" ||
      model.calibration.lightExecution.state === "running" ||
      model.calibration.lightExecution.state === "cancelling" ||
      model.registration.execution.state === "running" ||
      model.registration.execution.state === "cancelling" ||
      model.localNormalization.state === "running" ||
      model.localNormalization.state === "cancelling";
    const decisionBusy = model.decisionPending || importing || masterBusy;
    const decisionsAvailable = reviewCommandsAvailable(model);
    const lightRoleCount =
      model.roles.find((role) => role.role === "light")?.count ?? 0;
    elements.framesWorkspace.hidden = model.activeWorkspace !== "frames";
    elements.calibrationWorkspace.hidden =
      model.activeWorkspace !== "calibration";
    elements.registrationWorkspace.hidden =
      model.activeWorkspace !== "registration";
    elements.normalizationWorkspace.hidden =
      model.activeWorkspace !== "normalization";
    elements.runWorkspace.hidden = model.activeWorkspace !== "run";
    elements.resultsWorkspace.hidden = model.activeWorkspace !== "results";
    elements.settingsWorkspace.hidden = model.activeWorkspace !== "settings";
    for (const item of elements.workspaceNavigation) {
      const selected = item.dataset.workspace === model.activeWorkspace;
      item.toggleAttribute("aria-current", selected);
      item.classList.toggle("nav-item--active", selected);
    }
    renderCalibration(elements, model);
    const nextSourceEvidenceReportSha256 =
      model.registration.stack.reportInspection?.reportSha256 ?? null;
    if (nextSourceEvidenceReportSha256 !== sourceEvidenceReportSha256) {
      sourceEvidenceReportSha256 = nextSourceEvidenceReportSha256;
      sourceEvidenceFilter = "all";
      sourceEvidenceQuery = "";
      sourceEvidenceLimit = SOURCE_EVIDENCE_PAGE_SIZE;
    }
    renderRegistration(
      elements,
      model,
      sourceEvidenceFilter,
      sourceEvidenceQuery,
      sourceEvidenceLimit,
    );
    localNormalizationPanel.update(model.localNormalization);
    renderWorkflowOverview(
      elements.runWorkspace,
      elements.resultsWorkspace,
      model,
    );
    renderWorkflowCoach(elements.workflowCoach, model);
    syncStepperStates(root);
    elements.importSession.disabled = cancellingImport || masterBusy;
    elements.importSession.classList.toggle("button--danger", importing);
    elements.importSession.textContent = cancellingImport
      ? "Cancelling…"
      : importing
        ? "× Cancel import"
        : "＋ Import session";
    elements.reviewPlan.disabled = lightRoleCount === 0 || decisionBusy;
    elements.reviewPlan.title =
      lightRoleCount === 0
        ? "Import Light frames to configure automatic quality review"
        : decisionBusy
          ? "Finish the active operation before changing the review plan"
          : "Open the automatic Light-frame quality review";
    renderRoles(elements.roleTabs, model);
    renderRows(elements.tableBody, model, frameQuery);
    renderFrameSelection(elements, model);
    const activeRole = model.roles.find(
      (role) => role.role === model.activeRole,
    );
    const calibratedLightView =
      model.activeRole === "light" && model.lightFrameView === "calibrated";
    const activeRoleLabel = calibratedLightView
      ? "Calibrated Lights"
      : (activeRole?.label ?? "Frames");
    const frame = selectedFrame(model);
    const position = frame
      ? model.frames.findIndex((candidate) => candidate.id === frame.id) + 1
      : 0;
    elements.roleHeading.textContent = activeRoleLabel;
    const calibratedFrameCount =
      model.calibration.lightExecution.result?.calibratedFrames.length ?? 0;
    elements.lightFrameView.hidden = model.activeRole !== "light";
    elements.rawLightView.setAttribute(
      "aria-pressed",
      String(model.lightFrameView === "raw"),
    );
    elements.calibratedLightView.setAttribute(
      "aria-pressed",
      String(model.lightFrameView === "calibrated"),
    );
    elements.calibratedLightView.disabled = calibratedFrameCount === 0;
    elements.calibratedLightView.textContent =
      calibratedFrameCount > 0
        ? `Calibrated · ${calibratedFrameCount}`
        : "Calibrated";
    elements.frameTable.setAttribute(
      "aria-label",
      `${activeRoleLabel} review metrics`,
    );
    elements.frameFilter.setAttribute(
      "aria-label",
      `Filter ${activeRoleLabel.toLocaleLowerCase("en-US")}`,
    );
    elements.frameFilter.placeholder = `Find ${activeRoleLabel.toLocaleLowerCase("en-US")}`;
    updateFrameListSummary(elements, model, frameQuery);
    elements.framePosition.textContent = `${position} / ${model.frames.length}`;
    elements.selectedLabel.textContent = frame?.label ?? "No frame selected";
    const resolvedPreview =
      frame && model.preview?.frameId === frame.id ? model.preview : null;
    elements.previewImage.hidden = resolvedPreview === null;
    elements.previewPlaceholder.hidden = resolvedPreview !== null;
    if (resolvedPreview && frame) {
      elements.previewImage.src = resolvedPreview.url;
      elements.previewImage.alt = `Display preview of ${frame.label}`;
    } else {
      elements.previewImage.removeAttribute("src");
      elements.previewImage.alt = "";
    }
    elements.previewBadges.hidden = frame === null;
    elements.previewTitle.textContent = frame
      ? frame.sourcePath
        ? "Rendering FITS preview"
        : "Native FITS renderer ready"
      : "No frame selected";
    elements.previewDescription.textContent = frame
      ? frame.sourcePath
        ? "Reducing pixels with the locked display stretch"
        : "Import a session to inspect its bounded pixel preview"
      : "Select a frame to inspect its display preview";
    elements.preview.setAttribute(
      "aria-label",
      frame
        ? `Display-only preview for ${frame.label}`
        : "No frame preview selected",
    );
    elements.preview.dataset.scale = model.viewerScale;
    elements.fitPreview.setAttribute(
      "aria-pressed",
      String(model.viewerScale === "fit"),
    );
    elements.actualPreview.setAttribute(
      "aria-pressed",
      String(model.viewerScale === "actual"),
    );
    elements.statisticsButton.disabled = !frame?.sourcePath;
    const qualityActionable =
      model.activeRole === "light" &&
      !model.qualityBatchRunning &&
      !!frame?.sourcePath &&
      !!frame.bayerPattern &&
      (frame.qualityState === "idle" || frame.qualityState === "error");
    elements.qualityButton.disabled = !qualityActionable;
    elements.qualityButton.textContent = qualityButtonLabel(frame);
    elements.qualityButton.title = frame?.qualityMessage ?? "No frame selected";
    const qualityCandidateCount = model.frames.filter(
      (candidate) =>
        candidate.sourcePath !== null &&
        candidate.bayerPattern !== null &&
        (candidate.qualityState === "idle" ||
          candidate.qualityState === "error"),
    ).length;
    const qualityMeasurementActive = model.frames.some(
      (candidate) => candidate.qualityState === "loading",
    );
    elements.qualityBatchButton.hidden = model.activeRole !== "light";
    elements.qualityBatchButton.disabled =
      model.qualityBatchRunning ||
      qualityMeasurementActive ||
      qualityCandidateCount === 0;
    elements.qualityBatchButton.textContent = model.qualityBatchRunning
      ? model.qualityBatchProgress
        ? `Analyzing ${model.qualityBatchProgress.completed} / ${model.qualityBatchProgress.total}`
        : "Measuring…"
      : qualityCandidateCount > 0
        ? `Measure all · ${qualityCandidateCount}`
        : model.frames.length === 0
          ? "No Lights"
          : "Quality complete";
    elements.qualityBatchButton.setAttribute(
      "aria-label",
      model.qualityBatchRunning
        ? model.qualityBatchProgress
          ? `Processed diagnostic quality for ${model.qualityBatchProgress.completed} of ${model.qualityBatchProgress.total} eligible light frames`
          : "Measuring diagnostic quality for all eligible light frames"
        : qualityMeasurementActive
          ? "Wait for the active diagnostic quality measurement"
          : qualityCandidateCount > 0
            ? `Measure diagnostic quality for ${qualityCandidateCount} eligible light frames`
            : model.frames.length === 0
              ? "Import Light frames to measure diagnostic quality"
              : "All eligible light frames have diagnostic quality measurements",
    );
    elements.cfaBadge.textContent =
      calibratedLightView && frame?.previewContent.kind === "rgb"
        ? "CALIBRATED RGB · LINEAR"
        : frame?.bayerPattern
          ? `${calibratedLightView ? "CALIBRATED CFA" : "RAW CFA"} · ${frame.bayerPattern.toUpperCase()}`
          : "LINEAR · UNRESOLVED";
    elements.qualityBadge.hidden = frame === null;
    elements.qualityBadge.dataset.state = frame?.qualityState ?? "unavailable";
    elements.qualityBadge.textContent = qualityBadgeLabel(frame);
    elements.qualityBadge.title = frame?.qualityMessage ?? "";
    renderSessionDiagnostics(
      elements,
      model,
      diagnosticsFilter,
      diagnosticsQuery,
    );
    renderStatisticsPanel(elements, model);
    elements.stretch.textContent = model.sharedStretchLabel;
    renderSelectedMetrics(elements, frame);
    elements.play.setAttribute("aria-pressed", String(model.playing));
    elements.play.disabled = model.frames.length < 2;
    for (const button of elements.stepButtons) {
      button.disabled = model.frames.length < 2;
    }
    for (const button of elements.decisionButtons) {
      button.disabled = frame === null || !decisionsAvailable;
    }
    elements.accept.disabled ||= frame?.state === "accepted";
    elements.clearDecision.disabled ||= frame?.state === "undecided";
    elements.undo.disabled = !model.canUndo || !decisionsAvailable;
    elements.undo.setAttribute(
      "aria-label",
      decisionBusy
        ? "Review decision transaction in progress"
        : model.canUndo
          ? "Undo last review decision"
          : "No review decision to undo",
    );
    elements.undo.title = model.canUndo
      ? "Undo the last native review transaction (⌘Z / Ctrl+Z)"
      : "No review decision to undo";
    elements.decisionControls.setAttribute("aria-busy", String(decisionBusy));
    elements.decisionControls.title = model.reviewSessionReady
      ? "Manual decisions are committed by the native review engine"
      : "Import a FITS session to enable manual review decisions";
    elements.play.setAttribute(
      "aria-label",
      model.playing ? "Pause Blink playback" : "Start Blink playback",
    );
    required<HTMLElement>(elements.play, "[data-play-icon]").textContent =
      model.playing ? "Ⅱ" : "▶";
    if (frame) {
      elements.liveRegion.textContent = `${frame.label}, ${stateLabel(frame.state)}, ${frame.qualityMessage}, frame ${position} of ${model.frames.length}`;
    }
  };

  const destroy = (): void => {
    localNormalizationPanel.destroy();
    uiPreferences.destroy();
    root.removeEventListener("click", onClick);
    root.removeEventListener("change", onChange);
    root.removeEventListener("input", onInput);
    root.removeEventListener("keydown", onKeyDown);
    root.replaceChildren();
  };

  root.addEventListener("click", onClick);
  root.addEventListener("change", onChange);
  root.addEventListener("input", onInput);
  root.addEventListener("keydown", onKeyDown);
  update(initialModel);

  const setSessionDropState: ReviewScreen["setSessionDropState"] = (
    state,
    message,
  ) => {
    const overlay = required<HTMLElement>(root, "[data-session-drop-overlay]");
    overlay.hidden = state === "hidden";
    overlay.dataset.state = state;
    required<HTMLElement>(overlay, "[data-session-drop-message]").textContent =
      message ?? "Drop one session directory";
  };

  return { update, setSessionDropState, destroy };
}

function isReviewRejectionReason(
  value: string | undefined,
): value is ReviewRejectionReason {
  return rejectionReasons.some(([code]) => code === value);
}

function reviewCommandsAvailable(model: ReviewViewModel): boolean {
  return (
    model.reviewSessionReady &&
    !model.decisionPending &&
    model.sessionStatus.tone !== "busy"
  );
}

function isTextEntryTarget(target: Element | null): boolean {
  return (
    target instanceof HTMLInputElement ||
    target instanceof HTMLTextAreaElement ||
    target instanceof HTMLSelectElement ||
    (target?.closest('[contenteditable="true"]') ?? null) !== null
  );
}

function syncStepperStates(root: HTMLElement): void {
  for (const stepper of root.querySelectorAll<HTMLElement>("[data-stepper]")) {
    const input = stepper.querySelector<HTMLInputElement>(
      'input[type="number"]',
    );
    if (!input) continue;
    for (const button of stepper.querySelectorAll<HTMLButtonElement>(
      'button[data-action="step-number"]',
    )) {
      button.disabled = input.disabled;
    }
  }
}

function qualityButtonLabel(frame: ReviewFrame | null): string {
  switch (frame?.qualityState) {
    case "loading":
      return "Measuring…";
    case "ready":
      return "Quality measured";
    case "error":
      return "Retry quality";
    case "idle":
    case "unavailable":
    case undefined:
      return "Measure quality";
  }
}

function qualityBadgeLabel(frame: ReviewFrame | null): string {
  switch (frame?.qualityState) {
    case "loading":
      return "QUALITY · MEASURING";
    case "ready":
      return frame.qualityOrigin === "restored"
        ? "QUALITY · RESTORED"
        : "QUALITY · DIAGNOSTIC";
    case "error":
      return "QUALITY · FAILED";
    case "idle":
      return "QUALITY · NOT MEASURED";
    case "unavailable":
    case undefined:
      return "QUALITY · BLOCKED";
  }
}

function renderRoles(container: HTMLElement, model: ReviewViewModel): void {
  container.replaceChildren();
  for (const role of model.roles) {
    const button = document.createElement("button");
    const selected = role.role === model.activeRole;
    button.className = "role-tab";
    button.type = "button";
    button.role = "tab";
    button.dataset.action = "select-role";
    button.dataset.role = role.role;
    button.setAttribute("aria-selected", String(selected));
    button.tabIndex = selected ? 0 : -1;
    button.innerHTML = `<span class="role-tab__label"></span><span class="role-tab__count"></span>`;
    required<HTMLElement>(button, ".role-tab__label").textContent = role.label;
    required<HTMLElement>(button, ".role-tab__count").textContent = String(
      role.count,
    );
    if (role.unresolved > 0) {
      button.setAttribute(
        "aria-label",
        `${role.label}, ${role.count} frames, ${role.unresolved} unresolved`,
      );
    }
    container.append(button);
  }
}

function readFrameSelectionRules(
  rows: readonly HTMLElement[],
): readonly FrameSelectionRule[] | null {
  const rules: FrameSelectionRule[] = [];
  const seen = new Set<FrameSelectionMetric>();
  for (const row of rows) {
    const metricValue = required<HTMLSelectElement>(
      row,
      "[data-selection-metric]",
    ).value;
    const metric = selectionMetricOptions.find(
      ([candidate]) => candidate === metricValue,
    )?.[0];
    const comparator = required<HTMLSelectElement>(
      row,
      "[data-selection-comparator]",
    ).value;
    const missingPolicy = required<HTMLSelectElement>(
      row,
      "[data-selection-missing]",
    ).value;
    const threshold = required<HTMLInputElement>(
      row,
      "[data-selection-threshold]",
    ).valueAsNumber;
    if (
      !metric ||
      seen.has(metric) ||
      (comparator !== "less_than" && comparator !== "greater_than") ||
      (missingPolicy !== "retain" && missingPolicy !== "reject") ||
      !Number.isFinite(threshold) ||
      !validSelectionThreshold(metric, threshold)
    ) {
      return null;
    }
    const countMetric =
      metric === "detected_stars" || metric === "usable_stars";
    if (countMetric && !Number.isSafeInteger(threshold)) return null;
    seen.add(metric);
    rules.push({
      metric,
      comparator,
      threshold: countMetric
        ? { kind: "count", value: threshold }
        : { kind: "scalar", value: threshold },
      missingPolicy,
    });
  }
  return rules;
}

function selectionRuleRows(container: ParentNode): readonly HTMLElement[] {
  return [...container.querySelectorAll<HTMLElement>("[data-selection-rule]")];
}

function defaultSelectionRule(
  metric: FrameSelectionMetric,
): FrameSelectionRule {
  switch (metric) {
    case "signal_to_noise":
      return {
        metric,
        comparator: "greater_than",
        threshold: { kind: "scalar", value: 10 },
        missingPolicy: "reject",
      };
    case "detected_stars":
    case "usable_stars":
      return {
        metric,
        comparator: "greater_than",
        threshold: { kind: "count", value: 100 },
        missingPolicy: "reject",
      };
    case "eccentricity":
      return {
        metric,
        comparator: "less_than",
        threshold: { kind: "scalar", value: 0.6 },
        missingPolicy: "reject",
      };
    case "background":
      return {
        metric,
        comparator: "less_than",
        threshold: { kind: "scalar", value: 0 },
        missingPolicy: "reject",
      };
    case "noise":
      return {
        metric,
        comparator: "less_than",
        threshold: { kind: "scalar", value: 1 },
        missingPolicy: "reject",
      };
    case "fwhm_pixels":
      return {
        metric,
        comparator: "less_than",
        threshold: { kind: "scalar", value: 4.5 },
        missingPolicy: "reject",
      };
  }
}

function configureAvailableSelectionMetrics(
  rows: readonly HTMLElement[],
): void {
  const selected = rows.map(
    (row) => required<HTMLSelectElement>(row, "[data-selection-metric]").value,
  );
  for (const [index, row] of rows.entries()) {
    const select = required<HTMLSelectElement>(row, "[data-selection-metric]");
    for (const option of select.options) {
      option.disabled =
        option.value !== selected[index] && selected.includes(option.value);
    }
  }
}

function validSelectionThreshold(
  metric: FrameSelectionMetric,
  threshold: number,
): boolean {
  switch (metric) {
    case "background":
      return true;
    case "signal_to_noise":
      return threshold > 0;
    case "eccentricity":
      return threshold >= 0 && threshold <= 1;
    case "noise":
    case "fwhm_pixels":
    case "detected_stars":
    case "usable_stars":
      return threshold >= 0;
  }
}

function configureSelectionThreshold(row: HTMLElement): void {
  const metric = required<HTMLSelectElement>(
    row,
    "[data-selection-metric]",
  ).value;
  const input = required<HTMLInputElement>(row, "[data-selection-threshold]");
  const countMetric = metric === "detected_stars" || metric === "usable_stars";
  input.step = countMetric ? "1" : "any";
  input.removeAttribute("min");
  input.removeAttribute("max");
  if (metric !== "background") {
    input.min = metric === "signal_to_noise" ? "0.000001" : "0";
  }
  if (metric === "eccentricity") input.max = "1";
  if (countMetric && Number.isFinite(input.valueAsNumber)) {
    input.value = String(Math.max(0, Math.round(input.valueAsNumber)));
  }
}

function renderFrameSelection(
  elements: {
    selectionPanel: HTMLDetailsElement;
    selectionRules: HTMLElement;
    selectionAddRule: HTMLButtonElement;
    selectionPreview: HTMLButtonElement;
    selectionApply: HTMLButtonElement;
    selectionStatus: HTMLElement;
    selectionRetained: HTMLElement;
    selectionRejected: HTMLElement;
    selectionDigest: HTMLElement;
    selectionEvidence: HTMLElement;
    selectionEvidenceFrame: HTMLElement;
    selectionEvidenceList: HTMLOListElement;
  },
  model: ReviewViewModel,
): void {
  const selection = model.frameSelection;
  elements.selectionPanel.hidden = model.activeRole !== "light";
  elements.selectionPanel.dataset.state = selection.state;
  let rows = selectionRuleRows(elements.selectionRules);
  if (rows.length !== selection.rules.length) {
    elements.selectionRules.innerHTML = selection.rules
      .map((rule, index) => selectionRuleMarkup(index, rule))
      .join("");
    rows = selectionRuleRows(elements.selectionRules);
  }
  for (const [index, row] of rows.entries()) {
    const rule = selection.rules[index];
    if (!rule) continue;
    required<HTMLElement>(row, "legend span").textContent = `Rule ${index + 1}`;
    required<HTMLSelectElement>(row, "[data-selection-metric]").value =
      rule.metric;
    configureSelectionThreshold(row);
    required<HTMLSelectElement>(row, "[data-selection-comparator]").value =
      rule.comparator;
    required<HTMLInputElement>(row, "[data-selection-threshold]").value =
      String(rule.threshold.value);
    required<HTMLSelectElement>(row, "[data-selection-missing]").value =
      rule.missingPolicy;
    const remove = required<HTMLButtonElement>(
      row,
      '[data-action="remove-selection-rule"]',
    );
    remove.dataset.ruleIndex = String(index);
    remove.disabled = selection.rules.length <= 1;
    remove.setAttribute("aria-label", `Remove rule ${index + 1}`);
  }
  configureAvailableSelectionMetrics(rows);
  elements.selectionAddRule.disabled =
    selection.rules.length >= selectionMetricOptions.length;
  const allMeasured =
    model.frames.length > 0 &&
    model.frames.every(
      (frame) => frame.sourcePath !== null && frame.qualityState === "ready",
    );
  elements.selectionPreview.disabled =
    model.activeRole !== "light" ||
    !allMeasured ||
    selection.state === "previewing";
  elements.selectionPreview.textContent =
    selection.state === "previewing"
      ? "Evaluating…"
      : "Preview recommendations";
  const proposedChanges = automaticSelectionChangeCount(model);
  elements.selectionApply.disabled =
    selection.state !== "ready" ||
    selection.plan === null ||
    proposedChanges === 0 ||
    model.decisionPending;
  elements.selectionApply.textContent =
    proposedChanges === 0
      ? "Apply recommendations"
      : `Apply ${proposedChanges} undecided`;
  elements.selectionStatus.textContent = selection.message;
  const retained =
    selection.plan?.frames.filter((frame) => frame.proposal === "retain")
      .length ?? null;
  const rejected =
    selection.plan?.frames.filter((frame) => frame.proposal === "reject")
      .length ?? null;
  elements.selectionRetained.textContent =
    retained === null ? "—" : String(retained);
  elements.selectionRejected.textContent =
    rejected === null ? "—" : String(rejected);
  elements.selectionDigest.textContent = selection.plan
    ? `${selection.plan.algorithmId} · ${selection.plan.planSha256}`
    : "Canonical plan digest appears after native preview";
  renderSelectionEvidence(elements, model);
}

function renderSelectionEvidence(
  elements: {
    selectionEvidence: HTMLElement;
    selectionEvidenceFrame: HTMLElement;
    selectionEvidenceList: HTMLOListElement;
  },
  model: ReviewViewModel,
): void {
  const frame = selectedFrame(model);
  const plan = model.frameSelection.plan;
  const result = plan?.frames.find(
    (candidate) => candidate.frameId === frame?.id,
  );
  elements.selectionEvidenceList.replaceChildren();
  elements.selectionEvidence.hidden = !frame || !plan || !result;
  if (!frame || !plan || !result) return;
  elements.selectionEvidenceFrame.textContent = frame.label;
  for (const [index, evidence] of result.evidence.entries()) {
    const rule = plan.rules[index];
    if (!rule) continue;
    const item = document.createElement("li");
    item.dataset.state = evidence.state;
    const metric = selectionMetricOptions.find(
      ([candidate]) => candidate === rule.metric,
    )?.[1];
    const measured = evidence.measured;
    const value =
      measured === null ? "missing" : formatSelectionValue(measured.value);
    const comparator = rule.comparator === "less_than" ? "<" : ">";
    item.innerHTML = `<span class="selection-evidence__lamp" aria-hidden="true"></span><span class="selection-evidence__metric"></span><code class="selection-evidence__equation"></code><strong class="selection-evidence__state"></strong>`;
    required<HTMLElement>(item, ".selection-evidence__metric").textContent =
      metric ?? rule.metric;
    required<HTMLElement>(item, ".selection-evidence__equation").textContent =
      `${value} ${comparator} ${formatSelectionValue(rule.threshold.value)}`;
    required<HTMLElement>(item, ".selection-evidence__state").textContent =
      selectionEvidenceStateLabel(evidence.state);
    elements.selectionEvidenceList.append(item);
  }
}

function formatSelectionValue(value: number): string {
  return Number.isInteger(value)
    ? value.toLocaleString("en-US")
    : value.toLocaleString("en-US", { maximumFractionDigits: 6 });
}

function selectionEvidenceStateLabel(
  state: FrameSelectionFrameResult["evidence"][number]["state"],
): string {
  switch (state) {
    case "passed":
      return "Pass";
    case "failed":
      return "Fail";
    case "missing_retained":
      return "Missing · retain";
    case "missing_rejected":
      return "Missing · reject";
  }
}

function automaticSelectionChangeCount(model: ReviewViewModel): number {
  if (!model.frameSelection.plan) return 0;
  const byIdentity = new Map(model.frames.map((frame) => [frame.id, frame]));
  return model.frameSelection.plan.frames.filter(
    (result) => byIdentity.get(result.frameId)?.state === "undecided",
  ).length;
}

function renderRows(
  container: HTMLTableSectionElement,
  model: ReviewViewModel,
  query: string,
): void {
  container.replaceChildren();
  const frames = filteredReviewFrames(model.frames, query);
  if (frames.length === 0) {
    const emptyRow = document.createElement("tr");
    const emptyCell = textCell(
      model.frames.length === 0
        ? "No frames loaded for this type"
        : "No frame names match this filter",
      "empty-row",
    );
    emptyCell.colSpan = 7;
    emptyRow.append(emptyCell);
    container.append(emptyRow);
    return;
  }
  for (const frame of frames) {
    const row = document.createElement("tr");
    const selected = frame.id === model.selectedFrameId;
    row.role = "row";
    row.tabIndex = selected ? 0 : -1;
    row.dataset.action = "select-frame";
    row.dataset.frameId = frame.id;
    row.dataset.state = frame.state;
    const selectionResult = model.frameSelection.plan?.frames.find(
      (candidate) => candidate.frameId === frame.id,
    );
    const proposal = selectionResult?.proposal;
    if (proposal) row.dataset.proposal = proposal;
    row.setAttribute("aria-selected", String(selected));
    if (selected) row.classList.add("is-selected");

    const state = cell("td", "state-cell");
    state.innerHTML = `<span class="state-marker" aria-hidden="true"></span><span class="sr-only"></span>`;
    required<HTMLElement>(state, ".state-marker").textContent = stateSymbol(
      frame.state,
    );
    required<HTMLElement>(state, ".sr-only").textContent = stateLabel(
      frame.state,
    );
    const frameName = textCell(frame.label, "frame-name");
    if (proposal) {
      const recommendation = document.createElement("span");
      recommendation.className = "selection-proposal";
      recommendation.dataset.proposal = proposal;
      const presentation = selectionProposalPresentation(
        model.frameSelection.plan,
        selectionResult,
      );
      recommendation.textContent = presentation.label;
      recommendation.title = presentation.description;
      recommendation.setAttribute("aria-label", presentation.description);
      frameName.append(" ", recommendation);
    }
    if (frame.classificationWarning) {
      const warning = document.createElement("span");
      warning.className = "classification-warning";
      warning.title = frame.classificationWarning;
      warning.setAttribute("aria-label", frame.classificationWarning);
      warning.textContent = "!";
      frameName.append(" ", warning);
    }
    row.append(
      state,
      frameName,
      textCell(
        frame.exposureSeconds === null
          ? "—"
          : `${frame.exposureSeconds.toFixed(1)} s`,
        "numeric",
      ),
      textCell(formatTemperature(frame.temperatureCelsius), "numeric"),
      textCell(formatMetric(frame.metrics.fwhmPixels, 2), "numeric"),
      textCell(formatMetric(frame.metrics.eccentricity, 2), "numeric"),
      textCell(
        frame.metrics.detectedStars === null
          ? "—"
          : String(frame.metrics.detectedStars),
        "numeric",
      ),
    );
    container.append(row);
  }
}

function filteredReviewFrames(
  frames: readonly ReviewFrame[],
  query: string,
): readonly ReviewFrame[] {
  const normalized = query.trim().toLocaleLowerCase("en-US");
  if (!normalized) return frames;
  return frames.filter((frame) =>
    frame.label.toLocaleLowerCase("en-US").includes(normalized),
  );
}

function updateFrameListSummary(
  elements: {
    readonly selectionCount: HTMLElement;
    readonly clearFrameFilter: HTMLButtonElement;
  },
  model: ReviewViewModel,
  query: string,
): void {
  const shown = filteredReviewFrames(model.frames, query).length;
  const activeRole = model.roles.find((role) => role.role === model.activeRole);
  elements.selectionCount.textContent = activeRole
    ? `${shown} shown · ${activeRole.count} total`
    : `${shown} shown`;
  elements.clearFrameFilter.disabled = query.length === 0;
}

function selectionProposalPresentation(
  plan: FrameSelectionPlan | null,
  result: FrameSelectionFrameResult | undefined,
): { readonly label: string; readonly description: string } {
  if (!plan || !result || result.proposal === "retain") {
    return {
      label: "AUTO KEEP",
      description:
        "Automatic recommendation: retain. All quality gates passed.",
    };
  }
  const failedMetrics = result.evidence.flatMap((evidence, index) => {
    if (evidence.state !== "failed" && evidence.state !== "missing_rejected") {
      return [];
    }
    const metric = plan.rules[index]?.metric;
    const label = selectionMetricOptions.find(
      ([candidate]) => candidate === metric,
    )?.[1];
    return label ? [label] : [];
  });
  const detail = failedMetrics.join(", ");
  return {
    label:
      failedMetrics.length === 1
        ? `AUTO REJECT · ${failedMetrics[0]?.toLocaleUpperCase("en-US")}`
        : failedMetrics.length > 1
          ? `AUTO REJECT · ${failedMetrics.length} GATES`
          : "AUTO REJECT",
    description: detail
      ? `Automatic recommendation: reject. Failed quality gates: ${detail}.`
      : "Automatic recommendation: reject.",
  };
}

function renderSelectedMetrics(
  elements: {
    state: HTMLElement;
    signalToNoise: HTMLElement;
    fwhm: HTMLElement;
    eccentricity: HTMLElement;
    stars: HTMLElement;
    background: HTMLElement;
    noise: HTMLElement;
  },
  frame: ReviewFrame | null,
): void {
  elements.state.dataset.state = frame?.state ?? "undecided";
  elements.state.textContent = frame ? stateLabel(frame.state) : "No selection";
  elements.signalToNoise.textContent = formatMetric(
    frame?.metrics.signalToNoise ?? null,
    1,
  );
  elements.fwhm.textContent = formatMetric(
    frame?.metrics.fwhmPixels ?? null,
    2,
  );
  elements.eccentricity.textContent = formatMetric(
    frame?.metrics.eccentricity ?? null,
    2,
  );
  elements.stars.textContent =
    frame?.metrics.detectedStars === null ||
    frame?.metrics.detectedStars === undefined
      ? "—"
      : String(frame.metrics.detectedStars);
  elements.background.textContent = formatMetric(
    frame?.metrics.background ?? null,
    1,
  );
  elements.noise.textContent = formatMetric(frame?.metrics.noise ?? null, 1);
}

function renderStatisticsPanel(
  elements: {
    statisticsDialog: HTMLElement;
    statisticsFrame: HTMLElement;
    statisticsStatus: HTMLElement;
    statisticsContent: HTMLElement;
    statisticsAlgorithm: HTMLElement;
    statisticsAxes: HTMLElement;
    statisticsFormat: HTMLElement;
    statisticsHeader: HTMLElement;
    statisticsUsable: HTMLElement;
    statisticsExcluded: HTMLElement;
    statisticsMinimum: HTMLElement;
    statisticsMaximum: HTMLElement;
    statisticsMean: HTMLElement;
    statisticsDeviation: HTMLElement;
    statisticsSampleDeviation: HTMLElement;
  },
  model: ReviewViewModel,
): void {
  const panel = model.statisticsPanel;
  const statistics = panel.statistics;
  elements.statisticsDialog.hidden = !panel.open;
  elements.statisticsFrame.textContent =
    panel.frameLabel ?? "No frame selected";
  elements.statisticsContent.hidden = panel.state !== "ready" || !statistics;
  elements.statisticsStatus.hidden = panel.state === "ready" && !!statistics;
  elements.statisticsStatus.dataset.state = panel.state;
  elements.statisticsStatus.textContent =
    panel.message ??
    (panel.state === "loading"
      ? "Reading the primary array in three deterministic passes…"
      : "Statistics are not available.");
  if (!statistics) return;

  elements.statisticsAlgorithm.textContent = statistics.algorithmId;
  elements.statisticsAxes.textContent = statistics.axes.join(" × ");
  elements.statisticsFormat.textContent = statistics.storedFormat;
  elements.statisticsHeader.textContent = statistics.headerConformant
    ? `Conformant · ${statistics.headerDiagnostics} diagnostics`
    : `Accepted with ${statistics.headerDiagnostics} diagnostics`;
  elements.statisticsUsable.textContent = `${formatCount(statistics.usableSamples)} / ${formatCount(statistics.totalSamples)}`;
  elements.statisticsExcluded.textContent = `${formatCount(statistics.undefinedSamples)} undefined · ${formatCount(statistics.nonFiniteSamples)} non-finite`;
  elements.statisticsMinimum.textContent = formatScientificValue(
    statistics.minimum,
  );
  elements.statisticsMaximum.textContent = formatScientificValue(
    statistics.maximum,
  );
  elements.statisticsMean.textContent = formatScientificValue(statistics.mean);
  elements.statisticsDeviation.textContent = formatScientificValue(
    statistics.populationStandardDeviation,
  );
  elements.statisticsSampleDeviation.textContent =
    statistics.sampleStandardDeviation === null
      ? "Undefined"
      : formatScientificValue(statistics.sampleStandardDeviation);
}

function renderSessionDiagnostics(
  elements: {
    diagnosticsState: HTMLElement;
    diagnosticsFiles: HTMLElement;
    diagnosticsBytes: HTMLElement;
    diagnosticsScanTime: HTMLElement;
    diagnosticsWorkers: HTMLElement;
    diagnosticsFrames: HTMLElement;
    diagnosticsConflicts: HTMLElement;
    diagnosticsFailures: HTMLElement;
    diagnosticsUnassigned: HTMLElement;
    diagnosticsRestored: HTMLElement;
    diagnosticsMissing: HTMLElement;
    diagnosticsRejected: HTMLElement;
    diagnosticsItems: HTMLOListElement;
    diagnosticsEmpty: HTMLElement;
    diagnosticsOmitted: HTMLElement;
    diagnosticsSearch: HTMLInputElement;
    diagnosticsFilters: readonly HTMLButtonElement[];
    diagnosticsEvidenceSummary: HTMLElement;
    diagnosticsExport: HTMLButtonElement;
    diagnosticsExportStatus: HTMLElement;
    diagnosticsInspect: HTMLButtonElement;
    diagnosticsInspectionStatus: HTMLElement;
    diagnosticsMaintenance: HTMLButtonElement;
    diagnosticsMaintenanceStatus: HTMLElement;
    diagnosticsMaintenanceFacts: HTMLElement;
    diagnosticsMaintenanceRemove: HTMLButtonElement;
    cacheMaintenanceSummary: HTMLElement;
    cacheMaintenanceSeal: HTMLElement;
  },
  model: ReviewViewModel,
  filter: DiagnosticsEvidenceFilter,
  query: string,
): void {
  const diagnostics = model.sessionDiagnostics;
  const attention =
    diagnostics.classificationConflicts +
    diagnostics.recoverableFailures +
    diagnostics.unassignedSources +
    diagnostics.qualityEvidenceRejected;
  elements.diagnosticsState.dataset.tone = attention > 0 ? "warning" : "ready";
  elements.diagnosticsState.textContent =
    attention > 0
      ? `${formatCount(attention)} item${attention === 1 ? "" : "s"} require attention`
      : "All mandatory FITS checks passed";
  elements.diagnosticsFiles.textContent = formatCount(
    diagnostics.filesConsidered,
  );
  elements.diagnosticsBytes.textContent = formatByteCount(
    diagnostics.fingerprintedSourceBytes,
  );
  elements.diagnosticsScanTime.textContent = formatElapsedMilliseconds(
    diagnostics.scanElapsedMilliseconds,
  );
  elements.diagnosticsWorkers.textContent = formatCount(
    diagnostics.sourceAnalysisParallelism,
  );
  elements.diagnosticsFrames.textContent = formatCount(
    diagnostics.verifiedFrames,
  );
  elements.diagnosticsConflicts.textContent = formatCount(
    diagnostics.classificationConflicts,
  );
  elements.diagnosticsFailures.textContent = formatCount(
    diagnostics.recoverableFailures,
  );
  elements.diagnosticsUnassigned.textContent = formatCount(
    diagnostics.unassignedSources,
  );
  elements.diagnosticsRestored.textContent = formatCount(
    diagnostics.qualityEvidenceRestored,
  );
  elements.diagnosticsMissing.textContent = formatCount(
    diagnostics.qualityEvidenceMissing,
  );
  elements.diagnosticsRejected.textContent = formatCount(
    diagnostics.qualityEvidenceRejected,
  );
  const normalizedQuery = query.trim().toLocaleLowerCase("en-US");
  const filteredItems = diagnostics.items.filter((item) => {
    const categoryMatches = filter === "all" || item.category === filter;
    const queryMatches =
      normalizedQuery.length === 0 ||
      item.source.toLocaleLowerCase("en-US").includes(normalizedQuery) ||
      item.code.toLocaleLowerCase("en-US").includes(normalizedQuery) ||
      diagnosticCategoryLabel(item.category)
        .toLocaleLowerCase("en-US")
        .includes(normalizedQuery);
    return categoryMatches && queryMatches;
  });
  if (elements.diagnosticsSearch.value !== query) {
    elements.diagnosticsSearch.value = query;
  }
  for (const button of elements.diagnosticsFilters) {
    button.setAttribute(
      "aria-pressed",
      String(button.dataset.diagnosticsFilter === filter),
    );
  }
  elements.diagnosticsEvidenceSummary.textContent = `${formatCount(filteredItems.length)} of ${formatCount(diagnostics.items.length)} displayed issues`;
  const rows = filteredItems.map((item) => {
    const row = document.createElement("li");
    row.className = "diagnostics-item";
    const category = document.createElement("span");
    category.className = "diagnostics-item__category";
    category.textContent = diagnosticCategoryLabel(item.category);
    const source = document.createElement("strong");
    source.textContent = item.source;
    source.title = item.source;
    const code = document.createElement("code");
    code.textContent = item.code;
    row.append(category, source, code);
    return row;
  });
  elements.diagnosticsItems.replaceChildren(...rows);
  elements.diagnosticsItems.hidden = rows.length === 0;
  elements.diagnosticsEmpty.hidden = rows.length > 0;
  elements.diagnosticsEmpty.textContent =
    diagnostics.items.length === 0
      ? "No source-level issue evidence is present."
      : "No issue evidence matches the current filter.";
  elements.diagnosticsOmitted.hidden = diagnostics.omittedItems === 0;
  elements.diagnosticsOmitted.textContent =
    diagnostics.omittedItems > 0
      ? `${formatCount(diagnostics.omittedItems)} additional item${diagnostics.omittedItems === 1 ? "" : "s"} omitted by the display bound`
      : "";
  elements.diagnosticsExport.disabled =
    !model.reviewSessionReady || diagnostics.exportState === "exporting";
  elements.diagnosticsExport.textContent =
    diagnostics.exportState === "exporting"
      ? "Exporting…"
      : "Export redacted JSON";
  elements.diagnosticsExportStatus.dataset.state = diagnostics.exportState;
  elements.diagnosticsExportStatus.textContent = diagnostics.exportMessage;
  elements.diagnosticsInspect.disabled =
    diagnostics.inspectionState === "inspecting";
  elements.diagnosticsInspect.textContent =
    diagnostics.inspectionState === "inspecting"
      ? "Verifying…"
      : "Verify report";
  elements.diagnosticsInspectionStatus.dataset.state =
    diagnostics.inspectionState;
  elements.diagnosticsInspectionStatus.textContent =
    diagnostics.inspectionMessage;
  elements.diagnosticsMaintenance.disabled =
    diagnostics.qualityEvidenceRejected === 0 ||
    diagnostics.maintenanceState === "inspecting" ||
    diagnostics.maintenanceState === "applying";
  elements.diagnosticsMaintenance.textContent =
    diagnostics.maintenanceState === "inspecting"
      ? "Inspecting…"
      : "Preview cleanup";
  elements.diagnosticsMaintenanceStatus.dataset.state =
    diagnostics.maintenanceState;
  elements.diagnosticsMaintenanceStatus.textContent =
    diagnostics.maintenanceMessage;
  elements.diagnosticsMaintenanceFacts.hidden =
    diagnostics.maintenanceState !== "ready";
  elements.diagnosticsMaintenanceFacts.textContent =
    diagnostics.maintenanceState === "ready"
      ? `${formatCount(diagnostics.maintenanceEligible)} removable · ${formatByteCount(diagnostics.maintenanceBytes)} · ${formatCount(diagnostics.maintenanceBlocked)} blocked · plan ${diagnostics.maintenancePlanSha256?.slice(0, 12) ?? "unsealed"}…`
      : "";
  elements.diagnosticsMaintenanceRemove.hidden =
    diagnostics.maintenanceState !== "ready" ||
    diagnostics.maintenanceEligible === 0;
  elements.diagnosticsMaintenanceRemove.disabled =
    diagnostics.maintenanceState === "applying";
  elements.cacheMaintenanceSummary.textContent = `${formatCount(diagnostics.maintenanceEligible)} rejected artifact${diagnostics.maintenanceEligible === 1 ? "" : "s"} · ${formatByteCount(diagnostics.maintenanceBytes)}`;
  elements.cacheMaintenanceSeal.textContent = diagnostics.maintenancePlanSha256
    ? `Plan sha256 ${diagnostics.maintenancePlanSha256}`
    : "No sealed maintenance plan";
}

function diagnosticCategoryLabel(
  category: ReviewViewModel["sessionDiagnostics"]["items"][number]["category"],
): string {
  switch (category) {
    case "classification":
      return "Classification";
    case "fits":
      return "FITS";
    case "grouping":
      return "Grouping";
    case "quality_cache":
      return "Quality cache";
  }
}

interface CalibrationElements {
  readonly calibrationStatus: HTMLElement;
  readonly calibrationProducts: HTMLElement;
  readonly calibrationDigest: HTMLElement;
  readonly pedestalPolicy: HTMLSelectElement;
  readonly exposureTolerance: HTMLInputElement;
  readonly temperatureTolerance: HTMLInputElement;
  readonly lightTemperatureTolerance: HTMLInputElement;
  readonly lightAssociations: HTMLElement;
  readonly lightDigest: HTMLElement;
  readonly lightOutputMode: HTMLSelectElement;
  readonly refreshMasterPlan: HTMLButtonElement;
  readonly executeMasterPlan: HTMLButtonElement;
  readonly cancelMasterPlan: HTMLButtonElement;
  readonly executeLightPlan: HTMLButtonElement;
  readonly cancelLightPlan: HTMLButtonElement;
  readonly masterExecution: HTMLElement;
  readonly masterExecutionMessage: HTMLElement;
  readonly masterExecutionProgress: HTMLProgressElement;
  readonly masterExecutionOutput: HTMLElement;
  readonly lightExecution: HTMLElement;
  readonly lightExecutionMessage: HTMLElement;
  readonly lightExecutionProgress: HTMLProgressElement;
  readonly lightExecutionOutput: HTMLElement;
  readonly lightExecutionHeading: HTMLElement;
  readonly defectCorrection: HTMLElement;
  readonly defectCorrectionMessage: HTMLElement;
  readonly defectCorrectionProgress: HTMLProgressElement;
  readonly defectCorrectionOutput: HTMLElement;
  readonly defectCorrectionEvidence: HTMLElement;
  readonly defectBatchReport: HTMLElement;
  readonly defectReportInspection: HTMLElement;
  readonly executeDefectCorrection: HTMLButtonElement;
  readonly executeAllDefectCorrections: HTMLButtonElement;
  readonly exportDefectBatchReport: HTMLButtonElement;
  readonly resumeDefectBatch: HTMLButtonElement;
  readonly inspectDefectBatchReport: HTMLButtonElement;
  readonly cancelDefectCorrection: HTMLButtonElement;
  readonly defectStrideButtons: readonly HTMLButtonElement[];
  readonly defectDetectionRadius: HTMLInputElement;
  readonly defectDetectionMinimumNeighbours: HTMLInputElement;
  readonly defectDarkHotSigma: HTMLInputElement;
  readonly defectDarkColdSigma: HTMLInputElement;
  readonly defectDarkFloor: HTMLInputElement;
  readonly defectFlatHotSigma: HTMLInputElement;
  readonly defectFlatColdSigma: HTMLInputElement;
  readonly defectFlatFloor: HTMLInputElement;
  readonly defectCorrectionRadius: HTMLInputElement;
  readonly defectCorrectionMinimumNeighbours: HTMLInputElement;
  readonly linearDefect: HTMLElement;
  readonly linearDefectMessage: HTMLElement;
  readonly linearDefectProgress: HTMLProgressElement;
  readonly linearDefectEvidence: HTMLElement;
  readonly linearDefectOutput: HTMLElement;
  readonly linearDefectAxisButtons: readonly HTMLButtonElement[];
  readonly linearDefectInputs: readonly HTMLInputElement[];
  readonly executeLinearDefectCorrection: HTMLButtonElement;
  readonly cancelLinearDefectCorrection: HTMLButtonElement;
  readonly defectPreviewButtons: readonly HTMLButtonElement[];
  readonly defectPreviewImage: HTMLImageElement;
  readonly defectPreviewPlaceholder: HTMLElement;
  readonly defectPreviewMessage: HTMLElement;
}

interface RegistrationElements {
  readonly registrationReference: HTMLSelectElement;
  readonly registrationSource: HTMLSelectElement;
  readonly registrationGeometryModels: readonly HTMLButtonElement[];
  readonly analyzeRegistration: HTMLButtonElement;
  readonly executeRegistration: HTMLButtonElement;
  readonly cancelRegistration: HTMLButtonElement;
  readonly drizzle: HTMLElement;
  readonly executeDrizzle: HTMLButtonElement;
  readonly cancelDrizzle: HTMLButtonElement;
  readonly drizzleScaleButtons: readonly HTMLButtonElement[];
  readonly drizzleWeightingButtons: readonly HTMLButtonElement[];
  readonly drizzleWeightStatus: HTMLElement;
  readonly drizzleDropShrink: HTMLInputElement;
  readonly drizzleDropShrinkValue: HTMLOutputElement;
  readonly drizzleMaximumContributions: HTMLInputElement;
  readonly drizzleMaximumContributionsValue: HTMLOutputElement;
  readonly drizzleMaximumBandHeight: HTMLInputElement;
  readonly drizzleMaximumBandHeightValue: HTMLOutputElement;
  readonly drizzleMessage: HTMLElement;
  readonly drizzleProgress: HTMLProgressElement;
  readonly drizzleOutput: HTMLElement;
  readonly drizzleProducts: readonly HTMLButtonElement[];
  readonly drizzlePreviewImage: HTMLImageElement;
  readonly drizzlePreviewPlaceholder: HTMLElement;
  readonly inspectDrizzleStatistics: HTMLButtonElement;
  readonly drizzlePixelX: HTMLInputElement;
  readonly drizzlePixelY: HTMLInputElement;
  readonly inspectDrizzlePixel: HTMLButtonElement;
  readonly drizzlePixelReadout: HTMLElement;
  readonly registrationStatus: HTMLElement;
  readonly registrationRms: HTMLElement;
  readonly registrationInliers: HTMLElement;
  readonly registrationCoverage: HTMLElement;
  readonly registrationRotation: HTMLElement;
  readonly registrationScale: HTMLElement;
  readonly registrationReflection: HTMLElement;
  readonly registrationSupport: HTMLElement;
  readonly registrationCropText: HTMLElement;
  readonly registrationCrop: HTMLElement;
  readonly registrationMatrix: HTMLElement;
  readonly registrationRejections: HTMLElement;
  readonly registrationPlanProgress: HTMLElement;
  readonly registrationPlanFrames: HTMLOListElement;
  readonly registrationPlanDigest: HTMLElement;
  readonly registrationExecution: HTMLElement;
  readonly registrationExecutionMessage: HTMLElement;
  readonly registrationExecutionProgress: HTMLProgressElement;
  readonly registrationExecutionOutput: HTMLElement;
  readonly executeRegisteredStack: HTMLButtonElement;
  readonly cancelRegisteredStack: HTMLButtonElement;
  readonly registeredStack: HTMLElement;
  readonly registeredStackMessage: HTMLElement;
  readonly registeredStackProgress: HTMLProgressElement;
  readonly registeredStackOutput: HTMLElement;
  readonly registeredStackSourceRejections: HTMLElement;
  readonly registeredStackSourceRejectionSummary: HTMLElement;
  readonly registeredStackSourceRejectionRows: HTMLElement;
  readonly registeredStackSpatialPromotions: HTMLElement;
  readonly registeredStackSpatialPromotionTotal: HTMLElement;
  readonly registeredStackSpatialPromotionBreakdown: HTMLElement;
  readonly registeredStackReport: HTMLElement;
  readonly registeredStackReportPath: HTMLElement;
  readonly registeredStackReportDigest: HTMLElement;
  readonly inspectRegisteredStackReport: HTMLButtonElement;
  readonly registeredStackReportSummary: HTMLElement;
  readonly registeredStackReportProducts: HTMLElement;
  readonly registeredStackReportSources: HTMLElement;
  readonly registeredStackSourceSummary: HTMLElement;
  readonly registeredStackSourceSearch: HTMLInputElement;
  readonly clearRegisteredStackSourceSearch: HTMLButtonElement;
  readonly showMoreRegisteredStackSources: HTMLButtonElement;
  readonly registeredStackSourceFilters: readonly HTMLButtonElement[];
  readonly verifyRegisteredStackSources: HTMLButtonElement;
  readonly cancelRegisteredStackSourceVerification: HTMLButtonElement;
  readonly registeredStackSourceProgress: HTMLProgressElement;
  readonly registeredStackSourceVerification: HTMLElement;
  readonly returnToActiveStack: HTMLButtonElement;
  readonly openRegisteredStackReport: HTMLButtonElement;
  readonly registeredStackModes: readonly HTMLButtonElement[];
  readonly registeredStackAutomaticPlan: HTMLElement;
  readonly registeredStackAutomaticState: HTMLElement;
  readonly registeredStackAutomaticTier: HTMLElement;
  readonly registeredStackAutomaticEstimator: HTMLElement;
  readonly registeredStackAutomaticRationale: HTMLElement;
  readonly registeredStackAutomaticSeal: HTMLElement;
  readonly registeredStackAdvanced: HTMLDetailsElement;
  readonly registeredStackEstimator: HTMLSelectElement;
  readonly registeredStackLowFraction: HTMLInputElement;
  readonly registeredStackHighFraction: HTMLInputElement;
  readonly registeredStackLowSigma: HTMLInputElement;
  readonly registeredStackHighSigma: HTMLInputElement;
  readonly registeredStackEsdOutlierFraction: HTMLInputElement;
  readonly registeredStackEsdSignificance: HTMLInputElement;
  readonly registeredStackMaximumIterations: HTMLInputElement;
  readonly registeredStackLargeScale: HTMLElement;
  readonly registeredStackLargeScaleLowEnabled: HTMLInputElement;
  readonly registeredStackLargeScaleHighEnabled: HTMLInputElement;
  readonly registeredStackLargeScaleLowLayers: HTMLInputElement;
  readonly registeredStackLargeScaleHighLayers: HTMLInputElement;
  readonly registeredStackLargeScaleLowGrowth: HTMLInputElement;
  readonly registeredStackLargeScaleHighGrowth: HTMLInputElement;
  readonly registeredStackMinimumRetained: HTMLInputElement;
  readonly registeredStackRejectionMaps: HTMLInputElement;
  readonly registeredStackSupportMap: HTMLInputElement;
  readonly registeredStackEstimatorLabel: HTMLElement;
  readonly registeredStackWeightPreflight: HTMLElement;
  readonly registeredStackWeightStatus: HTMLElement;
  readonly registeredStackWeightSeal: HTMLElement;
  readonly registeredStackWeightAlgorithm: HTMLElement;
  readonly registeredStackWeightDigest: HTMLElement;
  readonly registeredStackWeightReference: HTMLSelectElement;
  readonly registeredStackWeightRows: HTMLTableSectionElement;
  readonly registeredStackProducts: readonly HTMLButtonElement[];
  readonly registeredStackPreviewImage: HTMLImageElement;
  readonly registeredStackScienceImage: HTMLImageElement;
  readonly registeredStackOverlayControl: HTMLElement;
  readonly registeredStackOverlayOpacity: HTMLInputElement;
  readonly registeredStackOverlayValue: HTMLOutputElement;
  readonly registeredStackHistogram: HTMLElement;
  readonly registeredStackHistogramSummary: HTMLElement;
  readonly registeredStackHistogramBins: HTMLElement;
  readonly registeredStackPixelReadout: HTMLElement;
  readonly registeredStackPixelX: HTMLInputElement;
  readonly registeredStackPixelY: HTMLInputElement;
  readonly inspectRegisteredStackPixel: HTMLButtonElement;
  readonly registeredStackPreviewPlaceholder: HTMLElement;
  readonly registeredFrame: HTMLSelectElement;
  readonly registeredPreviewImage: HTMLImageElement;
  readonly registeredPreviewPlaceholder: HTMLElement;
  readonly registeredPreviewMessage: HTMLElement;
  readonly registeredPreviewPosition: HTMLElement;
  readonly registeredPreviewStretch: HTMLElement;
  readonly registeredPreviewPlay: HTMLButtonElement;
}

type SourceEvidenceFilter = "all" | "issues" | "verified" | "unverified";

type DiagnosticsEvidenceFilter =
  "all" | "classification" | "fits" | "grouping" | "quality_cache";

const SOURCE_EVIDENCE_PAGE_SIZE = 250;

function isSourceEvidenceFilter(
  value: string | undefined,
): value is SourceEvidenceFilter {
  return (
    value === "all" ||
    value === "issues" ||
    value === "verified" ||
    value === "unverified"
  );
}

function isDiagnosticsEvidenceFilter(
  value: string | undefined,
): value is DiagnosticsEvidenceFilter {
  return (
    value === "all" ||
    value === "classification" ||
    value === "fits" ||
    value === "grouping" ||
    value === "quality_cache"
  );
}

function renderRegistration(
  elements: RegistrationElements,
  model: ReviewViewModel,
  sourceEvidenceFilter: SourceEvidenceFilter,
  sourceEvidenceQuery: string,
  sourceEvidenceLimit: number,
): void {
  const registration = model.registration;
  const options = registration.frames.map((frame) => {
    const option = document.createElement("option");
    option.value = frame.id;
    option.textContent = frame.label;
    return option;
  });
  elements.registrationReference.replaceChildren(
    ...options.map((option) => option.cloneNode(true)),
  );
  elements.registrationSource.replaceChildren(...options);
  elements.registrationReference.value = registration.referenceFrameId ?? "";
  elements.registrationSource.value = registration.sourceFrameId ?? "";
  for (const button of elements.registrationGeometryModels) {
    const selected =
      button.dataset.registrationGeometryModel === registration.geometryModel;
    button.setAttribute("aria-pressed", String(selected));
    button.dataset.selected = String(selected);
  }

  const reference = registration.frames.find(
    (frame) => frame.id === registration.referenceFrameId,
  );
  const source = registration.frames.find(
    (frame) => frame.id === registration.sourceFrameId,
  );
  const executionBusy =
    registration.execution.state === "running" ||
    registration.execution.state === "cancelling";
  const stackBusy =
    registration.stack.state === "running" ||
    registration.stack.state === "cancelling";
  const drizzleBusy =
    registration.drizzle.state === "running" ||
    registration.drizzle.state === "cancelling";
  const normalizationBusy =
    model.localNormalization.state === "running" ||
    model.localNormalization.state === "cancelling";
  const running =
    registration.state === "running" ||
    registration.planState === "building" ||
    executionBusy ||
    stackBusy ||
    drizzleBusy ||
    normalizationBusy;
  const pairReady =
    !!reference?.sourcePath &&
    !!source?.sourcePath &&
    reference.id !== source.id;
  elements.registrationReference.disabled =
    running || registration.frames.length === 0;
  elements.registrationSource.disabled =
    running || registration.frames.length === 0;
  for (const button of elements.registrationGeometryModels) {
    button.disabled = running;
  }
  elements.analyzeRegistration.disabled = running || !pairReady;
  elements.analyzeRegistration.textContent = running
    ? "Solving geometry…"
    : "Analyze geometry";
  elements.registrationStatus.dataset.state = registration.state;
  elements.registrationStatus.textContent = registration.message;

  const requiredSolutions = Math.max(0, registration.frames.length - 1);
  const acceptedSourceIds = new Set(
    registration.solutions.map((solution) => solution.sourceFrameId),
  );
  elements.registrationPlanProgress.textContent =
    registration.planState === "building"
      ? "Native verification…"
      : registration.planState === "ready"
        ? `${registration.plan?.frames.length ?? 0} frames sealed`
        : `${acceptedSourceIds.size} / ${requiredSolutions} transforms accepted`;
  elements.registrationPlanProgress.dataset.ready = String(
    registration.planState === "ready",
  );
  elements.registrationPlanDigest.textContent = registration.plan
    ? `SHA-256 ${registration.plan.planSha256}`
    : "The final digest appears after native reconstruction";
  const calibratedFrames =
    model.calibration.lightExecution.result?.calibratedFrames ?? [];
  const artifactSetReady =
    registration.plan !== null &&
    registration.plan.frames.every((planned) =>
      calibratedFrames.some(
        (artifact) => artifact.sourceFrameId === planned.frameId,
      ),
    );
  elements.executeRegistration.disabled =
    registration.planState !== "ready" || !artifactSetReady || executionBusy;
  elements.executeRegistration.hidden = executionBusy;
  elements.cancelRegistration.hidden = !executionBusy;
  elements.cancelRegistration.disabled =
    registration.execution.state === "cancelling";
  elements.registrationExecution.dataset.state = registration.execution.state;
  elements.registrationExecutionMessage.textContent =
    registration.execution.message;
  elements.registrationExecutionOutput.textContent =
    registration.execution.outputDirectory ??
    (artifactSetReady
      ? "Calibrated identity set verified"
      : "Calibrated Light artifacts required");
  elements.registrationExecutionOutput.title =
    registration.execution.outputDirectory ?? "";
  const executionProgress = registration.execution.progress;
  if (executionProgress?.totalUnits) {
    elements.registrationExecutionProgress.max = executionProgress.totalUnits;
    elements.registrationExecutionProgress.value =
      executionProgress.completedUnits;
  } else {
    elements.registrationExecutionProgress.removeAttribute("value");
    elements.registrationExecutionProgress.max = 1;
  }
  elements.registrationExecutionProgress.hidden = !executionBusy;
  const drizzle = registration.drizzle;
  const drizzleArtifactSetReady =
    registration.plan !== null &&
    registration.plan.frames.every((planned) =>
      calibratedFrames.some(
        (artifact) => artifact.sourceFrameId === planned.frameId,
      ),
    );
  const drizzlePlanCurrent =
    drizzle.result === null ||
    drizzle.result.registrationPlanSha256 === registration.plan?.planSha256;
  elements.drizzle.dataset.state = drizzlePlanCurrent ? drizzle.state : "error";
  elements.drizzleMessage.textContent = drizzlePlanCurrent
    ? drizzle.message
    : "The registration plan changed · run Drizzle again";
  elements.drizzleOutput.textContent = drizzle.result
    ? `${drizzle.result.width} × ${drizzle.result.height} · RGB · ${drizzle.result.sourceCount} sources · ${drizzle.result.unsupportedPixels.toLocaleString("en-US")} unsupported px`
    : (drizzle.outputDirectory ??
      "Science + weight + support · atomic FITS set");
  elements.drizzleOutput.title = drizzle.result
    ? `${drizzle.result.sciencePath}\n${drizzle.result.weightPath}\n${drizzle.result.supportPath}`
    : (drizzle.outputDirectory ?? "");
  for (const button of elements.drizzleScaleButtons) {
    const selected =
      Number(button.dataset.drizzleScale) === drizzle.settings.scale;
    button.dataset.selected = String(selected);
    button.setAttribute("aria-pressed", String(selected));
    button.disabled = running;
  }
  const drizzleWeightEvidence = buildQualityWeightPreflight(
    registration.plan,
    model.activeRole === "light" && model.lightFrameView === "calibrated"
      ? model.frames
      : [],
    drizzle.weightReferenceFrameId,
  );
  for (const button of elements.drizzleWeightingButtons) {
    const selected = button.dataset.drizzleWeighting === drizzle.weighting;
    button.dataset.selected = String(selected);
    button.setAttribute("aria-pressed", String(selected));
    button.disabled = running;
  }
  elements.drizzleWeightStatus.dataset.ready = String(
    drizzle.weighting === "uniform" || drizzleWeightEvidence.ready,
  );
  const currentDrizzleWeightSeal =
    drizzle.weightPreflight?.planSha256 === registration.plan?.planSha256
      ? drizzle.weightPreflight
      : null;
  elements.drizzleWeightStatus.textContent =
    drizzle.weighting === "uniform"
      ? "Equal identity-bound contribution per frame"
      : currentDrizzleWeightSeal
        ? `${currentDrizzleWeightSeal.weights.length} balanced weights sealed in Rust`
        : drizzleWeightEvidence.ready
          ? `${drizzleWeightEvidence.evidence.length} frames ready · native seal required`
          : `${drizzleWeightEvidence.evidence.length} / ${drizzleWeightEvidence.rows.length} frames have complete metrics`;
  elements.drizzleDropShrink.value = String(drizzle.settings.dropShrink);
  elements.drizzleDropShrinkValue.value =
    drizzle.settings.dropShrink.toFixed(2);
  elements.drizzleDropShrinkValue.textContent =
    elements.drizzleDropShrinkValue.value;
  elements.drizzleMaximumContributions.value = String(
    drizzle.settings.maximumContributions,
  );
  elements.drizzleMaximumContributionsValue.value = String(
    drizzle.settings.maximumContributions,
  );
  elements.drizzleMaximumContributionsValue.textContent =
    elements.drizzleMaximumContributionsValue.value;
  elements.drizzleMaximumBandHeight.value = String(
    drizzle.settings.maximumBandHeight,
  );
  elements.drizzleMaximumBandHeightValue.value = `${drizzle.settings.maximumBandHeight} px`;
  elements.drizzleMaximumBandHeightValue.textContent =
    elements.drizzleMaximumBandHeightValue.value;
  elements.drizzleDropShrink.disabled = running;
  elements.drizzleMaximumContributions.disabled = running;
  elements.drizzleMaximumBandHeight.disabled = running;
  elements.executeDrizzle.disabled =
    registration.planState !== "ready" ||
    !drizzleArtifactSetReady ||
    (drizzle.weighting === "balanced_psf" && !drizzleWeightEvidence.ready) ||
    executionBusy ||
    stackBusy ||
    normalizationBusy ||
    drizzleBusy;
  elements.executeDrizzle.hidden = drizzleBusy;
  elements.cancelDrizzle.hidden = !drizzleBusy;
  elements.cancelDrizzle.disabled = drizzle.state === "cancelling";
  if (drizzle.progress?.totalUnits) {
    elements.drizzleProgress.max = drizzle.progress.totalUnits;
    elements.drizzleProgress.value = drizzle.progress.completedUnits;
  } else {
    elements.drizzleProgress.removeAttribute("value");
    elements.drizzleProgress.max = 1;
  }
  elements.drizzleProgress.hidden = !drizzleBusy;
  for (const button of elements.drizzleProducts) {
    const product = button.dataset.drizzleProduct;
    const selected = product === drizzle.selectedProduct;
    button.disabled = drizzle.result === null || drizzleBusy;
    button.setAttribute("aria-selected", String(selected));
    button.tabIndex = selected ? 0 : -1;
  }
  const drizzlePreviewReady =
    drizzle.previewState === "ready" &&
    drizzle.preview?.frameId ===
      `${drizzle.result?.drizzlePlanSha256}:${drizzle.selectedProduct}`;
  elements.drizzlePreviewImage.hidden = !drizzlePreviewReady;
  elements.drizzlePreviewImage.src = drizzlePreviewReady
    ? (drizzle.preview?.url ?? "")
    : "";
  elements.drizzlePreviewImage.alt = drizzlePreviewReady
    ? `${humanize(drizzle.selectedProduct)} Drizzle product preview`
    : "";
  elements.drizzlePreviewPlaceholder.hidden = drizzlePreviewReady;
  elements.drizzlePreviewPlaceholder.textContent =
    drizzle.previewState === "loading"
      ? `Rendering the ${drizzle.selectedProduct} product…`
      : drizzle.previewState === "error"
        ? "The FITS product is valid, but its display preview is unavailable"
        : "The atomic Drizzle product set will appear here";
  elements.inspectDrizzleStatistics.disabled =
    drizzle.result === null || drizzleBusy;
  renderDrizzlePixelReadout(elements, drizzle);
  const registeredSetReady =
    registration.execution.state === "completed" &&
    registration.execution.result?.planSha256 === registration.plan?.planSha256;
  const stackSettings = registration.stack.settings;
  const automaticMode = registration.stack.integrationMode === "automatic";
  const automaticPreview = registration.stack.automaticPreview;
  const automaticReady =
    automaticMode &&
    registration.stack.automaticPreviewState === "ready" &&
    automaticPreview?.registrationPlanSha256 === registration.plan?.planSha256;
  for (const button of elements.registeredStackModes) {
    const selected =
      button.dataset.stackMode === registration.stack.integrationMode;
    button.setAttribute("aria-pressed", String(selected));
    button.disabled = stackBusy;
  }
  elements.registeredStackAutomaticPlan.hidden = !automaticMode;
  elements.registeredStackAutomaticPlan.dataset.state =
    registration.stack.automaticPreviewState;
  elements.registeredStackAutomaticState.textContent =
    registration.stack.automaticPreviewState === "loading"
      ? "Resolving…"
      : automaticReady
        ? "Sealed"
        : registration.stack.automaticPreviewState === "error"
          ? "Unavailable"
          : "Not requested";
  elements.registeredStackAutomaticTier.textContent = automaticPreview
    ? `${humanize(automaticPreview.populationTier)} · ${automaticPreview.sourceCount.toLocaleString("en-US")} frames`
    : "—";
  elements.registeredStackAutomaticEstimator.textContent = automaticPreview
    ? humanize(automaticPreview.settings.estimator)
    : "—";
  elements.registeredStackAutomaticRationale.textContent =
    automaticPreview?.rationale ??
    "The sealed registration population determines the recommendation.";
  elements.registeredStackAutomaticSeal.textContent = automaticPreview
    ? `sha256 ${automaticPreview.automaticPlanSha256}`
    : "Awaiting native seal";
  elements.registeredStackAdvanced.hidden = automaticMode;
  const weightPreflight = buildQualityWeightPreflight(
    registration.plan,
    model.activeRole === "light" && model.lightFrameView === "calibrated"
      ? model.frames
      : [],
    stackSettings.weightReferenceFrameId,
  );
  const weightedEstimator = stackSettings.estimator === "weighted_mean";
  elements.executeRegisteredStack.disabled =
    !registeredSetReady ||
    stackBusy ||
    (automaticMode && !automaticReady) ||
    (weightedEstimator && !weightPreflight.ready);
  elements.executeRegisteredStack.hidden = stackBusy;
  elements.cancelRegisteredStack.hidden = !stackBusy;
  elements.cancelRegisteredStack.disabled =
    registration.stack.state === "cancelling";
  elements.registeredStack.dataset.state = registration.stack.state;
  elements.registeredStackMessage.textContent = registration.stack.message;
  elements.registeredStackOutput.textContent =
    registration.stack.outputPath ??
    (registeredSetReady
      ? "Registered identity set verified"
      : "Published registered artifacts required");
  elements.registeredStackOutput.title = registration.stack.outputPath ?? "";
  const integrationReport = registration.stack.result;
  const sourceDispositions = integrationReport?.sourceDispositions ?? [];
  const spatialPromotions = integrationReport?.spatialPromotions ?? null;
  const sourceLabels = new Map(
    registration.frames.map((frame) => [frame.id, frame.label]),
  );
  const totalAttributedSamples = sourceDispositions.reduce(
    (total, source) => total + source.total,
    0,
  );
  const totalRejectedSamples = sourceDispositions.reduce(
    (total, source) => total + source.rejectedLow + source.rejectedHigh,
    0,
  );
  const rejectedFraction =
    totalAttributedSamples === 0
      ? 0
      : (totalRejectedSamples / totalAttributedSamples) * 100;
  elements.registeredStackSourceRejections.hidden =
    sourceDispositions.length === 0;
  elements.registeredStackSourceRejectionSummary.textContent =
    sourceDispositions.length === 0
      ? "No per-source attribution for this estimator"
      : `${formatCountedNoun(sourceDispositions.length, "source")} · ${totalRejectedSamples.toLocaleString("en-US")} rejected samples · ${rejectedFraction.toFixed(4)}%`;
  elements.registeredStackSpatialPromotions.hidden = spatialPromotions === null;
  elements.registeredStackSpatialPromotionTotal.textContent =
    spatialPromotions === null
      ? ""
      : `${spatialPromotions.total.toLocaleString("en-US")} additional spatial ${spatialPromotions.total === 1 ? "rejection" : "rejections"}`;
  elements.registeredStackSpatialPromotionBreakdown.textContent =
    spatialPromotions === null
      ? ""
      : `${spatialPromotions.low.toLocaleString("en-US")} low tail · ${spatialPromotions.high.toLocaleString("en-US")} high tail`;
  const rankedSourceDispositions = [...sourceDispositions]
    .sort(
      (left, right) =>
        right.rejectedLow +
        right.rejectedHigh -
        (left.rejectedLow + left.rejectedHigh),
    )
    .slice(0, 50);
  elements.registeredStackSourceRejectionRows.replaceChildren(
    ...rankedSourceDispositions.map((source) => {
      const row = document.createElement("li");
      const identity = document.createElement("span");
      identity.textContent = sourceLabels.get(source.frameId) ?? source.frameId;
      identity.title = source.frameId;
      const rejected = source.rejectedLow + source.rejectedHigh;
      const fraction = source.total === 0 ? 0 : (rejected / source.total) * 100;
      const metrics = document.createElement("span");
      metrics.className = "source-rejection-evidence__metrics";
      const evidence = document.createElement("span");
      evidence.textContent = `${rejected.toLocaleString("en-US")} rejected (${fraction.toFixed(4)}%) · ${source.rejectedLow.toLocaleString("en-US")} low · ${source.rejectedHigh.toLocaleString("en-US")} high · ${source.masked.toLocaleString("en-US")} masked · ${source.nonFinite.toLocaleString("en-US")} non-finite`;
      metrics.append(evidence);
      if (
        source.spatialPromotions !== null &&
        source.spatialPromotions.total > 0
      ) {
        const promoted = document.createElement("span");
        promoted.className = "source-rejection-evidence__spatial";
        promoted.textContent = `Spatial +${source.spatialPromotions.total.toLocaleString("en-US")} · ${source.spatialPromotions.low.toLocaleString("en-US")} low · ${source.spatialPromotions.high.toLocaleString("en-US")} high`;
        metrics.append(promoted);
      }
      row.append(identity, metrics);
      return row;
    }),
  );
  const reportInspection = registration.stack.reportInspection;
  const reportPath =
    registration.stack.reportInspectionPath ?? integrationReport?.reportPath;
  const reportDigest =
    reportInspection?.reportSha256 ??
    (integrationReport !== null && reportPath === integrationReport.reportPath
      ? integrationReport.reportSha256
      : null);
  elements.registeredStackReport.hidden = reportPath === null;
  elements.registeredStackReportPath.textContent = reportPath ?? "";
  elements.registeredStackReportPath.title = reportPath ?? "";
  elements.registeredStackReportDigest.textContent =
    reportDigest ?? "Digest available after native verification";
  elements.registeredStackReportDigest.title = reportDigest ?? "";
  elements.inspectRegisteredStackReport.disabled =
    reportPath === null ||
    registration.stack.reportInspectionState === "loading";
  elements.openRegisteredStackReport.disabled =
    registration.stack.reportInspectionState === "loading";
  elements.inspectRegisteredStackReport.textContent =
    registration.stack.reportInspectionState === "loading"
      ? "Verifying…"
      : reportInspection
        ? "Verify again"
        : "Verify report";
  elements.registeredStackReport.dataset.state =
    registration.stack.reportInspectionState;
  elements.registeredStackReportSummary.textContent = reportInspection
    ? `${formatCountedNoun(reportInspection.sourceCount, "source")} · ${formatCountedNoun(reportInspection.productCount, "product")} · ${formatEstimatorName(reportInspection.estimator)} · ${reportInspection.geometryModel ? `${reportInspection.geometryModel} geometry` : "legacy geometry unspecified"} · schema ${reportInspection.schemaVersion} · ${reportInspection.allProductsVerified ? "all FITS verified" : "product evidence incomplete"}`
    : registration.stack.reportInspectionState === "error"
      ? "Native verification failed · report evidence is not trusted"
      : "Verify natively before using this provenance as evidence";
  elements.registeredStackReportProducts.replaceChildren(
    ...(reportInspection?.products ?? []).map((product) => {
      const row = document.createElement("li");
      row.dataset.status = product.status;
      const identity = document.createElement("span");
      identity.textContent = `${formatRegisteredProductRole(product.role)} · ${product.fileName}`;
      identity.title = product.path;
      const evidence = document.createElement("span");
      evidence.textContent = `${formatByteCount(product.bytesWritten)} · ${formatRegisteredProductStatus(product.status)}`;
      row.append(identity, evidence);
      return row;
    }),
  );
  const sourceVerification = registration.stack.sourceVerification;
  const verifiedSources = new Map(
    (sourceVerification?.sources ?? []).map((source) => [
      source.frameId,
      source,
    ]),
  );
  const sourceVerificationBusy =
    registration.stack.sourceVerificationState === "loading" ||
    registration.stack.sourceVerificationState === "cancelling";
  elements.verifyRegisteredStackSources.disabled =
    reportInspection === null || sourceVerificationBusy;
  elements.verifyRegisteredStackSources.hidden = sourceVerificationBusy;
  elements.cancelRegisteredStackSourceVerification.hidden =
    !sourceVerificationBusy;
  elements.cancelRegisteredStackSourceVerification.disabled =
    registration.stack.sourceVerificationState === "cancelling";
  elements.verifyRegisteredStackSources.textContent = sourceVerification
    ? "Verify another folder"
    : "Verify source folder";
  const sourceProgress = registration.stack.sourceVerificationProgress;
  elements.registeredStackSourceProgress.hidden = !sourceVerificationBusy;
  elements.registeredStackSourceProgress.max =
    sourceProgress?.totalBytes || sourceProgress?.totalSources || 1;
  elements.registeredStackSourceProgress.value =
    sourceProgress?.completedBytes || sourceProgress?.completedSources || 0;
  elements.registeredStackSourceVerification.textContent = sourceVerification
    ? `${sourceVerification.allSourcesVerified ? "All source fingerprints verified" : "Source evidence mismatch"} · ${sourceVerification.sourceDirectory}`
    : sourceVerificationBusy
      ? `${registration.stack.sourceVerificationState === "cancelling" ? "Cancelling after the current 64 KiB block" : "Hashing archived sources"} · ${sourceProgress?.completedSources ?? 0}/${sourceProgress?.totalSources ?? reportInspection?.sourceCount ?? 0}${sourceProgress?.totalBytes ? ` · ${formatByteCount(sourceProgress.completedBytes)}/${formatByteCount(sourceProgress.totalBytes)}` : ""}${sourceProgress?.currentFileName ? ` · ${sourceProgress.currentFileName}${sourceProgress.currentFileTotalBytes ? ` ${Math.min(100, Math.round((sourceProgress.currentFileBytes / sourceProgress.currentFileTotalBytes) * 100))}%` : ""}` : ""}`
      : registration.stack.sourceVerificationState === "error"
        ? "Source verification failed safely"
        : "Choose the directory containing the registered source FITS files";
  const sourceEvidence = (reportInspection?.sources ?? []).map((source) => ({
    source,
    status: verifiedSources.get(source.frameId)?.status ?? "unverified",
  }));
  const verifiedSourceCount = sourceEvidence.filter(
    ({ status }) => status === "verified",
  ).length;
  const unverifiedSourceCount = sourceEvidence.filter(
    ({ status }) => status === "unverified",
  ).length;
  const issueSourceCount =
    sourceEvidence.length - verifiedSourceCount - unverifiedSourceCount;
  elements.registeredStackSourceSearch.value = sourceEvidenceQuery;
  elements.clearRegisteredStackSourceSearch.hidden = sourceEvidenceQuery === "";
  for (const button of elements.registeredStackSourceFilters) {
    const selected =
      button.dataset.sourceEvidenceFilter === sourceEvidenceFilter;
    button.setAttribute("aria-pressed", String(selected));
  }
  const normalizedSourceQuery = sourceEvidenceQuery
    .trim()
    .toLocaleLowerCase("en-US");
  const visibleSourceEvidence = sourceEvidence.filter(({ source, status }) => {
    const matchesStatus =
      sourceEvidenceFilter === "all" ||
      (sourceEvidenceFilter === "issues"
        ? status !== "verified" && status !== "unverified"
        : status === sourceEvidenceFilter);
    const matchesQuery =
      normalizedSourceQuery === "" ||
      source.fileName
        .toLocaleLowerCase("en-US")
        .includes(normalizedSourceQuery);
    return matchesStatus && matchesQuery;
  });
  const renderedSourceEvidence = visibleSourceEvidence.slice(
    0,
    sourceEvidenceLimit,
  );
  const remainingSourceEvidence =
    visibleSourceEvidence.length - renderedSourceEvidence.length;
  elements.registeredStackSourceSummary.textContent = `${formatCountedNoun(sourceEvidence.length, "source")} · ${verifiedSourceCount} verified · ${issueSourceCount} ${issueSourceCount === 1 ? "issue" : "issues"} · ${unverifiedSourceCount} pending · ${renderedSourceEvidence.length} shown${remainingSourceEvidence > 0 ? ` · ${visibleSourceEvidence.length} matching` : ""}`;
  elements.showMoreRegisteredStackSources.hidden =
    remainingSourceEvidence === 0;
  elements.showMoreRegisteredStackSources.textContent =
    remainingSourceEvidence > 0
      ? `Show next ${Math.min(SOURCE_EVIDENCE_PAGE_SIZE, remainingSourceEvidence)}`
      : "All matching sources shown";
  const sourceRows = renderedSourceEvidence.map(({ source, status }) => {
    const verification = verifiedSources.get(source.frameId);
    const row = document.createElement("li");
    row.dataset.status = status;
    const identity = document.createElement("span");
    identity.textContent = `${source.fileName} · ${formatByteCount(source.byteLength)}`;
    const seals = document.createElement("code");
    seals.textContent = verification
      ? formatRegisteredSourceStatus(verification.status)
      : `frame ${source.frameId.slice(0, 12)}… · sha256 ${source.sha256.slice(0, 12)}…`;
    seals.title = `Frame identity: ${source.frameId}\nSource SHA-256: ${source.sha256}`;
    row.append(identity);
    if (
      source.spatialPromotions !== null &&
      source.spatialPromotions.total > 0
    ) {
      const promotions = document.createElement("span");
      promotions.className = "integration-report__source-promotions";
      promotions.textContent = `Spatial +${source.spatialPromotions.total.toLocaleString("en-US")}`;
      promotions.title = `${source.spatialPromotions.low.toLocaleString("en-US")} low-tail and ${source.spatialPromotions.high.toLocaleString("en-US")} high-tail promotions`;
      row.append(promotions);
    }
    row.append(seals);
    return row;
  });
  if (sourceEvidence.length > 0 && sourceRows.length === 0) {
    const empty = document.createElement("li");
    empty.dataset.empty = "true";
    empty.textContent = "No sources match the current filters.";
    sourceRows.push(empty);
  }
  elements.registeredStackReportSources.replaceChildren(...sourceRows);
  const reportIsExternal =
    reportInspection !== null &&
    (integrationReport === null || reportPath !== integrationReport.reportPath);
  elements.returnToActiveStack.hidden = !reportIsExternal;
  elements.returnToActiveStack.textContent = integrationReport
    ? "Back to active result"
    : "Close archived report";
  const previewResult = reportIsExternal ? null : integrationReport;
  const rejectionEstimator = stackSettings.estimator === "percentile_clipped";
  const sigmaEstimator =
    stackSettings.estimator === "sigma_clipped" ||
    stackSettings.estimator === "winsorized_sigma_clipped";
  const linearFitEstimator = stackSettings.estimator === "linear_fit_clipped";
  const generalizedEsdEstimator = stackSettings.estimator === "generalized_esd";
  const residualEstimator = sigmaEstimator || linearFitEstimator;
  const winsorizedSigmaEstimator =
    stackSettings.estimator === "winsorized_sigma_clipped";
  const medianEstimator = stackSettings.estimator === "median";
  elements.registeredStackEstimator.value = stackSettings.estimator;
  elements.registeredStackLowFraction.value = String(stackSettings.lowFraction);
  elements.registeredStackHighFraction.value = String(
    stackSettings.highFraction,
  );
  elements.registeredStackLowSigma.value = String(stackSettings.lowSigma);
  elements.registeredStackHighSigma.value = String(stackSettings.highSigma);
  elements.registeredStackEsdOutlierFraction.value = String(
    stackSettings.esdOutlierFraction,
  );
  elements.registeredStackEsdSignificance.value = String(
    stackSettings.esdSignificance,
  );
  elements.registeredStackMaximumIterations.value = String(
    stackSettings.maximumIterations,
  );
  elements.registeredStackLargeScaleLowEnabled.checked =
    stackSettings.largeScaleLowEnabled;
  elements.registeredStackLargeScaleHighEnabled.checked =
    stackSettings.largeScaleHighEnabled;
  elements.registeredStackLargeScaleLowLayers.value = String(
    stackSettings.largeScaleLowLayers,
  );
  elements.registeredStackLargeScaleHighLayers.value = String(
    stackSettings.largeScaleHighLayers,
  );
  elements.registeredStackLargeScaleLowGrowth.value = String(
    stackSettings.largeScaleLowGrowth,
  );
  elements.registeredStackLargeScaleHighGrowth.value = String(
    stackSettings.largeScaleHighGrowth,
  );
  elements.registeredStackMinimumRetained.value = String(
    stackSettings.minimumRetainedSamples,
  );
  elements.registeredStackMinimumRetained.min =
    linearFitEstimator || generalizedEsdEstimator ? "3" : "1";
  elements.registeredStackRejectionMaps.checked =
    stackSettings.generateRejectionMaps;
  elements.registeredStackSupportMap.checked = stackSettings.generateSupportMap;
  elements.registeredStackEstimator.disabled = stackBusy;
  elements.registeredStackLowFraction.disabled =
    stackBusy || !rejectionEstimator;
  elements.registeredStackHighFraction.disabled =
    stackBusy || !rejectionEstimator;
  elements.registeredStackLowSigma.disabled = stackBusy || !residualEstimator;
  elements.registeredStackHighSigma.disabled = stackBusy || !residualEstimator;
  elements.registeredStackEsdOutlierFraction.disabled =
    stackBusy || !generalizedEsdEstimator;
  elements.registeredStackEsdSignificance.disabled =
    stackBusy || !generalizedEsdEstimator;
  elements.registeredStackMaximumIterations.disabled =
    stackBusy || !sigmaEstimator;
  elements.registeredStackLargeScale.hidden = !generalizedEsdEstimator;
  elements.registeredStackLargeScaleLowEnabled.disabled =
    stackBusy || !generalizedEsdEstimator;
  elements.registeredStackLargeScaleHighEnabled.disabled =
    stackBusy || !generalizedEsdEstimator;
  elements.registeredStackLargeScaleLowLayers.disabled =
    stackBusy || !stackSettings.largeScaleLowEnabled;
  elements.registeredStackLargeScaleLowGrowth.disabled =
    stackBusy || !stackSettings.largeScaleLowEnabled;
  elements.registeredStackLargeScaleHighLayers.disabled =
    stackBusy || !stackSettings.largeScaleHighEnabled;
  elements.registeredStackLargeScaleHighGrowth.disabled =
    stackBusy || !stackSettings.largeScaleHighEnabled;
  elements.registeredStackMinimumRetained.disabled =
    stackBusy ||
    (!rejectionEstimator && !residualEstimator && !generalizedEsdEstimator);
  elements.registeredStackRejectionMaps.disabled =
    stackBusy ||
    (!rejectionEstimator && !residualEstimator && !generalizedEsdEstimator);
  elements.registeredStackSupportMap.disabled = stackBusy;
  setControlFieldVisibility(
    elements.registeredStackLowFraction,
    rejectionEstimator,
  );
  setControlFieldVisibility(
    elements.registeredStackHighFraction,
    rejectionEstimator,
  );
  setControlFieldVisibility(
    elements.registeredStackLowSigma,
    residualEstimator,
  );
  setControlFieldVisibility(
    elements.registeredStackHighSigma,
    residualEstimator,
  );
  setControlFieldVisibility(
    elements.registeredStackEsdOutlierFraction,
    generalizedEsdEstimator,
  );
  setControlFieldVisibility(
    elements.registeredStackEsdSignificance,
    generalizedEsdEstimator,
  );
  setControlFieldVisibility(
    elements.registeredStackMaximumIterations,
    sigmaEstimator,
  );
  setControlFieldVisibility(
    elements.registeredStackMinimumRetained,
    rejectionEstimator || residualEstimator || generalizedEsdEstimator,
  );
  const mapToggle = elements.registeredStackRejectionMaps.closest<HTMLElement>(
    ".registered-stack__map-toggle",
  );
  if (mapToggle)
    mapToggle.hidden =
      !rejectionEstimator && !residualEstimator && !generalizedEsdEstimator;
  elements.registeredStackEstimatorLabel.textContent = weightedEstimator
    ? "BALANCED PSF WEIGHT"
    : rejectionEstimator
      ? "PERCENTILE F64"
      : winsorizedSigmaEstimator
        ? "WINSORIZED SIGMA F64"
        : linearFitEstimator
          ? "LINEAR FIT F64"
          : generalizedEsdEstimator
            ? "GENERALIZED ESD F64"
            : sigmaEstimator
              ? "ITERATIVE SIGMA F64"
              : medianEstimator
                ? "EXACT F64 MEDIAN"
                : "STRICT F64 MEAN";
  elements.registeredStackWeightPreflight.hidden = !weightedEstimator;
  elements.registeredStackWeightPreflight.dataset.ready = String(
    weightPreflight.ready,
  );
  const automaticReference = document.createElement("option");
  automaticReference.value = "";
  const recommended = weightPreflight.rows.find(
    (row) => row.frameId === weightPreflight.recommendedReferenceFrameId,
  );
  automaticReference.textContent = recommended
    ? `Auto · ${recommended.label}`
    : "Auto · awaiting complete metrics";
  elements.registeredStackWeightReference.replaceChildren(
    automaticReference,
    ...weightPreflight.rows.map((row) => {
      const option = document.createElement("option");
      option.value = row.frameId;
      option.textContent = row.label;
      option.disabled = row.evidence === null;
      return option;
    }),
  );
  elements.registeredStackWeightReference.value =
    stackSettings.weightReferenceFrameId ?? "";
  elements.registeredStackWeightReference.disabled =
    stackBusy || weightPreflight.rows.length === 0;
  elements.registeredStackWeightStatus.textContent = weightPreflight.ready
    ? `${weightPreflight.rows.length} / ${weightPreflight.rows.length} frames ready · native recomputation required`
    : `${weightPreflight.evidence.length} / ${weightPreflight.rows.length} frames have valid metrics`;
  const nativeWeightSeal = registration.stack.weightPreflight;
  const currentWeightSeal =
    weightedEstimator &&
    weightPreflight.ready &&
    nativeWeightSeal !== null &&
    nativeWeightSeal.planSha256 === registration.plan?.planSha256 &&
    nativeWeightSeal.referenceFrameId === weightPreflight.referenceFrameId &&
    nativeWeightSeal.weights.length === weightPreflight.rows.length &&
    weightPreflight.rows.every((row) =>
      nativeWeightSeal.weights.some((weight) => weight.frameId === row.frameId),
    )
      ? nativeWeightSeal
      : null;
  elements.registeredStackWeightSeal.hidden = currentWeightSeal === null;
  elements.registeredStackWeightAlgorithm.textContent =
    currentWeightSeal?.algorithmId ?? "";
  elements.registeredStackWeightDigest.textContent =
    currentWeightSeal?.parametersSha256 ?? "";
  elements.registeredStackWeightDigest.title =
    currentWeightSeal?.parametersSha256 ?? "";
  if (currentWeightSeal) {
    elements.registeredStackWeightStatus.textContent = `${currentWeightSeal.weights.length} / ${weightPreflight.rows.length} frames sealed natively`;
  }
  elements.registeredStackWeightRows.replaceChildren(
    ...weightPreflight.rows.map((row) => {
      const tableRow = document.createElement("tr");
      if (row.issue) tableRow.dataset.state = "blocked";
      const name = document.createElement("th");
      name.scope = "row";
      name.textContent = row.reference ? `${row.label} · reference` : row.label;
      const snr = document.createElement("td");
      snr.textContent = formatWeightMetric(row.evidence?.signalToNoise, 2);
      const fwhm = document.createElement("td");
      fwhm.textContent = formatWeightMetric(row.evidence?.fwhmPixels, 3);
      const eccentricity = document.createElement("td");
      eccentricity.textContent = formatWeightMetric(
        row.evidence?.eccentricity,
        3,
      );
      const weight = document.createElement("td");
      weight.textContent = row.issue
        ? row.issue
        : formatRelativeWeight(row.relativeWeight);
      tableRow.append(name, snr, fwhm, eccentricity, weight);
      return tableRow;
    }),
  );
  for (const button of elements.registeredStackProducts) {
    const product = button.dataset.stackProduct;
    const reportProductAvailable = reportInspection?.products.some(
      (candidate) =>
        candidate.role === product && candidate.status === "verified",
    );
    const available = previewResult
      ? product === "science"
        ? true
        : product === "rejection_low"
          ? Boolean(previewResult.lowRejectionMapPath)
          : product === "rejection_high"
            ? Boolean(previewResult.highRejectionMapPath)
            : product === "support"
              ? Boolean(previewResult.supportMapPath)
              : false
      : Boolean(reportProductAvailable);
    const selected = product === registration.stack.selectedProduct;
    button.disabled = !available || stackBusy;
    button.setAttribute("aria-selected", String(selected));
    button.tabIndex = selected ? 0 : -1;
  }
  const stackPreviewIdentity = previewResult
    ? `${previewResult.planSha256}:registered-stack`
    : reportInspection
      ? `${reportInspection.reportSha256}:reported-stack`
      : null;
  const stackPreviewReady =
    registration.stack.previewState === "ready" &&
    registration.stack.preview?.frameId ===
      `${stackPreviewIdentity}:${registration.stack.selectedProduct}`;
  const diagnosticProduct = registration.stack.selectedProduct !== "science";
  const scienceUnderlayReady =
    stackPreviewReady &&
    diagnosticProduct &&
    registration.stack.sciencePreview?.frameId ===
      `${stackPreviewIdentity}:science`;
  elements.registeredStackPreviewImage.hidden = !stackPreviewReady;
  elements.registeredStackScienceImage.hidden = !scienceUnderlayReady;
  elements.registeredStackPreviewPlaceholder.hidden = stackPreviewReady;
  elements.registeredStackScienceImage.src = scienceUnderlayReady
    ? (registration.stack.sciencePreview?.url ?? "")
    : "";
  elements.registeredStackScienceImage.alt = scienceUnderlayReady
    ? "Integrated science preview beneath the rejection overlay"
    : "";
  elements.registeredStackPreviewImage.src = stackPreviewReady
    ? (registration.stack.preview?.url ?? "")
    : "";
  elements.registeredStackPreviewImage.style.opacity = diagnosticProduct
    ? String(registration.stack.overlayOpacity)
    : "1";
  elements.registeredStackPreviewImage.alt = stackPreviewReady
    ? registration.stack.selectedProduct === "science"
      ? "Integrated registered common-crop preview"
      : registration.stack.selectedProduct === "rejection_low"
        ? "Low-tail rejection map preview"
        : registration.stack.selectedProduct === "rejection_high"
          ? "High-tail rejection map preview"
          : "Accepted source support map preview"
    : "";
  elements.registeredStackOverlayControl.hidden =
    !diagnosticProduct || !stackPreviewReady;
  elements.registeredStackOverlayOpacity.value = String(
    Math.round(registration.stack.overlayOpacity * 100),
  );
  elements.registeredStackOverlayValue.value = `${Math.round(registration.stack.overlayOpacity * 100)}%`;
  elements.registeredStackOverlayValue.textContent =
    elements.registeredStackOverlayValue.value;
  renderRejectionHistogram(elements, registration.stack);
  renderStackPixelReadout(elements, registration.stack);
  elements.registeredStackPreviewPlaceholder.textContent =
    registration.stack.previewState === "loading"
      ? "Rendering the integrated FITS preview…"
      : registration.stack.previewState === "error"
        ? "The integrated FITS is valid, but its display preview is unavailable"
        : reportInspection
          ? "Select a verified report product to preview it"
          : "The integrated common crop will appear here after publication";
  const stackProgress = registration.stack.progress;
  if (stackProgress?.totalUnits) {
    elements.registeredStackProgress.max = stackProgress.totalUnits;
    elements.registeredStackProgress.value = stackProgress.completedUnits;
  } else {
    elements.registeredStackProgress.removeAttribute("value");
    elements.registeredStackProgress.max = 1;
  }
  elements.registeredStackProgress.hidden = !stackBusy;
  renderRegisteredResult(elements, registration.resultReview);
  elements.registrationPlanFrames.replaceChildren(
    ...registration.frames.map((frame) => {
      const item = document.createElement("li");
      const isReference = frame.id === registration.referenceFrameId;
      const isCurrent = frame.id === registration.sourceFrameId;
      const rejectedCurrent =
        isCurrent &&
        registration.state === "rejected" &&
        !acceptedSourceIds.has(frame.id);
      const state = isReference
        ? "reference"
        : acceptedSourceIds.has(frame.id)
          ? "accepted"
          : rejectedCurrent
            ? "rejected"
            : "pending";
      item.dataset.state = state;
      if (isCurrent) item.dataset.current = "true";
      const label = document.createElement("span");
      label.textContent = frame.label;
      const status = document.createElement("strong");
      status.textContent = humanize(state);
      item.append(label, status);
      return item;
    }),
  );

  const diagnostic = registration.diagnostic;
  const pairPlan = diagnostic?.acceptedPlan ?? null;
  elements.registrationRms.textContent = diagnostic
    ? (diagnostic.consensus.rmsResidualDetectionPixels * 2).toFixed(3)
    : "—";
  elements.registrationInliers.textContent = diagnostic
    ? diagnostic.consensus.inlierFeaturePairs.toLocaleString("en-US")
    : "—";
  const coverage = pairPlan
    ? (100 * pairPlan.coveredPixels) /
      (pairPlan.referenceWidth * pairPlan.referenceHeight)
    : null;
  elements.registrationCoverage.textContent =
    coverage === null ? "—" : coverage.toFixed(2);
  elements.registrationRotation.textContent = diagnostic
    ? ((diagnostic.consensus.rotationRadians * 180) / Math.PI).toFixed(4)
    : "—";
  elements.registrationScale.textContent = diagnostic
    ? diagnostic.consensus.scale.toFixed(8)
    : "—";
  elements.registrationReflection.textContent = diagnostic
    ? diagnostic.consensus.reflected
      ? "Detected"
      : "No"
    : "—";
  elements.registrationSupport.textContent = diagnostic
    ? diagnostic.confidence.winnerSupportMargin === null
      ? "Unique winner"
      : diagnostic.confidence.winnerSupportMargin.toFixed(3)
    : "—";
  const geometryEvidence: string[] = [];
  if (pairPlan) {
    geometryEvidence.push(
      `Accepted affine · source pixels\n[${pairPlan.transformCoefficientsSourcePixels.map((value) => value.toPrecision(10)).join(", ")}]\n${pairPlan.footprintAlgorithmId}`,
    );
  }
  if (diagnostic) {
    const projective = diagnostic.projectiveAdequacy;
    const validation = projective.crossValidation;
    const recommendation = projective.recommendation;
    const failedGates = [
      [recommendation.supportSufficient, "support"],
      [recommendation.validationFoldsSufficient, "validation folds"],
      [recommendation.foldWinsSufficient, "fold wins"],
      [recommendation.absoluteGainSufficient, "absolute gain"],
      [recommendation.relativeGainSufficient, "relative gain"],
      [recommendation.rankSeparationSufficient, "DLT stability"],
      [recommendation.projectiveRmsAcceptable, "projective RMS"],
      [recommendation.worstResidualNotIncreased, "worst residual"],
      [recommendation.modelSeparationSufficient, "field separation"],
    ]
      .filter(([passed]) => !passed)
      .map(([, label]) => label);
    const recommendationText = recommendation.recommended
      ? "recommended by every conservative gate"
      : `not recommended · failed: ${failedGates.join(", ")}`;
    geometryEvidence.push(
      `Diagnostic projective · detection pixels · not selected\nAdvisory: ${recommendationText}\n${projective.transformCoefficientsDetectionPixels
        .map(
          (row) => `[${row.map((value) => value.toPrecision(10)).join(", ")}]`,
        )
        .join(
          "\n",
        )}\nFit RMS ${projective.projectiveRmsResidualDetectionPixels.toFixed(4)} px · fit gain ${projective.rmsImprovementDetectionPixels.toFixed(4)} px · held-out gain ${validation.rmsImprovementDetectionPixels.toFixed(4)} px · fold wins ${validation.projectiveBetterFolds}/${validation.foldCount} · field Δ ${projective.maximumModelSeparationDetectionPixels.toFixed(4)} px · ${projective.matchCount.toLocaleString("en-US")} matches`,
    );
  }
  elements.registrationMatrix.textContent =
    geometryEvidence.join("\n\n") || "Awaiting geometric evidence";

  const canonicalPlan = registration.plan;
  const crop = canonicalPlan?.autocrop ?? pairPlan?.autocrop ?? null;
  const cropWidth = canonicalPlan?.referenceWidth ?? pairPlan?.referenceWidth;
  const cropHeight =
    canonicalPlan?.referenceHeight ?? pairPlan?.referenceHeight;
  const cropCoverage = canonicalPlan
    ? (100 * canonicalPlan.coveredPixels) /
      (canonicalPlan.referenceWidth * canonicalPlan.referenceHeight)
    : coverage;
  const sensorStage = elements.registrationCrop.parentElement;
  if (crop && cropWidth && cropHeight) {
    const left = (100 * crop.x) / cropWidth;
    const top = (100 * crop.y) / cropHeight;
    const width = (100 * crop.width) / cropWidth;
    const height = (100 * crop.height) / cropHeight;
    elements.registrationCrop.style.left = `${left}%`;
    elements.registrationCrop.style.top = `${top}%`;
    elements.registrationCrop.style.width = `${width}%`;
    elements.registrationCrop.style.height = `${height}%`;
    elements.registrationCrop.hidden = false;
    elements.registrationCropText.textContent = `${crop.width} × ${crop.height} px · origin ${crop.x}, ${crop.y} · ${cropCoverage?.toFixed(2)}% retained${canonicalPlan ? " · canonical all-frame crop" : ""}`;
    sensorStage?.setAttribute(
      "aria-label",
      `Common crop ${crop.width} by ${crop.height} pixels at ${crop.x}, ${crop.y}; ${cropCoverage?.toFixed(2)} percent retained`,
    );
  } else {
    elements.registrationCrop.removeAttribute("style");
    elements.registrationCrop.hidden = true;
    elements.registrationCropText.textContent = "No accepted footprint yet";
    sensorStage?.setAttribute("aria-label", "No accepted common footprint yet");
  }

  const rejections = diagnostic?.confidence.rejections ?? [];
  elements.registrationRejections.hidden = rejections.length === 0;
  if (rejections.length === 0) {
    elements.registrationRejections.replaceChildren();
  } else {
    const heading = document.createElement("strong");
    heading.textContent = "Confidence gate rejected this solution";
    const list = document.createElement("ul");
    for (const rejection of rejections) {
      const item = document.createElement("li");
      item.textContent = humanize(rejection);
      list.append(item);
    }
    elements.registrationRejections.replaceChildren(heading, list);
  }
}

function renderRejectionHistogram(
  elements: RegistrationElements,
  stack: ReviewViewModel["registration"]["stack"],
): void {
  const visible = stack.selectedProduct !== "science";
  elements.registeredStackHistogram.hidden = !visible;
  elements.registeredStackHistogram.dataset.product = stack.selectedProduct;
  elements.registeredStackHistogramBins.replaceChildren();
  if (!visible) return;
  if (stack.histogramState === "loading") {
    elements.registeredStackHistogramSummary.textContent =
      stack.selectedProduct === "support"
        ? "Reading exact accepted-source counts…"
        : "Reading exact rejection counts…";
    return;
  }
  if (stack.histogramState === "error" || !stack.histogram) {
    elements.registeredStackHistogramSummary.textContent =
      "The map is valid, but its count distribution is unavailable.";
    return;
  }

  const histogram = stack.histogram;
  const fraction =
    histogram.totalSamples === 0
      ? 0
      : (100 * histogram.rejectedSamples) / histogram.totalSamples;
  elements.registeredStackHistogramSummary.textContent =
    stack.selectedProduct === "support"
      ? `${histogram.rejectedSamples.toLocaleString("en-US")} covered pixels · ${fraction.toFixed(3)}% · maximum ${histogram.maximumRejectedCount} accepted sources`
      : `${histogram.rejectedSamples.toLocaleString("en-US")} affected samples · ${fraction.toFixed(3)}% · maximum ${histogram.maximumRejectedCount}`;
  const positiveBins = histogram.bins.filter((bin) => bin.rejectedCount > 0);
  const visibleBins = positiveBins.slice(0, 32);
  const maximumSamples = Math.max(1, ...visibleBins.map((bin) => bin.samples));
  const rows = visibleBins.map((bin) => {
    const row = document.createElement("div");
    row.className = "rejection-histogram__bin";
    const label = document.createElement("span");
    label.textContent = `${bin.rejectedCount}×`;
    const track = document.createElement("i");
    const fill = document.createElement("b");
    fill.style.width = `${(100 * bin.samples) / maximumSamples}%`;
    track.append(fill);
    const samples = document.createElement("strong");
    samples.textContent = bin.samples.toLocaleString("en-US");
    row.append(label, track, samples);
    return row;
  });
  elements.registeredStackHistogramBins.replaceChildren(...rows);
  if (positiveBins.length > visibleBins.length) {
    const remainder = document.createElement("small");
    remainder.textContent = `+ ${positiveBins.length - visibleBins.length} higher-count bins retained in the exact result`;
    elements.registeredStackHistogramBins.append(remainder);
  }
}

function renderStackPixelReadout(
  elements: RegistrationElements,
  stack: ReviewViewModel["registration"]["stack"],
): void {
  const readout = elements.registeredStackPixelReadout;
  const result = stack.result;
  const available = result !== null && stack.state === "completed";
  elements.registeredStackPixelX.disabled = !available;
  elements.registeredStackPixelY.disabled = !available;
  elements.inspectRegisteredStackPixel.disabled = !available;
  elements.registeredStackPixelX.max = result ? String(result.width - 1) : "0";
  elements.registeredStackPixelY.max = result ? String(result.height - 1) : "0";
  readout.dataset.state = stack.pixelInspectionState;
  if (stack.pixelInspectionState === "loading") {
    readout.textContent = "Reading exact FITS coordinate…";
    return;
  }
  if (stack.pixelInspectionState === "error") {
    readout.textContent =
      "Exact pixel inspection failed; products remain valid.";
    return;
  }
  const inspection = stack.pixelInspection;
  if (!inspection) {
    readout.textContent = "Click the image to inspect exact FITS values.";
    return;
  }
  elements.registeredStackPixelX.value = String(inspection.x);
  elements.registeredStackPixelY.value = String(inspection.y);
  const values = inspection.scienceValues
    .map((value) => (value === null ? "missing" : value.toPrecision(8)))
    .join(" / ");
  const counts = (source: readonly (number | null)[] | null) =>
    source?.map((value) => value ?? "missing").join(" / ") ?? "not published";
  readout.textContent = `x ${inspection.x} · y ${inspection.y} · science ${values} · low ${counts(inspection.lowRejectionCounts)} · high ${counts(inspection.highRejectionCounts)}`;
}

function renderDrizzlePixelReadout(
  elements: RegistrationElements,
  drizzle: ReviewViewModel["registration"]["drizzle"],
): void {
  const result = drizzle.result;
  const available = result !== null && drizzle.state === "completed";
  elements.drizzlePixelX.disabled = !available;
  elements.drizzlePixelY.disabled = !available;
  elements.inspectDrizzlePixel.disabled = !available;
  elements.drizzlePixelX.max = result ? String(result.width - 1) : "0";
  elements.drizzlePixelY.max = result ? String(result.height - 1) : "0";
  const readout = elements.drizzlePixelReadout;
  readout.dataset.state = drizzle.pixelInspectionState;
  if (drizzle.pixelInspectionState === "loading") {
    readout.textContent = "Reading the exact Drizzle coordinate…";
    return;
  }
  if (drizzle.pixelInspectionState === "error") {
    readout.textContent =
      "Exact coordinate inspection failed; the published product remains valid.";
    return;
  }
  const inspection = drizzle.pixelInspection;
  if (!inspection) {
    readout.textContent = "Enter an output coordinate to inspect every plane.";
    return;
  }
  elements.drizzlePixelX.value = String(inspection.x);
  elements.drizzlePixelY.value = String(inspection.y);
  const values = inspection.scienceValues
    .map((value) => (value === null ? "missing" : value.toPrecision(10)))
    .join(" / ");
  readout.textContent = `x ${inspection.x} · y ${inspection.y} · ${drizzle.selectedProduct} ${values}`;
}

function renderRegisteredResult(
  elements: RegistrationElements,
  review: ReviewViewModel["registration"]["resultReview"],
): void {
  elements.registeredFrame.replaceChildren(
    ...review.frames.map((frame) => {
      const option = document.createElement("option");
      option.value = frame.id;
      option.textContent = frame.label;
      return option;
    }),
  );
  elements.registeredFrame.value = review.selectedFrameId ?? "";
  elements.registeredFrame.disabled = review.frames.length === 0;
  const selectedIndex = review.frames.findIndex(
    (frame) => frame.id === review.selectedFrameId,
  );
  const expectedPreviewId =
    selectedIndex >= 0 && review.preview
      ? review.preview.frameId.endsWith(`:registered:${review.selectedFrameId}`)
      : false;
  const previewReady = review.state === "ready" && expectedPreviewId;
  elements.registeredPreviewImage.hidden = !previewReady;
  if (previewReady && review.preview) {
    elements.registeredPreviewImage.src = review.preview.url;
    elements.registeredPreviewImage.alt = `Registered preview of ${review.frames[selectedIndex]?.label ?? "Light frame"}`;
  } else {
    elements.registeredPreviewImage.removeAttribute("src");
    elements.registeredPreviewImage.alt = "";
  }
  elements.registeredPreviewPlaceholder.hidden = previewReady;
  elements.registeredPreviewMessage.textContent = review.message;
  elements.registeredPreviewPosition.textContent =
    selectedIndex >= 0
      ? `${selectedIndex + 1} / ${review.frames.length}`
      : `0 / ${review.frames.length}`;
  elements.registeredPreviewStretch.textContent = review.sharedStretchLabel;
  elements.registeredPreviewPlay.disabled = review.frames.length < 2;
  elements.registeredPreviewPlay.setAttribute(
    "aria-pressed",
    String(review.playing),
  );
  elements.registeredPreviewPlay.setAttribute(
    "aria-label",
    review.playing ? "Pause registered Blink" : "Start registered Blink",
  );
  elements.registeredPreviewPlay.textContent = review.playing ? "Ⅱ" : "▶";
  for (const button of [
    elements.registeredPreviewPlay.previousElementSibling,
    elements.registeredPreviewPlay.nextElementSibling,
  ]) {
    if (button instanceof HTMLButtonElement) {
      button.disabled = review.frames.length < 2;
    }
  }
}

function setControlFieldVisibility(
  input: HTMLInputElement,
  visible: boolean,
): void {
  const field = input.closest<HTMLElement>(".control-field");
  if (field) field.hidden = !visible;
}

function drizzleSettings(
  elements: Pick<
    RegistrationElements,
    | "drizzleDropShrink"
    | "drizzleMaximumContributions"
    | "drizzleMaximumBandHeight"
  >,
  model: ReviewViewModel,
): DrizzleExecutionSettings | null {
  const dropShrink = elements.drizzleDropShrink.valueAsNumber;
  const maximumContributions =
    elements.drizzleMaximumContributions.valueAsNumber;
  const maximumBandHeight = elements.drizzleMaximumBandHeight.valueAsNumber;
  if (
    !Number.isFinite(dropShrink) ||
    dropShrink <= 0 ||
    dropShrink > 1 ||
    !Number.isSafeInteger(maximumContributions) ||
    maximumContributions < 1 ||
    !Number.isSafeInteger(maximumBandHeight) ||
    maximumBandHeight < 1
  ) {
    return null;
  }
  return {
    ...model.registration.drizzle.settings,
    dropShrink,
    maximumContributions,
    maximumBandHeight,
  };
}

function registeredStackSettings(
  elements: Pick<
    RegistrationElements,
    | "registeredStackEstimator"
    | "registeredStackLowFraction"
    | "registeredStackHighFraction"
    | "registeredStackLowSigma"
    | "registeredStackHighSigma"
    | "registeredStackEsdOutlierFraction"
    | "registeredStackEsdSignificance"
    | "registeredStackMaximumIterations"
    | "registeredStackLargeScaleLowEnabled"
    | "registeredStackLargeScaleHighEnabled"
    | "registeredStackLargeScaleLowLayers"
    | "registeredStackLargeScaleHighLayers"
    | "registeredStackLargeScaleLowGrowth"
    | "registeredStackLargeScaleHighGrowth"
    | "registeredStackMinimumRetained"
    | "registeredStackRejectionMaps"
    | "registeredStackSupportMap"
    | "registeredStackWeightReference"
  >,
): RegisteredStackIntegrationSettings | null {
  const estimator = elements.registeredStackEstimator.value;
  if (
    estimator !== "strict_mean" &&
    estimator !== "median" &&
    estimator !== "weighted_mean" &&
    estimator !== "percentile_clipped" &&
    estimator !== "sigma_clipped" &&
    estimator !== "winsorized_sigma_clipped" &&
    estimator !== "linear_fit_clipped" &&
    estimator !== "generalized_esd"
  ) {
    return null;
  }
  const lowFraction = elements.registeredStackLowFraction.valueAsNumber;
  const highFraction = elements.registeredStackHighFraction.valueAsNumber;
  const lowSigma = elements.registeredStackLowSigma.valueAsNumber;
  const highSigma = elements.registeredStackHighSigma.valueAsNumber;
  const esdOutlierFraction =
    elements.registeredStackEsdOutlierFraction.valueAsNumber;
  const esdSignificance = elements.registeredStackEsdSignificance.valueAsNumber;
  const maximumIterations =
    elements.registeredStackMaximumIterations.valueAsNumber;
  const minimumRetainedSamples =
    elements.registeredStackMinimumRetained.valueAsNumber;
  const largeScaleLowLayers =
    elements.registeredStackLargeScaleLowLayers.valueAsNumber;
  const largeScaleHighLayers =
    elements.registeredStackLargeScaleHighLayers.valueAsNumber;
  const largeScaleLowGrowth =
    elements.registeredStackLargeScaleLowGrowth.valueAsNumber;
  const largeScaleHighGrowth =
    elements.registeredStackLargeScaleHighGrowth.valueAsNumber;
  if (
    !Number.isFinite(lowFraction) ||
    lowFraction < 0 ||
    lowFraction >= 1 ||
    !Number.isFinite(highFraction) ||
    highFraction < 0 ||
    highFraction >= 1 ||
    lowFraction + highFraction >= 1 ||
    !Number.isFinite(lowSigma) ||
    lowSigma <= 0 ||
    !Number.isFinite(highSigma) ||
    highSigma <= 0 ||
    !Number.isFinite(esdOutlierFraction) ||
    esdOutlierFraction <= 0 ||
    esdOutlierFraction > 0.5 ||
    !Number.isFinite(esdSignificance) ||
    esdSignificance <= 0 ||
    esdSignificance >= 1 ||
    !Number.isSafeInteger(maximumIterations) ||
    maximumIterations < 1 ||
    maximumIterations > 4_294_967_295 ||
    !Number.isSafeInteger(minimumRetainedSamples) ||
    minimumRetainedSamples <
      (estimator === "linear_fit_clipped" || estimator === "generalized_esd"
        ? 3
        : 1) ||
    minimumRetainedSamples > 4_294_967_295 ||
    !Number.isSafeInteger(largeScaleLowLayers) ||
    largeScaleLowLayers < 1 ||
    largeScaleLowLayers > 12 ||
    !Number.isSafeInteger(largeScaleHighLayers) ||
    largeScaleHighLayers < 1 ||
    largeScaleHighLayers > 12 ||
    !Number.isSafeInteger(largeScaleLowGrowth) ||
    largeScaleLowGrowth < 0 ||
    largeScaleLowGrowth > 256 ||
    !Number.isSafeInteger(largeScaleHighGrowth) ||
    largeScaleHighGrowth < 0 ||
    largeScaleHighGrowth > 256
  ) {
    return null;
  }
  return {
    estimator,
    weightReferenceFrameId:
      elements.registeredStackWeightReference.value || null,
    lowFraction,
    highFraction,
    lowSigma,
    highSigma,
    esdOutlierFraction,
    esdSignificance,
    maximumIterations,
    minimumRetainedSamples,
    generateRejectionMaps:
      (estimator === "percentile_clipped" ||
        estimator === "sigma_clipped" ||
        estimator === "winsorized_sigma_clipped" ||
        estimator === "linear_fit_clipped" ||
        estimator === "generalized_esd") &&
      elements.registeredStackRejectionMaps.checked,
    generateSupportMap: elements.registeredStackSupportMap.checked,
    largeScaleLowEnabled:
      estimator === "generalized_esd" &&
      elements.registeredStackLargeScaleLowEnabled.checked,
    largeScaleHighEnabled:
      estimator === "generalized_esd" &&
      elements.registeredStackLargeScaleHighEnabled.checked,
    largeScaleLowLayers,
    largeScaleHighLayers,
    largeScaleLowGrowth,
    largeScaleHighGrowth,
  };
}

function formatWeightMetric(value: number | undefined, digits: number): string {
  return value === undefined ? "—" : value.toFixed(digits);
}

function formatRelativeWeight(value: number | null): string {
  if (value === null) return "—";
  if (value >= 0.001 && value < 10_000) return `${value.toFixed(4)}×`;
  return `${value.toExponential(3)}×`;
}

function formatEstimatorName(
  estimator:
    | "strict_mean"
    | "median"
    | "weighted_mean"
    | "percentile_clipped"
    | "sigma_clipped"
    | "winsorized_sigma_clipped"
    | "linear_fit_clipped"
    | "generalized_esd",
): string {
  switch (estimator) {
    case "strict_mean":
      return "strict mean";
    case "median":
      return "exact median";
    case "weighted_mean":
      return "balanced PSF weight";
    case "percentile_clipped":
      return "percentile clipped";
    case "sigma_clipped":
      return "iterative sigma clipped";
    case "winsorized_sigma_clipped":
      return "Winsorized sigma clipped";
    case "linear_fit_clipped":
      return "linear fit clipped";
    case "generalized_esd":
      return "generalized ESD";
  }
}

function formatCountedNoun(count: number, noun: string): string {
  return `${count} ${noun}${count === 1 ? "" : "s"}`;
}

function formatRegisteredProductRole(
  role: RegisteredStackReportProductInspection["role"],
): string {
  switch (role) {
    case "science":
      return "Science";
    case "rejection_low":
      return "Low rejection map";
    case "rejection_high":
      return "High rejection map";
    case "support":
      return "Accepted support map";
  }
}

function formatRegisteredProductStatus(
  status: RegisteredStackReportProductInspection["status"],
): string {
  switch (status) {
    case "verified":
      return "FITS verified";
    case "missing":
      return "FITS missing";
    case "non_regular":
      return "unsafe file type";
    case "byte_length_mismatch":
      return "byte length mismatch";
    case "invalid_fits":
      return "invalid FITS";
    case "metadata_mismatch":
      return "provenance mismatch";
    case "checksum_mismatch":
      return "checksum mismatch";
  }
}

function formatRegisteredSourceStatus(
  status: RegisteredStackSourceVerification["status"],
): string {
  switch (status) {
    case "verified":
      return "SHA-256 verified";
    case "missing":
      return "source missing";
    case "non_regular":
      return "unsafe file type";
    case "byte_length_mismatch":
      return "byte length mismatch";
    case "read_failed":
      return "source unreadable";
    case "fingerprint_mismatch":
      return "SHA-256 mismatch";
  }
}

function formatByteCount(bytes: number): string {
  if (bytes < 1_024) return `${bytes} B`;
  if (bytes < 1_024 * 1_024) return `${(bytes / 1_024).toFixed(1)} KiB`;
  if (bytes < 1_024 * 1_024 * 1_024) {
    return `${(bytes / (1_024 * 1_024)).toFixed(1)} MiB`;
  }
  return `${(bytes / (1_024 * 1_024 * 1_024)).toFixed(1)} GiB`;
}

function formatElapsedMilliseconds(milliseconds: number): string {
  if (milliseconds < 1) return "<1 ms";
  if (milliseconds < 1_000) return `${milliseconds} ms`;
  return `${(milliseconds / 1_000).toFixed(2)} s`;
}

function calibrationSettings(
  elements: Pick<
    CalibrationElements,
    | "pedestalPolicy"
    | "exposureTolerance"
    | "temperatureTolerance"
    | "lightTemperatureTolerance"
  >,
): MasterPlanSettings | null {
  const policy = elements.pedestalPolicy.value;
  if (
    policy !== "prefer_matched_dark_then_bias" &&
    policy !== "require_matched_dark" &&
    policy !== "require_bias"
  ) {
    return null;
  }
  const maximumExposureDeltaSeconds = elements.exposureTolerance.valueAsNumber;
  const maximumTemperatureDeltaC = elements.temperatureTolerance.valueAsNumber;
  const maximumLightDarkTemperatureDeltaC =
    elements.lightTemperatureTolerance.valueAsNumber;
  if (
    !Number.isFinite(maximumExposureDeltaSeconds) ||
    maximumExposureDeltaSeconds < 0 ||
    !Number.isFinite(maximumTemperatureDeltaC) ||
    maximumTemperatureDeltaC < 0 ||
    !Number.isFinite(maximumLightDarkTemperatureDeltaC) ||
    maximumLightDarkTemperatureDeltaC < 0
  ) {
    return null;
  }
  return {
    flatPedestalPolicy: policy,
    maximumExposureDeltaSeconds,
    maximumTemperatureDeltaC,
    maximumLightDarkTemperatureDeltaC,
  };
}

function defectCorrectionSettings(
  elements: CalibrationElements,
  model: ReviewViewModel,
): DefectCorrectionSettings | null {
  const values = {
    detectionRadius: elements.defectDetectionRadius.valueAsNumber,
    detectionMinimumNeighbours:
      elements.defectDetectionMinimumNeighbours.valueAsNumber,
    darkHotSigma: elements.defectDarkHotSigma.valueAsNumber,
    darkColdSigma: elements.defectDarkColdSigma.valueAsNumber,
    darkFloor: elements.defectDarkFloor.valueAsNumber,
    flatHotSigma: elements.defectFlatHotSigma.valueAsNumber,
    flatColdSigma: elements.defectFlatColdSigma.valueAsNumber,
    flatFloor: elements.defectFlatFloor.valueAsNumber,
    correctionRadius: elements.defectCorrectionRadius.valueAsNumber,
    correctionMinimumNeighbours:
      elements.defectCorrectionMinimumNeighbours.valueAsNumber,
  };
  if (
    !Object.values(values).every(Number.isFinite) ||
    !Number.isSafeInteger(values.detectionRadius) ||
    values.detectionRadius < 1 ||
    values.detectionRadius > 8 ||
    !Number.isSafeInteger(values.detectionMinimumNeighbours) ||
    values.detectionMinimumNeighbours < 1 ||
    values.darkHotSigma <= 0 ||
    values.darkColdSigma <= 0 ||
    values.darkFloor < 0 ||
    values.flatHotSigma <= 0 ||
    values.flatColdSigma <= 0 ||
    values.flatFloor < 0 ||
    !Number.isSafeInteger(values.correctionRadius) ||
    values.correctionRadius < 1 ||
    values.correctionRadius > 8 ||
    !Number.isSafeInteger(values.correctionMinimumNeighbours) ||
    values.correctionMinimumNeighbours < 1
  ) {
    return null;
  }
  const current = model.calibration.defectCorrection.settings;
  return {
    ...current,
    darkDetection: {
      radius: values.detectionRadius,
      stride: current.darkDetection.stride,
      minimumNeighbours: values.detectionMinimumNeighbours,
      hotSigma: values.darkHotSigma,
      coldSigma: values.darkColdSigma,
      minimumAbsoluteDeviation: values.darkFloor,
    },
    flatDetection: {
      radius: values.detectionRadius,
      stride: current.flatDetection.stride,
      minimumNeighbours: values.detectionMinimumNeighbours,
      hotSigma: values.flatHotSigma,
      coldSigma: values.flatColdSigma,
      minimumAbsoluteDeviation: values.flatFloor,
    },
    correctionRadius: values.correctionRadius,
    correctionMinimumNeighbours: values.correctionMinimumNeighbours,
  };
}

function linearDefectSettings(
  elements: CalibrationElements,
  model: ReviewViewModel,
): LinearDefectSettings | null {
  const values = new Map(
    elements.linearDefectInputs.map((input) => [
      input.dataset.linearDefectSetting ?? "",
      input.valueAsNumber,
    ]),
  );
  const integer = (
    name: string,
    minimum: number,
    maximum: number,
  ): number | null => {
    const value = values.get(name);
    return value !== undefined &&
      Number.isSafeInteger(value) &&
      value >= minimum &&
      value <= maximum
      ? value
      : null;
  };
  const finite = (name: string, minimum: number): number | null => {
    const value = values.get(name);
    return value !== undefined && Number.isFinite(value) && value >= minimum
      ? value
      : null;
  };
  const perpendicularRadius = integer("perpendicularRadius", 1, 8);
  const minimumPerpendicularNeighbours = integer(
    "minimumPerpendicularNeighbours",
    1,
    16,
  );
  const minimumAffectedSamples = integer(
    "minimumAffectedSamples",
    1,
    10_000_000,
  );
  const affectedPercent = finite("minimumAffectedPercent", 0.0001);
  const hotSigma = finite("hotSigma", 0.01);
  const coldSigma = finite("coldSigma", 0.01);
  const minimumAbsoluteDeviation = finite("minimumAbsoluteDeviation", 0);
  const correctionRadius = integer("correctionRadius", 1, 8);
  const correctionMinimumNeighbours = integer(
    "correctionMinimumNeighbours",
    1,
    16,
  );
  if (
    perpendicularRadius === null ||
    minimumPerpendicularNeighbours === null ||
    minimumAffectedSamples === null ||
    affectedPercent === null ||
    affectedPercent > 100 ||
    hotSigma === null ||
    coldSigma === null ||
    minimumAbsoluteDeviation === null ||
    correctionRadius === null ||
    correctionMinimumNeighbours === null
  )
    return null;
  const current = model.calibration.defectCorrection.linear.settings;
  return {
    ...current,
    perpendicularRadius,
    minimumPerpendicularNeighbours,
    minimumAffectedSamples,
    minimumAffectedFractionPpm: Math.round(affectedPercent * 10_000),
    hotSigma,
    coldSigma,
    minimumAbsoluteDeviation,
    correctionRadius,
    correctionMinimumNeighbours,
  };
}

function renderCalibration(
  elements: CalibrationElements,
  model: ReviewViewModel,
): void {
  const calibration = model.calibration;
  elements.pedestalPolicy.value = calibration.settings.flatPedestalPolicy;
  elements.exposureTolerance.value = String(
    calibration.settings.maximumExposureDeltaSeconds,
  );
  elements.temperatureTolerance.value = String(
    calibration.settings.maximumTemperatureDeltaC,
  );
  elements.lightTemperatureTolerance.value = String(
    calibration.settings.maximumLightDarkTemperatureDeltaC,
  );
  elements.calibrationStatus.dataset.state = calibration.state;
  elements.calibrationStatus.textContent = calibration.message;
  const execution = calibration.execution;
  const masterBusy =
    execution.state === "running" || execution.state === "cancelling";
  const lightExecution = calibration.lightExecution;
  const lightBusy =
    lightExecution.state === "running" || lightExecution.state === "cancelling";
  const registrationBusy =
    model.registration.execution.state === "running" ||
    model.registration.execution.state === "cancelling";
  const normalizationBusy =
    model.localNormalization.state === "running" ||
    model.localNormalization.state === "cancelling";
  const otherExecutionBusy =
    masterBusy || lightBusy || registrationBusy || normalizationBusy;
  const defectBusy =
    calibration.defectCorrection.state === "running" ||
    calibration.defectCorrection.state === "cancelling";
  const executionBusy = otherExecutionBusy || defectBusy;
  elements.refreshMasterPlan.disabled =
    calibration.state === "loading" ||
    !model.reviewSessionReady ||
    executionBusy;
  elements.pedestalPolicy.disabled = executionBusy;
  elements.exposureTolerance.disabled = executionBusy;
  elements.temperatureTolerance.disabled = executionBusy;
  elements.lightTemperatureTolerance.disabled = executionBusy;
  elements.lightOutputMode.value = calibration.lightSettings.outputMode;
  elements.lightOutputMode.disabled = executionBusy;
  elements.executeLightPlan.textContent =
    calibration.lightSettings.outputMode === "calibrated_frames"
      ? "Calibrate frames"
      : "Calibrate & integrate";
  elements.lightExecutionHeading.textContent =
    calibration.lightSettings.outputMode === "calibrated_frames"
      ? "Lossless calibrated frames"
      : "Calibrate + strict integration";
  elements.executeMasterPlan.disabled =
    !calibration.plan?.ready || executionBusy || !model.reviewSessionReady;
  elements.executeMasterPlan.hidden = masterBusy;
  elements.cancelMasterPlan.hidden = !masterBusy;
  elements.cancelMasterPlan.disabled = execution.state === "cancelling";
  const lightPlan = calibration.plan?.lightPlan;
  elements.executeLightPlan.disabled =
    !lightPlan?.ready ||
    lightPlan.products.length === 0 ||
    !lightExecution.masterDirectory ||
    executionBusy ||
    !model.reviewSessionReady;
  elements.executeLightPlan.hidden = lightBusy;
  elements.cancelLightPlan.hidden = !lightBusy;
  elements.cancelLightPlan.disabled = lightExecution.state === "cancelling";
  renderMasterExecution(elements, model);
  renderLightExecution(elements, model);
  renderDefectCorrection(elements, model, otherExecutionBusy);
  const plan = calibration.plan;
  elements.calibrationDigest.textContent = plan
    ? `PLAN ${plan.planSha256.slice(0, 12)}`
    : "PLAN —";
  elements.calibrationDigest.title = plan?.planSha256 ?? "No native plan yet";

  const products = plan?.products ?? [];
  const nodes = products.map(masterProductCard);
  if (nodes.length === 0) {
    const empty = document.createElement("div");
    empty.className = "master-empty";
    const aperture = document.createElement("span");
    aperture.className = "master-empty__aperture";
    aperture.setAttribute("aria-hidden", "true");
    const title = document.createElement("strong");
    title.textContent =
      calibration.state === "loading"
        ? "Resolving calibration groups"
        : "No master groups available";
    const detail = document.createElement("span");
    detail.textContent = model.reviewSessionReady
      ? "Check imported metadata and grouping diagnostics"
      : "Import a FITS session to build the native dependency graph";
    empty.append(aperture, title, detail);
    nodes.push(empty);
  }
  elements.calibrationProducts.replaceChildren(...nodes);
  renderLightAssociations(elements, plan);
}

function renderDefectCorrection(
  elements: CalibrationElements,
  model: ReviewViewModel,
  otherExecutionBusy: boolean,
): void {
  const correction = model.calibration.defectCorrection;
  const settings = correction.settings;
  const busy =
    correction.state === "running" || correction.state === "cancelling";
  const linearBusy =
    correction.linear.state === "running" ||
    correction.linear.state === "cancelling";
  const controlsBlocked = otherExecutionBusy || linearBusy;
  const hasInputs =
    (model.calibration.lightExecution.result?.calibratedFrames.length ?? 0) >
      0 && (model.calibration.execution.result?.products.length ?? 0) > 0;
  elements.defectCorrection.dataset.state = correction.state;
  elements.defectCorrectionMessage.textContent = correction.message;
  if (correction.progress?.totalUnits) {
    elements.defectCorrectionProgress.max = correction.progress.totalUnits;
    elements.defectCorrectionProgress.value =
      correction.progress.completedUnits;
  } else {
    elements.defectCorrectionProgress.removeAttribute("value");
    elements.defectCorrectionProgress.max = 1;
  }
  elements.defectCorrectionProgress.hidden =
    correction.state !== "running" && correction.state !== "cancelling";
  elements.defectCorrectionOutput.textContent = correction.result
    ? correction.result.correctedOutputPath
    : correction.outputDirectory
      ? correction.outputDirectory
      : "Destination selected at run time";
  elements.defectCorrectionOutput.title =
    correction.result?.correctedOutputPath ?? correction.outputDirectory ?? "";
  elements.defectCorrectionEvidence.textContent = correction.result
    ? `Dark H${correction.result.darkDetection.hotSamples} / C${correction.result.darkDetection.coldSamples} · Flat H${correction.result.flatDetection.hotSamples} / C${correction.result.flatDetection.coldSamples} · Map ${correction.result.mapSummary.defectiveSamples} unique, ${correction.result.mapSummary.conflictingSamples} conflicts · ${correction.result.correctedSamples} corrected`
    : "No correction evidence published";
  const report = correction.batchReport;
  elements.defectBatchReport.hidden = report === null;
  elements.defectBatchReport.textContent = report
    ? `${report.state.toUpperCase()} · ${report.completedItems}/${report.totalItems} pairs · ${report.correctedSamples}/${report.requestedSamples} samples repaired · H${report.hotSamples} C${report.coldSamples} conflicts ${report.conflictingSamples} · peak ${formatByteCount(report.peakReservedBytes)} · plan ${report.planSha256.slice(0, 12)}…`
    : "";
  elements.defectBatchReport.title = report
    ? `Plan SHA-256 ${report.planSha256} · parameters SHA-256 ${report.parametersSha256}`
    : "";
  const inspection = correction.reportInspection;
  const repairEfficiency =
    inspection?.repairEfficiencyPpm === null
      ? "no mapped defects"
      : inspection
        ? `${(inspection.repairEfficiencyPpm / 10_000).toFixed(2)}% repaired`
        : "";
  elements.defectReportInspection.dataset.state =
    correction.reportInspectionState;
  elements.defectReportInspection.replaceChildren();
  if (inspection) {
    const verdict = defectReportVerdict(
      inspection.defectiveSamples,
      inspection.unresolvedSamples,
    );
    elements.defectReportInspection.dataset.verdict = verdict.tone;
    const heading = document.createElement("div");
    heading.className = "defect-report-verdict__heading";
    const badge = document.createElement("span");
    badge.className = "defect-report-verdict__badge";
    badge.textContent = "Verified";
    const label = document.createElement("strong");
    label.textContent = verdict.label;
    heading.append(badge, label);
    const summary = document.createElement("p");
    summary.textContent = verdict.summary;
    const metrics = document.createElement("dl");
    metrics.className = "defect-report-verdict__metrics";
    const appendMetric = (name: string, value: string, key: string): void => {
      const item = document.createElement("div");
      item.dataset.metric = key;
      const term = document.createElement("dt");
      term.textContent = name;
      const description = document.createElement("dd");
      description.textContent = value;
      item.append(term, description);
      metrics.append(item);
    };
    appendMetric("Mapped", String(inspection.defectiveSamples), "mapped");
    appendMetric("Efficiency", repairEfficiency, "efficiency");
    appendMetric(
      "Unresolved",
      String(inspection.unresolvedSamples),
      "unresolved",
    );
    appendMetric(
      "Conflicts",
      String(inspection.conflictingSamples),
      "conflicts",
    );
    appendMetric(
      "Memory peak",
      formatByteCount(inspection.peakReservedBytes),
      "memory",
    );
    const provenance = document.createElement("small");
    provenance.textContent = correction.reportInspectionMessage;
    elements.defectReportInspection.append(
      heading,
      summary,
      metrics,
      provenance,
    );
  } else {
    delete elements.defectReportInspection.dataset.verdict;
    elements.defectReportInspection.textContent =
      correction.reportInspectionMessage;
  }
  elements.defectReportInspection.title = inspection
    ? `Plan SHA-256 ${inspection.planSha256} · parameters SHA-256 ${inspection.parametersSha256}`
    : "";
  if (report && busy) {
    const currentUnits = Math.min(
      correction.progress?.completedUnits ?? 0,
      correction.progress?.totalUnits ?? 5,
    );
    elements.defectCorrectionProgress.max = report.totalItems * 5;
    elements.defectCorrectionProgress.value =
      report.completedItems * 5 + currentUnits;
  }
  for (const button of elements.defectPreviewButtons) {
    const view = button.dataset.defectPreview;
    const selected = view === correction.previewView;
    button.setAttribute("aria-selected", String(selected));
    button.tabIndex = selected ? 0 : -1;
    button.disabled =
      !correction.result || correction.previewState === "loading";
  }
  const previewReady =
    correction.previewState === "ready" && correction.preview !== null;
  elements.defectPreviewImage.hidden = !previewReady;
  elements.defectPreviewPlaceholder.hidden = previewReady;
  elements.defectPreviewPlaceholder.textContent =
    correction.previewState === "loading"
      ? "Rendering native FITS pixels…"
      : correction.previewState === "error"
        ? "Preview validation failed"
        : "Publish one correction to unlock the shared before / after view";
  elements.defectPreviewImage.src = correction.preview?.url ?? "";
  elements.defectPreviewImage.alt = previewReady
    ? `${correction.previewView === "before" ? "Calibrated input" : correction.previewView === "after" ? "Corrected output" : "HOT and COLD defect map"} display preview`
    : "";
  elements.defectPreviewMessage.textContent = correction.previewMessage;
  elements.executeDefectCorrection.disabled =
    !hasInputs || controlsBlocked || busy || !model.reviewSessionReady;
  elements.executeDefectCorrection.hidden = busy;
  elements.executeAllDefectCorrections.disabled =
    !hasInputs || controlsBlocked || busy || !model.reviewSessionReady;
  elements.executeAllDefectCorrections.hidden = busy;
  elements.exportDefectBatchReport.hidden = busy;
  elements.exportDefectBatchReport.disabled =
    correction.batchReport?.state !== "completed" || controlsBlocked;
  elements.resumeDefectBatch.hidden = busy || !correction.resumeAvailable;
  elements.resumeDefectBatch.disabled = controlsBlocked;
  elements.inspectDefectBatchReport.disabled =
    busy || correction.reportInspectionState === "loading";
  const eligibleCount =
    model.calibration.lightExecution.result?.calibratedFrames.length ?? 0;
  elements.executeAllDefectCorrections.textContent = `Correct all ${eligibleCount} eligible ${eligibleCount === 1 ? "Light" : "Lights"}`;
  elements.cancelDefectCorrection.hidden = !busy;
  elements.cancelDefectCorrection.disabled = correction.state === "cancelling";

  elements.defectDetectionRadius.value = String(settings.darkDetection.radius);
  elements.defectDetectionMinimumNeighbours.value = String(
    settings.darkDetection.minimumNeighbours,
  );
  elements.defectDarkHotSigma.value = String(settings.darkDetection.hotSigma);
  elements.defectDarkColdSigma.value = String(settings.darkDetection.coldSigma);
  elements.defectDarkFloor.value = String(
    settings.darkDetection.minimumAbsoluteDeviation,
  );
  elements.defectFlatHotSigma.value = String(settings.flatDetection.hotSigma);
  elements.defectFlatColdSigma.value = String(settings.flatDetection.coldSigma);
  elements.defectFlatFloor.value = String(
    settings.flatDetection.minimumAbsoluteDeviation,
  );
  elements.defectCorrectionRadius.value = String(settings.correctionRadius);
  elements.defectCorrectionMinimumNeighbours.value = String(
    settings.correctionMinimumNeighbours,
  );
  for (const button of elements.defectStrideButtons) {
    const selected =
      Number(button.dataset.defectStride) === settings.correctionStride;
    button.setAttribute("aria-pressed", String(selected));
    button.disabled = busy || controlsBlocked;
  }
  for (const input of [
    elements.defectDetectionRadius,
    elements.defectDetectionMinimumNeighbours,
    elements.defectDarkHotSigma,
    elements.defectDarkColdSigma,
    elements.defectDarkFloor,
    elements.defectFlatHotSigma,
    elements.defectFlatColdSigma,
    elements.defectFlatFloor,
    elements.defectCorrectionRadius,
    elements.defectCorrectionMinimumNeighbours,
  ]) {
    input.disabled = busy || controlsBlocked;
  }
  renderLinearDefectCorrection(elements, model, otherExecutionBusy || busy);
}

function renderLinearDefectCorrection(
  elements: CalibrationElements,
  model: ReviewViewModel,
  otherExecutionBusy: boolean,
): void {
  const linear = model.calibration.defectCorrection.linear;
  const settings = linear.settings;
  const busy = linear.state === "running" || linear.state === "cancelling";
  const hasInputs = selectedCalibratedLightCount(model) > 0;
  elements.linearDefect.dataset.state = linear.state;
  elements.linearDefectMessage.textContent = linear.message;
  elements.linearDefectProgress.hidden = !busy;
  if (linear.progress?.totalUnits) {
    elements.linearDefectProgress.max = linear.progress.totalUnits;
    elements.linearDefectProgress.value = linear.progress.completedUnits;
  } else {
    elements.linearDefectProgress.max = 1;
    elements.linearDefectProgress.removeAttribute("value");
  }
  const result = linear.result;
  elements.linearDefectEvidence.textContent = result
    ? `${result.hotLines} hot / ${result.coldLines} cold ${settings.axis} · ${result.correctedSamples}/${result.requestedSamples} samples repaired · ${formatByteCount(result.reservedBytes)} reserved · seal ${result.parametersSha256.slice(0, 12)}…`
    : "No coherent-line evidence published";
  elements.linearDefectOutput.textContent =
    result?.correctedOutputPath ??
    linear.outputDirectory ??
    "Destination selected at run time";
  elements.linearDefectOutput.title = result?.correctedOutputPath ?? "";
  for (const button of elements.linearDefectAxisButtons) {
    const selected = button.dataset.linearDefectAxis === settings.axis;
    button.setAttribute("aria-pressed", String(selected));
    button.disabled = busy || otherExecutionBusy;
  }
  const values: Readonly<Record<string, number>> = {
    perpendicularRadius: settings.perpendicularRadius,
    minimumPerpendicularNeighbours: settings.minimumPerpendicularNeighbours,
    minimumAffectedSamples: settings.minimumAffectedSamples,
    minimumAffectedPercent: settings.minimumAffectedFractionPpm / 10_000,
    hotSigma: settings.hotSigma,
    coldSigma: settings.coldSigma,
    minimumAbsoluteDeviation: settings.minimumAbsoluteDeviation,
    correctionRadius: settings.correctionRadius,
    correctionMinimumNeighbours: settings.correctionMinimumNeighbours,
  };
  for (const input of elements.linearDefectInputs) {
    const key = input.dataset.linearDefectSetting ?? "";
    if (key in values) input.value = String(values[key]);
    input.disabled = busy || otherExecutionBusy;
  }
  elements.executeLinearDefectCorrection.hidden = busy;
  elements.executeLinearDefectCorrection.disabled =
    !hasInputs || otherExecutionBusy || !model.reviewSessionReady;
  elements.cancelLinearDefectCorrection.hidden = !busy;
  elements.cancelLinearDefectCorrection.disabled =
    linear.state === "cancelling";
}

function selectedCalibratedLightCount(model: ReviewViewModel): number {
  return model.calibration.lightExecution.result?.calibratedFrames.length ?? 0;
}

function renderLightAssociations(
  elements: Pick<CalibrationElements, "lightAssociations" | "lightDigest">,
  plan: ReviewViewModel["calibration"]["plan"],
): void {
  const lightPlan = plan?.lightPlan ?? null;
  elements.lightDigest.textContent = lightPlan
    ? `LIGHT ${lightPlan.planSha256.slice(0, 12)}`
    : "LIGHT —";
  elements.lightDigest.title =
    lightPlan?.planSha256 ?? "Light associations wait for a ready master plan";

  if (!lightPlan) {
    const blocked = document.createElement("div");
    blocked.className = "light-matrix__empty";
    const title = document.createElement("strong");
    title.textContent = "Light associations are gated";
    const detail = document.createElement("span");
    detail.textContent =
      "Resolve every Flat pedestal before matching Lights to generated masters.";
    blocked.append(title, detail);
    elements.lightAssociations.replaceChildren(blocked);
    return;
  }

  if (lightPlan.products.length === 0) {
    const empty = document.createElement("div");
    empty.className = "light-matrix__empty";
    const title = document.createElement("strong");
    title.textContent = "No Light groups in this session";
    const detail = document.createElement("span");
    detail.textContent =
      "The master plan remains valid and can be built independently.";
    empty.append(title, detail);
    elements.lightAssociations.replaceChildren(empty);
    return;
  }

  const table = document.createElement("table");
  table.className = "light-matrix";
  table.setAttribute("aria-label", "Light calibration associations");
  const head = document.createElement("thead");
  const heading = document.createElement("tr");
  for (const label of [
    "Light group",
    "Dark master",
    "Flat master",
    "State",
    "Evidence",
  ]) {
    const cell = document.createElement("th");
    cell.scope = "col";
    cell.textContent = label;
    heading.append(cell);
  }
  head.append(heading);
  const body = document.createElement("tbody");
  for (const product of lightPlan.products)
    body.append(lightAssociationRow(product));
  table.append(head, body);
  elements.lightAssociations.replaceChildren(table);
}

function lightAssociationRow(
  product: LightCalibrationProduct,
): HTMLTableRowElement {
  const row = document.createElement("tr");
  row.dataset.state =
    product.dark.status === "matched" && product.flat.status === "matched"
      ? "ready"
      : "blocked";
  const identity = document.createElement("th");
  identity.scope = "row";
  identity.textContent = product.groupId;
  identity.title = product.groupId;
  row.append(
    identity,
    lightAssociationCell(product.dark),
    lightAssociationCell(product.flat),
  );
  const state = document.createElement("td");
  const badge = document.createElement("span");
  badge.className = "association-state";
  badge.textContent = row.dataset.state === "ready" ? "Ready" : "Blocked";
  state.append(badge);

  const evidence = document.createElement("td");
  const details = document.createElement("details");
  details.className = "candidate-evidence";
  const summary = document.createElement("summary");
  const compatible = product.candidates.filter(
    (candidate) => candidate.status === "compatible",
  ).length;
  summary.textContent =
    product.candidates.length === 0
      ? "No master candidates"
      : `${compatible}/${product.candidates.length} compatible`;
  const list = document.createElement("ul");
  for (const candidate of product.candidates) {
    const item = document.createElement("li");
    const reasons = candidate.mismatches
      .map(
        (mismatch) =>
          `${humanize(mismatch.field)}: ${humanize(mismatch.reason)}`,
      )
      .join(", ");
    item.textContent = `${candidate.kind.toUpperCase()} · ${candidate.groupId} · ${
      candidate.status === "compatible" ? "compatible" : reasons
    }`;
    list.append(item);
  }
  details.append(summary, list);
  evidence.append(details);
  row.append(state, evidence);
  return row;
}

function lightAssociationCell(
  association: LightMasterAssociation,
): HTMLTableCellElement {
  const output = document.createElement("td");
  const name = document.createElement("strong");
  name.textContent =
    association.selectedGroupId ?? associationBlockingLabel(association);
  name.title = association.selectedGroupId ?? "Unresolved association";
  output.append(name);
  if (association.temperatureDeltaCelsius !== null) {
    const temperature = document.createElement("small");
    temperature.textContent = `${association.temperatureDeltaCelsius.toFixed(2)} °C · ${
      association.temperatureBasis === "sensor" ? "sensor" : "set point"
    }`;
    output.append(temperature);
  }
  return output;
}

function associationBlockingLabel(association: LightMasterAssociation): string {
  if (association.ambiguousGroupIds.length > 0) {
    return `Ambiguous · ${association.ambiguousGroupIds.length} candidates`;
  }
  if (association.missingFields.length > 0) {
    return `Missing · ${association.missingFields.map(humanize).join(", ")}`;
  }
  return association.blockingReason === "no_compatible_candidate"
    ? "No compatible master"
    : "Unresolved";
}

function renderMasterExecution(
  elements: Pick<
    CalibrationElements,
    | "masterExecution"
    | "masterExecutionMessage"
    | "masterExecutionProgress"
    | "masterExecutionOutput"
  >,
  model: ReviewViewModel,
): void {
  const execution = model.calibration.execution;
  elements.masterExecution.dataset.state = execution.state;
  elements.masterExecutionMessage.textContent = execution.message;
  elements.masterExecutionOutput.textContent = execution.outputDirectory
    ? execution.outputDirectory
    : "Output directory selected at run time";
  elements.masterExecutionOutput.title = execution.outputDirectory ?? "";
  const progress = execution.progress;
  if (progress?.totalUnits) {
    elements.masterExecutionProgress.max = progress.totalUnits;
    elements.masterExecutionProgress.value = progress.completedUnits;
  } else {
    elements.masterExecutionProgress.removeAttribute("value");
    elements.masterExecutionProgress.max = 1;
  }
  elements.masterExecutionProgress.hidden =
    execution.state !== "running" && execution.state !== "cancelling";
}

function renderLightExecution(
  elements: Pick<
    CalibrationElements,
    | "lightExecution"
    | "lightExecutionMessage"
    | "lightExecutionProgress"
    | "lightExecutionOutput"
  >,
  model: ReviewViewModel,
): void {
  const execution = model.calibration.lightExecution;
  elements.lightExecution.dataset.state = execution.state;
  elements.lightExecutionMessage.textContent = execution.message;
  elements.lightExecutionOutput.textContent = execution.outputDirectory
    ? execution.outputDirectory
    : execution.masterDirectory
      ? "Output directory selected at run time"
      : "Build masters to unlock Light execution";
  elements.lightExecutionOutput.title = execution.outputDirectory ?? "";
  const progress = execution.progress;
  if (progress?.totalUnits) {
    elements.lightExecutionProgress.max = progress.totalUnits;
    elements.lightExecutionProgress.value = progress.completedUnits;
  } else {
    elements.lightExecutionProgress.removeAttribute("value");
    elements.lightExecutionProgress.max = 1;
  }
  elements.lightExecutionProgress.hidden =
    execution.state !== "running" && execution.state !== "cancelling";
}

function masterProductCard(product: MasterProductPlan): HTMLElement {
  const card = document.createElement("article");
  card.className = "master-graph-card";
  card.dataset.kind = product.kind;
  card.dataset.status = product.pedestal.status;

  const header = document.createElement("header");
  const heading = document.createElement("h4");
  heading.textContent = masterKindLabel(product.kind);
  const state = document.createElement("span");
  state.className = "master-graph-card__state";
  state.textContent =
    product.pedestal.status === "unresolved" ? "Needs attention" : "Ready";
  header.append(heading, state);

  const rail = document.createElement("div");
  rail.className = "master-graph-card__rail";
  rail.setAttribute("role", "img");
  rail.setAttribute("aria-label", masterFlowLabel(product));

  const source = document.createElement("div");
  source.className = "master-graph-node master-graph-node--source";
  const sourceRole = document.createElement("span");
  sourceRole.textContent = masterSourceRole(product);
  const sourceName = document.createElement("strong");
  sourceName.textContent = masterSourceName(product);
  const sourceDetail = document.createElement("small");
  sourceDetail.textContent = masterSourceDetail(product);
  source.append(sourceRole, sourceName, sourceDetail);

  const connector = document.createElement("div");
  connector.className = "master-graph-connector";
  connector.setAttribute("aria-hidden", "true");
  const connectorLabel = document.createElement("span");
  connectorLabel.textContent =
    product.kind === "flat" ? "calibrates" : "integrates";
  connector.append(connectorLabel);

  const output = document.createElement("div");
  output.className = "master-graph-node master-graph-node--output";
  const outputRole = document.createElement("span");
  outputRole.textContent = "Output";
  const outputName = document.createElement("strong");
  outputName.textContent = masterKindLabel(product.kind);
  const outputDetail = document.createElement("small");
  outputDetail.textContent = product.groupId;
  outputDetail.title = product.groupId;
  output.append(outputRole, outputName, outputDetail);
  rail.append(source, connector, output);

  const summary = document.createElement("div");
  summary.className = "master-graph-card__summary";
  const dependencyLight = document.createElement("span");
  dependencyLight.className = "dependency-light";
  dependencyLight.setAttribute("aria-hidden", "true");
  const summaryText = document.createElement("div");
  const summaryTitle = document.createElement("strong");
  summaryTitle.textContent = pedestalTitle(product);
  const summaryDetail = document.createElement("span");
  summaryDetail.textContent = pedestalDetail(product);
  summaryText.append(summaryTitle, summaryDetail);
  summary.append(dependencyLight, summaryText);

  const technicalDetails = document.createElement("details");
  technicalDetails.className = "master-graph-card__details";
  const technicalSummary = document.createElement("summary");
  technicalSummary.textContent = "Technical details";

  const metadata = document.createElement("dl");
  metadata.className = "master-graph-card__metadata";
  appendCompactMetric(
    metadata,
    "Frames",
    `${formatCount(product.frameCount)} frame${product.frameCount === 1 ? "" : "s"}`,
  );
  appendCompactMetric(metadata, "Camera", product.camera ?? "Unresolved");
  appendCompactMetric(metadata, "Axes", product.axes.join(" × "));
  appendCompactMetric(
    metadata,
    "Exposure",
    product.exposureSeconds === null ? "—" : `${product.exposureSeconds} s`,
  );
  appendCompactMetric(
    metadata,
    "Temperature",
    product.sensorTemperatureCelsius === null
      ? "—"
      : `${product.sensorTemperatureCelsius.toFixed(1)} °C`,
  );
  appendCompactMetric(
    metadata,
    "Gain / offset",
    `${product.gain ?? "—"} / ${product.offset ?? "—"}`,
  );
  appendCompactMetric(
    metadata,
    "Sampling",
    `${product.binning ? `${product.binning.x}×${product.binning.y}` : "—"} · ${product.bayerPattern ?? "Mono / unknown"}`,
  );
  technicalDetails.append(technicalSummary, metadata);
  if (product.kind === "flat" && product.candidates.length > 0) {
    technicalDetails.append(candidateDisclosure(product));
  }
  card.append(header, rail, summary, technicalDetails);
  return card;
}

function masterSourceRole(product: MasterProductPlan): string {
  if (product.kind !== "flat") return "Input";
  switch (product.pedestal.status) {
    case "matched_dark":
      return "Dark pedestal";
    case "bias":
      return "Bias pedestal";
    case "unresolved":
      return "Pedestal needed";
    case "not_applicable":
      return "Input";
  }
}

function masterSourceName(product: MasterProductPlan): string {
  if (product.kind === "flat") {
    return product.pedestal.selectedGroupId ?? "No compatible source";
  }
  return `${formatCount(product.frameCount)} ${humanize(product.kind)} frame${product.frameCount === 1 ? "" : "s"}`;
}

function masterSourceDetail(product: MasterProductPlan): string {
  if (product.kind === "flat") {
    return product.pedestal.status === "unresolved"
      ? "Choose or import a matching Dark / Bias"
      : "Subtracted before Flat integration";
  }
  return "Verified FITS source group";
}

function masterFlowLabel(product: MasterProductPlan): string {
  return `${masterSourceName(product)} ${product.kind === "flat" ? "calibrates" : "integrates into"} ${masterKindLabel(product.kind)} ${product.groupId}. ${pedestalTitle(product)}.`;
}

function appendCompactMetric(
  container: HTMLDListElement,
  label: string,
  value: string,
): void {
  const item = document.createElement("div");
  const term = document.createElement("dt");
  term.textContent = label;
  const description = document.createElement("dd");
  description.textContent = value;
  item.append(term, description);
  container.append(item);
}

function candidateDisclosure(product: MasterProductPlan): HTMLDetailsElement {
  const details = document.createElement("details");
  details.className = "candidate-evidence";
  const summary = document.createElement("summary");
  const compatible = product.candidates.filter(
    (candidate) => candidate.status === "compatible",
  ).length;
  summary.textContent = `${product.candidates.length} pedestal candidate${product.candidates.length === 1 ? "" : "s"} · ${compatible} compatible`;
  const list = document.createElement("ul");
  for (const candidate of product.candidates) {
    const item = document.createElement("li");
    item.dataset.status = candidate.status;
    const title = document.createElement("strong");
    title.textContent = `${candidate.sourceKind.toUpperCase()} · ${candidate.groupId}`;
    const reason = document.createElement("span");
    reason.textContent =
      candidate.status === "compatible"
        ? `Compatible · Δt ${candidate.exposureDeltaSeconds ?? 0} s · ΔT ${candidate.temperatureDeltaCelsius ?? 0} °C`
        : candidate.mismatches
            .map(
              (mismatch) =>
                `${humanize(mismatch.field)} ${humanize(mismatch.reason)}`,
            )
            .join(" · ");
    item.append(title, reason);
    list.append(item);
  }
  details.append(summary, list);
  return details;
}

function masterKindLabel(kind: MasterProductPlan["kind"]): string {
  switch (kind) {
    case "bias":
      return "Master Bias";
    case "dark":
      return "Master Dark";
    case "flat":
      return "Master Flat";
  }
}

function pedestalTitle(product: MasterProductPlan): string {
  switch (product.pedestal.status) {
    case "not_applicable":
      return "Direct strict-mean integration";
    case "matched_dark":
      return "Matched dark selected";
    case "bias":
      return "True bias selected";
    case "unresolved":
      return "Pedestal unresolved";
  }
}

function pedestalDetail(product: MasterProductPlan): string {
  const pedestal = product.pedestal;
  if (pedestal.status === "not_applicable") {
    return "No pedestal subtraction is applied to this source role";
  }
  if (pedestal.status === "unresolved") {
    const ambiguity = pedestal.ambiguousGroupIds.length
      ? ` · ${pedestal.ambiguousGroupIds.join(", ")}`
      : "";
    return `${humanize(pedestal.blockingReason ?? "manual decision required")}${ambiguity}`;
  }
  return `${pedestal.selectedGroupId ?? "Unknown group"} · Δt ${pedestal.exposureDeltaSeconds ?? 0} s · ΔT ${pedestal.temperatureDeltaCelsius ?? 0} °C`;
}

function selectedFrame(model: ReviewViewModel): ReviewFrame | null {
  return (
    model.frames.find((frame) => frame.id === model.selectedFrameId) ?? null
  );
}

function stateLabel(state: ReviewState): string {
  switch (state) {
    case "accepted":
      return "Accepted";
    case "rejected":
      return "Rejected";
    case "undecided":
      return "Undecided";
  }
}

function stateSymbol(state: ReviewState): string {
  switch (state) {
    case "accepted":
      return "✓";
    case "rejected":
      return "×";
    case "undecided":
      return "·";
  }
}

function formatMetric(value: number | null, precision: number): string {
  return value === null ? "—" : value.toFixed(precision);
}

function formatTemperature(value: number | null): string {
  return value === null ? "—" : `${value.toFixed(1)} °C`;
}

function formatCount(value: number): string {
  return value.toLocaleString("en-US");
}

function formatScientificValue(value: number): string {
  const magnitude = Math.abs(value);
  if (magnitude !== 0 && (magnitude >= 1_000_000 || magnitude < 0.001)) {
    return value.toExponential(6);
  }
  return value.toLocaleString("en-US", { maximumFractionDigits: 6 });
}

function humanize(field: string): string {
  return field.replaceAll("_", " ");
}

function isFrameRole(
  value: string | undefined,
): value is ReviewViewModel["activeRole"] {
  return (
    value === "bias" ||
    value === "dark" ||
    value === "flat" ||
    value === "light"
  );
}

function textCell(text: string, className: string): HTMLTableCellElement {
  const output = cell("td", className);
  output.textContent = text;
  return output;
}

function cell(tagName: "td", className: string): HTMLTableCellElement {
  const output = document.createElement(tagName);
  output.className = className;
  return output;
}

function required<T extends Element>(root: ParentNode, selector: string): T {
  const element = root.querySelector<T>(selector);
  if (!element) throw new Error(`Missing required review element: ${selector}`);
  return element;
}

function requiredAll<T extends Element>(
  root: ParentNode,
  selector: string,
): readonly T[] {
  const elements = [...root.querySelectorAll<T>(selector)];
  if (elements.length === 0) {
    throw new Error(`Missing required review elements: ${selector}`);
  }
  return elements;
}

const selectionMetricOptions: readonly [FrameSelectionMetric, string][] = [
  ["fwhm_pixels", "FWHM"],
  ["eccentricity", "Eccentricity"],
  ["signal_to_noise", "Stellar SNR"],
  ["detected_stars", "Detected stars"],
  ["usable_stars", "Usable stars"],
  ["background", "Background"],
  ["noise", "Noise"],
];

function selectionRuleMarkup(index: number, rule: FrameSelectionRule): string {
  const options = selectionMetricOptions
    .map(
      ([value, optionLabel]) =>
        `<option value="${value}"${value === rule.metric ? " selected" : ""}>${optionLabel}</option>`,
    )
    .join("");
  return `<fieldset class="selection-rule" data-selection-rule data-rule-index="${index}">
    <legend><span>Rule ${index + 1}</span></legend>
    <label><span>Metric</span><select class="instrument-select" data-selection-control data-selection-metric>${options}</select></label>
    <label><span>Condition</span><select class="instrument-select" data-selection-control data-selection-comparator>
      <option value="less_than"${rule.comparator === "less_than" ? " selected" : ""}>below</option>
      <option value="greater_than"${rule.comparator === "greater_than" ? " selected" : ""}>above</option>
    </select></label>
    <label><span>Threshold</span><input class="instrument-input" data-selection-control data-selection-threshold type="number" min="0" step="any" value="${rule.threshold.value}" inputmode="decimal" /></label>
    <label><span>When missing</span><select class="instrument-select" data-selection-control data-selection-missing>
      <option value="reject"${rule.missingPolicy === "reject" ? " selected" : ""}>reject</option>
      <option value="retain"${rule.missingPolicy === "retain" ? " selected" : ""}>retain</option>
    </select></label>
    <button class="selection-rule__remove" type="button" data-action="remove-selection-rule" data-rule-index="${index}" aria-label="Remove rule ${index + 1}" title="Remove this quality gate">×</button>
  </fieldset>`;
}

function shellMarkup(): string {
  const reasonButtons = rejectionReasons
    .map(
      ([code, label]) =>
        `<button class="reason-button" type="button" data-action="reject-reason" data-reason="${code}">${label}</button>`,
    )
    .join("");
  return `
    <div class="app-shell">
      <aside class="sidebar" aria-label="Main navigation">
        <div class="brand" aria-label="AetherStack home">
          <span class="brand__mark" aria-hidden="true">A</span>
          <span class="brand__name">AetherStack</span>
        </div>
        <nav class="primary-nav" aria-label="Workflow">
          ${navigationItem("frames", "Frames", "▦")}
          ${navigationItem("calibration", "Calibration", "◫")}
          ${navigationItem("registration", "Registration", "⌖")}
          ${navigationItem("normalization", "Normalize", "≋")}
          ${navigationItem("run", "Run", "▷")}
          ${navigationItem("results", "Results", "◉")}
        </nav>
        <button class="nav-item sidebar__settings" type="button" data-action="select-workspace" data-workspace="settings">
          <span class="nav-item__icon" aria-hidden="true">⚙</span>
          <span>Settings</span>
        </button>
      </aside>

      <main class="workspace" id="workspace">
        <header class="topbar">
          <div>
            <p class="eyebrow">Active session</p>
            <h1 data-session-name></h1>
          </div>
          <div class="topbar__actions">
            <div class="session-activity">
              <span class="health-chip" data-session-status data-tone="ready"><span aria-hidden="true">●</span><span data-session-status-label>Demo ready</span></span>
              <progress class="session-activity__progress" data-session-import-progress aria-label="Session import progress" max="1" hidden></progress>
            </div>
            <button class="button button--quiet" type="button" data-action="open-diagnostics">Diagnostics</button>
            <button class="button button--primary" type="button" data-action="open-selection-panel">Review plan</button>
          </div>
        </header>

        ${workflowCoachMarkup()}

        <section class="frames-workspace" aria-labelledby="frames-heading" data-frames-workspace>
          <div class="workspace-heading">
            <div>
              <p class="eyebrow">Frames</p>
              <h2 id="frames-heading">Review acquisition data</h2>
            </div>
            <div class="workspace-heading__actions">
              <button class="icon-button" type="button" data-action="undo" aria-label="No review decision to undo" aria-keyshortcuts="Meta+Z Control+Z" title="No review decision to undo" disabled>↶</button>
              <button class="button button--quiet" type="button" data-action="import-session" data-import-session>＋ Import session</button>
            </div>
          </div>

          <div class="role-tabs" role="tablist" aria-label="Frame types" data-role-tabs></div>

          <div class="review-layout">
            <section class="frame-browser" aria-labelledby="light-table-heading">
              <div class="panel-heading">
                <div>
                  <h3 id="light-table-heading" data-role-heading></h3>
                  <p><span data-selection-count></span> · metrics are diagnostic</p>
                </div>
                <div class="panel-heading__actions">
                  <div class="frame-stage-switch" role="group" aria-label="Light pixel stage" data-light-frame-view>
                    <button class="tool-button" type="button" data-action="select-light-frame-view" data-light-frame-view-value="raw" aria-pressed="true">Raw</button>
                    <button class="tool-button" type="button" data-action="select-light-frame-view" data-light-frame-view-value="calibrated" aria-pressed="false" disabled>Calibrated</button>
                  </div>
                  <button class="tool-button" type="button" data-action="measure-all-quality" aria-label="All eligible light frames have diagnostic quality measurements" aria-live="polite" aria-atomic="true" disabled>Quality complete</button>
                  <label class="frame-filter"><span class="sr-only">Filter frames by file name</span><span aria-hidden="true">⌕</span><input type="search" data-frame-filter autocomplete="off" spellcheck="false" /><button type="button" data-action="clear-frame-filter" aria-label="Clear frame filter">×</button></label>
                </div>
              </div>
              <div class="table-scroll" tabindex="0" aria-label="Scrollable frame table">
                <table class="frame-table" data-frame-table aria-label="Frame review metrics">
                  <thead>
                    <tr>
                      <th scope="col"><span class="sr-only">Review state</span></th>
                      ${sortableHeading("Frame", "label")}
                      <th scope="col" class="numeric">Exposure</th>
                      <th scope="col" class="numeric">Temp.</th>
                      ${sortableHeading("FWHM", "fwhm_major", true)}
                      ${sortableHeading("Ecc.", "eccentricity", true)}
                      ${sortableHeading("Stars", "detected_stars", true)}
                    </tr>
                  </thead>
                  <tbody data-frame-rows></tbody>
                </table>
              </div>
              <div class="table-legend" aria-label="Review state legend">
                <span data-state="accepted"><b aria-hidden="true">✓</b> Accepted</span>
                <span data-state="rejected"><b aria-hidden="true">×</b> Rejected</span>
                <span data-state="undecided"><b aria-hidden="true">·</b> Undecided</span>
              </div>
              <details class="selection-console" data-selection-panel>
                <summary>
                  <span><small>AUTOMATIC REVIEW</small><strong>Quality gates</strong></span>
                  <span class="selection-console__lamp" aria-hidden="true"></span>
                </summary>
                <div class="selection-console__body">
                  <p>Preview-only rules. Rust evaluates native measurements and never overwrites manual decisions.</p>
                  <div class="selection-rules" data-selection-rules aria-label="Automatic frame selection rules"></div>
                  <button class="selection-add-rule" type="button" data-action="add-selection-rule">＋ Add quality gate</button>
                  <section class="selection-evidence" data-selection-evidence aria-labelledby="selection-evidence-title" hidden>
                    <div class="selection-evidence__heading">
                      <span id="selection-evidence-title">Selected-frame evidence</span>
                      <strong data-selection-evidence-frame></strong>
                    </div>
                    <ol data-selection-evidence-list></ol>
                  </section>
                  <div class="selection-console__footer">
                    <div class="selection-plan-summary" aria-live="polite">
                      <span><b data-selection-retained>—</b> retain</span>
                      <span><b data-selection-rejected>—</b> reject</span>
                      <span data-selection-status>Measure every Light, then preview</span>
                    </div>
                    <div class="selection-console__actions">
                      <button class="button button--quiet" type="button" data-action="open-selection-confirmation" disabled>Apply recommendations</button>
                      <button class="button button--primary" type="button" data-action="preview-frame-selection">Preview recommendations</button>
                    </div>
                  </div>
                  <code class="selection-digest" data-selection-digest>Canonical plan digest appears after native preview</code>
                </div>
              </details>
            </section>

            <section class="viewer" aria-labelledby="viewer-heading">
              <div class="viewer__topbar">
                <div>
                  <p class="eyebrow">Display-only preview</p>
                  <h3 id="viewer-heading" data-selected-label></h3>
                </div>
                <div class="viewer__tools" aria-label="Viewer controls">
                  <button class="tool-button" type="button" data-action="viewer-fit" aria-label="Fit preview to viewer" aria-pressed="true">Fit</button>
                  <button class="tool-button" type="button" data-action="viewer-actual" aria-label="Show preview pixels at one hundred percent" aria-pressed="false">1:1</button>
                  <button class="tool-button" type="button" data-action="measure-quality" aria-label="Measure diagnostic frame quality">Measure quality</button>
                  <button class="tool-button" type="button" data-action="open-statistics" aria-label="Inspect exact FITS statistics">Statistics</button>
                </div>
              </div>

              <div class="preview-frame">
                <div class="preview-surface" role="group" data-preview>
                  <img class="preview-image" data-preview-image alt="" hidden />
                  <div class="preview-placeholder" data-preview-placeholder>
                    <span class="preview-placeholder__aperture" aria-hidden="true"></span>
                    <strong data-preview-title></strong>
                    <span data-preview-description></span>
                  </div>
                </div>
                <div class="preview-badges" data-preview-badges>
                  <span class="viewer-chip" data-cfa-badge></span>
                  <span class="viewer-chip viewer-chip--quality" data-quality-badge></span>
                  <span class="viewer-chip viewer-chip--lock" data-stretch-label></span>
                </div>
                <dl class="metric-strip" aria-label="Selected frame metrics">
                  ${metric("Stellar SNR", "data-metric-signal-to-noise", "")}
                  ${metric("FWHM", "data-metric-fwhm", "px")}
                  ${metric("Eccentricity", "data-metric-eccentricity", "")}
                  ${metric("Stars", "data-metric-stars", "")}
                  ${metric("Background", "data-metric-background", "DN")}
                  ${metric("Noise", "data-metric-noise", "DN")}
                </dl>
              </div>

              <div class="review-controls">
                <div class="blink-controls" aria-label="Blink playback">
                  <button class="transport-button" type="button" data-action="previous" data-step-action aria-label="Previous frame">‹</button>
                  <button class="transport-button transport-button--play" type="button" data-action="toggle-play" aria-label="Start Blink playback" aria-pressed="false"><span data-play-icon aria-hidden="true">▶</span></button>
                  <button class="transport-button" type="button" data-action="next" data-step-action aria-label="Next frame">›</button>
                  <span class="frame-position" data-frame-position></span>
                </div>
                <div class="decision-controls" aria-label="Frame decision" data-decision-controls aria-busy="false">
                  <span class="state-chip" data-review-state data-state="undecided">Undecided</span>
                  <button class="button button--quiet" type="button" data-action="clear-decision" data-decision-action aria-keyshortcuts="C" title="Clear decision (C)">Clear</button>
                  <button class="button button--danger" type="button" data-action="open-reject" data-decision-action aria-keyshortcuts="R" title="Reject with a reason (R)">Reject</button>
                  <button class="button button--success" type="button" data-action="accept" data-decision-action aria-keyshortcuts="A" title="Accept selected frame (A)">Accept</button>
                </div>
              </div>
            </section>
          </div>
        </section>

        <section class="registration-workspace" aria-labelledby="registration-heading" data-registration-workspace hidden>
          <div class="workspace-heading registration-heading">
            <div>
              <p class="eyebrow">Registration laboratory</p>
              <h2 id="registration-heading">Solve geometry before moving pixels</h2>
              <p class="workspace-intro">Inspect a deterministic star-field solution, its confidence evidence and the exact common footprint. Geometry review never writes pixels; execution publishes only a complete sealed set.</p>
            </div>
            <div class="workspace-heading__actions">
              <span class="registration-readiness" data-registration-status role="status" aria-live="polite"></span>
              <button class="button button--primary" type="button" data-action="analyze-registration">Analyze geometry</button>
            </div>
          </div>

          <div class="registration-layout">
            <aside class="registration-console" aria-labelledby="registration-pair-heading">
              <div class="panel-heading panel-heading--compact">
                <div>
                  <p class="eyebrow">Light frame pair</p>
                  <h3 id="registration-pair-heading">Reference geometry</h3>
                </div>
                <span class="hardware-light" aria-hidden="true"></span>
              </div>
              <fieldset class="geometry-model" aria-label="Registration geometry model">
                <legend>Geometry model</legend>
                <div class="geometry-model__choices">
                  <button class="geometry-model__choice" type="button" data-action="select-registration-geometry" data-registration-geometry-model="affine" aria-pressed="true">
                    <strong>Affine</strong><span>Stable default</span>
                  </button>
                  <button class="geometry-model__choice" type="button" data-action="select-registration-geometry" data-registration-geometry-model="projective" aria-pressed="false">
                    <strong>Projective</strong><span>Only when recommended</span>
                  </button>
                </div>
              </fieldset>
              <label class="control-field">
                <span>Reference frame</span>
                <select class="instrument-select" data-registration-reference></select>
              </label>
              <label class="control-field">
                <span>Source frame</span>
                <select class="instrument-select" data-registration-source></select>
              </label>
              <p class="control-note">The solver detects on the CFA-safe luminance plane, then lifts the accepted transform back to exact source-pixel coordinates.</p>
              <section class="registration-plan" aria-labelledby="registration-plan-heading">
                <div class="registration-plan__heading">
                  <div>
                    <p class="eyebrow">Multi-frame plan</p>
                    <h4 id="registration-plan-heading">Transform review</h4>
                  </div>
                  <strong data-registration-plan-progress data-ready="false">0 / 0 transforms accepted</strong>
                </div>
                <ol data-registration-plan-frames aria-label="Registration plan frame status"></ol>
                <code class="registration-plan__digest" data-registration-plan-digest>The final digest appears after native reconstruction</code>
              </section>
              <section class="registration-execution" data-registration-execution data-state="idle" aria-labelledby="registration-execution-heading">
                <div class="registration-execution__heading">
                  <div>
                    <p class="eyebrow">Atomic execution</p>
                    <h4 id="registration-execution-heading">Register calibrated Lights</h4>
                  </div>
                  <span class="instrument-label">RGB / MONO</span>
                </div>
                <p data-registration-execution-message>Calibrate the reviewed Lights to unlock registration</p>
                <progress data-registration-execution-progress aria-label="Registration progress" hidden></progress>
                <code data-registration-execution-output>Calibrated Light artifacts required</code>
                <div class="registration-execution__actions">
                  <button class="button button--primary" type="button" data-action="execute-registration" disabled>Register all frames</button>
                  <button class="button button--danger" type="button" data-action="cancel-registration" hidden>Cancel</button>
                </div>
              </section>
              <section class="drizzle-console" data-drizzle data-state="idle" aria-labelledby="drizzle-heading">
                <div class="drizzle-console__heading">
                  <div>
                    <p class="eyebrow">Native CFA reconstruction</p>
                    <h4 id="drizzle-heading">Drizzle laboratory</h4>
                  </div>
                  <span class="instrument-label">F64 · PROJECTIVE</span>
                </div>
                <p data-drizzle-message>Seal a registration plan to unlock CFA Drizzle</p>
                <div class="drizzle-console__scale" role="group" aria-label="Drizzle output scale">
                  <span>Output scale</span>
                  <div class="segmented-control segmented-control--compact">
                    <button type="button" data-action="select-drizzle-scale" data-drizzle-scale="1" aria-pressed="false">1×</button>
                    <button type="button" data-action="select-drizzle-scale" data-drizzle-scale="2" aria-pressed="true">2×</button>
                    <button type="button" data-action="select-drizzle-scale" data-drizzle-scale="3" aria-pressed="false">3×</button>
                  </div>
                </div>
                <div class="drizzle-console__weighting" role="group" aria-label="Drizzle frame weighting">
                  <span>Frame weighting</span>
                  <div class="segmented-control segmented-control--compact">
                    <button type="button" data-action="select-drizzle-weighting" data-drizzle-weighting="uniform" aria-pressed="true">Uniform</button>
                    <button type="button" data-action="select-drizzle-weighting" data-drizzle-weighting="balanced_psf" aria-pressed="false">Balanced PSF</button>
                  </div>
                </div>
                <output class="drizzle-console__weight-status" data-drizzle-weight-status data-ready="true">Equal identity-bound contribution per frame</output>
                <div class="drizzle-console__controls">
                  <label>
                    <span><strong>Drop shrink</strong><output data-drizzle-drop-shrink-value>0.80</output></span>
                    <input data-drizzle-drop-shrink type="range" min="0.25" max="1" step="0.05" value="0.8" />
                  </label>
                  <label>
                    <span><strong>Contribution ceiling</strong><output data-drizzle-maximum-contributions-value>64</output></span>
                    <input data-drizzle-maximum-contributions type="range" min="4" max="256" step="4" value="64" />
                  </label>
                  <label>
                    <span><strong>Band height</strong><output data-drizzle-maximum-band-height-value>128 px</output></span>
                    <input data-drizzle-maximum-band-height type="range" min="16" max="512" step="16" value="128" />
                  </label>
                </div>
                <progress data-drizzle-progress aria-label="Drizzle progress" hidden></progress>
                <code data-drizzle-output>Science + weight + support · atomic FITS set</code>
                <div class="drizzle-console__product-tabs" role="tablist" aria-label="Drizzle product view">
                  <button type="button" role="tab" data-action="select-drizzle-product" data-drizzle-product="science" aria-label="Drizzle science" aria-selected="true" disabled>Science</button>
                  <button type="button" role="tab" data-action="select-drizzle-product" data-drizzle-product="weight" aria-label="Drizzle weight" aria-selected="false" disabled>Weight</button>
                  <button type="button" role="tab" data-action="select-drizzle-product" data-drizzle-product="support" aria-label="Drizzle support" aria-selected="false" disabled>Support</button>
                </div>
                <div class="drizzle-console__preview" aria-live="polite">
                  <img data-drizzle-preview-image alt="" hidden />
                  <div data-drizzle-preview-placeholder>The atomic Drizzle product set will appear here</div>
                </div>
                <div class="drizzle-console__pixel-controls" aria-label="Exact Drizzle FITS coordinate">
                  <label><span>X</span><input data-drizzle-pixel-x aria-label="Drizzle X" type="number" min="0" max="0" step="1" value="0" inputmode="numeric" disabled /></label>
                  <label><span>Y</span><input data-drizzle-pixel-y aria-label="Drizzle Y" type="number" min="0" max="0" step="1" value="0" inputmode="numeric" disabled /></label>
                  <button class="button button--quiet" type="button" data-action="inspect-drizzle-pixel" aria-label="Inspect Drizzle pixel" disabled>Inspect pixel</button>
                </div>
                <output class="drizzle-console__pixel-readout" data-drizzle-pixel-readout aria-live="polite">Enter an output coordinate to inspect every plane.</output>
                <div class="drizzle-console__actions">
                  <button class="button button--quiet" type="button" data-action="inspect-drizzle-statistics" disabled>Exact statistics</button>
                  <button class="button button--primary" type="button" data-action="execute-drizzle" disabled>Build Drizzle set</button>
                  <button class="button button--danger" type="button" data-action="cancel-drizzle" hidden>Cancel</button>
                </div>
              </section>
              <div class="registration-signal" aria-hidden="true">
                <span></span><span></span><span></span><span></span><span></span>
              </div>
              <details class="advanced-settings registration-evidence">
                <summary>Exact geometric evidence</summary>
                <code data-registration-matrix>Awaiting geometric evidence</code>
              </details>
            </aside>

            <section class="registration-instrument" aria-labelledby="registration-solution-heading">
              <div class="panel-heading">
                <div>
                  <p class="eyebrow">Confidence-gated solution</p>
                  <h3 id="registration-solution-heading">Geometric solution</h3>
                </div>
                <span class="instrument-label">F64 · Lanczos-3</span>
              </div>
              <dl class="registration-metrics" aria-label="Registration quality metrics">
                ${registrationMetric("RMS residual", "data-registration-rms", "source px")}
                ${registrationMetric("Inlier pairs", "data-registration-inliers", "matches")}
                ${registrationMetric("Coverage", "data-registration-coverage", "%")}
                ${registrationMetric("Rotation", "data-registration-rotation", "°")}
              </dl>
              <div class="registration-readouts">
                <div><span>Scale</span><strong data-registration-scale>—</strong></div>
                <div><span>Reflection</span><strong data-registration-reflection>—</strong></div>
                <div><span>Winner support</span><strong data-registration-support>—</strong></div>
              </div>
              <section class="footprint-panel" aria-labelledby="footprint-heading">
                <div>
                  <p class="eyebrow">Analytical common footprint</p>
                  <h4 id="footprint-heading">Autocrop preview</h4>
                  <p data-registration-crop-text>No accepted footprint yet</p>
                </div>
                <div class="sensor-stage" role="img" aria-label="No accepted common footprint yet">
                  <span class="sensor-stage__grid" aria-hidden="true"></span>
                  <span class="sensor-stage__crop" data-registration-crop aria-hidden="true"></span>
                </div>
              </section>
              <div class="registration-rejections" data-registration-rejections hidden></div>
              <section class="registered-review" aria-labelledby="registered-review-heading">
                <div class="registered-review__heading">
                  <div>
                    <p class="eyebrow">Published pixel inspection</p>
                    <h4 id="registered-review-heading">Registered Blink</h4>
                  </div>
                  <span class="viewer-chip viewer-chip--lock" data-registered-preview-stretch>Registered stretch · awaiting pixels</span>
                </div>
                <div class="registered-review__toolbar">
                  <label>
                    <span class="sr-only">Registered frame</span>
                    <select class="instrument-select" data-registered-frame aria-label="Registered frame" disabled></select>
                  </label>
                  <span data-registered-preview-position>0 / 0</span>
                </div>
                <div class="registered-review__viewport" aria-live="polite">
                  <img data-registered-preview-image alt="" hidden />
                  <div class="registered-review__placeholder" data-registered-preview-placeholder>
                    <span class="preview-placeholder__aperture" aria-hidden="true"></span>
                    <strong>Atomic result viewer</strong>
                    <span data-registered-preview-message>Registered pixels will appear here after atomic publication</span>
                  </div>
                </div>
                <div class="registered-review__transport" aria-label="Registered Blink playback">
                  <button class="transport-button" type="button" data-action="previous-registered" aria-label="Previous registered frame" disabled>‹</button>
                  <button class="transport-button transport-button--play" type="button" data-action="toggle-registered-play" aria-label="Start registered Blink" aria-pressed="false" disabled>▶</button>
                  <button class="transport-button" type="button" data-action="next-registered" aria-label="Next registered frame" disabled>›</button>
                </div>
                <section class="registered-stack" data-registered-stack data-state="idle" aria-labelledby="registered-stack-heading">
                  <div class="registered-stack__heading">
                    <div>
                      <p class="eyebrow">Final scientific product</p>
                      <h5 id="registered-stack-heading">Integrate common crop</h5>
                    </div>
                    <span class="instrument-label" data-registered-stack-estimator-label>STRICT F64 MEAN</span>
                  </div>
                  <p data-registered-stack-message>Register the reviewed Lights to unlock integration</p>
                  <button class="button button--quiet registered-stack__open-report" type="button" data-action="open-registered-stack-report">Open prior report</button>
                  <div class="integration-mode" role="group" aria-label="Integration planning mode">
                    <button type="button" data-action="select-registered-stack-mode" data-stack-mode="automatic" aria-pressed="false">
                      <span>Automatic</span><small>Quality policy</small>
                    </button>
                    <button type="button" data-action="select-registered-stack-mode" data-stack-mode="manual" aria-pressed="true">
                      <span>Manual</span><small>Explicit controls</small>
                    </button>
                  </div>
                  <section class="automatic-plan" data-registered-stack-automatic-plan hidden aria-labelledby="automatic-plan-heading">
                    <div class="automatic-plan__heading">
                      <div><span class="status-light" aria-hidden="true"></span><strong id="automatic-plan-heading">Native quality plan</strong></div>
                      <span data-registered-stack-automatic-state>Not requested</span>
                    </div>
                    <dl>
                      <div><dt>Population</dt><dd data-registered-stack-automatic-tier>—</dd></div>
                      <div><dt>Estimator</dt><dd data-registered-stack-automatic-estimator>—</dd></div>
                    </dl>
                    <p data-registered-stack-automatic-rationale>The sealed registration population determines the recommendation.</p>
                    <code data-registered-stack-automatic-seal>Awaiting native seal</code>
                  </section>
                  <details class="advanced-settings registered-stack__advanced">
                    <summary>Advanced integration</summary>
                    <div class="registered-stack__control-grid">
                      <label class="control-field registered-stack__estimator">
                        <span>Estimator</span>
                        <select class="instrument-select" data-registered-stack-estimator>
                          <option value="strict_mean">Strict compensated mean</option>
                          <option value="median">Exact median</option>
                          <option value="weighted_mean">Balanced PSF weighting</option>
                          <option value="percentile_clipped">Percentile-clipped mean</option>
                          <option value="sigma_clipped">Iterative sigma clipping</option>
                          <option value="winsorized_sigma_clipped">Winsorized sigma</option>
                          <option value="linear_fit_clipped">Linear fit clipping</option>
                          <option value="generalized_esd">Generalized ESD</option>
                        </select>
                      </label>
                      <label class="control-field">
                        <span>Low-tail fraction</span>
                        <span class="instrument-stepper" data-stepper>
                          <button type="button" data-action="step-number" data-step-direction="down" aria-label="Decrease low-tail fraction">−</button>
                          <input data-registered-stack-low-fraction aria-label="Low-tail fraction" type="number" min="0" max="0.49" step="0.01" inputmode="decimal" />
                          <button type="button" data-action="step-number" data-step-direction="up" aria-label="Increase low-tail fraction">+</button>
                        </span>
                      </label>
                      <label class="control-field">
                        <span>High-tail fraction</span>
                        <span class="instrument-stepper" data-stepper>
                          <button type="button" data-action="step-number" data-step-direction="down" aria-label="Decrease high-tail fraction">−</button>
                          <input data-registered-stack-high-fraction aria-label="High-tail fraction" type="number" min="0" max="0.49" step="0.01" inputmode="decimal" />
                          <button type="button" data-action="step-number" data-step-direction="up" aria-label="Increase high-tail fraction">+</button>
                        </span>
                      </label>
                      <label class="control-field">
                        <span>Low sigma</span>
                        <span class="instrument-stepper" data-stepper>
                          <button type="button" data-action="step-number" data-step-direction="down" aria-label="Decrease low sigma">−</button>
                          <input data-registered-stack-low-sigma aria-label="Low sigma" type="number" min="0.1" step="0.1" inputmode="decimal" />
                          <button type="button" data-action="step-number" data-step-direction="up" aria-label="Increase low sigma">+</button>
                        </span>
                      </label>
                      <label class="control-field">
                        <span>High sigma</span>
                        <span class="instrument-stepper" data-stepper>
                          <button type="button" data-action="step-number" data-step-direction="down" aria-label="Decrease high sigma">−</button>
                          <input data-registered-stack-high-sigma aria-label="High sigma" type="number" min="0.1" step="0.1" inputmode="decimal" />
                          <button type="button" data-action="step-number" data-step-direction="up" aria-label="Increase high sigma">+</button>
                        </span>
                      </label>
                      <label class="control-field">
                        <span>Maximum outlier fraction</span>
                        <span class="instrument-stepper" data-stepper>
                          <button type="button" data-action="step-number" data-step-direction="down" aria-label="Decrease maximum ESD outlier fraction">−</button>
                          <input data-registered-stack-esd-outlier-fraction aria-label="Maximum ESD outlier fraction" type="number" min="0.01" max="0.5" step="0.01" inputmode="decimal" />
                          <button type="button" data-action="step-number" data-step-direction="up" aria-label="Increase maximum ESD outlier fraction">+</button>
                        </span>
                      </label>
                      <label class="control-field">
                        <span>ESD significance</span>
                        <span class="instrument-stepper" data-stepper>
                          <button type="button" data-action="step-number" data-step-direction="down" aria-label="Decrease ESD significance">−</button>
                          <input data-registered-stack-esd-significance aria-label="ESD significance" type="number" min="0.001" max="0.999" step="0.001" inputmode="decimal" />
                          <button type="button" data-action="step-number" data-step-direction="up" aria-label="Increase ESD significance">+</button>
                        </span>
                      </label>
                      <label class="control-field">
                        <span>Maximum clipping passes</span>
                        <span class="instrument-stepper" data-stepper>
                          <button type="button" data-action="step-number" data-step-direction="down" aria-label="Decrease maximum clipping passes">−</button>
                          <input data-registered-stack-maximum-iterations aria-label="Maximum clipping passes" type="number" min="1" max="4294967295" step="1" inputmode="numeric" />
                          <button type="button" data-action="step-number" data-step-direction="up" aria-label="Increase maximum clipping passes">+</button>
                        </span>
                      </label>
                      <label class="control-field">
                        <span>Minimum retained samples</span>
                        <span class="instrument-stepper" data-stepper>
                          <button type="button" data-action="step-number" data-step-direction="down" aria-label="Decrease minimum retained samples">−</button>
                          <input data-registered-stack-minimum-retained aria-label="Minimum retained samples" type="number" min="1" max="4294967295" step="1" inputmode="numeric" />
                          <button type="button" data-action="step-number" data-step-direction="up" aria-label="Increase minimum retained samples">+</button>
                        </span>
                      </label>
                    </div>
                    <fieldset class="large-scale-rejection" data-registered-stack-large-scale hidden>
                      <legend>Large-scale pixel rejection</legend>
                      <p>Expand source-owned ESD evidence across coherent structures. Exact halos keep results independent of processing band size.</p>
                      <div class="large-scale-rejection__tails">
                        <section class="large-scale-tail" aria-labelledby="large-scale-low-title">
                          <label class="large-scale-tail__switch">
                            <input data-registered-stack-large-scale-low-enabled type="checkbox" />
                            <span><strong id="large-scale-low-title">Low tail</strong><small>Cold trails and broad negative defects</small></span>
                          </label>
                          <label class="control-field">
                            <span>Detection layers</span>
                            <input data-registered-stack-large-scale-low-layers aria-label="Low-tail large-scale detection layers" type="number" min="1" max="12" step="1" inputmode="numeric" />
                          </label>
                          <label class="control-field">
                            <span>Growth radius</span>
                            <input data-registered-stack-large-scale-low-growth aria-label="Low-tail large-scale growth radius" type="number" min="0" max="256" step="1" inputmode="numeric" />
                          </label>
                        </section>
                        <section class="large-scale-tail" aria-labelledby="large-scale-high-title">
                          <label class="large-scale-tail__switch">
                            <input data-registered-stack-large-scale-high-enabled type="checkbox" />
                            <span><strong id="large-scale-high-title">High tail</strong><small>Satellite trails and broad positive defects</small></span>
                          </label>
                          <label class="control-field">
                            <span>Detection layers</span>
                            <input data-registered-stack-large-scale-high-layers aria-label="High-tail large-scale detection layers" type="number" min="1" max="12" step="1" inputmode="numeric" />
                          </label>
                          <label class="control-field">
                            <span>Growth radius</span>
                            <input data-registered-stack-large-scale-high-growth aria-label="High-tail large-scale growth radius" type="number" min="0" max="256" step="1" inputmode="numeric" />
                          </label>
                        </section>
                      </div>
                    </fieldset>
                    <label class="registered-stack__map-toggle">
                      <input data-registered-stack-rejection-maps type="checkbox" />
                      <span><strong>Publish rejection evidence</strong><small>Create separate low-tail and high-tail FITS maps beside the science product.</small></span>
                    </label>
                    <label class="registered-stack__map-toggle registered-stack__map-toggle--support">
                      <input data-registered-stack-support-map type="checkbox" />
                      <span><strong>Publish accepted support</strong><small>Create an exact per-pixel FITS map of source samples retained by integration.</small></span>
                    </label>
                    <section class="weight-preflight" data-registered-stack-weight-preflight hidden aria-labelledby="weight-preflight-title">
                      <div class="weight-preflight__heading">
                        <div>
                          <strong id="weight-preflight-title">Weight evidence</strong>
                          <small>Auto selects the strongest balanced quality score</small>
                        </div>
                        <span data-registered-stack-weight-status aria-live="polite">Metrics required</span>
                      </div>
                      <label class="weight-preflight__reference">
                        <span>Weight reference</span>
                        <select class="instrument-select" data-registered-stack-weight-reference aria-label="Weight reference"></select>
                      </label>
                      <div class="weight-preflight__seal" data-registered-stack-weight-seal hidden>
                        <span>Rust evidence seal</span>
                        <strong data-registered-stack-weight-algorithm></strong>
                        <code data-registered-stack-weight-digest></code>
                      </div>
                      <div class="weight-preflight__table-wrap">
                        <table>
                          <thead><tr><th scope="col">Frame</th><th scope="col">SNR</th><th scope="col">FWHM</th><th scope="col">Ecc.</th><th scope="col">Relative weight</th></tr></thead>
                          <tbody data-registered-stack-weight-rows></tbody>
                        </table>
                      </div>
                    </section>
                    <p class="registered-stack__advanced-note">Strict mean is the reproducibility reference. Balanced PSF weighting requires measured calibrated Lights. Percentile, sigma, Winsorized sigma, one-pass ordered Linear Fit, and two-sided generalized ESD estimators publish exact low/high rejection evidence. ESD remains inactive below 15 usable samples.</p>
                  </details>
                  <progress data-registered-stack-progress aria-label="Registered stack progress" hidden></progress>
                  <code data-registered-stack-output>Published registered artifacts required</code>
                  <section class="source-rejection-evidence" data-registered-stack-source-rejections aria-labelledby="source-rejection-evidence-title" hidden>
                    <div class="source-rejection-evidence__heading">
                      <h6 id="source-rejection-evidence-title">Source rejection evidence</h6>
                      <output data-registered-stack-source-rejection-summary aria-live="polite"></output>
                    </div>
                    <div class="spatial-promotion-evidence" data-registered-stack-spatial-promotions role="status" aria-labelledby="spatial-promotion-evidence-title" hidden>
                      <span class="spatial-promotion-evidence__icon" aria-hidden="true">◎</span>
                      <span>
                        <small id="spatial-promotion-evidence-title">Large-scale spatial pass</small>
                        <strong data-registered-stack-spatial-promotion-total></strong>
                        <span data-registered-stack-spatial-promotion-breakdown></span>
                      </span>
                    </div>
                    <ol data-registered-stack-source-rejection-rows aria-label="Per-source rejection totals"></ol>
                  </section>
                  <div class="integration-report" data-registered-stack-report hidden>
                    <div class="integration-report__heading">
                      <span>Integration report</span>
                      <div class="integration-report__actions">
                        <button class="button button--quiet" type="button" data-action="return-to-active-stack" hidden>Close archived report</button>
                        <button class="button button--quiet" type="button" data-action="inspect-registered-stack-report">Verify report</button>
                      </div>
                    </div>
                    <code data-registered-stack-report-path></code>
                    <code data-registered-stack-report-digest></code>
                    <output data-registered-stack-report-summary aria-live="polite"></output>
                    <ul class="integration-report__products" data-registered-stack-report-products aria-label="Verified integration products"></ul>
                    <details class="integration-report__source-browser">
                      <summary>Source evidence</summary>
                      <div class="integration-report__source-actions">
                        <output data-registered-stack-source-verification>Choose the directory containing the registered source FITS files</output>
                        <button class="button button--quiet" type="button" data-action="verify-registered-stack-sources">Verify source folder</button>
                        <button class="button button--quiet" type="button" data-action="cancel-registered-stack-source-verification" hidden>Cancel hashing</button>
                      </div>
                      <progress data-registered-stack-source-progress aria-label="Archived source verification progress" hidden></progress>
                      <div class="integration-report__source-toolbar">
                        <output data-registered-stack-source-summary>0 sources · 0 verified · 0 issues · 0 pending</output>
                        <label class="integration-report__source-search">
                          <span class="sr-only">Search source filenames</span>
                          <span aria-hidden="true">⌕</span>
                          <input type="text" data-registered-stack-source-search placeholder="Find FITS" autocomplete="off" spellcheck="false" />
                          <button type="button" data-action="clear-source-evidence-search" aria-label="Clear source search" hidden>×</button>
                        </label>
                        <div class="integration-report__source-filters" role="group" aria-label="Filter source evidence">
                          <button type="button" data-action="filter-source-evidence" data-source-evidence-filter="all" aria-pressed="true">All</button>
                          <button type="button" data-action="filter-source-evidence" data-source-evidence-filter="issues" aria-pressed="false">Issues</button>
                          <button type="button" data-action="filter-source-evidence" data-source-evidence-filter="verified" aria-pressed="false">Verified</button>
                          <button type="button" data-action="filter-source-evidence" data-source-evidence-filter="unverified" aria-pressed="false">Pending</button>
                        </div>
                      </div>
                      <ol data-registered-stack-report-sources aria-label="Sealed source evidence"></ol>
                      <button class="integration-report__show-more" type="button" data-action="show-more-source-evidence" hidden>Show next 250</button>
                    </details>
                  </div>
                  <div class="registered-stack__product-tabs" role="tablist" aria-label="Integrated product view">
                    <button type="button" role="tab" data-action="select-registered-stack-product" data-stack-product="science" aria-selected="true">Science</button>
                    <button type="button" role="tab" data-action="select-registered-stack-product" data-stack-product="rejection_low" aria-selected="false" disabled>Low reject</button>
                    <button type="button" role="tab" data-action="select-registered-stack-product" data-stack-product="rejection_high" aria-selected="false" disabled>High reject</button>
                    <button type="button" role="tab" data-action="select-registered-stack-product" data-stack-product="support" aria-selected="false" disabled>Support</button>
                  </div>
                  <div class="registered-stack__preview" data-action="inspect-registered-stack-pixel">
                    <img class="registered-stack__science-layer" data-registered-stack-science-image alt="" hidden />
                    <img class="registered-stack__diagnostic-layer" data-registered-stack-preview-image alt="" hidden />
                    <div data-registered-stack-preview-placeholder>The integrated common crop will appear here after publication</div>
                  </div>
                  <label class="registered-stack__overlay-control" data-registered-stack-overlay-control hidden>
                    <span>Science overlay</span>
                    <input id="registered-stack-overlay-opacity" data-registered-stack-overlay-opacity type="range" min="0" max="100" step="1" value="65" aria-label="Rejection map opacity over science" />
                    <output data-registered-stack-overlay-value for="registered-stack-overlay-opacity">65%</output>
                  </label>
                  <section class="rejection-histogram" data-registered-stack-histogram aria-label="Exact rejection-count distribution" hidden>
                    <div class="rejection-histogram__heading">
                      <span>Exact count distribution</span>
                      <strong data-registered-stack-histogram-summary></strong>
                    </div>
                    <div class="rejection-histogram__bins" data-registered-stack-histogram-bins></div>
                  </section>
                  <div class="registered-stack__pixel-controls" aria-label="Exact FITS coordinate">
                    <label><span>X</span><input data-registered-stack-pixel-x type="number" min="0" step="1" value="0" inputmode="numeric" /></label>
                    <label><span>Y</span><input data-registered-stack-pixel-y type="number" min="0" step="1" value="0" inputmode="numeric" /></label>
                    <button class="button button--quiet" type="button" data-action="inspect-registered-stack-coordinate">Inspect pixel</button>
                  </div>
                  <output class="registered-stack__pixel-readout" data-registered-stack-pixel-readout aria-live="polite">Click the image to inspect exact FITS values.</output>
                  <div class="registered-stack__actions">
                    <button class="button button--primary" type="button" data-action="execute-registered-stack" disabled>Integrate crop</button>
                    <button class="button button--danger" type="button" data-action="cancel-registered-stack" hidden>Cancel</button>
                  </div>
                </section>
              </section>
            </section>
          </div>
        </section>

        ${localNormalizationMarkup()}

        <section class="calibration-workspace" aria-labelledby="calibration-heading" data-calibration-workspace hidden>
          <div class="workspace-heading calibration-heading">
            <div>
              <p class="eyebrow">Calibration laboratory</p>
              <h2 id="calibration-heading">Build exact master frames</h2>
              <p class="workspace-intro">Bias, Darks, Flats and Lights remain separate. The native planner binds every Flat to one pedestal, then every Light to one exact Dark and one normalized Flat.</p>
            </div>
            <div class="workspace-heading__actions">
              <span class="calibration-readiness" data-calibration-status role="status" aria-live="polite"></span>
              <button class="button button--quiet" type="button" data-action="refresh-master-plan">↻ Rebuild plan</button>
              <button class="button button--primary" type="button" data-action="execute-master-plan">Build masters</button>
              <button class="button button--danger" type="button" data-action="cancel-master-plan" hidden>Cancel build</button>
            </div>
          </div>

          <div class="calibration-layout">
            <aside class="calibration-console" aria-labelledby="matching-heading">
              <div class="panel-heading panel-heading--compact">
                <div>
                  <p class="eyebrow">Matching controls</p>
                  <h3 id="matching-heading">Flat pedestal</h3>
                </div>
                <span class="hardware-light" aria-hidden="true"></span>
              </div>
              <label class="control-field">
                <span>Selection policy</span>
                <select class="instrument-select" data-pedestal-policy>
                  <option value="prefer_matched_dark_then_bias">Prefer matched dark, then bias</option>
                  <option value="require_matched_dark">Require matched dark</option>
                  <option value="require_bias">Require true bias</option>
                </select>
              </label>
              <div class="control-pair">
                <label class="control-field">
                  <span>Exposure tolerance</span>
                  <span class="instrument-stepper number-control" data-stepper><button type="button" data-action="step-number" data-step-direction="down" aria-label="Decrease exposure tolerance">−</button><input data-exposure-tolerance type="number" min="0" step="0.01" inputmode="decimal" /><button type="button" data-action="step-number" data-step-direction="up" aria-label="Increase exposure tolerance">+</button><b>s</b></span>
                </label>
                <label class="control-field">
                  <span>Temperature tolerance</span>
                  <span class="instrument-stepper number-control" data-stepper><button type="button" data-action="step-number" data-step-direction="down" aria-label="Decrease temperature tolerance">−</button><input data-temperature-tolerance type="number" min="0" step="0.1" inputmode="decimal" /><button type="button" data-action="step-number" data-step-direction="up" aria-label="Increase temperature tolerance">+</button><b>°C</b></span>
                </label>
              </div>
              <label class="control-field">
                <span>Light ↔ Dark temperature tolerance</span>
                <span class="instrument-stepper number-control" data-stepper><button type="button" data-action="step-number" data-step-direction="down" aria-label="Decrease Light to Dark temperature tolerance">−</button><input data-light-temperature-tolerance type="number" min="0" step="0.1" inputmode="decimal" /><button type="button" data-action="step-number" data-step-direction="up" aria-label="Increase Light to Dark temperature tolerance">+</button><b>°C</b></span>
              </label>
              <p class="control-note">Exact camera, axes, gain, offset, binning and CFA phase are always required. Filter is not used to match darks or biases.</p>
              <details class="advanced-settings">
                <summary>Advanced matching evidence</summary>
                <p>Every compatible and rejected candidate is retained below with stable machine-readable reasons.</p>
              </details>
              <section class="master-execution" data-master-execution data-state="idle" aria-labelledby="master-execution-heading">
                <div>
                  <p class="eyebrow">Transactional build</p>
                  <h4 id="master-execution-heading">Native execution</h4>
                </div>
                <p data-master-execution-message></p>
                <progress data-master-execution-progress aria-label="Master build progress" hidden></progress>
                <code data-master-execution-output></code>
              </section>
            </aside>

            <section class="master-rack" aria-labelledby="master-rack-heading">
              <div class="panel-heading">
                <div>
                  <p class="eyebrow">Calibration map</p>
                  <h3 id="master-rack-heading">How your masters are built</h3>
                  <p class="master-rack__guide"><strong>Read left to right.</strong> Each line shows the input used to create one master.</p>
                </div>
                <code class="plan-digest" data-calibration-digest></code>
              </div>
              <div class="master-product-list" data-calibration-products></div>
              <section class="light-association-panel" aria-labelledby="light-association-heading">
                <div class="light-association-panel__heading">
                  <div>
                    <p class="eyebrow">Execution gate</p>
                    <h3 id="light-association-heading">Light calibration matrix</h3>
                  </div>
                  <div class="light-association-panel__actions">
                    <code class="plan-digest" data-light-digest></code>
                    <label class="light-output-mode">
                      <span>Output</span>
                      <select class="instrument-select" data-light-output-mode aria-label="Light output mode">
                        <option value="calibrated_frames">Calibrated frames</option>
                        <option value="integrated">Integrated group</option>
                      </select>
                    </label>
                    <button class="button button--primary" type="button" data-action="execute-light-plan">Calibrate &amp; integrate</button>
                    <button class="button button--danger" type="button" data-action="cancel-light-plan" hidden>Cancel Lights</button>
                  </div>
                </div>
                <div class="light-matrix-scroll" data-light-associations></div>
                <section class="light-execution" data-light-execution data-state="idle" aria-labelledby="light-execution-heading">
                  <div>
                    <p class="eyebrow">Atomic Light run</p>
                    <h4 id="light-execution-heading" data-light-execution-heading>Calibrate + strict integration</h4>
                  </div>
                  <p data-light-execution-message></p>
                  <progress data-light-execution-progress aria-label="Light calibration progress" hidden></progress>
                  <code data-light-execution-output></code>
                </section>
                <section class="defect-console" data-defect-correction data-state="idle" aria-labelledby="defect-correction-heading">
                  <header class="defect-console__heading">
                    <div>
                      <p class="eyebrow">Detector laboratory</p>
                      <h4 id="defect-correction-heading">HOT / COLD correction</h4>
                    </div>
                    <span class="experimental-badge">Advanced · experimental</span>
                  </header>
                  <p class="defect-console__intro">Derive immutable defect evidence from the exact Dark and normalized Flat selected by the Light plan. The corrected Light and reason map publish together or not at all.</p>
                  <div class="defect-console__lattice" role="group" aria-label="Detector sampling lattice">
                    <span>Sampling lattice</span>
                    <button type="button" data-action="select-defect-stride" data-defect-stride="2" aria-pressed="true">CFA phase · 2 px</button>
                    <button type="button" data-action="select-defect-stride" data-defect-stride="1" aria-pressed="false">Mono · 1 px</button>
                  </div>
                  <details class="defect-console__advanced">
                    <summary>Detection and replacement controls</summary>
                    <div class="defect-console__grid">
                      <label><span>Detection radius</span><input data-defect-detection-radius type="number" min="1" max="8" step="1" inputmode="numeric" /></label>
                      <label><span>Minimum neighbours</span><input data-defect-detection-minimum-neighbours type="number" min="1" step="1" inputmode="numeric" /></label>
                      <label><span>Dark HOT σ</span><input data-defect-dark-hot-sigma type="number" min="0.01" step="0.1" inputmode="decimal" /></label>
                      <label><span>Dark COLD σ</span><input data-defect-dark-cold-sigma type="number" min="0.01" step="0.1" inputmode="decimal" /></label>
                      <label><span>Dark residual floor</span><input data-defect-dark-floor type="number" min="0" step="0.01" inputmode="decimal" /></label>
                      <label><span>Flat HOT σ</span><input data-defect-flat-hot-sigma type="number" min="0.01" step="0.1" inputmode="decimal" /></label>
                      <label><span>Flat COLD σ</span><input data-defect-flat-cold-sigma type="number" min="0.01" step="0.1" inputmode="decimal" /></label>
                      <label><span>Flat residual floor</span><input data-defect-flat-floor type="number" min="0" step="0.000001" inputmode="decimal" /></label>
                      <label><span>Repair radius</span><input data-defect-correction-radius type="number" min="1" max="8" step="1" inputmode="numeric" /></label>
                      <label><span>Repair neighbours</span><input data-defect-correction-minimum-neighbours type="number" min="1" step="1" inputmode="numeric" /></label>
                    </div>
                  </details>
                  <section class="linear-defect" data-linear-defect data-state="idle" aria-labelledby="linear-defect-heading">
                    <header class="linear-defect__heading">
                      <div>
                        <p class="eyebrow">Coherent defect oracle</p>
                        <h5 id="linear-defect-heading">Row / column correction</h5>
                      </div>
                      <span class="linear-defect__seal">Atomic pair</span>
                    </header>
                    <p class="linear-defect__intro">Detect only defects supported across a strict fraction of one row or column, then repair perpendicular to the damaged line without mixing CFA phases.</p>
                    <div class="linear-defect__axis" role="group" aria-label="Linear defect direction">
                      <button type="button" data-action="select-linear-defect-axis" data-linear-defect-axis="rows" aria-pressed="true"><span aria-hidden="true">━</span> Rows</button>
                      <button type="button" data-action="select-linear-defect-axis" data-linear-defect-axis="columns" aria-pressed="false"><span aria-hidden="true">┃</span> Columns</button>
                    </div>
                    <div class="linear-defect__grid">
                      <label><span>Detection radius</span><input data-linear-defect-setting="perpendicularRadius" type="number" min="1" max="8" step="1" inputmode="numeric" /></label>
                      <label><span>Detection support</span><input data-linear-defect-setting="minimumPerpendicularNeighbours" type="number" min="1" max="16" step="1" inputmode="numeric" /></label>
                      <label><span>Samples per line</span><input data-linear-defect-setting="minimumAffectedSamples" type="number" min="1" step="1" inputmode="numeric" /></label>
                      <label><span>Required coverage (%)</span><input data-linear-defect-setting="minimumAffectedPercent" type="number" min="0.0001" max="100" step="0.01" inputmode="decimal" /></label>
                      <label><span>HOT threshold (σ)</span><input data-linear-defect-setting="hotSigma" type="number" min="0.01" step="0.1" inputmode="decimal" /></label>
                      <label><span>COLD threshold (σ)</span><input data-linear-defect-setting="coldSigma" type="number" min="0.01" step="0.1" inputmode="decimal" /></label>
                      <label><span>Residual floor</span><input data-linear-defect-setting="minimumAbsoluteDeviation" type="number" min="0" step="0.01" inputmode="decimal" /></label>
                      <label><span>Repair radius</span><input data-linear-defect-setting="correctionRadius" type="number" min="1" max="8" step="1" inputmode="numeric" /></label>
                      <label><span>Repair support</span><input data-linear-defect-setting="correctionMinimumNeighbours" type="number" min="1" max="16" step="1" inputmode="numeric" /></label>
                    </div>
                    <div class="linear-defect__readout" role="status" aria-live="polite">
                      <p data-linear-defect-message>Ready to inspect coherent rows or columns</p>
                      <progress data-linear-defect-progress aria-label="Linear defect correction progress" hidden></progress>
                      <output data-linear-defect-evidence>No coherent-line evidence published</output>
                      <code data-linear-defect-output>Destination selected at run time</code>
                    </div>
                    <div class="linear-defect__actions">
                      <button class="button button--primary" type="button" data-action="execute-linear-defect-correction" disabled>Correct coherent lines</button>
                      <button class="button button--danger" type="button" data-action="cancel-linear-defect-correction" hidden>Cancel linear correction</button>
                    </div>
                  </section>
                  <div class="defect-console__status">
                    <p data-defect-correction-message role="status" aria-live="polite"></p>
                    <progress data-defect-correction-progress aria-label="Detector correction progress" hidden></progress>
                    <output data-defect-correction-evidence>No correction evidence published</output>
                    <output class="defect-console__batch-report" data-defect-batch-report hidden></output>
                    <section class="defect-console__report-inspection" data-defect-report-inspection data-state="idle" role="status" aria-live="polite" aria-atomic="true">Inspect an exported report independently</section>
                    <code data-defect-correction-output></code>
                  </div>
                  <section class="defect-preview" aria-label="Detector correction comparison">
                    <div class="defect-preview__tabs" role="tablist" aria-label="Correction inputs, output, and defect map">
                      <button type="button" role="tab" aria-controls="defect-preview-panel" data-action="select-defect-preview" data-defect-preview="before" aria-selected="false" disabled>Before</button>
                      <button type="button" role="tab" aria-controls="defect-preview-panel" data-action="select-defect-preview" data-defect-preview="after" aria-selected="true" disabled>After</button>
                      <button type="button" role="tab" aria-controls="defect-preview-panel" data-action="select-defect-preview" data-defect-preview="map" aria-selected="false" disabled>Map</button>
                    </div>
                    <div class="defect-preview__legend" aria-label="Defect map color legend">
                      <span data-kind="hot"><i aria-hidden="true"></i>HOT</span>
                      <span data-kind="cold"><i aria-hidden="true"></i>COLD</span>
                      <span data-kind="conflict"><i aria-hidden="true"></i>CONFLICT</span>
                    </div>
                    <div class="defect-preview__stage" id="defect-preview-panel" role="tabpanel" tabindex="0">
                      <img data-defect-preview-image alt="" hidden />
                      <div data-defect-preview-placeholder>Publish one correction to unlock the shared before / after view</div>
                    </div>
                    <p data-defect-preview-message>Publish one correction to compare native FITS pixels</p>
                  </section>
                  <div class="defect-console__actions">
                    <button class="button button--primary" type="button" data-action="execute-defect-correction" disabled>Correct selected Light</button>
                    <button class="button" type="button" data-action="execute-all-defect-corrections" disabled>Correct all 0 eligible Lights</button>
                    <button class="button" type="button" data-action="export-defect-batch-report" disabled>Export verified report</button>
                    <button class="button button--primary" type="button" data-action="resume-defect-batch" hidden>Resume remaining Lights</button>
                    <button class="button" type="button" data-action="inspect-defect-batch-report">Inspect report</button>
                    <button class="button button--danger" type="button" data-action="cancel-defect-correction" hidden>Cancel correction</button>
                  </div>
                </section>
              </section>
            </section>
          </div>
        </section>
        ${workflowOverviewMarkup()}
        ${uiPreferencesMarkup()}
      </main>
    </div>

    <div class="session-drop-overlay" data-session-drop-overlay data-state="ready" role="status" aria-live="polite" aria-atomic="true" hidden>
      <div class="session-drop-overlay__instrument" aria-hidden="true">↓</div>
      <strong data-session-drop-message>Drop one session directory</strong>
      <span>FITS sources stay read-only · native validation starts after drop</span>
    </div>

    <div class="dialog-backdrop" role="presentation" data-diagnostics-dialog hidden>
      <section class="statistics-dialog diagnostics-dialog" role="dialog" aria-modal="true" aria-labelledby="diagnostics-title" aria-describedby="diagnostics-description">
        <div class="statistics-dialog__heading">
          <div>
            <p class="eyebrow">Session integrity</p>
            <h2 id="diagnostics-title">Import diagnostics</h2>
          </div>
          <button class="icon-button" type="button" data-action="close-diagnostics" aria-label="Close import diagnostics">×</button>
        </div>
        <p id="diagnostics-description">FITS validation and optional quality-cache recovery are reported separately. Missing quality evidence is normal and will be measured from the immutable source when requested.</p>
        <p class="diagnostics-state" data-diagnostics-state data-tone="ready"></p>
        <section class="diagnostics-bank" aria-labelledby="diagnostics-import-heading">
          <div class="diagnostics-bank__heading"><span aria-hidden="true">●</span><h3 id="diagnostics-import-heading">FITS import</h3></div>
          <dl class="statistics-grid diagnostics-grid">
            ${statistic("Sources considered", "data-diagnostics-files")}
            ${statistic("Fingerprinted bytes", "data-diagnostics-bytes")}
            ${statistic("Native scan time", "data-diagnostics-scan-time")}
            ${statistic("Import worker limit", "data-diagnostics-workers")}
            ${statistic("Verified frames", "data-diagnostics-frames")}
            ${statistic("Classification conflicts", "data-diagnostics-conflicts")}
            ${statistic("Recoverable failures", "data-diagnostics-failures")}
            ${statistic("Unassigned sources", "data-diagnostics-unassigned")}
          </dl>
        </section>
        <section class="diagnostics-bank" aria-labelledby="diagnostics-cache-heading">
          <div class="diagnostics-bank__heading"><span aria-hidden="true">●</span><h3 id="diagnostics-cache-heading">Verified quality cache</h3></div>
          <dl class="statistics-grid diagnostics-grid">
            ${statistic("Restored", "data-diagnostics-restored")}
            ${statistic("Missing", "data-diagnostics-missing")}
            ${statistic("Rejected", "data-diagnostics-rejected")}
          </dl>
          <p class="diagnostics-note">Rejected entries never enter a selection plan. They remain untouched in shared cache storage and quality is recomputed safely.</p>
          <div class="diagnostics-maintenance">
            <div>
              <strong>Safe maintenance preview</strong>
              <p data-diagnostics-maintenance-status aria-live="polite">Rejected cache artifacts have not been inspected</p>
              <code data-diagnostics-maintenance-facts hidden></code>
            </div>
            <div class="diagnostics-maintenance__actions">
              <button class="button button--quiet" type="button" data-action="preview-cache-maintenance" disabled>Preview cleanup</button>
              <button class="button button--danger" type="button" data-action="open-cache-maintenance-confirmation" hidden>Remove inspected files</button>
            </div>
          </div>
        </section>
        <section class="diagnostics-bank" aria-labelledby="diagnostics-evidence-heading">
          <div class="diagnostics-bank__heading"><span aria-hidden="true">●</span><h3 id="diagnostics-evidence-heading">Issue evidence</h3></div>
          <div class="diagnostics-evidence-toolbar">
            <label class="diagnostics-search">
              <span class="sr-only">Search diagnostic evidence</span>
              <span aria-hidden="true">⌕</span>
              <input type="search" data-diagnostics-search placeholder="Find source or issue code" autocomplete="off" spellcheck="false" />
            </label>
            <div class="diagnostics-filters" role="group" aria-label="Filter diagnostic evidence">
              <button type="button" data-action="filter-diagnostics-evidence" data-diagnostics-filter="all" aria-pressed="true">All</button>
              <button type="button" data-action="filter-diagnostics-evidence" data-diagnostics-filter="classification" aria-pressed="false">Classification</button>
              <button type="button" data-action="filter-diagnostics-evidence" data-diagnostics-filter="fits" aria-pressed="false">FITS</button>
              <button type="button" data-action="filter-diagnostics-evidence" data-diagnostics-filter="grouping" aria-pressed="false">Grouping</button>
              <button type="button" data-action="filter-diagnostics-evidence" data-diagnostics-filter="quality_cache" aria-pressed="false">Quality cache</button>
            </div>
          </div>
          <output class="diagnostics-evidence-summary" data-diagnostics-evidence-summary aria-live="polite">0 of 0 displayed issues</output>
          <p class="diagnostics-empty" data-diagnostics-empty>No source-level issue evidence is present.</p>
          <ol class="diagnostics-items" data-diagnostics-items hidden></ol>
          <p class="diagnostics-note" data-diagnostics-omitted hidden></p>
        </section>
        <footer class="diagnostics-export">
          <div class="diagnostics-export__status">
            <p data-diagnostics-export-status aria-live="polite">No redacted report exported</p>
            <p data-diagnostics-inspection-status aria-live="polite">No diagnostics report verified</p>
          </div>
          <div class="diagnostics-export__actions">
            <button class="button button--quiet" type="button" data-action="inspect-diagnostics-report">Verify report</button>
            <button class="button button--primary" type="button" data-action="export-diagnostics">Export redacted JSON</button>
          </div>
        </footer>
      </section>
    </div>

    <div class="dialog-backdrop" role="presentation" data-cache-maintenance-dialog hidden>
      <section class="reason-dialog maintenance-confirmation" role="dialog" aria-modal="true" aria-labelledby="cache-maintenance-title" aria-describedby="cache-maintenance-description">
        <p class="eyebrow">Confirmed cache maintenance</p>
        <h2 id="cache-maintenance-title">Remove only the inspected artifacts?</h2>
        <p id="cache-maintenance-description">These rejected cache files are disposable evidence copies. Original FITS files are never targeted, and every cache file is revalidated against the sealed preview before removal.</p>
        <strong data-cache-maintenance-summary></strong>
        <code data-cache-maintenance-seal></code>
        <div class="maintenance-confirmation__actions">
          <button class="button button--quiet" type="button" data-action="cancel-cache-maintenance">Keep files</button>
          <button class="button button--danger" type="button" data-action="confirm-cache-maintenance">Remove inspected files</button>
        </div>
      </section>
    </div>

    <div class="dialog-backdrop" role="presentation" data-reject-dialog hidden>
      <section class="reason-dialog" role="dialog" aria-modal="true" aria-labelledby="reject-title" aria-describedby="reject-description">
        <p class="eyebrow">Manual review</p>
        <h2 id="reject-title">Why reject this frame?</h2>
        <p id="reject-description">The reason is stored with <strong data-reject-frame></strong> and remains visible in the run plan.</p>
        <div class="reason-grid">${reasonButtons}</div>
        <button class="button button--quiet dialog-cancel" type="button" data-action="cancel-reject">Cancel</button>
      </section>
    </div>
    <div class="dialog-backdrop" role="presentation" data-selection-confirmation hidden>
      <section class="reason-dialog selection-confirmation" role="dialog" aria-modal="true" aria-labelledby="selection-confirmation-title" aria-describedby="selection-confirmation-description">
        <p class="eyebrow">Confirmed batch decision</p>
        <h2 id="selection-confirmation-title">Apply the native recommendations?</h2>
        <p id="selection-confirmation-description"><strong data-selection-confirmation-count>0</strong> undecided Lights will receive explicit decisions. Existing manual decisions stay untouched, and one Undo restores the complete batch.</p>
        <div class="selection-confirmation__actions">
          <button class="button button--quiet" type="button" data-action="cancel-frame-selection">Cancel</button>
          <button class="button button--primary" type="button" data-action="confirm-frame-selection">Apply reviewed plan</button>
        </div>
      </section>
    </div>
    <div class="dialog-backdrop" role="presentation" data-statistics-dialog hidden>
      <section class="statistics-dialog" role="dialog" aria-modal="true" aria-labelledby="statistics-title" aria-describedby="statistics-description">
        <div class="statistics-dialog__heading">
          <div>
            <p class="eyebrow">Exact FITS statistics</p>
            <h2 id="statistics-title" data-statistics-frame></h2>
          </div>
          <button class="icon-button" type="button" data-action="close-statistics" aria-label="Close FITS statistics">×</button>
        </div>
        <p id="statistics-description">The complete primary array is decoded in canonical order with fixed-size buffers. These moments are diagnostic and never alter scientific pixels.</p>
        <p class="statistics-status" data-statistics-status></p>
        <dl class="statistics-grid" data-statistics-content hidden>
          ${statistic("Algorithm", "data-statistics-algorithm")}
          ${statistic("Axes", "data-statistics-axes")}
          ${statistic("Stored format", "data-statistics-format")}
          ${statistic("Header", "data-statistics-header")}
          ${statistic("Usable samples", "data-statistics-usable")}
          ${statistic("Excluded samples", "data-statistics-excluded")}
          ${statistic("Minimum", "data-statistics-minimum")}
          ${statistic("Maximum", "data-statistics-maximum")}
          ${statistic("Arithmetic mean", "data-statistics-mean")}
          ${statistic("Population σ", "data-statistics-deviation")}
          ${statistic("Sample σ", "data-statistics-sample-deviation")}
        </dl>
      </section>
    </div>
    <p class="sr-only" aria-live="polite" aria-atomic="true" data-live-region></p>
  `;
}

function navigationItem(id: string, label: string, icon: string): string {
  return `<button class="nav-item" type="button" data-action="select-workspace" data-workspace="${id}">
    <span class="nav-item__icon" aria-hidden="true">${icon}</span><span>${label}</span>
  </button>`;
}

function sortableHeading(
  label: string,
  field: SortField,
  numeric = false,
): string {
  return `<th scope="col"${numeric ? ' class="numeric"' : ""}>
    <button class="sort-button" type="button" data-action="sort" data-sort-field="${field}" aria-label="Sort ${label} ascending">${label}<span aria-hidden="true">↕</span></button>
  </th>`;
}

function metric(label: string, attribute: string, unit: string): string {
  return `<div><dt>${label}</dt><dd><span ${attribute}>—</span>${unit ? ` <small>${unit}</small>` : ""}</dd></div>`;
}

function registrationMetric(
  label: string,
  attribute: string,
  unit: string,
): string {
  return `<div><dt>${label}</dt><dd><strong ${attribute}>—</strong><small>${unit}</small></dd></div>`;
}

function statistic(label: string, attribute: string): string {
  return `<div><dt>${label}</dt><dd ${attribute}>—</dd></div>`;
}
