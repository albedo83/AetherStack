import { invoke } from "@tauri-apps/api/core";
import { afterEach, describe, expect, it, vi } from "vitest";

import { demoReviewModel } from "./demo-data.ts";
import {
  applyFrameSelection,
  previewFrameSelection,
  type FrameSelectionPlan,
  type FrameSelectionRule,
} from "./selection-bridge.ts";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

afterEach(() => {
  vi.clearAllMocks();
});

describe("native frame-selection bridge", () => {
  it("passes artifact identities and typed rules without evaluating them", async () => {
    const frames = demoReviewModel.frames.slice(0, 2);
    const rules: readonly FrameSelectionRule[] = [
      {
        metric: "fwhm_pixels",
        comparator: "less_than",
        threshold: { kind: "scalar", value: 4.2 },
        missingPolicy: "reject",
      },
      {
        metric: "usable_stars",
        comparator: "greater_than",
        threshold: { kind: "count", value: 500 },
        missingPolicy: "reject",
      },
    ];
    const response: FrameSelectionPlan = {
      schemaVersion: 1,
      algorithmId: "frame-selection-rules-v1",
      rules,
      frames: [],
      planSha256: "a".repeat(64),
    };
    vi.mocked(invoke).mockResolvedValue(response);

    await expect(previewFrameSelection(frames, rules)).resolves.toBe(response);
    expect(invoke).toHaveBeenCalledWith("preview_frame_selection", {
      request: {
        frames: frames.map((frame) => ({
          frameId: frame.id,
          sourcePath: frame.sourcePath,
        })),
        rules,
      },
    });
  });

  it("confirms only the sealed digest and lets Rust rebuild the transaction", async () => {
    const frames = demoReviewModel.frames.slice(0, 2);
    const rules = demoReviewModel.frameSelection.rules;
    const planSha256 = "d".repeat(64);
    const response = { generation: 9, canUndo: true, changes: [] };
    vi.mocked(invoke).mockResolvedValue(response);

    await expect(applyFrameSelection(frames, rules, planSha256)).resolves.toBe(
      response,
    );
    expect(invoke).toHaveBeenCalledWith("apply_frame_selection", {
      request: {
        frames: frames.map((frame) => ({
          frameId: frame.id,
          sourcePath: frame.sourcePath,
        })),
        rules,
        planSha256,
      },
    });
  });
});
