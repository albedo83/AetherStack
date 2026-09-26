import { invoke } from "@tauri-apps/api/core";
import { afterEach, describe, expect, it, vi } from "vitest";

import {
  inspectCfaFrameQuality,
  inspectRgbFrameQuality,
} from "./quality-bridge.ts";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

afterEach(() => {
  vi.clearAllMocks();
});

describe("native frame-quality bridge", () => {
  it("declares the selected Bayer phase and immutable source", async () => {
    const expected = {
      profileId: "desktop-diagnostic-quality-v1",
      detectionPlaneAlgorithmId: "cfa-cell-mean-v1",
      detectedStars: 42,
    };
    vi.mocked(invoke).mockResolvedValue(expected);

    await expect(
      inspectCfaFrameQuality("/session/LIGHTS/a.fits", "rggb"),
    ).resolves.toBe(expected);
    expect(invoke).toHaveBeenCalledWith("inspect_frame_quality", {
      request: {
        path: "/session/LIGHTS/a.fits",
        interpretation: { kind: "bayer_cell_mean", pattern: "rggb" },
      },
    });
  });

  it("requests linked linear luminance for a calibrated RGB product", async () => {
    const expected = {
      profileId: "desktop-diagnostic-quality-v1",
      detectionPlaneAlgorithmId: "linear-rec709-luminance-v1",
      detectedStars: 42,
    };
    vi.mocked(invoke).mockResolvedValue(expected);

    await expect(
      inspectRgbFrameQuality("/session/CALIBRATED/rgb.fits"),
    ).resolves.toBe(expected);
    expect(invoke).toHaveBeenCalledWith("inspect_frame_quality", {
      request: {
        path: "/session/CALIBRATED/rgb.fits",
        interpretation: { kind: "rgb_luminance" },
      },
    });
  });
});
