import { invoke } from "@tauri-apps/api/core";

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
