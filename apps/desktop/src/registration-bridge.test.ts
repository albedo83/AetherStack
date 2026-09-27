import { Channel, invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { afterEach, describe, expect, it, vi } from "vitest";

import {
  cancelRegistrationPlan,
  diagnoseFitsRegistration,
  executeRegistrationPlan,
  previewRegistrationPlan,
  selectRegistrationOutputDirectory,
  type RegistrationExecutionProgress,
} from "./registration-bridge.ts";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
  Channel: vi.fn(function MockChannel(this: { onmessage?: unknown }) {
    this.onmessage = undefined;
  }),
}));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));

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

  it("binds atomic execution to the sealed digest and artifact identities", async () => {
    const result = { planSha256: "a".repeat(64), frames: [] };
    vi.mocked(invoke).mockResolvedValue(result);
    const planning = {
      referenceFrameId: "1".repeat(64),
      sourceFrameIds: ["2".repeat(64)],
    };
    const artifacts = [
      { frameId: "1".repeat(64), path: "/linear/reference.fits" },
      { frameId: "2".repeat(64), path: "/linear/source.fits" },
    ];
    const onProgress = vi.fn<(event: RegistrationExecutionProgress) => void>();

    await expect(
      executeRegistrationPlan(
        "/registered",
        planning,
        "a".repeat(64),
        artifacts,
        { bandHeight: 128, memoryLimitBytes: 1_073_741_824 },
        onProgress,
      ),
    ).resolves.toBe(result);

    expect(Channel).toHaveBeenCalledOnce();
    expect(invoke).toHaveBeenCalledWith("execute_registration_plan", {
      request: {
        planning,
        expectedPlanSha256: "a".repeat(64),
        artifacts,
        outputDirectory: "/registered",
        bandHeight: 128,
        memoryLimitBytes: 1_073_741_824,
      },
      onProgress: expect.objectContaining({ onmessage: onProgress }),
    });
  });

  it("uses native destination selection and cancellation", async () => {
    vi.mocked(open).mockResolvedValue("/registered");
    vi.mocked(invoke).mockResolvedValue(true);

    await expect(selectRegistrationOutputDirectory()).resolves.toBe(
      "/registered",
    );
    expect(open).toHaveBeenCalledWith({
      directory: true,
      multiple: false,
      title: "Select a directory for registered Light frames",
    });
    await expect(cancelRegistrationPlan()).resolves.toBe(true);
    expect(invoke).toHaveBeenLastCalledWith("cancel_registration_plan");
  });
});
