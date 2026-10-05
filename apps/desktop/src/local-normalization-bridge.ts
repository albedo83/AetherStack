import { Channel, invoke } from "@tauri-apps/api/core";
import { open, save } from "@tauri-apps/plugin-dialog";

/** Every native local-normalization control that can affect scientific output. */
export interface LocalNormalizationSettings {
  readonly backgroundClippingSigma: number;
  readonly backgroundMaximumIterations: number;
  readonly backgroundMinimumSamples: number;
  readonly detectionSigma: number;
  readonly measurementFloorSigma: number;
  readonly measurementRadius: number;
  readonly minimumSeparation: number;
  readonly minimumMeasurementPixels: number;
  readonly maximumCandidates: number;
  readonly saturationLevel: number | null;
  readonly protectionGrowthFactor: number;
  readonly saturatedGrowthFactor: number;
  readonly minimumProtectionRadius: number;
  readonly maximumProtectionRadius: number;
  readonly maximumProtectedSources: number;
  readonly maximumProtectionPixelVisits: number;
  readonly cellWidth: number;
  readonly cellHeight: number;
  readonly maximumSamplesPerCell: number;
  readonly maximumCells: number;
  readonly minimumFitSamples: number;
  readonly maximumFitSamples: number;
  readonly maximumPairwiseSlopes: number;
  readonly minimumAbsoluteScale: number;
  readonly minimumControlPoints: number;
  readonly minimumSurfaceNeighbors: number;
  readonly maximumSurfaceNeighbors: number;
  readonly maximumSurfaceDistance: number;
}

/** Conservative quality-first defaults; advanced controls remain explicit and inspectable. */
export const defaultLocalNormalizationSettings: LocalNormalizationSettings = {
  backgroundClippingSigma: 3,
  backgroundMaximumIterations: 8,
  backgroundMinimumSamples: 1_024,
  detectionSigma: 6,
  measurementFloorSigma: 2,
  measurementRadius: 6,
  minimumSeparation: 4,
  minimumMeasurementPixels: 6,
  maximumCandidates: 24_576,
  saturationLevel: null,
  protectionGrowthFactor: 1.5,
  saturatedGrowthFactor: 2,
  minimumProtectionRadius: 2,
  maximumProtectionRadius: 24,
  maximumProtectedSources: 24_576,
  maximumProtectionPixelVisits: 64_000_000,
  cellWidth: 128,
  cellHeight: 128,
  maximumSamplesPerCell: 4_096,
  maximumCells: 262_144,
  minimumFitSamples: 256,
  maximumFitSamples: 4_096,
  maximumPairwiseSlopes: 1_000_000,
  minimumAbsoluteScale: 1e-12,
  minimumControlPoints: 16,
  minimumSurfaceNeighbors: 4,
  maximumSurfaceNeighbors: 16,
  maximumSurfaceDistance: 1_024,
};

export interface LocalNormalizationRequest {
  readonly sourcePath: string;
  readonly referencePath: string;
  readonly outputPath: string;
  readonly groupId: string;
  readonly memoryLimitBytes: number;
  readonly settings: LocalNormalizationSettings;
}

export interface LocalNormalizationPreflightRequest {
  readonly sourcePath: string;
  readonly referencePath: string;
  readonly memoryLimitBytes: number;
  readonly settings: LocalNormalizationSettings;
}

export interface LocalNormalizationPreflight {
  readonly width: number;
  readonly height: number;
  readonly planes: number;
  readonly requiredBytes: number;
  readonly memoryLimitBytes: number;
  readonly headroomBytes: number;
  readonly fitsMemoryLimit: boolean;
}

export interface LocalNormalizationProgress {
  readonly sequence: number;
  readonly stage: "local-normalization";
  readonly state: "started" | "running" | "completed" | "cancelled" | "failed";
  readonly completedUnits: number;
  readonly totalUnits: number | null;
  readonly code: string | null;
}

