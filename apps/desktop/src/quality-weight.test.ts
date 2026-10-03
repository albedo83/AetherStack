import { describe, expect, it } from "vitest";

import type { RegistrationPlanPreview } from "./registration-bridge.ts";
import type { ReviewFrame } from "./model.ts";
import {
  balancedPsfWeight,
  buildQualityWeightPreflight,
} from "./quality-weight.ts";

const id = (value: string): string => value.repeat(64);

function frame(
  value: string,
  signalToNoise: number | null,
  fwhmPixels: number | null,
  eccentricity: number | null,
): ReviewFrame {
  return {
    id: id(value),
    label: `light-${value}.fits`,
    sourcePath: `/lights/${value}.fits`,
    previewContent: { kind: "rgb" },
    exposureSeconds: 60,
    temperatureCelsius: -10,
    classificationWarning: null,
    bayerPattern: "rggb",
    qualityState: "ready",
    qualityMessage: "Measured",
    qualityProfileId: "test-quality-v1",
    qualityOrigin: "measured",
    state: "accepted",
    rejectionReason: null,
    metrics: {
      signalToNoise,
      fwhmPixels,
      eccentricity,
      detectedStars: 100,
      usableStars: 90,
      background: 20,
      noise: 2,
    },
  };
}

function plan(): RegistrationPlanPreview {
  return {
    schemaVersion: 1,
    planSha256: id("f"),
    referenceFrameId: id("a"),
    referenceWidth: 100,
    referenceHeight: 80,
    coveredPixels: 8_000,
    autocrop: { x: 0, y: 0, width: 100, height: 80 },
    frames: [
      {
        frameId: id("a"),
        sourceWidth: 100,
        sourceHeight: 80,
        transformCoefficientsSourcePixels: [1, 0, 0, 0, 1, 0],
        reference: true,
      },
      {
        frameId: id("b"),
        sourceWidth: 100,
        sourceHeight: 80,
        transformCoefficientsSourcePixels: [1, 0, 0, 0, 1, 0],
        reference: false,
      },
    ],
  };
}

describe("quality-weight preflight", () => {
  it("uses an exact unit weight for the reference and previews the formula", () => {
    const result = buildQualityWeightPreflight(plan(), [
      frame("a", 10, 2, 0),
      frame("b", 20, 1, 0),
    ]);

    expect(result.ready).toBe(true);
    expect(result.referenceFrameId).toBe(id("b"));
    expect(result.recommendedReferenceFrameId).toBe(id("b"));
    expect(result.rows[0]?.relativeWeight).toBeCloseTo(1 / 16, 14);
    expect(result.rows[1]?.relativeWeight).toBe(1);
    expect(result.evidence).toHaveLength(2);
  });

  it("honors an explicit valid expert reference", () => {
    const result = buildQualityWeightPreflight(
      plan(),
      [frame("a", 10, 2, 0), frame("b", 20, 1, 0)],
      id("a"),
    );

    expect(result.ready).toBe(true);
    expect(result.referenceFrameId).toBe(id("a"));
    expect(result.rows[0]?.relativeWeight).toBe(1);
    expect(result.rows[1]?.relativeWeight).toBeCloseTo(16, 14);
  });

  it("blocks a set with missing or invalid scientific evidence", () => {
    const result = buildQualityWeightPreflight(plan(), [
      frame("a", 10, 2, 0.2),
      frame("b", null, 2.2, 0.3),
    ]);

    expect(result.ready).toBe(false);
    expect(result.rows[1]?.issue).toBe("Positive stellar SNR required");
    expect(result.evidence).toHaveLength(1);
  });

  it("preserves the documented shape penalty", () => {
    expect(
      balancedPsfWeight(
        { signalToNoise: 10, fwhmPixels: 2, eccentricity: Math.sqrt(0.75) },
        { signalToNoise: 10, fwhmPixels: 2, eccentricity: 0 },
      ),
    ).toBeCloseTo(0.25, 14);
  });
});
