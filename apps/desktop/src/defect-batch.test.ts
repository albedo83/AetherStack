import { describe, expect, it } from "vitest";

import {
  defectBatchProgressMessage,
  defectOutputPath,
  orderDefectFrames,
} from "./defect-batch.ts";

function frame(id: string, groupId: string, sourceIndex: number) {
  return {
    sourceFrameId: id,
    sourceIndex,
    groupId,
  };
}

describe("detector correction batch helpers", () => {
  it("prioritizes the reviewed Light without disturbing remaining native order", () => {
    const inputs = [
      frame("a", "g", 0),
      frame("b", "g", 1),
      frame("c", "g", 2),
    ].map((candidate) => ({ frame: candidate }));
    expect(
      orderDefectFrames(inputs, "b").map((item) => item.frame.sourceFrameId),
    ).toEqual(["b", "a", "c"]);
    expect(orderDefectFrames(inputs, "missing")).toEqual(inputs);
  });

  it("builds portable stable names and sanitizes group identifiers", () => {
    const input = frame("a", "M 31/L", 6);
    expect(defectOutputPath("/output/", input, "corrected")).toBe(
      "/output/M-31-L-0007-corrected.fits",
    );
    expect(defectOutputPath("C:\\output\\", input, "defects")).toBe(
      "C:\\output\\M-31-L-0007-defects.fits",
    );
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
