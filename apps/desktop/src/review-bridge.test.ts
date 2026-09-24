import { invoke } from "@tauri-apps/api/core";
import { afterEach, describe, expect, it, vi } from "vitest";

import { demoReviewModel } from "./demo-data.ts";
import { reorderReviewFrames, sortReviewFrames } from "./review-bridge.ts";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

afterEach(() => {
  vi.clearAllMocks();
});

describe("native review bridge", () => {
  it("sends identities, labels, and optional metrics to the Rust sorter", async () => {
    const expected = [...demoReviewModel.frames]
      .reverse()
      .map((frame) => frame.id);
    vi.mocked(invoke).mockResolvedValue(expected);

    await expect(
      sortReviewFrames(demoReviewModel.frames, "fwhm_major", "ascending"),
    ).resolves.toBe(expected);
    expect(invoke).toHaveBeenCalledWith("sort_review_frames", {
      request: {
        frames: demoReviewModel.frames.map((frame) => ({
          id: frame.id,
          label: frame.label,
          fwhmPixels: frame.metrics.fwhmPixels,
          eccentricity: frame.metrics.eccentricity,
          detectedStars: frame.metrics.detectedStars,
          background: frame.metrics.background,
          noise: frame.metrics.noise,
        })),
        field: "fwhm_major",
        direction: "ascending",
      },
    });
  });

  it("applies native identities to the latest reviewed frame objects", () => {
    const latest = demoReviewModel.frames.map((frame, index) =>
      index === 0 ? { ...frame, state: "rejected" as const } : frame,
    );
    const identities = latest.map((frame) => frame.id).reverse();

    const ordered = reorderReviewFrames(latest, identities);

    expect(ordered?.map((frame) => frame.id)).toEqual(identities);
    expect(ordered?.at(-1)).toBe(latest[0]);
    expect(ordered?.at(-1)?.state).toBe("rejected");
  });

  it("rejects incomplete, unknown, and duplicate native identity sets", () => {
    const identities = demoReviewModel.frames.map((frame) => frame.id);

    expect(
      reorderReviewFrames(demoReviewModel.frames, identities.slice(1)),
    ).toBe(null);
    expect(
      reorderReviewFrames(demoReviewModel.frames, [
        ...identities.slice(0, -1),
        "f".repeat(64),
      ]),
    ).toBe(null);
    expect(
      reorderReviewFrames(demoReviewModel.frames, [
        identities[0] ?? "",
        identities[0] ?? "",
        ...identities.slice(2),
      ]),
    ).toBe(null);
  });
});
