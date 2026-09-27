import { invoke } from "@tauri-apps/api/core";
import { afterEach, describe, expect, it, vi } from "vitest";

import {
  diagnoseFitsRegistration,
  previewRegistrationPlan,
} from "./registration-bridge.ts";

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

  it("submits only reviewed identities for native plan reconstruction", async () => {
    const plan = { schemaVersion: 1, planSha256: "a".repeat(64) };
    vi.mocked(invoke).mockResolvedValue(plan);

    await expect(
      previewRegistrationPlan({
        referenceFrameId: "1".repeat(64),
        sourceFrameIds: ["2".repeat(64), "3".repeat(64)],
      }),
    ).resolves.toBe(plan);
    expect(invoke).toHaveBeenCalledWith("preview_registration_plan", {
      request: {
        referenceFrameId: "1".repeat(64),
        sourceFrameIds: ["2".repeat(64), "3".repeat(64)],
      },
    });
  });
});
