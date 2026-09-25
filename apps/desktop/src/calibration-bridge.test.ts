import { invoke } from "@tauri-apps/api/core";
import { afterEach, describe, expect, it, vi } from "vitest";

import {
  previewMasterPlan,
  type MasterPlanPreview,
} from "./calibration-bridge.ts";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

afterEach(() => vi.clearAllMocks());

describe("native calibration bridge", () => {
  it("passes explicit scientific matching controls without source paths", async () => {
    const expected: MasterPlanPreview = {
      schemaVersion: 1,
      manifestSha256: "a".repeat(64),
      planSha256: "b".repeat(64),
      ready: true,
      products: [],
    };
    vi.mocked(invoke).mockResolvedValue(expected);
    const settings = {
      flatPedestalPolicy: "prefer_matched_dark_then_bias" as const,
      maximumExposureDeltaSeconds: 0.1,
      maximumTemperatureDeltaC: 2,
    };

    await expect(previewMasterPlan(settings)).resolves.toBe(expected);
    expect(invoke).toHaveBeenCalledWith("preview_master_plan", {
      request: settings,
    });
    expect(JSON.stringify(vi.mocked(invoke).mock.calls)).not.toContain("path");
  });
});
