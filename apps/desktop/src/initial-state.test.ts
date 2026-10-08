import { describe, expect, it } from "vitest";

import { createEmptyReviewModel } from "./demo-data.ts";

describe("production launch state", () => {
  it("contains no synthetic acquisition or published evidence", () => {
    const model = createEmptyReviewModel();

    expect(model.sessionName).toBe("No session loaded");
    expect(model.sessionStatus.label).toBe("Ready to import FITS");
    expect(model.roles.every((role) => role.count === 0)).toBe(true);
    expect(model.frames).toEqual([]);
    expect(model.selectedFrameId).toBeNull();
    expect(model.calibration.plan).toBeNull();
    expect(model.calibration.execution.result).toBeNull();
    expect(model.calibration.lightExecution.result).toBeNull();
    expect(model.registration.frames).toEqual([]);
    expect(model.registration.plan).toBeNull();
    expect(model.registration.execution.result).toBeNull();
    expect(model.registration.stack.result).toBeNull();
    expect(model.registration.drizzle.result).toBeNull();
    expect(model.localNormalization.result).toBeNull();
  });

  it("returns independent role arrays for repeated launches", () => {
    const first = createEmptyReviewModel();
    const second = createEmptyReviewModel();
    expect(first).not.toBe(second);
    expect(first.roles).not.toBe(second.roles);
  });
});
