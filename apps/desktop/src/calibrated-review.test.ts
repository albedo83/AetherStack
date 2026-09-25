import { describe, expect, it } from "vitest";

import type { ExecutedCalibratedLightFrame } from "./calibration-bridge.ts";
import {
  bindCalibratedLightFrames,
  frameArtifactKey,
} from "./calibrated-review.ts";
import type { ImportedFrame } from "./session-bridge.ts";

const source = (id: string, role: ImportedFrame["role"]): ImportedFrame => ({
  id,
  role,
  label: `${role}.fits`,
  relativePath: `${role.toUpperCase()}/${role}.fits`,
  path: `/session/${role}.fits`,
  exposureSeconds: 60,
  temperatureCelsius: -10,
  camera: "Synthetic camera",
  filter: "UVIR",
  bayerPattern: "rggb",
  axes: [4, 2],
  fitsDiagnosticCount: 0,
  classificationConflict: false,
});

const artifact = (sourceFrameId: string): ExecutedCalibratedLightFrame => ({
  groupId: "light-uvir",
  sourceIndex: 0,
  sourceFrameId,
  sourceLabel: "light.fits",
  sourceSha256: "f".repeat(64),
  outputPath: "/output/calibrated-light-000000.fits",
  totalSamples: 8,
  usableSamples: 8,
  maskedSamples: 0,
  nonFiniteSamples: 0,
  minimum: 1,
  maximum: 8,
  mean: 4.5,
  populationStandardDeviation: 2.29,
  samplesWritten: 8,
  substitutedSamples: 0,
  bytesWritten: 5_760,
  tilesProcessed: 1,
  tilesReused: 0,
});

describe("calibrated Blink binding", () => {
  it("preserves native artifact order while retaining raw review identities", () => {
    const first = "a".repeat(64);
    const second = "b".repeat(64);
    const result = bindCalibratedLightFrames(
      [source(second, "light"), source(first, "light")],
      [artifact(first), { ...artifact(second), sourceIndex: 1 }],
    );

    expect(result?.map((item) => item.source.id)).toEqual([first, second]);
  });

  it("rejects missing, duplicated, and non-Light source identities atomically", () => {
    const light = "a".repeat(64);
    const dark = "b".repeat(64);
    expect(
      bindCalibratedLightFrames([source(light, "light")], [artifact(dark)]),
    ).toBeNull();
    expect(
      bindCalibratedLightFrames(
        [source(light, "light")],
        [artifact(light), artifact(light)],
      ),
    ).toBeNull();
    expect(
      bindCalibratedLightFrames([source(dark, "dark")], [artifact(dark)]),
    ).toBeNull();
  });

  it("keeps caches distinct when one review identity has two pixel sources", () => {
    const id = "a".repeat(64);
    expect(frameArtifactKey(id, "/raw/light.fits")).not.toBe(
      frameArtifactKey(id, "/calibrated/light.fits"),
    );
  });
});