export interface LocalNormalizationResult {
  readonly planSha256: string;
  readonly parametersSha256: string;
  readonly outputPath: string;
  readonly width: number;
  readonly height: number;
  readonly planes: number;
  readonly memoryLimitBytes: number;
  readonly peakReservedBytes: number;
  readonly samplesWritten: number;
  readonly substitutedSamples: number;
  readonly bytesWritten: number;
  readonly transformedSamples: number;
  readonly inheritedMaskedSamples: number;
  readonly nonFiniteInputSamples: number;
  readonly unsupportedSurfaceSamples: number;
  readonly nonFiniteResultSamples: number;
  readonly measuredSources: number;
  readonly protectedPixels: number;
  readonly validControlPoints: number;
  readonly rejectedCells: number;
  readonly controlPoints: readonly LocalNormalizationControlPoint[];
  readonly cellDiagnostics: readonly LocalNormalizationCellDiagnostic[];
}

export interface LocalNormalizationCellDiagnostic {
  readonly plane: number;
  readonly x: number;
  readonly y: number;
  readonly width: number;
  readonly height: number;
  readonly protected: number;
  readonly sourceMasked: number;
  readonly referenceMasked: number;
  readonly nonFinite: number;
  readonly eligible: number;
  readonly retained: number;
  readonly accepted: boolean;
  readonly rejectionCode: string | null;
}

export interface LocalNormalizationControlPoint {
  readonly plane: number;
  readonly x: number;
  readonly y: number;
  readonly scale: number;
  readonly offset: number;
  readonly medianAbsoluteResidual: number;
}

const fitsFilters = [
  { name: "FITS images", extensions: ["fits", "fit", "fts"] },
];

async function selectFits(title: string): Promise<string | null> {
  const path = await open({
    directory: false,
    multiple: false,
    title,
    filters: [...fitsFilters],
  });
  return typeof path === "string" ? path : null;
}

/** Selects the calibrated image whose background will be normalized. */
export function selectLocalNormalizationSource(): Promise<string | null> {
  return selectFits("Select the calibrated image to normalize");
}

/** Selects the stable reference image that defines the target background. */
export function selectLocalNormalizationReference(): Promise<string | null> {
  return selectFits("Select the local-normalization reference image");
}

/** Selects a create-new FITS destination; native execution never overwrites it. */
export async function selectLocalNormalizationOutput(): Promise<string | null> {
  const path = await save({
    title: "Save the normalized FITS image",
    defaultPath: "normalized.fits",
    filters: [...fitsFilters],
  });
  return typeof path === "string" ? path : null;
}

/** Executes one source-bound native transaction and streams validated progress. */
export function executeLocalNormalization(
  request: LocalNormalizationRequest,
  onProgress: (progress: LocalNormalizationProgress) => void,
): Promise<LocalNormalizationResult> {
  const progress = new Channel<LocalNormalizationProgress>();
  progress.onmessage = onProgress;
  return invoke<LocalNormalizationResult>("execute_local_normalization", {
    request,
    onProgress: progress,
  });
}

/** Reads FITS headers and applies the executor's exact bounded-memory model. */
export function preflightLocalNormalization(
  request: LocalNormalizationPreflightRequest,
): Promise<LocalNormalizationPreflight> {
  return invoke<LocalNormalizationPreflight>("preflight_local_normalization", {
    request,
  });
}

/** Requests cooperative cancellation of the active native transaction. */
export function cancelLocalNormalization(): Promise<boolean> {
  return invoke<boolean>("cancel_local_normalization");
}

/** Converts the stable native failure code into concise, actionable UI copy. */
export function localNormalizationFailureMessage(error: unknown): string {
  const code =
    typeof error === "object" && error !== null && "code" in error
      ? (error as { readonly code?: unknown }).code
      : null;
  switch (code) {
    case "local_normalization_memory_insufficient":
      return "Memory ceiling is too low for this image pair · increase it and retry";
    case "local_normalization_dimensions_mismatch":
      return "Source and reference dimensions differ · choose a matching registered pair";
    case "local_normalization_destination_exists":
      return "Output already exists · choose a new create-only destination";
    case "local_normalization_source_changed":
      return "A source changed after fingerprinting · review both inputs and retry";
    case "local_normalization_publication_failed":
      return "Atomic FITS publication failed · no partial destination was accepted";
    case "local_normalization_configuration_invalid":
      return "A path, identifier, scientific control, or memory limit is invalid";
    case "local_normalization_input_invalid":
      return "A FITS input could not be opened or fingerprinted";
    case "local_normalization_interrupted":
      return "The native worker stopped before producing a validated result";
    case "local_normalization_cancelled":
      return "Normalization cancelled · no output was published";
    default:
      return "Normalization failed native validation · inputs remain unchanged";
  }
}
