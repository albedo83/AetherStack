import { invoke } from "@tauri-apps/api/core";

import type { BayerPattern } from "./model.ts";

export interface FrameQualityResult {
  readonly profileId: string;
  readonly backgroundAlgorithmId: string;
  readonly starAlgorithmId: string;
  readonly detectionPlaneAlgorithmId: string;
  readonly interpretation: string;
  readonly sourcePixelScale: number;
  readonly diagnosticOnly: boolean;
  readonly background: number;
  readonly noise: number;
  readonly initialUsableSamples: number;
  readonly retainedBackgroundSamples: number;
  readonly maskedSamples: number;
  readonly nonFiniteSamples: number;
  readonly detectedStars: number;
  readonly usableStars: number;
  readonly saturationLevel: number | null;
  readonly saturatedStars: number | null;
  readonly rawCandidates: number;
  readonly suppressedCandidates: number;
  readonly rejectedMeasurements: number;
  readonly fwhmPixels: number | null;
  readonly eccentricity: number | null;
}

/** Measures one raw CFA light without feeding display pixels into science. */
export function inspectCfaFrameQuality(
  path: string,
  pattern: BayerPattern,
): Promise<FrameQualityResult> {
  return invoke<FrameQualityResult>("inspect_frame_quality", {
    request: {
      path,
      interpretation: { kind: "bayer_cell_mean", pattern },
    },
  });
}

/** Measures a calibrated planar RGB light on linked linear Rec. 709 luminance. */
export function inspectRgbFrameQuality(
  path: string,
): Promise<FrameQualityResult> {
  return invoke<FrameQualityResult>("inspect_frame_quality", {
    request: {
      path,
      interpretation: { kind: "rgb_luminance" },
    },
  });
}
