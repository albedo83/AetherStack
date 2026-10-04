import type {
  LightExecutionProgress,
  LightExecutionResult,
  LightExecutionSettings,
  MasterBuildSettings,
  MasterExecutionProgress,
  MasterExecutionResult,
  MasterPlanPreview,
  MasterPlanSettings,
} from "./calibration-bridge.ts";
import type {
  RegistrationDiagnostic,
  RegistrationExecutionProgress,
  RegistrationExecutionResult,
  RegistrationPlanPreview,
  RegisteredStackIntegrationSettings,
  RegisteredStackProgress,
  RegisteredStackReportInspection,
  RegisteredStackResult,
  RegisteredStackSourceVerificationResult,
  RegisteredStackSourceVerificationProgress,
  RegisteredWeightPreflight,
} from "./registration-bridge.ts";
import type {
  FrameSelectionPlan,
  FrameSelectionRule,
} from "./selection-bridge.ts";

export type FrameRole = "bias" | "dark" | "flat" | "light";

export type WorkspaceView = "frames" | "calibration" | "registration";

export type LightFrameView = "raw" | "calibrated";

export type RegisteredStackProductView =
  "science" | "rejection_low" | "rejection_high";

export interface RejectionHistogramBin {
  readonly rejectedCount: number;
  readonly samples: number;
}

export interface RejectionHistogram {
  readonly algorithmId: string;
  readonly totalSamples: number;
  readonly zeroSamples: number;
  readonly rejectedSamples: number;
  readonly maximumRejectedCount: number;
  readonly bins: readonly RejectionHistogramBin[];
}

export interface StackPixelInspection {
  readonly x: number;
  readonly y: number;
  readonly scienceValues: readonly (number | null)[];
  readonly lowRejectionCounts: readonly (number | null)[] | null;
  readonly highRejectionCounts: readonly (number | null)[] | null;
}

export type ReviewState = "undecided" | "accepted" | "rejected";

export type ReviewRejectionReason =
  | "blur"
  | "trailing"
  | "cloud"
  | "intrusive_trail"
  | "gradient"
  | "framing"
  | "saturation"
  | "quality_rules";

export type MetricValue = number | null;

export type BayerPattern = "rggb" | "bggr" | "grbg" | "gbrg";

export type QualityState =
  "unavailable" | "idle" | "loading" | "ready" | "error";

export interface FrameMetrics {
  readonly signalToNoise: MetricValue;
  readonly fwhmPixels: MetricValue;
  readonly eccentricity: MetricValue;
  readonly detectedStars: number | null;
  readonly usableStars: number | null;
  readonly background: MetricValue;
  readonly noise: MetricValue;
}

export interface ReviewFrame {
  readonly id: string;
  readonly label: string;
  /** Absolute runtime-only source path; never persisted in portable state. */
  readonly sourcePath: string | null;
  /** Exact FITS primary-array interpretation used by the native viewer. */
  readonly previewContent:
    | { readonly kind: "scalar"; readonly plane: number }
    | { readonly kind: "rgb" };
  readonly exposureSeconds: number | null;
  readonly temperatureCelsius: number | null;
  readonly classificationWarning: string | null;
  readonly bayerPattern: BayerPattern | null;
  readonly qualityState: QualityState;
  readonly qualityMessage: string;
  readonly qualityProfileId: string | null;
  readonly qualityOrigin: "measured" | "restored" | null;
  readonly state: ReviewState;
  readonly rejectionReason: ReviewRejectionReason | null;
  readonly metrics: FrameMetrics;
}

export interface SessionStatus {
  readonly tone: "ready" | "busy" | "warning" | "error";
  readonly label: string;
}

export interface SessionDiagnostics {
  readonly filesConsidered: number;
  readonly fingerprintedSourceBytes: number;
  readonly scanElapsedMilliseconds: number;
  readonly sourceAnalysisParallelism: number;
  readonly verifiedFrames: number;
  readonly classificationConflicts: number;
  readonly recoverableFailures: number;
  readonly unassignedSources: number;
  readonly qualityEvidenceRestored: number;
  readonly qualityEvidenceMissing: number;
  readonly qualityEvidenceRejected: number;
  readonly items: readonly SessionDiagnosticItem[];
  readonly omittedItems: number;
  readonly exportState: "idle" | "exporting" | "ready" | "error";
  readonly exportMessage: string;
  readonly inspectionState: "idle" | "inspecting" | "ready" | "error";
  readonly inspectionMessage: string;
  readonly maintenanceState:
    "idle" | "inspecting" | "ready" | "applying" | "error";
  readonly maintenanceMessage: string;
  readonly maintenanceEligible: number;
  readonly maintenanceBlocked: number;
  readonly maintenanceBytes: number;
  readonly maintenancePlanSha256: string | null;
}

export interface SessionDiagnosticItem {
  readonly category: "classification" | "fits" | "grouping" | "quality_cache";
  readonly source: string;
  readonly code: string;
}

export interface RoleSummary {
  readonly role: FrameRole;
  readonly label: string;
  readonly count: number;
  readonly unresolved: number;
}

