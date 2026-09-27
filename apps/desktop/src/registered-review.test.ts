import { describe, expect, it } from "vitest";

import type { ExecutedCalibratedLightFrame } from "./calibration-bridge.ts";
import type { ExecutedRegisteredFrame } from "./registration-bridge.ts";
import type { RegistrationFrameOption } from "./model.ts";
import { bindRegisteredReviewFrames } from "./registered-review.ts";

const reviewed: readonly RegistrationFrameOption[] = [
  { id: "light-a", label: "Light A", sourcePath: "/raw/a.fits" },
  { id: "light-b", label: "Light B", sourcePath: "/raw/b.fits" },
];

function calibrated(
  id: string,
  rgbOutputPath: string | null,
): ExecutedCalibratedLightFrame {
  return {
    groupId: "group",
    sourceIndex: 0,
    sourceFrameId: id,
    sourceLabel: id,
    sourceSha256: "a".repeat(64),
    outputPath: `/calibrated/${id}.fits`,
    rgbOutputPath,
    totalSamples: 1,
    usableSamples: 1,
    maskedSamples: 0,
    nonFiniteSamples: 0,
    minimum: 0,
    maximum: 1,
    mean: 0.5,
    populationStandardDeviation: 0.1,
    samplesWritten: 1,
    substitutedSamples: 0,
    bytesWritten: 2_880,
    tilesProcessed: 1,
    tilesReused: 0,
  };
}

function registered(id: string): ExecutedRegisteredFrame {
  return {
    frameId: id,
    outputPath: `/registered/${id}.fits`,
    samplesWritten: 1,
    substitutedSamples: 0,
    bytesWritten: 2_880,
    interpolatedSamples: 1,
    outsideFootprintSamples: 0,
    maskedSupportSamples: 0,
  };
}

describe("registered result review binding", () => {
  it("preserves reviewed order and the calibrated pixel interpretation", () => {
    expect(
      bindRegisteredReviewFrames(
        reviewed,
        [calibrated("light-b", null), calibrated("light-a", "/rgb/a.fits")],
        [registered("light-b"), registered("light-a")],
      ),
    ).toEqual([
      {
        id: "light-a",
        label: "Light A",
        outputPath: "/registered/light-a.fits",
        previewContent: { kind: "rgb" },
      },
      {
        id: "light-b",
        label: "Light B",
        outputPath: "/registered/light-b.fits",
        previewContent: { kind: "scalar", plane: 0 },
      },
    ]);
  });

  it("rejects partial, duplicate, and foreign publication sets", () => {
    const calibratedSet = [
      calibrated("light-a", null),
      calibrated("light-b", null),
    ];
    expect(
      bindRegisteredReviewFrames(reviewed, calibratedSet, [
        registered("light-a"),
      ]),
    ).toBeNull();
    expect(
      bindRegisteredReviewFrames(reviewed, calibratedSet, [
        registered("light-a"),
        registered("light-a"),
      ]),
    ).toBeNull();
    expect(
      bindRegisteredReviewFrames(reviewed, calibratedSet, [
        registered("light-a"),
        registered("foreign"),
      ]),
    ).toBeNull();
  });
});
