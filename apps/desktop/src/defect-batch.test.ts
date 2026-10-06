import { describe, expect, it } from "vitest";

import {
  appendDefectBatchResult,
  defectBatchProgressMessage,
  reconcileActiveDefectBatch,
  reconcileDefectBatchPreview,
  startDefectBatchReport,
} from "./defect-batch.ts";

function frame(id: string, groupId: string, sourceIndex: number) {
  return {
    sourceFrameId: id,
    sourceIndex,
    groupId,
  };
}

describe("detector correction batch helpers", () => {
  it("accepts only a complete native-owned order and its destinations", () => {
    const inputs = [frame("a", "g", 0), frame("b", "g", 1)].map(
      (candidate) => ({ frame: candidate }),
    );
    const items = [inputs[1]!, inputs[0]!].map(({ frame: source }) => ({
      sourceFrameId: source.sourceFrameId,
      sourceIndex: source.sourceIndex,
      groupId: source.groupId,
      correctedOutputPath: `/output/${source.sourceFrameId}-corrected.fits`,
      mapOutputPath: `/output/${source.sourceFrameId}-defects.fits`,
      blockedByExistingOutput: false,
    }));
    const reconciled = reconcileDefectBatchPreview(
      {
        ready: true,
        planSha256: "a".repeat(64),
        parametersSha256: "b".repeat(64),
        itemCount: 2,
        blockedItemCount: 0,
        items,
      },
      inputs,
    );
    expect(reconciled.map(({ input }) => input.frame.sourceFrameId)).toEqual([
      "b",
      "a",
    ]);
    expect(reconciled[0]?.destination.correctedOutputPath).toBe(
      "/output/b-corrected.fits",
    );
  });

  it("fails closed for collisions, foreign identities, and incomplete plans", () => {
    const inputs = [{ frame: frame("a", "g", 0) }];
    const preview = {
      ready: true,
      planSha256: "a".repeat(64),
      parametersSha256: "b".repeat(64),
      itemCount: 1,
      blockedItemCount: 0,
      items: [
        {
          sourceFrameId: "foreign",
          sourceIndex: 0,
          groupId: "g",
          correctedOutputPath: "/output/same.fits",
          mapOutputPath: "/output/same.fits",
          blockedByExistingOutput: false,
        },
      ],
    };
    expect(() => reconcileDefectBatchPreview(preview, inputs)).toThrow(
      "does not match",
    );
    expect(() =>
      reconcileDefectBatchPreview({ ...preview, ready: false }, inputs),
    ).toThrow("not executable");
  });

  it("aggregates sealed evidence and retains the highest memory peak", () => {
    const preview = {
      ready: true,
      planSha256: "a".repeat(64),
      parametersSha256: "b".repeat(64),
      itemCount: 2,
      blockedItemCount: 0,
      items: [],
    };
    const result = {
      correctedOutputPath: "/output/a.fits",
      mapOutputPath: "/output/a-map.fits",
      parametersSha256: preview.parametersSha256,
      batchPlanSha256: preview.planSha256,
      batchItemIndex: 0,
      batchCompletedItems: 1,
      batchTotalItems: 2,
      batchComplete: false,
      reservedBytes: 100,
      requestedSamples: 7,
      correctedSamples: 5,
      insufficientSupportSamples: 1,
      blockedBySourceMaskSamples: 1,
      correctedSamplesWritten: 10,
      correctedSubstitutedSamples: 0,
      correctedBytesWritten: 40,
      mapSamplesWritten: 10,
      mapSubstitutedSamples: 0,
      mapBytesWritten: 40,
      darkDetection: {
        examinedSamples: 10,
        insufficientSupportSamples: 0,
        unavailableCentreSamples: 0,
        hotSamples: 2,
        coldSamples: 1,
      },
      flatDetection: {
        examinedSamples: 10,
        insufficientSupportSamples: 0,
        unavailableCentreSamples: 0,
        hotSamples: 1,
        coldSamples: 3,
      },
      mapSummary: {
        defectiveSamples: 6,
        hotSamples: 3,
        coldSamples: 4,
        conflictingSamples: 1,
      },
    };
    const first = appendDefectBatchResult(
      startDefectBatchReport(preview),
      result,
    );
    const second = appendDefectBatchResult(first, {
      ...result,
      reservedBytes: 80,
      batchItemIndex: 1,
      batchCompletedItems: 2,
      batchComplete: true,
    });
    expect(second).toMatchObject({
      completedItems: 2,
      requestedSamples: 14,
      correctedSamples: 10,
      insufficientSupportSamples: 2,
      blockedBySourceMaskSamples: 2,
      hotSamples: 6,
      coldSamples: 8,
      conflictingSamples: 2,
      peakReservedBytes: 100,
    });
    expect(() =>
      appendDefectBatchResult(second, {
        ...result,
        parametersSha256: "c".repeat(64),
        batchItemIndex: 2,
        batchCompletedItems: 3,
      }),
    ).toThrow("does not belong");
  });

  it("reports per-Light progress without claiming batch atomicity", () => {
    expect(
      defectBatchProgressMessage(1, 4, {
        sequence: 4,
        stage: "strict-defect-correction",
        state: "running",
        completedUnits: 3,
        totalUnits: 5,
        code: null,
      }),
    ).toBe(
      "Light 2/4 · Private products staged · validating checksums and sources · 3/5",
    );
  });

  it("accepts a resume only when the native cursor and aggregates match", () => {
    const report = {
      ...startDefectBatchReport({
        ready: true,
        planSha256: "a".repeat(64),
        parametersSha256: "b".repeat(64),
        itemCount: 3,
        blockedItemCount: 0,
        items: [],
      }),
      completedItems: 1,
      requestedSamples: 7,
      correctedSamples: 5,
      insufficientSupportSamples: 1,
      blockedBySourceMaskSamples: 1,
      conflictingSamples: 2,
      peakReservedBytes: 2048,
    };
    const native = {
      planSha256: report.planSha256,
      parametersSha256: report.parametersSha256,
      nextItemIndex: 1,
      totalItems: 3,
      complete: false,
      requestedSamples: 7,
      correctedSamples: 5,
      insufficientSupportSamples: 1,
      blockedBySourceMaskSamples: 1,
      hotSamples: 0,
      coldSamples: 0,
      defectiveSamples: 7,
      conflictingSamples: 2,
      peakReservedBytes: 2048,
    };

    expect(() => reconcileActiveDefectBatch(report, native, 1)).not.toThrow();
    expect(() =>
      reconcileActiveDefectBatch(report, { ...native, correctedSamples: 6 }, 1),
    ).toThrow("no longer matches");
    expect(() =>
      reconcileActiveDefectBatch(report, { ...native, complete: true }, 1),
    ).toThrow("no longer matches");
  });
});
