import { Channel, invoke } from "@tauri-apps/api/core";
import { open, save } from "@tauri-apps/plugin-dialog";

export type FlatPedestalPolicy =
  "require_matched_dark" | "require_bias" | "prefer_matched_dark_then_bias";

export interface MasterPlanSettings {
  readonly flatPedestalPolicy: FlatPedestalPolicy;
  readonly maximumExposureDeltaSeconds: number;
  readonly maximumTemperatureDeltaC: number;
  readonly maximumLightDarkTemperatureDeltaC: number;
}

export type MasterProductKind = "bias" | "dark" | "flat";
export type PedestalStatus =
  "not_applicable" | "matched_dark" | "bias" | "unresolved";

export interface MasterBinning {
  readonly x: number;
  readonly y: number;
}

export interface MasterPedestal {
  readonly status: PedestalStatus;
  readonly selectedGroupId: string | null;
  readonly exposureDeltaSeconds: number | null;
  readonly temperatureBasis: "sensor" | "set_point" | null;
  readonly temperatureDeltaCelsius: number | null;
  readonly blockingReason: string | null;
  readonly ambiguousGroupIds: readonly string[];
}

export interface MasterPedestalMismatch {
  readonly field: string;
  readonly reason: "missing" | "different" | "outside_tolerance";
}

export interface MasterPedestalCandidate {
  readonly groupId: string;
  readonly sourceKind: "dark" | "bias";
  readonly status: "compatible" | "rejected";
  readonly exposureDeltaSeconds: number | null;
  readonly temperatureBasis: "sensor" | "set_point" | null;
  readonly temperatureDeltaCelsius: number | null;
  readonly mismatches: readonly MasterPedestalMismatch[];
}

export interface MasterProductPlan {
  readonly groupId: string;
  readonly kind: MasterProductKind;
  readonly frameCount: number;
  readonly camera: string | null;
  readonly axes: readonly number[];
  readonly exposureSeconds: number | null;
  readonly sensorTemperatureCelsius: number | null;
  readonly setTemperatureCelsius: number | null;
  readonly gain: number | null;
  readonly offset: number | null;
  readonly binning: MasterBinning | null;
  readonly filter: string | null;
  readonly bayerPattern: string | null;
  readonly pedestal: MasterPedestal;
  readonly candidates: readonly MasterPedestalCandidate[];
}

export interface MasterPlanPreview {
  readonly schemaVersion: number;
  readonly manifestSha256: string;
  readonly planSha256: string;
  readonly ready: boolean;
  readonly products: readonly MasterProductPlan[];
  readonly lightPlan: LightCalibrationPlan | null;
}

export type LightMasterKind = "dark" | "flat";

export interface LightMasterAssociation {
  readonly kind: LightMasterKind;
  readonly status: "matched" | "unresolved";
  readonly selectedGroupId: string | null;
  readonly temperatureBasis: "sensor" | "set_point" | null;
  readonly temperatureDeltaCelsius: number | null;
  readonly blockingReason:
    | "missing_light_metadata"
    | "no_compatible_candidate"
    | "ambiguous_candidates"
    | null;
  readonly ambiguousGroupIds: readonly string[];
  readonly missingFields: readonly string[];
}

export interface LightMasterCandidate {
  readonly groupId: string;
  readonly kind: LightMasterKind;
  readonly status: "compatible" | "rejected";
  readonly temperatureBasis: "sensor" | "set_point" | null;
  readonly temperatureDeltaCelsius: number | null;
  readonly mismatches: readonly MasterPedestalMismatch[];
}

export interface LightCalibrationProduct {
  readonly groupId: string;
  readonly dark: LightMasterAssociation;
  readonly flat: LightMasterAssociation;
  readonly candidates: readonly LightMasterCandidate[];
}

export interface LightCalibrationPlan {
  readonly schemaVersion: number;
  readonly planSha256: string;
  readonly ready: boolean;
  readonly products: readonly LightCalibrationProduct[];
}

export interface MasterBuildSettings {
  readonly minimumFlatNormalizationSamples: number;
  readonly minimumPositiveFlatMedian: number;
  readonly tileWidth: number;
  readonly tileHeight: number;
  readonly memoryLimitBytes: number;
}

export interface MasterExecutionProgress {
  readonly productIndex: number;
  readonly productCount: number;
  readonly groupId: string;
  readonly kind: MasterProductKind;
  readonly sequence: number;
  readonly stage: string;
  readonly state: "started" | "running" | "completed" | "cancelled" | "failed";
  readonly completedUnits: number;
  readonly totalUnits: number | null;
  readonly code: string | null;
}

