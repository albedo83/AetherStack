import { Channel, invoke } from "@tauri-apps/api/core";
import { open, save } from "@tauri-apps/plugin-dialog";

import type { QualityWeightEvidence } from "./quality-weight.ts";

export interface RegistrationDiagnosticRequest {
  readonly sourcePath: string;
  readonly referencePath: string;
}

export interface RegistrationCrop {
  readonly x: number;
  readonly y: number;
  readonly width: number;
  readonly height: number;
}

export interface AcceptedRegistrationPlan {
  readonly footprintAlgorithmId: string;
  readonly transformCoefficientsSourcePixels: readonly [
    number,
    number,
    number,
    number,
    number,
    number,
  ];
  readonly referenceWidth: number;
  readonly referenceHeight: number;
  readonly coveredPixels: number;
  readonly autocrop: RegistrationCrop | null;
}

export interface RegistrationDiagnostic {
  readonly schemaVersion: number;
  readonly profileId: string;
  readonly diagnosticOnly: boolean;
  readonly source: {
    readonly contentSha256: string;
    readonly sourceWidth: number;
    readonly sourceHeight: number;
    readonly detectedStars: number;
    readonly medianFwhmSourcePixels: number | null;
    readonly medianEccentricity: number | null;
    readonly registrationFeatures: number;
  };
  readonly reference: {
    readonly contentSha256: string;
    readonly sourceWidth: number;
    readonly sourceHeight: number;
    readonly detectedStars: number;
    readonly medianFwhmSourcePixels: number | null;
    readonly medianEccentricity: number | null;
    readonly registrationFeatures: number;
  };
  readonly matching: {
    readonly retainedHypotheses: number;
    readonly geometricCandidates: number;
    readonly truncated: boolean;
  };
  readonly consensus: {
    readonly scale: number;
    readonly rotationRadians: number;
    readonly reflected: boolean;
    readonly inlierHypotheses: number;
    readonly inlierFeaturePairs: number;
    readonly rmsResidualDetectionPixels: number;
    readonly maximumResidualDetectionPixels: number;
  };
  readonly confidence: {
    readonly accepted: boolean;
    readonly inlierRatio: number;
    readonly winnerSupportMargin: number | null;
    readonly sourceAxisSpanFraction: readonly [number, number];
    readonly referenceAxisSpanFraction: readonly [number, number];
    readonly rejections: readonly string[];
  };
  readonly acceptedPlan: AcceptedRegistrationPlan | null;
}

export interface RegistrationPlanPreviewRequest {
  readonly referenceFrameId: string;
  readonly sourceFrameIds: readonly string[];
}

export interface RegistrationPlannedFrame {
  readonly frameId: string;
  readonly sourceWidth: number;
  readonly sourceHeight: number;
  readonly transformCoefficientsSourcePixels: readonly [
    number,
    number,
    number,
    number,
    number,
    number,
  ];
  readonly reference: boolean;
}

export interface RegistrationPlanPreview {
  readonly schemaVersion: number;
  readonly planSha256: string;
  readonly referenceFrameId: string;
  readonly referenceWidth: number;
  readonly referenceHeight: number;
  readonly coveredPixels: number;
  readonly autocrop: RegistrationCrop;
  readonly frames: readonly RegistrationPlannedFrame[];
}

export interface RegistrationArtifactInput {
  readonly frameId: string;
  readonly path: string;
}

export interface RegistrationExecutionSettings {
  readonly bandHeight: number;
  readonly memoryLimitBytes: number;
}

export interface RegisteredStackIntegrationSettings {
  readonly estimator: "strict_mean" | "weighted_mean" | "percentile_clipped";
  readonly lowFraction: number;
  readonly highFraction: number;
  readonly minimumRetainedSamples: number;
  readonly generateRejectionMaps: boolean;
}

export interface RegisteredStackExecutionSettings extends RegistrationExecutionSettings {
  readonly integration: RegisteredStackIntegrationSettings;
}

export interface RegistrationExecutionProgress {
  readonly frameIndex: number;
  readonly frameCount: number;
  readonly frameId: string;
  readonly sequence: number;
  readonly stage: string;
  readonly state: "started" | "running" | "completed" | "cancelled" | "failed";
  readonly completedUnits: number;
  readonly totalUnits: number | null;
  readonly code: string | null;
}

export interface ExecutedRegisteredFrame {
  readonly frameId: string;
  readonly outputPath: string;
  readonly samplesWritten: number;
  readonly substitutedSamples: number;
  readonly bytesWritten: number;
  readonly interpolatedSamples: number;
  readonly outsideFootprintSamples: number;
  readonly maskedSupportSamples: number;
}

