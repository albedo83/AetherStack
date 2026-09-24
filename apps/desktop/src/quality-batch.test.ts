import { describe, expect, it, vi } from "vitest";

import { runSerialBatch } from "./quality-batch.ts";

describe("serial quality batch", () => {
  it("processes identities in order and reports exact progress", async () => {
    const order: string[] = [];
    const progress = vi.fn();

    await expect(
      runSerialBatch(
        ["a", "b", "c"],
        async (item) => {
          order.push(item);
        },
        progress,
        () => false,
      ),
    ).resolves.toBe("completed");

    expect(order).toEqual(["a", "b", "c"]);
    expect(progress.mock.calls).toEqual([
      [{ completed: 1, total: 3 }],
      [{ completed: 2, total: 3 }],
      [{ completed: 3, total: 3 }],
    ]);
  });

  it("stops between items without publishing stale progress", async () => {
    let cancelled = false;
    const processed: string[] = [];
    const progress = vi.fn(() => {
      cancelled = true;
    });

    await expect(
      runSerialBatch(
        ["a", "b"],
        async (item) => {
          processed.push(item);
        },
        progress,
        () => cancelled,
      ),
    ).resolves.toBe("cancelled");

    expect(processed).toEqual(["a"]);
    expect(progress).toHaveBeenCalledOnce();
  });

  it("checks cancellation after an in-flight item settles", async () => {
    let cancelled = false;
    const progress = vi.fn();

    await expect(
      runSerialBatch(
        ["a", "b"],
        async () => {
          cancelled = true;
        },
        progress,
        () => cancelled,
      ),
    ).resolves.toBe("cancelled");

    expect(progress).not.toHaveBeenCalled();
  });

  it("propagates processing failures and handles an empty batch", async () => {
    const progress = vi.fn();
    await expect(
      runSerialBatch(
        ["a"],
        async () => {
          throw new Error("measurement failed unexpectedly");
        },
        progress,
        () => false,
      ),
    ).rejects.toThrow("measurement failed unexpectedly");
    expect(progress).not.toHaveBeenCalled();

    await expect(
      runSerialBatch(
        [],
        async () => undefined,
        progress,
        () => false,
      ),
    ).resolves.toBe("completed");
  });
});
