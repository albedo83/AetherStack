import { describe, expect, it } from "vitest";

import {
  defectBatchProgressMessage,
  reconcileDefectBatchPreview,
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
    const inputs = [
      frame("a", "g", 0),
      frame("b", "g", 1),
    ].map((candidate) => ({ frame: candidate }));
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
});