export interface RegistrationExecutionResult {
  readonly planSha256: string;
  readonly memoryLimitBytes: number;
  readonly peakReservedBytes: number;
  readonly frames: readonly ExecutedRegisteredFrame[];
}

export interface RegisteredStackProgress {
  readonly sequence: number;
  readonly stage: string;
  readonly state: "started" | "running" | "completed" | "cancelled" | "failed";
  readonly completedUnits: number;
  readonly totalUnits: number | null;
  readonly code: string | null;
}

export interface RegisteredStackResult {
  readonly planSha256: string;
  readonly outputPath: string;
  readonly width: number;
  readonly height: number;
  readonly planes: number;
  readonly samplesWritten: number;
  readonly substitutedSamples: number;
  readonly bytesWritten: number;
  readonly memoryLimitBytes: number;
  readonly peakReservedBytes: number;
  readonly estimator: string;
  readonly lowRejectionMapPath: string | null;
  readonly highRejectionMapPath: string | null;
  readonly rejectionMapSamplesWritten: number | null;
}

/** Runs the bounded native registration diagnostic without exposing file data to JavaScript. */
export function diagnoseFitsRegistration(
  request: RegistrationDiagnosticRequest,
): Promise<RegistrationDiagnostic> {
  return invoke<RegistrationDiagnostic>("diagnose_fits_registration", {
    request,
  });
}

/** Rebuilds every pair natively and returns the canonical immutable plan. */
export function previewRegistrationPlan(
  request: RegistrationPlanPreviewRequest,
): Promise<RegistrationPlanPreview> {
  return invoke<RegistrationPlanPreview>("preview_registration_plan", {
    request,
  });
}

/** Opens a native destination chooser for the complete registered frame set. */
export async function selectRegistrationOutputDirectory(): Promise<
  string | null
> {
  const path = await open({
    directory: true,
    multiple: false,
    title: "Select a directory for registered Light frames",
  });
  return typeof path === "string" ? path : null;
}

/** Executes the reviewed plan as one rollback-safe native publication transaction. */
export function executeRegistrationPlan(
  outputDirectory: string,
  planning: RegistrationPlanPreviewRequest,
  expectedPlanSha256: string,
  artifacts: readonly RegistrationArtifactInput[],
  settings: RegistrationExecutionSettings,
  onProgress: (progress: RegistrationExecutionProgress) => void,
): Promise<RegistrationExecutionResult> {
  const progress = new Channel<RegistrationExecutionProgress>();
  progress.onmessage = onProgress;
  return invoke<RegistrationExecutionResult>("execute_registration_plan", {
    request: {
      planning,
      expectedPlanSha256,
      artifacts,
      outputDirectory,
      ...settings,
    },
    onProgress: progress,
  });
}

/** Requests cooperative cancellation of the active registration transaction. */
export function cancelRegistrationPlan(): Promise<boolean> {
  return invoke<boolean>("cancel_registration_plan");
}

/** Chooses the create-new FITS destination for the registered common-crop stack. */
export async function selectRegisteredStackOutput(): Promise<string | null> {
  const path = await save({
    title: "Save the integrated registered common crop",
    defaultPath: "integrated-common-crop.fits",
    filters: [{ name: "FITS image", extensions: ["fits", "fit", "fts"] }],
  });
  return typeof path === "string" ? path : null;
}

/** Integrates the exact registered set on its sealed common footprint. */
export function executeRegisteredStack(
  outputPath: string,
  planning: RegistrationPlanPreviewRequest,
  expectedPlanSha256: string,
  artifacts: readonly RegistrationArtifactInput[],
  qualityEvidence: readonly QualityWeightEvidence[],
  settings: RegisteredStackExecutionSettings,
  onProgress: (progress: RegisteredStackProgress) => void,
): Promise<RegisteredStackResult> {
  const progress = new Channel<RegisteredStackProgress>();
  progress.onmessage = onProgress;
  return invoke<RegisteredStackResult>("execute_registered_stack", {
    request: {
      planning,
      expectedPlanSha256,
      artifacts,
      qualityEvidence,
      outputPath,
      ...settings,
    },
    onProgress: progress,
  });
}

/** Requests cooperative cancellation of the active common-crop integration. */
export function cancelRegisteredStack(): Promise<boolean> {
  return invoke<boolean>("cancel_registered_stack");
}
