import { invoke } from "@tauri-apps/api/core";
import { afterEach, describe, expect, it, vi } from "vitest";

import { diagnoseFitsRegistration } from "./registration-bridge.ts";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

afterEach(() => {
  vi.clearAllMocks();
});

describe("native registration bridge", () => {
  it("passes only the selected source and reference paths", async () => {
    const diagnostic = {
      schemaVersion: 2,
      confidence: { accepted: true },
      acceptedPlan: { autocrop: { x: 2, y: 5, width: 4137, height: 2815 } },
    };
    vi.mocked(invoke).mockResolvedValue(diagnostic);

    await expect(
      diagnoseFitsRegistration({
        sourcePath: "/session/light-02.fits",
        referencePath: "/session/light-01.fits",
      }),
    ).resolves.toBe(diagnostic);
    expect(invoke).toHaveBeenCalledWith("diagnose_fits_registration", {
      request: {
        sourcePath: "/session/light-02.fits",
        referencePath: "/session/light-01.fits",
      },
    });
  });
});
