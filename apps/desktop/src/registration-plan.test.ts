import { describe, expect, it } from "vitest";

import type { RegistrationDiagnostic } from "./registration-bridge.ts";
import { reconcileRegistrationSolutions } from "./registration-plan.ts";

const frames = ["a", "b", "c"].map((id) => ({
  id,
  label: `${id}.fits`,
  sourcePath: `/${id}.fits`,
}));

function diagnostic(accepted: boolean, marker: number): RegistrationDiagnostic {
  return {
    schemaVersion: 3,
    profileId: `profile-${marker}`,
    diagnosticOnly: true,
    source: {
      contentSha256: "a".repeat(64),
      sourceWidth: 10,
      sourceHeight: 8,
      detectedStars: 20,
      medianFwhmSourcePixels: 2,
      medianEccentricity: 0.2,
      registrationFeatures: 18,
    },
    reference: {
      contentSha256: "b".repeat(64),
      sourceWidth: 10,
      sourceHeight: 8,
      detectedStars: 21,
      medianFwhmSourcePixels: 2,
      medianEccentricity: 0.2,
      registrationFeatures: 19,
    },
    matching: {
      retainedHypotheses: 10,
      geometricCandidates: 3,
      truncated: false,
    },
    consensus: {
      scale: 1,
      rotationRadians: 0,
      reflected: false,
      inlierHypotheses: 9,
      inlierFeaturePairs: 16,
      rmsResidualDetectionPixels: 0.1,
      maximumResidualDetectionPixels: 0.2,
    },
    projectiveAdequacy: {
      selectionApplied: false,
      matchCount: 16,
      transformCoefficientsDetectionPixels: [
        [1, 0, marker * 0.5],
        [0, 1, 0],
        [0, 0, 1],
      ],
      similarityRmsResidualDetectionPixels: 0.1,
      similarityMaximumResidualDetectionPixels: 0.2,
      projectiveRmsResidualDetectionPixels: 0.09,
      projectiveMaximumResidualDetectionPixels: 0.18,
      rmsImprovementDetectionPixels: 0.01,
      relativeRmsImprovement: 0.1,
      maximumModelSeparationDetectionPixels: 0.03,
      rankSeparationRatio: 0.02,
      crossValidation: {
        foldCount: 5,
        projectiveBetterFolds: 2,
        similarityRmsResidualDetectionPixels: 0.11,
        similarityMaximumResidualDetectionPixels: 0.22,
        projectiveRmsResidualDetectionPixels: 0.12,
        projectiveMaximumResidualDetectionPixels: 0.24,
        rmsImprovementDetectionPixels: -0.01,
        relativeRmsImprovement: -0.09,
        minimumRankSeparationRatio: 0.018,
      },
      recommendation: {
        recommended: false,
        minimumMatches: 20,
        minimumValidationFolds: 5,
        minimumProjectiveBetterFolds: 5,
        minimumRmsImprovementDetectionPixels: 0.05,
        minimumRelativeRmsImprovement: 0.1,
        minimumRankSeparationRatio: 0.01,
        maximumProjectiveRmsDetectionPixels: 1,
        minimumModelSeparationDetectionPixels: 0.25,
        supportSufficient: false,
        validationFoldsSufficient: true,
        foldWinsSufficient: false,
        absoluteGainSufficient: false,
        relativeGainSufficient: false,
        rankSeparationSufficient: true,
        projectiveRmsAcceptable: true,
        worstResidualNotIncreased: false,
        modelSeparationSufficient: false,
      },
    },
    confidence: {
      accepted,
      inlierRatio: 0.9,
      winnerSupportMargin: 0.5,
      sourceAxisSpanFraction: [0.8, 0.8],
      referenceAxisSpanFraction: [0.8, 0.8],
      rejections: accepted ? [] : ["insufficient_inliers"],
    },
    acceptedPlan: accepted
      ? {
          footprintAlgorithmId: "common-footprint-v1",
          transformCoefficientsSourcePixels: [1, 0, 0, 1, marker, 0],
          referenceWidth: 10,
          referenceHeight: 8,
          coveredPixels: 72,
          autocrop: { x: 1, y: 0, width: 9, height: 8 },
        }
      : null,
  };
}

describe("registration plan evidence", () => {
  it("orders accepted solutions by frame order rather than click order", () => {
    const afterC = reconcileRegistrationSolutions(
      frames,
      "a",
      "c",
      [],
      diagnostic(true, 3),
    );
    const afterB = reconcileRegistrationSolutions(
      frames,
      "a",
      "b",
      afterC,
      diagnostic(true, 2),
    );

    expect(afterB.map((solution) => solution.sourceFrameId)).toEqual([
      "b",
      "c",
    ]);
  });

  it("replaces accepted evidence and removes it after rejection", () => {
    const first = reconcileRegistrationSolutions(
      frames,
      "a",
      "b",
      [],
      diagnostic(true, 1),
    );
    const replaced = reconcileRegistrationSolutions(
      frames,
      "a",
      "b",
      first,
      diagnostic(true, 2),
    );
    const rejected = reconcileRegistrationSolutions(
      frames,
      "a",
      "b",
      replaced,
      diagnostic(false, 3),
    );

    expect(replaced).toHaveLength(1);
    expect(replaced[0]?.diagnostic.profileId).toBe("profile-2");
    expect(rejected).toEqual([]);
  });

  it("drops stale reference and unknown identities", () => {
    const stale = [
      { sourceFrameId: "a", diagnostic: diagnostic(true, 1) },
      { sourceFrameId: "missing", diagnostic: diagnostic(true, 2) },
    ];

    expect(
      reconcileRegistrationSolutions(
        frames,
        "a",
        "b",
        stale,
        diagnostic(false, 3),
      ),
    ).toEqual([]);
  });
});
