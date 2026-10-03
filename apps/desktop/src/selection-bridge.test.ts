import { invoke } from "@tauri-apps/api/core";
import { afterEach, describe, expect, it, vi } from "vitest";

import { demoReviewModel } from "./demo-data.ts";
import {
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
});