export interface ExecutedMasterProduct {
  readonly groupId: string;
  readonly kind: MasterProductKind;
  readonly outputPath: string;
  readonly totalSamples: number;
  readonly usableSamples: number;
  readonly maskedSamples: number;
  readonly nonFiniteSamples: number;
  readonly minimum: number;
  readonly maximum: number;
  readonly mean: number;
  readonly populationStandardDeviation: number;
  readonly samplesWritten: number;
  readonly substitutedSamples: number;
  readonly bytesWritten: number;
  readonly normalization: number | null;
}

export interface MasterExecutionResult {
  readonly manifestSha256: string;
  readonly planSha256: string;
  readonly memoryLimitBytes: number;
  readonly peakReservedBytes: number;
  readonly products: readonly ExecutedMasterProduct[];
}

export interface LightExecutionSettings {
  readonly outputMode: "calibrated_frames" | "integrated";
  readonly minimumAbsoluteFlat: number;
  readonly tileWidth: number;
  readonly tileHeight: number;
  readonly memoryLimitBytes: number;
}

export interface LightExecutionProgress {
  readonly productIndex: number;
  readonly productCount: number;
  readonly groupId: string;
  readonly sequence: number;
  readonly stage: string;
  readonly state: "started" | "running" | "completed" | "cancelled" | "failed";
  readonly completedUnits: number;
  readonly totalUnits: number | null;
  readonly code: string | null;
  readonly sourceIndex: number | null;
  readonly sourceCount: number | null;
}

export interface ExecutedLightProduct {
  readonly groupId: string;
  readonly darkGroupId: string;
  readonly flatGroupId: string;
  readonly outputPath: string;
  readonly totalSamples: number;
  readonly usableSamples: number;
  readonly maskedSamples: number;
  readonly nonFiniteSamples: number;
  readonly minimum: number;
  readonly maximum: number;
  readonly mean: number;
  readonly populationStandardDeviation: number;
  readonly samplesWritten: number;
  readonly substitutedSamples: number;
  readonly bytesWritten: number;
  readonly tilesProcessed: number;
  readonly tilesReused: number;
}

export interface LightExecutionResult {
  readonly manifestSha256: string;
  readonly masterPlanSha256: string;
  readonly lightPlanSha256: string;
  readonly memoryLimitBytes: number;
  readonly peakReservedBytes: number;
  readonly outputMode: "calibrated_frames" | "integrated";
  readonly products: readonly ExecutedLightProduct[];
  readonly calibratedFrames: readonly ExecutedCalibratedLightFrame[];
}

export interface ExecutedCalibratedLightFrame {
  readonly groupId: string;
  readonly sourceIndex: number;
  /** Stable identity of the exact raw source in the native review book. */
  readonly sourceFrameId: string;
  readonly sourceLabel: string;
  readonly sourceSha256: string;
  readonly outputPath: string;
  /** Planar linear RGB product when the reviewed Light set is standard CFA. */
  readonly rgbOutputPath: string | null;
  readonly totalSamples: number;
  readonly usableSamples: number;
  readonly maskedSamples: number;
  readonly nonFiniteSamples: number;
  readonly minimum: number;
  readonly maximum: number;
  readonly mean: number;
  readonly populationStandardDeviation: number;
  readonly samplesWritten: number;
  readonly substitutedSamples: number;
  readonly bytesWritten: number;
  readonly tilesProcessed: number;
  readonly tilesReused: number;
}

export interface DefectDetectionSettings {
  readonly radius: number;
  readonly stride: 1 | 2;
  readonly minimumNeighbours: number;
  readonly hotSigma: number;
  readonly coldSigma: number;
  readonly minimumAbsoluteDeviation: number;
}

export interface DefectCorrectionSettings {
  readonly darkDetection: DefectDetectionSettings;
  readonly flatDetection: DefectDetectionSettings;
  readonly correctionRadius: number;
  readonly correctionStride: 1 | 2;
  readonly correctionMinimumNeighbours: number;
  readonly memoryLimitBytes: number;
}

/** One immutable native-owned destination in a reviewed correction batch. */
export interface DefectBatchPreviewItem {
  readonly sourceFrameId: string;
  readonly groupId: string;
  readonly sourceIndex: number;
  readonly correctedOutputPath: string;
  readonly mapOutputPath: string;
  readonly blockedByExistingOutput: boolean;
}

/** Sealed correction order returned before any expensive processing starts. */
export interface DefectBatchPreview {
  readonly ready: boolean;
  readonly planSha256: string;
  readonly parametersSha256: string;
  readonly itemCount: number;
  readonly blockedItemCount: number;
  readonly items: readonly DefectBatchPreviewItem[];
}

