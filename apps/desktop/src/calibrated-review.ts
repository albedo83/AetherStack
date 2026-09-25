import type { ExecutedCalibratedLightFrame } from "./calibration-bridge.ts";
import type { ImportedFrame } from "./session-bridge.ts";

export interface BoundCalibratedLightFrame {
  readonly source: ImportedFrame;
  readonly artifact: ExecutedCalibratedLightFrame;
}

/**
 * Binds native calibrated artifacts to the already imported review identities.
 * The operation is deliberately all-or-nothing: a missing, duplicated, or
 * non-Light identity must never produce a plausible partial Blink set.
 */
export function bindCalibratedLightFrames(
  sources: readonly ImportedFrame[],
  artifacts: readonly ExecutedCalibratedLightFrame[],
): readonly BoundCalibratedLightFrame[] | null {
  const lights = new Map(
    sources
      .filter((source) => source.role === "light")
      .map((source) => [source.id, source] as const),
  );
  const seen = new Set<string>();
  const bound: BoundCalibratedLightFrame[] = [];
  for (const artifact of artifacts) {
    const source = lights.get(artifact.sourceFrameId);
    if (!source || seen.has(artifact.sourceFrameId)) return null;
    seen.add(artifact.sourceFrameId);
    bound.push({ source, artifact });
  }
  return bound;
}

/** Separates statistics and diagnostic-quality caches by exact pixel source. */
export function frameArtifactKey(frameId: string, sourcePath: string): string {
  return `${frameId}\u0000${sourcePath}`;
}