export interface FramePreview {
  /** Stable frame identity that the decoded pixels were requested for. */
  readonly frameId: string;
  /** Browser object URL for a display-only artifact produced by Rust. */
  readonly url: string;
}

/** Exact three-pass moments for the complete primary FITS array. */
export interface FitsStatistics {
  readonly algorithmId: string;
  readonly axes: readonly number[];
  readonly storedFormat: string;
  readonly headerConformant: boolean;
  readonly headerDiagnostics: number;
  readonly totalSamples: number;
  readonly usableSamples: number;
  readonly undefinedSamples: number;
  readonly nonFiniteSamples: number;
  readonly minimum: number;
  readonly maximum: number;
  readonly mean: number;
  readonly populationStandardDeviation: number;
  readonly sampleStandardDeviation: number | null;
}

export interface StatisticsPanel {
  readonly open: boolean;
  readonly frameId: string | null;
  readonly frameLabel: string | null;
  readonly state: "idle" | "loading" | "ready" | "error";
  readonly statistics: FitsStatistics | null;
  readonly message: string | null;
}

export interface QualityBatchProgress {
  readonly completed: number;
  readonly total: number;
}

export interface FrameSelectionViewModel {
  readonly state: "idle" | "previewing" | "ready" | "error";
  readonly rules: readonly FrameSelectionRule[];
  readonly plan: FrameSelectionPlan | null;
  readonly message: string;
}

export interface ReviewViewModel {
  readonly activeWorkspace: WorkspaceView;
  readonly sessionName: string;
  readonly sessionStatus: SessionStatus;
  readonly sessionDiagnostics: SessionDiagnostics;
  readonly roles: readonly RoleSummary[];
  readonly activeRole: FrameRole;
  /** Pixel stage currently shown for Lights; review identities stay stable. */
  readonly lightFrameView: LightFrameView;
  readonly frames: readonly ReviewFrame[];
  readonly selectedFrameId: string | null;
  readonly playing: boolean;
  readonly reviewSessionReady: boolean;
  readonly canUndo: boolean;
  readonly decisionPending: boolean;
  readonly qualityBatchRunning: boolean;
  readonly qualityBatchProgress: QualityBatchProgress | null;
  readonly frameSelection: FrameSelectionViewModel;
  readonly sharedStretchLabel: string;
  readonly preview: FramePreview | null;
  readonly viewerScale: "fit" | "actual";
  readonly statisticsPanel: StatisticsPanel;
  readonly calibration: CalibrationViewModel;
  readonly registration: RegistrationViewModel;
}

export interface RegistrationFrameOption {
  readonly id: string;
  readonly label: string;
  readonly sourcePath: string | null;
}

/** One published registered artifact available for result Blink review. */
export interface RegisteredReviewFrame {
  readonly id: string;
  readonly label: string;
  readonly outputPath: string;
  readonly previewContent: ReviewFrame["previewContent"];
}

export interface RegistrationResultReview {
  readonly frames: readonly RegisteredReviewFrame[];
  readonly selectedFrameId: string | null;
  readonly state: "idle" | "loading" | "ready" | "error";
  readonly preview: FramePreview | null;
  readonly playing: boolean;
  readonly message: string;
  readonly sharedStretchLabel: string;
}

export interface AcceptedRegistrationSolution {
  readonly sourceFrameId: string;
  readonly diagnostic: RegistrationDiagnostic;
}

/** UI state for reviewed pair evidence that will form one multi-Light plan. */
export interface RegistrationViewModel {
  readonly state: "idle" | "running" | "accepted" | "rejected" | "error";
  readonly frames: readonly RegistrationFrameOption[];
  readonly referenceFrameId: string | null;
  readonly sourceFrameId: string | null;
  readonly diagnostic: RegistrationDiagnostic | null;
  readonly solutions: readonly AcceptedRegistrationSolution[];
  readonly planState: "idle" | "building" | "ready" | "error";
  readonly plan: RegistrationPlanPreview | null;
  readonly execution: {
    readonly state: "idle" | "running" | "cancelling" | "completed" | "error";
    readonly outputDirectory: string | null;
    readonly progress: RegistrationExecutionProgress | null;
    readonly result: RegistrationExecutionResult | null;
    readonly message: string;
  };
  readonly stack: {
    readonly state: "idle" | "running" | "cancelling" | "completed" | "error";
    readonly outputPath: string | null;
    readonly progress: RegisteredStackProgress | null;
    readonly result: RegisteredStackResult | null;
    readonly previewState: "idle" | "loading" | "ready" | "error";
    readonly preview: FramePreview | null;
    readonly sciencePreview: FramePreview | null;
    readonly selectedProduct: RegisteredStackProductView;
    readonly overlayOpacity: number;
    readonly histogramState: "idle" | "loading" | "ready" | "error";
    readonly histogram: RejectionHistogram | null;
    readonly pixelInspectionState: "idle" | "loading" | "ready" | "error";
    readonly pixelInspection: StackPixelInspection | null;
    /** Last native weight seal, retained only while its plan and settings remain current. */
    readonly weightPreflight: RegisteredWeightPreflight | null;
    readonly reportInspectionState: "idle" | "loading" | "ready" | "error";
    readonly reportInspection: RegisteredStackReportInspection | null;
    readonly reportInspectionPath: string | null;
    readonly sourceVerificationState:
      "idle" | "loading" | "cancelling" | "ready" | "error";
    readonly sourceVerification: RegisteredStackSourceVerificationResult | null;
    readonly sourceVerificationProgress: RegisteredStackSourceVerificationProgress | null;
    readonly settings: RegisteredStackIntegrationSettings;
    readonly message: string;
  };
  readonly resultReview: RegistrationResultReview;
  readonly message: string;
}

