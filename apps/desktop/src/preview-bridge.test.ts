import { invoke } from "@tauri-apps/api/core";
import { afterEach, describe, expect, it, vi } from "vitest";

import {
  estimateFitsPreviewTransform,
  inspectRejectionHistogram,
  inspectStackPixel,
  requestFitsPreview,
} from "./preview-bridge.ts";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const createObjectUrl = vi.fn(() => "blob:aether-preview");
const revokeObjectUrl = vi.fn();

Object.defineProperties(URL, {
  createObjectURL: { configurable: true, value: createObjectUrl },
  revokeObjectURL: { configurable: true, value: revokeObjectUrl },
});

afterEach(() => {
  vi.clearAllMocks();
});

describe("native preview bridge", () => {
  it("requests an auditable reference stretch from Rust", async () => {
    const estimate = {
      algorithmId: "aether-preview-auto-stretch-v1",
      blackPoint: 900,
      whitePoint: 4_500,
      midtone: 0.22,
      finiteSamples: 50_000,
      median: 1_100,
      scaledMad: 45,
      highQuantile: 4_500,
    };
    vi.mocked(invoke).mockResolvedValue(estimate);

    await expect(
      estimateFitsPreviewTransform({
        path: "/selected/reference.fits",
        content: { kind: "scalar", plane: 0 },
        maximumWidth: 1_600,
        maximumHeight: 1_200,
      }),
    ).resolves.toBe(estimate);
    expect(invoke).toHaveBeenCalledWith("estimate_fits_preview_transform", {
      request: {
        path: "/selected/reference.fits",
        content: { kind: "scalar", plane: 0 },
        maximumWidth: 1_600,
        maximumHeight: 1_200,
      },
    });
  });

  it("keeps identity out of the native pixel request and binds it on return", async () => {
    vi.mocked(invoke).mockResolvedValue(new ArrayBuffer(8));

    const resource = await requestFitsPreview({
      frameId: "a".repeat(64),
      path: "/selected/frame.fits",
      content: { kind: "rgb" },
      maximumWidth: 1_600,
      maximumHeight: 1_200,
      blackPoint: 1_800,
      whitePoint: 4_200,
      midtone: 0.25,
      transfer: { kind: "midtones" },
    });

    expect(invoke).toHaveBeenCalledWith("render_fits_preview", {
      request: {
        path: "/selected/frame.fits",
        content: { kind: "rgb" },
        maximumWidth: 1_600,
        maximumHeight: 1_200,
        blackPoint: 1_800,
        whitePoint: 4_200,
        midtone: 0.25,
        transfer: { kind: "midtones" },
      },
    });
    expect(resource.preview).toEqual({
      frameId: "a".repeat(64),
      url: "blob:aether-preview",
    });
    expect(resource.byteLength).toBe(8);
  });

  it("revokes each browser object URL at most once", async () => {
    vi.mocked(invoke).mockResolvedValue(new ArrayBuffer(8));
    const resource = await requestFitsPreview({
      frameId: "b".repeat(64),
      path: "/selected/frame.fits",
      content: { kind: "scalar", plane: 0 },
      maximumWidth: 800,
      maximumHeight: 600,
      blackPoint: 0,
      whitePoint: 1,
      midtone: 0.5,
      transfer: { kind: "linear" },
    });

    resource.revoke();
    resource.revoke();
    expect(revokeObjectUrl).toHaveBeenCalledTimes(1);
    expect(revokeObjectUrl).toHaveBeenCalledWith("blob:aether-preview");
  });

  it("forwards an explicit diagnostic palette without changing scalar content", async () => {
    vi.mocked(invoke).mockResolvedValue(new ArrayBuffer(8));

    await requestFitsPreview({
      frameId: "c".repeat(64),
      path: "/selected/rejection-low.fits",
      content: { kind: "scalar", plane: 0 },
      maximumWidth: 800,
      maximumHeight: 600,
      blackPoint: 0,
      whitePoint: 12,
      midtone: 0.5,
      transfer: { kind: "linear" },
      palette: "rejection_low",
    });

    expect(invoke).toHaveBeenCalledWith("render_fits_preview", {
      request: expect.objectContaining({
        content: { kind: "scalar", plane: 0 },
        palette: "rejection_low",
      }),
    });
  });

  it("requests an exact native rejection-count histogram", async () => {
    const histogram = {
      algorithmId: "rejection-count-histogram-v1",
      totalSamples: 8,
      zeroSamples: 2,
      rejectedSamples: 6,
      maximumRejectedCount: 3,
      bins: [
        { rejectedCount: 0, samples: 2 },
        { rejectedCount: 1, samples: 4 },
        { rejectedCount: 3, samples: 2 },
      ],
    };
    vi.mocked(invoke).mockResolvedValue(histogram);

    await expect(
      inspectRejectionHistogram("/selected/rejection-low.fits"),
    ).resolves.toBe(histogram);
    expect(invoke).toHaveBeenCalledWith("inspect_rejection_histogram", {
      request: { path: "/selected/rejection-low.fits" },
    });
  });

  it("requests exact stack values at one integer coordinate", async () => {
    const inspection = {
      x: 12,
      y: 34,
      scienceValues: [1, 2, 3],
      lowRejectionCounts: [0, 1, 0],
      highRejectionCounts: [2, 0, 1],
    };
    vi.mocked(invoke).mockResolvedValue(inspection);
    const request = {
      sciencePath: "/results/science.fits",
      lowRejectionPath: "/results/low.fits",
      highRejectionPath: "/results/high.fits",
      x: 12,
      y: 34,
    };

    await expect(inspectStackPixel(request)).resolves.toBe(inspection);
    expect(invoke).toHaveBeenCalledWith("inspect_stack_pixel", { request });
  });
});
