import { Channel, invoke } from "@tauri-apps/api/core";
import { open, save } from "@tauri-apps/plugin-dialog";
import { afterEach, describe, expect, it, vi } from "vitest";

import {
  cancelDefectCorrection,
  cancelLightPlan,
  cancelMasterPlan,
  executeDefectCorrection,
  executeLightPlan,
  executeMasterPlan,
  exportDefectBatchReport,
  previewDefectBatch,
  previewMasterPlan,
  selectDefectOutputDirectory,
  selectDefectBatchReportDestination,
  selectLightOutputDirectory,
  selectMasterOutputDirectory,
  type LightExecutionProgress,
  type LightExecutionResult,
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
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn(), save: vi.fn() }));

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

  it("binds Light execution to all three reviewed digests", async () => {
    const expected: LightExecutionResult = {
      manifestSha256: "a".repeat(64),
      masterPlanSha256: "b".repeat(64),
      lightPlanSha256: "c".repeat(64),
      memoryLimitBytes: 1_073_741_824,
      peakReservedBytes: 8_388_608,
      outputMode: "calibrated_frames",
      products: [],
      calibratedFrames: [],
    };
    vi.mocked(invoke).mockResolvedValue(expected);
    const planning = {
      flatPedestalPolicy: "require_matched_dark" as const,
      maximumExposureDeltaSeconds: 0.1,
      maximumTemperatureDeltaC: 2,
      maximumLightDarkTemperatureDeltaC: 2,
    };
    const execution = {
      outputMode: "calibrated_frames" as const,
      minimumAbsoluteFlat: 1e-12,
      tileWidth: 256,
      tileHeight: 256,
      memoryLimitBytes: 1_073_741_824,
    };
    const onProgress = vi.fn<(progress: LightExecutionProgress) => void>();

    await expect(
      executeLightPlan(
        "/session/masters",
        "/session/lights",
        planning,
        execution,
        {
          manifestSha256: expected.manifestSha256,
          planSha256: expected.masterPlanSha256,
        },
        { planSha256: expected.lightPlanSha256 },
        onProgress,
      ),
    ).resolves.toBe(expected);

    const invocation = vi.mocked(invoke).mock.calls[0];
    expect(invocation?.[0]).toBe("execute_light_plan");
    expect(invocation?.[1]).toMatchObject({
      request: {
        masterDirectory: "/session/masters",
        outputDirectory: "/session/lights",
        planning,
        expectedManifestSha256: expected.manifestSha256,
        expectedMasterPlanSha256: expected.masterPlanSha256,
        expectedLightPlanSha256: expected.lightPlanSha256,
        ...execution,
      },
    });
    expect(
      (invocation?.[1] as { onProgress?: { onmessage?: unknown } }).onProgress
        ?.onmessage,
    ).toBe(onProgress);
  });

  it("selects Light output and cancels through native commands", async () => {
    vi.mocked(open).mockResolvedValue("/session/lights");
    vi.mocked(invoke).mockResolvedValue(true);

    await expect(selectLightOutputDirectory()).resolves.toBe("/session/lights");
    expect(open).toHaveBeenCalledWith({
      directory: true,
      multiple: false,
      title: "Select a directory for calibrated Light products",
    });
    await expect(cancelLightPlan()).resolves.toBe(true);
    expect(invoke).toHaveBeenCalledWith("cancel_light_plan");
  });

  it("seals explicit dark, flat, and replacement controls for defect correction", async () => {
    const expected = {
      correctedOutputPath: "/session/corrected/light-0001.fits",
      mapOutputPath: "/session/corrected/light-0001-map.fits",
      parametersSha256: "d".repeat(64),
      batchPlanSha256: "e".repeat(64),
      batchItemIndex: 0,
      batchCompletedItems: 1,
      batchTotalItems: 1,
      batchComplete: true,
      reservedBytes: 234_020_736,
      requestedSamples: 31,
      correctedSamples: 30,
      insufficientSupportSamples: 1,
      blockedBySourceMaskSamples: 0,
      correctedSamplesWritten: 11_693_168,
      correctedSubstitutedSamples: 0,
      correctedBytesWritten: 93_548_224,
      mapSamplesWritten: 11_693_168,
      mapSubstitutedSamples: 0,
      mapBytesWritten: 93_548_224,
      darkDetection: {
        examinedSamples: 11_685_708,
        insufficientSupportSamples: 0,
        unavailableCentreSamples: 0,
        hotSamples: 30,
        coldSamples: 2,
      },
      flatDetection: {
        examinedSamples: 11_685_708,
        insufficientSupportSamples: 0,
        unavailableCentreSamples: 0,
        hotSamples: 1,
        coldSamples: 4,
      },
      mapSummary: {
        defectiveSamples: 35,
        hotSamples: 31,
        coldSamples: 6,
        conflictingSamples: 2,
      },
    };
    vi.mocked(invoke).mockResolvedValue(expected);
    const request = {
      sourceFrameId: "f".repeat(64),
      correctedOutputPath: expected.correctedOutputPath,
      mapOutputPath: expected.mapOutputPath,
      groupId: "light-uvir",
      expectedManifestSha256: "a".repeat(64),
      expectedLightPlanSha256: "c".repeat(64),
      expectedBatchPlanSha256: "d".repeat(64),
      batchItemIndex: 0,
      darkDetection: {
        radius: 2,
        stride: 2 as const,
        minimumNeighbours: 8,
        hotSigma: 6,
        coldSigma: 8,
        minimumAbsoluteDeviation: 1,
      },
      flatDetection: {
        radius: 2,
        stride: 2 as const,
        minimumNeighbours: 8,
        hotSigma: 8,
        coldSigma: 6,
        minimumAbsoluteDeviation: 0.000_001,
      },
      correctionRadius: 2,
      correctionStride: 2 as const,
      correctionMinimumNeighbours: 8,
      memoryLimitBytes: 1_073_741_824,
    };

    const onProgress = vi.fn();
    await expect(executeDefectCorrection(request, onProgress)).resolves.toBe(
      expected,
    );
    const invocation = vi.mocked(invoke).mock.calls.at(-1);
    expect(invocation?.[0]).toBe("execute_defect_correction");
    expect(invocation?.[1]).toMatchObject({ request });
    expect(
      (invocation?.[1] as { onProgress?: { onmessage?: unknown } }).onProgress
        ?.onmessage,
    ).toBe(onProgress);
    const payload = JSON.stringify(vi.mocked(invoke).mock.calls[0]?.[1]);
    expect(payload).not.toContain("calibratedLightPath");
    expect(payload).not.toContain("darkMasterPath");
    expect(payload).not.toContain("flatMasterPath");
  });

  it("selects a defect destination and exposes cooperative cancellation", async () => {
    vi.mocked(open).mockResolvedValue("/session/corrected");
    vi.mocked(invoke).mockResolvedValue(true);

    await expect(selectDefectOutputDirectory()).resolves.toBe(
      "/session/corrected",
    );
    expect(open).toHaveBeenCalledWith({
      directory: true,
      multiple: false,
      title: "Select a directory for corrected Light products",
    });
    await expect(cancelDefectCorrection()).resolves.toBe(true);
    expect(invoke).toHaveBeenCalledWith("cancel_defect_correction");
  });

  it("asks native code to seal the complete defect batch before execution", async () => {
    const expected = {
      ready: true,
      planSha256: "a".repeat(64),
      parametersSha256: "b".repeat(64),
      itemCount: 0,
      blockedItemCount: 0,
      items: [],
    };
    vi.mocked(invoke).mockResolvedValue(expected);
    const request = {
      outputDirectory: "/session/corrected",
      focusFrameId: "light-1",
      allEligible: true,
      expectedManifestSha256: "c".repeat(64),
      expectedLightPlanSha256: "d".repeat(64),
      darkDetection: {
        radius: 2,
        stride: 2 as const,
        minimumNeighbours: 8,
        hotSigma: 6,
        coldSigma: 8,
        minimumAbsoluteDeviation: 1,
      },
      flatDetection: {
        radius: 2,
        stride: 2 as const,
        minimumNeighbours: 8,
        hotSigma: 8,
        coldSigma: 6,
        minimumAbsoluteDeviation: 0.000_001,
      },
      correctionRadius: 2,
      correctionStride: 2 as const,
      correctionMinimumNeighbours: 8,
      memoryLimitBytes: 1_073_741_824,
    };

    await expect(previewDefectBatch(request)).resolves.toBe(expected);
    expect(invoke).toHaveBeenCalledWith("preview_defect_batch", { request });
  });

  it("selects and publishes a verified native defect report", async () => {
    vi.mocked(save).mockResolvedValue("/session/defect-report.json");
    const expected = {
      path: "/session/defect-report.json",
      reportSha256: "f".repeat(64),
      itemCount: 2,
    };
    vi.mocked(invoke).mockResolvedValue(expected);

    await expect(
      selectDefectBatchReportDestination("a".repeat(64)),
    ).resolves.toBe("/session/defect-report.json");
    expect(save).toHaveBeenCalledWith({
      title: "Export verified detector correction report",
      defaultPath: "aetherstack-defect-aaaaaaaaaaaa.json",
      filters: [{ name: "JSON report", extensions: ["json"] }],
    });
    await expect(
      exportDefectBatchReport("/session/defect-report.json", "a".repeat(64)),
    ).resolves.toBe(expected);
    expect(invoke).toHaveBeenLastCalledWith("export_defect_batch_report", {
      path: "/session/defect-report.json",
      expectedBatchPlanSha256: "a".repeat(64),
    });
  });
});
