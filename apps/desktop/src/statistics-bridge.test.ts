import { invoke } from "@tauri-apps/api/core";
import { afterEach, describe, expect, it, vi } from "vitest";

import { inspectFitsStatistics } from "./statistics-bridge.ts";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

afterEach(() => {
  vi.clearAllMocks();
});

describe("native FITS statistics bridge", () => {
  it("passes the exact selected source path to the bounded Rust command", async () => {
    const expected = {
      algorithmId: "fits-three-pass-moments-v1",
      axes: [4, 2],
      storedFormat: "IEEE 754 binary64",
      headerConformant: true,
      headerDiagnostics: 0,
      totalSamples: 8,
      usableSamples: 8,
      undefinedSamples: 0,
      nonFiniteSamples: 0,
      minimum: 0,
      maximum: 7,
      mean: 3.5,
      populationStandardDeviation: 2.291287847,
      sampleStandardDeviation: 2.449489743,
    };
    vi.mocked(invoke).mockResolvedValue(expected);

    await expect(inspectFitsStatistics("/session/LIGHTS/a.fits")).resolves.toBe(
      expected,
    );
    expect(invoke).toHaveBeenCalledWith("inspect_fits_statistics", {
      request: { path: "/session/LIGHTS/a.fits" },
    });
  });
});
