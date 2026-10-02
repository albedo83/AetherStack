import type { RegistrationPlanPreview } from "./registration-bridge.ts";
import type { FrameMetrics, ReviewFrame } from "./model.ts";

/** Metrics accepted by the versioned balanced-PSF weighting expression. */
export interface QualityWeightEvidence {
  readonly frameId: string;
  readonly signalToNoise: number;
  readonly fwhmPixels: number;
  readonly eccentricity: number;
}

export interface QualityWeightPreviewRow {
  readonly frameId: string;
  readonly label: string;
  readonly reference: boolean;
  readonly evidence: QualityWeightEvidence | null;
  readonly relativeWeight: number | null;
  readonly issue: string | null;
}

export interface QualityWeightPreflight {
  readonly ready: boolean;
  readonly referenceFrameId: string | null;
  readonly rows: readonly QualityWeightPreviewRow[];
  readonly evidence: readonly QualityWeightEvidence[];
}

/**
 * Builds display evidence for quality-weighted integration.
 *
 * Rust independently validates the same metrics and recomputes every weight;
 * these values are a transparent preflight, never an execution authority.
 */
export function buildQualityWeightPreflight(
  plan: RegistrationPlanPreview | null,
  frames: readonly ReviewFrame[],
): QualityWeightPreflight {
  if (!plan) {
    return { ready: false, referenceFrameId: null, rows: [], evidence: [] };
  }
  const byId = new Map(frames.map((frame) => [frame.id, frame]));
  const referenceFrame = byId.get(plan.referenceFrameId);
  const referenceEvidence = referenceFrame
    ? validatedEvidence(referenceFrame.id, referenceFrame.metrics)
    : null;
  const evidence: QualityWeightEvidence[] = [];
  const rows = plan.frames.map((planned) => {
    const frame = byId.get(planned.frameId);
    const current = frame ? validatedEvidence(frame.id, frame.metrics) : null;
    if (current) evidence.push(current);
    const issue = !frame
      ? "Frame is absent from the active review set"
      : !current
        ? metricIssue(frame.metrics)
        : !referenceEvidence
          ? "Reference metrics are unavailable"
          : null;
    return {
      frameId: planned.frameId,
      label: frame?.label ?? shortFrameId(planned.frameId),
      reference: planned.frameId === plan.referenceFrameId,
      evidence: current,
      relativeWeight:
        current && referenceEvidence
          ? balancedPsfWeight(current, referenceEvidence)
          : null,
      issue,
    };
  });
  return {
    ready: rows.length > 0 && rows.every((row) => row.issue === null),
    referenceFrameId: plan.referenceFrameId,
    rows,
    evidence,
  };
}

/** Mirrors the documented algorithm for inspection only. */
export function balancedPsfWeight(
  metrics: Omit<QualityWeightEvidence, "frameId">,
  reference: Omit<QualityWeightEvidence, "frameId">,
): number {
  if (
    metrics.signalToNoise === reference.signalToNoise &&
    metrics.fwhmPixels === reference.fwhmPixels &&
    metrics.eccentricity === reference.eccentricity
  ) {
    return 1;
  }
  const logarithmicWeight =
    2 * Math.log(metrics.signalToNoise / reference.signalToNoise) +
    2 * Math.log(reference.fwhmPixels / metrics.fwhmPixels) +
    Math.log(
      (1 - metrics.eccentricity ** 2) / (1 - reference.eccentricity ** 2),
    );
  return Math.exp(
    Math.min(
      Math.log(Number.MAX_VALUE),
      Math.max(Math.log(Number.MIN_VALUE), logarithmicWeight),
    ),
  );
}

function validatedEvidence(
  frameId: string,
  metrics: FrameMetrics,
): QualityWeightEvidence | null {
  const { signalToNoise, fwhmPixels, eccentricity } = metrics;
  return Number.isFinite(signalToNoise) &&
    signalToNoise !== null &&
    signalToNoise > 0 &&
    Number.isFinite(fwhmPixels) &&
    fwhmPixels !== null &&
    fwhmPixels > 0 &&
    Number.isFinite(eccentricity) &&
    eccentricity !== null &&
    eccentricity >= 0 &&
    eccentricity < 1
    ? { frameId, signalToNoise, fwhmPixels, eccentricity }
    : null;
}

function metricIssue(metrics: FrameMetrics): string {
  if (!(
    Number.isFinite(metrics.signalToNoise) && (metrics.signalToNoise ?? 0) > 0
  )) {
    return "Positive stellar SNR required";
  }
  if (!(Number.isFinite(metrics.fwhmPixels) && (metrics.fwhmPixels ?? 0) > 0)) {
    return "Positive FWHM required";
  }
  return "Eccentricity must be between 0 and 1";
}

function shortFrameId(frameId: string): string {
  return `${frameId.slice(0, 8)}…${frameId.slice(-6)}`;
}
