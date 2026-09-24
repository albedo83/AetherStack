import { invoke } from "@tauri-apps/api/core";
import { afterEach, describe, expect, it, vi } from "vitest";

import {
  estimateFitsPreviewTransform,
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
        plane: 0,
        maximumWidth: 1_600,
        maximumHeight: 1_200,
      }),
    ).resolves.toBe(estimate);
    expect(invoke).toHaveBeenCalledWith("estimate_fits_preview_transform", {
      request: {
        path: "/selected/reference.fits",
        plane: 0,
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
      plane: 0,
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
        plane: 0,
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
  });

  it("revokes each browser object URL at most once", async () => {
    vi.mocked(invoke).mockResolvedValue(new ArrayBuffer(8));
    const resource = await requestFitsPreview({
      frameId: "b".repeat(64),
      path: "/selected/frame.fits",
      plane: 0,
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
});
