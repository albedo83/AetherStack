import { describe, expect, it, vi } from "vitest";

import {
  adjacentPreviewFrames,
  PreviewPrefetchCoordinator,
} from "./preview-prefetch.ts";

interface Deferred<T> {
  readonly promise: Promise<T>;
  readonly resolve: (value: T) => void;
}

function deferred<T>(): Deferred<T> {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((resolvePromise) => {
    resolve = resolvePromise;
  });
  return { promise, resolve };
}

function artifact() {
  return { revoke: vi.fn<() => void>() };
}

describe("preview prefetch coordinator", () => {
  it("coalesces equal work and transfers ownership exactly once", async () => {
    const coordinator = new PreviewPrefetchCoordinator<
      ReturnType<typeof artifact>
    >();
    const pending = deferred<ReturnType<typeof artifact>>();
    const loaded = artifact();
    const load = vi.fn(() => pending.promise);
    const consume = vi.fn(() => true);

    const first = coordinator.schedule("same", load, consume);
    const second = coordinator.schedule("same", load, consume);
    expect(second).toBe(first);
    expect(coordinator.pending("same")).toBe(first);
    expect(coordinator.pendingCount).toBe(1);

    pending.resolve(loaded);
    await first;
    expect(load).toHaveBeenCalledOnce();
    expect(consume).toHaveBeenCalledOnce();
    expect(loaded.revoke).not.toHaveBeenCalled();
    expect(coordinator.pendingCount).toBe(0);
  });

  it("revokes a rejected artifact and contains loader or consumer failures", async () => {
    const coordinator = new PreviewPrefetchCoordinator<
      ReturnType<typeof artifact>
    >();
    const rejected = artifact();
    await coordinator.schedule(
      "rejected",
      async () => rejected,
      () => false,
    );
    expect(rejected.revoke).toHaveBeenCalledOnce();

    const thrown = artifact();
    await expect(
      coordinator.schedule(
        "throwing-consumer",
        async () => thrown,
        () => {
          throw new Error("cache unavailable");
        },
      ),
    ).resolves.toBeUndefined();
    expect(thrown.revoke).toHaveBeenCalledOnce();

    await expect(
      coordinator.schedule(
        "failed-load",
        async () => {
          throw new Error("preview unavailable");
        },
        () => true,
      ),
    ).resolves.toBeUndefined();
  });

  it("revokes an artifact that arrives after generation cancellation", async () => {
    const coordinator = new PreviewPrefetchCoordinator<
      ReturnType<typeof artifact>
    >();
    const pending = deferred<ReturnType<typeof artifact>>();
    const late = artifact();
    const consume = vi.fn(() => true);
    const completion = coordinator.schedule(
      "late",
      () => pending.promise,
      consume,
    );

    coordinator.cancel();
    expect(coordinator.pending("late")).toBeNull();
    pending.resolve(late);
    await completion;

    expect(consume).not.toHaveBeenCalled();
    expect(late.revoke).toHaveBeenCalledOnce();
  });
});

describe("adjacent preview selection", () => {
  const frames = ["a", "b", "c", "d", "e"].map((id) => ({ id }));

  it("prioritizes forward playback and then symmetric backward frames", () => {
    expect(
      adjacentPreviewFrames(frames, "c", 4).map((frame) => frame.id),
    ).toEqual(["d", "b", "e", "a"]);
  });

  it("wraps without returning duplicates or the selected frame", () => {
    expect(
      adjacentPreviewFrames(frames.slice(0, 3), "a", 5).map(
        (frame) => frame.id,
      ),
    ).toEqual(["b", "c"]);
  });

  it("returns no candidates for invalid selections and rejects invalid limits", () => {
    expect(adjacentPreviewFrames(frames, "missing", 2)).toEqual([]);
    expect(adjacentPreviewFrames(frames, "a", 0)).toEqual([]);
    expect(() => adjacentPreviewFrames(frames, "a", -1)).toThrow(RangeError);
  });
});
