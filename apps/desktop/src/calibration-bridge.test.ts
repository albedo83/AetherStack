import { Channel, invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { afterEach, describe, expect, it, vi } from "vitest";

import {
  cancelMasterPlan,
  executeMasterPlan,
  previewMasterPlan,
  selectMasterOutputDirectory,
  type MasterExecutionProgress,
  type MasterExecutionResult,
  type MasterPlanPreview,
} from "./calibration-bridge.ts";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
  Channel: vi.fn(function MockChannel(this: { onmessage?: unknown }) {
    this.onmessage = undefined;
  }),
}));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));

afterEach(() => vi.clearAllMocks());

describe("native calibration bridge", () => {
  it("passes explicit scientific matching controls without source paths", async () => {
    const expected: MasterPlanPreview = {
      schemaVersion: 1,
      manifestSha256: "a".repeat(64),
      planSha256: "b".repeat(64),
      ready: true,
      products: [],
      lightPlan: null,
    };
    vi.mocked(invoke).mockResolvedValue(expected);
    const settings = {
      flatPedestalPolicy: "prefer_matched_dark_then_bias" as const,
      maximumExposureDeltaSeconds: 0.1,
      maximumTemperatureDeltaC: 2,
      maximumLightDarkTemperatureDeltaC: 2,
    };

    await expect(previewMasterPlan(settings)).resolves.toBe(expected);
    expect(invoke).toHaveBeenCalledWith("preview_master_plan", {
      request: settings,
    });
    expect(JSON.stringify(vi.mocked(invoke).mock.calls)).not.toContain("path");
  });

  it("streams progress through one channel and keeps execution controls explicit", async () => {
    const expected: MasterExecutionResult = {
      manifestSha256: "a".repeat(64),
      planSha256: "b".repeat(64),
      memoryLimitBytes: 1_073_741_824,
      peakReservedBytes: 4_194_304,
      products: [],
    };
    vi.mocked(invoke).mockResolvedValue(expected);
    const onProgress = vi.fn<(progress: MasterExecutionProgress) => void>();
    const planning = {
      flatPedestalPolicy: "prefer_matched_dark_then_bias" as const,
      maximumExposureDeltaSeconds: 0.1,
      maximumTemperatureDeltaC: 2,
      maximumLightDarkTemperatureDeltaC: 2,
    };
    const build = {
      minimumFlatNormalizationSamples: 1_024,
      minimumPositiveFlatMedian: 1e-12,
      tileWidth: 256,
      tileHeight: 256,
      memoryLimitBytes: 1_073_741_824,
    };

    await expect(
      executeMasterPlan(
        "/session/masters",
        planning,
        build,
        expected,
        onProgress,
      ),
    ).resolves.toBe(expected);

    expect(Channel).toHaveBeenCalledOnce();
    const invocation = vi.mocked(invoke).mock.calls[0];
    expect(invocation?.[0]).toBe("execute_master_plan");
    expect(invocation?.[1]).toMatchObject({
      request: {
        outputDirectory: "/session/masters",
        planning,
        expectedManifestSha256: expected.manifestSha256,
        expectedPlanSha256: expected.planSha256,
        ...build,
      },
    });
    expect(
      (invocation?.[1] as { onProgress?: { onmessage?: unknown } }).onProgress
        ?.onmessage,
    ).toBe(onProgress);
  });

  it("uses native output selection and exposes cooperative cancellation", async () => {
    vi.mocked(open).mockResolvedValue("/session/masters");
    vi.mocked(invoke).mockResolvedValue(true);

    await expect(selectMasterOutputDirectory()).resolves.toBe(
      "/session/masters",
    );
    expect(open).toHaveBeenCalledWith({
      directory: true,
      multiple: false,
      title: "Select a directory for calibration masters",
    });
    await expect(cancelMasterPlan()).resolves.toBe(true);
    expect(invoke).toHaveBeenCalledWith("cancel_master_plan");
  });
});