export interface CalibrationViewModel {
  readonly state: "idle" | "loading" | "ready" | "error";
  readonly settings: MasterPlanSettings;
  readonly plan: MasterPlanPreview | null;
  readonly message: string;
  readonly buildSettings: MasterBuildSettings;
  readonly execution: CalibrationExecutionViewModel;
  readonly lightSettings: LightExecutionSettings;
  readonly lightExecution: LightCalibrationExecutionViewModel;
}

export interface CalibrationExecutionViewModel {
  readonly state: "idle" | "running" | "cancelling" | "completed" | "error";
  readonly outputDirectory: string | null;
  readonly progress: MasterExecutionProgress | null;
  readonly result: MasterExecutionResult | null;
  readonly message: string;
}

export interface LightCalibrationExecutionViewModel {
  readonly state: "idle" | "running" | "cancelling" | "completed" | "error";
  readonly masterDirectory: string | null;
  readonly outputDirectory: string | null;
  readonly progress: LightExecutionProgress | null;
  readonly result: LightExecutionResult | null;
  readonly message: string;
}

export type SortField =
  | "processing_order"
  | "label"
  | "fwhm_major"
  | "eccentricity"
  | "detected_stars"
  | "background";

export type SortDirection = "ascending" | "descending";

export interface ReviewActions {
  readonly onSelectWorkspace: (workspace: WorkspaceView) => void;
  readonly onSelectRegistrationReference: (frameId: string) => void;
  readonly onSelectRegistrationSource: (frameId: string) => void;
  readonly onAnalyzeRegistration: () => void;
  readonly onExecuteRegistration: () => void;
  readonly onCancelRegistration: () => void;
  readonly onExecuteRegisteredStack: () => void;
  readonly onCancelRegisteredStack: () => void;
  readonly onUpdateRegisteredStackSettings: (
    settings: RegisteredStackIntegrationSettings,
  ) => void;
  readonly onSelectRegisteredStackProduct: (
    product: RegisteredStackProductView,
  ) => void;
  readonly onSetRegisteredStackOverlayOpacity: (opacity: number) => void;
  readonly onInspectRegisteredStackPixel: (x: number, y: number) => void;
  readonly onInspectRegisteredStackReport: () => void;
  readonly onOpenRegisteredStackReport: () => void;
  readonly onReturnToActiveStack: () => void;
  readonly onVerifyRegisteredStackSources: () => void;
  readonly onCancelRegisteredStackSourceVerification: () => void;
  readonly onSelectRegisteredFrame: (frameId: string) => void;
  readonly onSetRegisteredPlaying: (playing: boolean) => void;
  readonly onStepRegisteredFrame: (direction: "backward" | "forward") => void;
  readonly onUpdateCalibrationSettings: (settings: MasterPlanSettings) => void;
  readonly onUpdateLightOutputMode: (
    mode: LightExecutionSettings["outputMode"],
  ) => void;
  readonly onRefreshMasterPlan: () => void;
  readonly onExecuteMasterPlan: () => void;
  readonly onCancelMasterPlan: () => void;
  readonly onExecuteLightPlan: () => void;
  readonly onCancelLightPlan: () => void;
  readonly onImportSession: () => void;
  readonly onExportDiagnostics: () => void;
  readonly onInspectDiagnosticsReport: () => void;
  readonly onPreviewQualityCacheMaintenance: () => void;
  readonly onApplyQualityCacheMaintenance: () => void;
  readonly onSelectRole: (role: FrameRole) => void;
  readonly onSelectLightFrameView: (view: LightFrameView) => void;
  readonly onSelectFrame: (frameId: string) => void;
  readonly onSort: (field: SortField, direction: SortDirection) => void;
  readonly onSetDecision: (
    frameId: string,
    state: Exclude<ReviewState, "undecided">,
    reason: ReviewRejectionReason | null,
  ) => void;
  readonly onClearDecision: (frameId: string) => void;
  readonly onUndo: () => void;
  readonly onSetPlaying: (playing: boolean) => void;
  readonly onRequestStep: (direction: "backward" | "forward") => void;
  readonly onSetViewerScale: (scale: "fit" | "actual") => void;
  readonly onOpenStatistics: (frameId: string) => void;
  readonly onCloseStatistics: () => void;
  readonly onMeasureQuality: (frameId: string) => void;
  readonly onMeasureAllQuality: () => void;
  readonly onUpdateFrameSelectionRules: (
    rules: readonly FrameSelectionRule[],
  ) => void;
  readonly onPreviewFrameSelection: () => void;
  readonly onApplyFrameSelection: () => void;
}