export interface DefectCorrectionResult {
  readonly correctedOutputPath: string;
  readonly mapOutputPath: string;
  readonly parametersSha256: string;
  readonly batchPlanSha256: string;
  readonly batchItemIndex: number;
  readonly batchCompletedItems: number;
  readonly batchTotalItems: number;
  readonly batchComplete: boolean;
  readonly reservedBytes: number;
  readonly requestedSamples: number;
  readonly correctedSamples: number;
  readonly insufficientSupportSamples: number;
  readonly blockedBySourceMaskSamples: number;
  readonly correctedSamplesWritten: number;
  readonly correctedSubstitutedSamples: number;
  readonly correctedBytesWritten: number;
  readonly mapSamplesWritten: number;
  readonly mapSubstitutedSamples: number;
  readonly mapBytesWritten: number;
  readonly darkDetection: DefectDetectionEvidence;
  readonly flatDetection: DefectDetectionEvidence;
  readonly mapSummary: DefectMapSummary;
}

export interface DefectBatchReportExport {
  readonly path: string;
  readonly reportSha256: string;
  readonly itemCount: number;
}

export interface DefectBatchReportInspection {
  readonly schemaVersion: number;
  readonly algorithmId: string;
  readonly reportSha256: string;
  readonly planSha256: string;
  readonly parametersSha256: string;
  readonly completedItems: number;
  readonly totalItems: number;
  readonly correctedSamples: number;
  readonly requestedSamples: number;
  readonly conflictingSamples: number;
  readonly defectiveSamples: number;
  readonly peakReservedBytes: number;
  readonly unresolvedSamples: number;
  readonly repairEfficiencyPpm: number | null;
}

export interface ActiveDefectBatch {
  readonly planSha256: string;
  readonly parametersSha256: string;
  readonly nextItemIndex: number;
  readonly totalItems: number;
  readonly complete: boolean;
  readonly requestedSamples: number;
  readonly correctedSamples: number;
  readonly insufficientSupportSamples: number;
  readonly blockedBySourceMaskSamples: number;
  readonly hotSamples: number;
  readonly coldSamples: number;
  readonly defectiveSamples: number;
  readonly conflictingSamples: number;
  readonly peakReservedBytes: number;
}

export interface DefectExecutionProgress {
  readonly sequence: number;
  readonly stage: string;
  readonly state: "started" | "running" | "completed" | "cancelled" | "failed";
  readonly completedUnits: number;
  readonly totalUnits: number | null;
  readonly code: string | null;
}

export interface DefectDetectionEvidence {
  readonly examinedSamples: number;
  readonly insufficientSupportSamples: number;
  readonly unavailableCentreSamples: number;
  readonly hotSamples: number;
  readonly coldSamples: number;
}

export interface DefectMapSummary {
  readonly defectiveSamples: number;
  readonly hotSamples: number;
  readonly coldSamples: number;
  readonly conflictingSamples: number;
}

/**
 * Asks the native planner to derive calibration products from the immutable
 * imported manifest. No source paths or browser-reconstructed groups cross the
 * command boundary.
 */
export function previewMasterPlan(
  settings: MasterPlanSettings,
): Promise<MasterPlanPreview> {
  return invoke<MasterPlanPreview>("preview_master_plan", {
    request: settings,
  });
}

/** Opens a native directory chooser without exposing filesystem enumeration to the webview. */
export async function selectMasterOutputDirectory(): Promise<string | null> {
  const path = await open({
    directory: true,
    multiple: false,
    title: "Select a directory for calibration masters",
  });
  return typeof path === "string" ? path : null;
}

/** Executes the exact native plan while streaming bounded progress snapshots. */
export function executeMasterPlan(
  outputDirectory: string,
  planning: MasterPlanSettings,
  build: MasterBuildSettings,
  reviewedPlan: Pick<MasterPlanPreview, "manifestSha256" | "planSha256">,
  onProgress: (progress: MasterExecutionProgress) => void,
): Promise<MasterExecutionResult> {
  const progress = new Channel<MasterExecutionProgress>();
  progress.onmessage = onProgress;
  return invoke<MasterExecutionResult>("execute_master_plan", {
    request: {
      outputDirectory,
      planning,
      expectedManifestSha256: reviewedPlan.manifestSha256,
      expectedPlanSha256: reviewedPlan.planSha256,
      ...build,
    },
    onProgress: progress,
  });
}

/** Requests cooperative cancellation of the single active native master task. */
export function cancelMasterPlan(): Promise<boolean> {
  return invoke<boolean>("cancel_master_plan");
}

/** Opens a native directory chooser for the calibrated integrated Light products. */
export async function selectLightOutputDirectory(): Promise<string | null> {
  const path = await open({
    directory: true,
    multiple: false,
    title: "Select a directory for calibrated Light products",
  });
  return typeof path === "string" ? path : null;
}

