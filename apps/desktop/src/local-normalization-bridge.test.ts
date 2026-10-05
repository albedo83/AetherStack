import { Channel, invoke } from "@tauri-apps/api/core";
import { open, save } from "@tauri-apps/plugin-dialog";
import { afterEach, describe, expect, it, vi } from "vitest";

import {
  cancelLocalNormalization,
  defaultLocalNormalizationSettings,
  executeLocalNormalization,
  localNormalizationFailureMessage,
  preflightLocalNormalization,
  selectLocalNormalizationOutput,
  selectLocalNormalizationReference,
  selectLocalNormalizationSource,
  type LocalNormalizationProgress,
  type LocalNormalizationResult,
} from "./local-normalization-bridge.ts";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
  Channel: vi.fn(function MockChannel(this: { onmessage?: unknown }) {
    this.onmessage = undefined;
  }),
}));
vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: vi.fn(),
  save: vi.fn(),
}));

afterEach(() => vi.clearAllMocks());

describe("native local-normalization bridge", () => {
  it("requests a header-only bounded-memory preflight", async () => {
    const expected = {
      width: 4_144,
      height: 2_822,
      planes: 1,
      requiredBytes: 768_000_000,
      memoryLimitBytes: 2_147_483_648,
      headroomBytes: 1_379_483_648,
      fitsMemoryLimit: true,
    };
    vi.mocked(invoke).mockResolvedValue(expected);
    const request = {
      sourcePath: "/session/source.fits",
      referencePath: "/session/reference.fits",
      memoryLimitBytes: expected.memoryLimitBytes,
      settings: defaultLocalNormalizationSettings,
    };

    await expect(preflightLocalNormalization(request)).resolves.toBe(expected);
    expect(invoke).toHaveBeenCalledWith("preflight_local_normalization", {
      request,
    });
  });

  it("passes every scientific control through one progress channel", async () => {
    const expected: LocalNormalizationResult = {
      planSha256: "a".repeat(64),
      parametersSha256: "b".repeat(64),
      outputPath: "/session/normalized.fits",
      width: 4_144,
      height: 2_822,
      planes: 3,
      memoryLimitBytes: 2_147_483_648,
      peakReservedBytes: 1_073_741_824,
      samplesWritten: 35_087_124,
      substitutedSamples: 0,
      bytesWritten: 280_700_000,
      transformedSamples: 35_087_124,
      inheritedMaskedSamples: 0,
      nonFiniteInputSamples: 0,
      unsupportedSurfaceSamples: 0,
      nonFiniteResultSamples: 0,
      measuredSources: 12_000,
      protectedPixels: 900_000,
      validControlPoints: 759,
      rejectedCells: 4,
      controlPoints: [
        {
          plane: 0,
          x: 64,
          y: 64,
          scale: 1.002,
          offset: -12.5,
          medianAbsoluteResidual: 0.8,
        },
      ],
      cellDiagnostics: [
        {
          plane: 0,
          x: 0,
          y: 0,
          width: 128,
          height: 128,
          protected: 512,
          sourceMasked: 0,
          referenceMasked: 0,
          nonFinite: 0,
          eligible: 15_872,
          retained: 4_096,
          accepted: true,
          rejectionCode: null,
        },
      ],
    };
    vi.mocked(invoke).mockResolvedValue(expected);
    const onProgress = vi.fn<(event: LocalNormalizationProgress) => void>();
    const request = {
      sourcePath: "/session/calibrated/light-0001.fits",
      referencePath: "/session/registered/reference.fits",
      outputPath: expected.outputPath,
      groupId: "light-uvir",
      memoryLimitBytes: expected.memoryLimitBytes,
      settings: defaultLocalNormalizationSettings,
    };

    await expect(executeLocalNormalization(request, onProgress)).resolves.toBe(
      expected,
    );
    expect(Channel).toHaveBeenCalledOnce();
    const invocation = vi.mocked(invoke).mock.calls[0];
    expect(invocation?.[0]).toBe("execute_local_normalization");
    expect(invocation?.[1]).toMatchObject({ request });
    expect(
      (invocation?.[1] as { onProgress?: { onmessage?: unknown } }).onProgress
        ?.onmessage,
    ).toBe(onProgress);
    expect(Object.keys(defaultLocalNormalizationSettings)).toHaveLength(28);
  });

  it("uses native FITS choosers and exposes cooperative cancellation", async () => {
    vi.mocked(open)
      .mockResolvedValueOnce("/session/source.fits")
      .mockResolvedValueOnce("/session/reference.fit");
    vi.mocked(save).mockResolvedValue("/session/normalized.fits");
    vi.mocked(invoke).mockResolvedValue(true);

    await expect(selectLocalNormalizationSource()).resolves.toBe(
      "/session/source.fits",
    );
    await expect(selectLocalNormalizationReference()).resolves.toBe(
      "/session/reference.fit",
    );
    await expect(selectLocalNormalizationOutput()).resolves.toBe(
      "/session/normalized.fits",
    );
    expect(open).toHaveBeenNthCalledWith(
      1,
      expect.objectContaining({
        directory: false,
        multiple: false,
        title: "Select the calibrated image to normalize",
      }),
    );
    expect(save).toHaveBeenCalledWith(
      expect.objectContaining({ defaultPath: "normalized.fits" }),
    );
    await expect(cancelLocalNormalization()).resolves.toBe(true);
    expect(invoke).toHaveBeenCalledWith("cancel_local_normalization");
  });

  it.each([
    ["local_normalization_memory_insufficient", "Memory ceiling"],
    ["local_normalization_dimensions_mismatch", "dimensions differ"],
    ["local_normalization_destination_exists", "already exists"],
    ["local_normalization_source_changed", "changed after fingerprinting"],
    ["local_normalization_publication_failed", "Atomic FITS publication"],
    ["local_normalization_configuration_invalid", "scientific control"],
    ["local_normalization_input_invalid", "opened or fingerprinted"],
    ["local_normalization_interrupted", "native worker stopped"],
    ["local_normalization_cancelled", "cancelled"],
  ])("maps native failure %s to actionable copy", (code, expected) => {
    expect(localNormalizationFailureMessage({ code })).toContain(expected);
  });

  it("fails closed for malformed or unknown errors", () => {
    expect(localNormalizationFailureMessage(null)).toContain(
      "failed native validation",
    );
    expect(localNormalizationFailureMessage({ code: 17 })).toContain(
      "failed native validation",
    );
  });
});
