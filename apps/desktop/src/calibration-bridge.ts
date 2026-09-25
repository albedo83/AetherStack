import { Channel, invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";

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
  readonly sourceSha256: string;
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