/** Executes all reviewed Light associations as one native publication transaction. */
export function executeLightPlan(
  masterDirectory: string,
  outputDirectory: string,
  planning: MasterPlanSettings,
  execution: LightExecutionSettings,
  reviewedMasterPlan: Pick<MasterPlanPreview, "manifestSha256" | "planSha256">,
  reviewedLightPlan: Pick<LightCalibrationPlan, "planSha256">,
  onProgress: (progress: LightExecutionProgress) => void,
): Promise<LightExecutionResult> {
  const progress = new Channel<LightExecutionProgress>();
  progress.onmessage = onProgress;
  return invoke<LightExecutionResult>("execute_light_plan", {
    request: {
      masterDirectory,
      outputDirectory,
      planning,
      expectedManifestSha256: reviewedMasterPlan.manifestSha256,
      expectedMasterPlanSha256: reviewedMasterPlan.planSha256,
      expectedLightPlanSha256: reviewedLightPlan.planSha256,
      ...execution,
    },
    onProgress: progress,
  });
}

/** Requests cooperative cancellation of the active native Light transaction. */
export function cancelLightPlan(): Promise<boolean> {
  return invoke<boolean>("cancel_light_plan");
}

/** Opens a destination directory for one atomic corrected-Light product set. */
export async function selectDefectOutputDirectory(): Promise<string | null> {
  const path = await open({
    directory: true,
    multiple: false,
    title: "Select a directory for corrected Light products",
  });
  return typeof path === "string" ? path : null;
}

/**
 * Lets the backend resolve, name, validate, order, and seal the complete batch.
 * No destination path is synthesized by browser code.
 */
export function previewDefectBatch(
  request: {
    readonly outputDirectory: string;
    readonly focusFrameId: string;
    readonly allEligible: boolean;
    readonly expectedManifestSha256: string;
    readonly expectedLightPlanSha256: string;
  } & DefectCorrectionSettings,
): Promise<DefectBatchPreview> {
  return invoke<DefectBatchPreview>("preview_defect_batch", { request });
}

/**
 * Runs strict native defect analysis and publishes the corrected Light and its
 * exact HOT/COLD map together. The webview supplies only the stable imported
 * frame identity; the backend resolves every generated input from its native
 * artifact registry, then fingerprints and revalidates every byte.
 */
export function executeDefectCorrection(
  request: {
    readonly sourceFrameId: string;
    readonly correctedOutputPath: string;
    readonly mapOutputPath: string;
    readonly groupId: string;
    readonly expectedManifestSha256: string;
    readonly expectedLightPlanSha256: string;
    readonly expectedBatchPlanSha256: string;
    readonly batchItemIndex: number;
  } & DefectCorrectionSettings,
  onProgress: (progress: DefectExecutionProgress) => void,
): Promise<DefectCorrectionResult> {
  const progress = new Channel<DefectExecutionProgress>();
  progress.onmessage = onProgress;
  return invoke<DefectCorrectionResult>("execute_defect_correction", {
    request,
    onProgress: progress,
  });
}

/** Requests cooperative cancellation before the atomic publication boundary. */
export function cancelDefectCorrection(): Promise<boolean> {
  return invoke<boolean>("cancel_defect_correction");
}

/** Chooses a create-new JSON destination for the completed native report. */
export async function selectDefectBatchReportDestination(
  planSha256: string,
): Promise<string | null> {
  const path = await save({
    title: "Export verified detector correction report",
    defaultPath: `aetherstack-defect-${planSha256.slice(0, 12)}.json`,
    filters: [{ name: "JSON report", extensions: ["json"] }],
  });
  return typeof path === "string" ? path : null;
}

/** Publishes the completed native aggregate with its canonical SHA-256. */
export function exportDefectBatchReport(
  path: string,
  expectedBatchPlanSha256: string,
): Promise<DefectBatchReportExport> {
  return invoke<DefectBatchReportExport>("export_defect_batch_report", {
    path,
    expectedBatchPlanSha256,
  });
}

/** Chooses one previously exported detector-correction JSON envelope. */
export async function selectDefectBatchReportSource(): Promise<string | null> {
  const path = await open({
    directory: false,
    multiple: false,
    title: "Inspect a detector correction report",
    filters: [{ name: "JSON report", extensions: ["json"] }],
  });
  return typeof path === "string" ? path : null;
}

/** Reopens and verifies a report entirely in native code. */
export function inspectDefectBatchReport(
  path: string,
): Promise<DefectBatchReportInspection> {
  return invoke<DefectBatchReportInspection>("inspect_defect_batch_report", {
    path,
  });
}

/** Reads the native anti-replay cursor before an in-process resume. */
export function inspectActiveDefectBatch(
  expectedBatchPlanSha256: string,
): Promise<ActiveDefectBatch> {
  return invoke<ActiveDefectBatch>("inspect_active_defect_batch", {
    expectedBatchPlanSha256,
  });
}
