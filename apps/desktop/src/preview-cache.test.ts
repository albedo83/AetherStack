import { describe, expect, it, vi } from "vitest";

import { BoundedPreviewCache, previewCacheKey } from "./preview-cache.ts";
import type { FitsPreviewRequest, PreviewResource } from "./preview-bridge.ts";

function resource(frameDigit: string, byteLength: number) {
  const revoke = vi.fn<() => void>();
  return {
    preview: {
      frameId: frameDigit.repeat(64),
      url: `blob:${frameDigit}`,
    },
    byteLength,
    revoke,
  } satisfies PreviewResource;
}

function request(frameDigit: string): FitsPreviewRequest {
  return {
    frameId: frameDigit.repeat(64),
    path: `/private-session/${frameDigit}.fits`,
    plane: 0,
    maximumWidth: 1_600,
    maximumHeight: 1_200,
    blackPoint: 900,
    whitePoint: 4_500,
    midtone: 0.22,
    transfer: { kind: "midtones" },
  };
}

describe("bounded preview cache", () => {
  it("evicts the least recently used URL within both hard bounds", () => {
    const cache = new BoundedPreviewCache(2, 100);
    const first = resource("a", 30);
    const second = resource("b", 30);
    const third = resource("c", 50);

    expect(cache.put("first", first)).toBe(true);
    expect(cache.put("second", second)).toBe(true);
    expect(cache.has("first")).toBe(true);
    expect(cache.get("first")).toBe(first);
    expect(cache.put("third", third)).toBe(true);

    expect(second.revoke).toHaveBeenCalledOnce();
    expect(first.revoke).not.toHaveBeenCalled();
    expect(cache.get("second")).toBeUndefined();
    expect(cache.size).toBe(2);
    expect(cache.encodedBytes).toBe(80);
  });

  it("replaces and clears owned resources without double-accounting", () => {
    const cache = new BoundedPreviewCache(3, 100);
    const original = resource("a", 40);
    const replacement = resource("a", 25);
    cache.put("same", original);

    expect(cache.put("same", replacement)).toBe(true);
    expect(original.revoke).toHaveBeenCalledOnce();
    expect(cache.encodedBytes).toBe(25);

    cache.clear();
    cache.clear();
    expect(replacement.revoke).toHaveBeenCalledOnce();
    expect(cache.size).toBe(0);
    expect(cache.encodedBytes).toBe(0);
  });

  it("evicts by encoded-byte budget even below the entry limit", () => {
    const cache = new BoundedPreviewCache(4, 60);
    const first = resource("a", 40);
    const second = resource("b", 30);

    cache.put("first", first);
    cache.put("second", second);

    expect(first.revoke).toHaveBeenCalledOnce();
    expect(second.revoke).not.toHaveBeenCalled();
    expect(cache.size).toBe(1);
    expect(cache.encodedBytes).toBe(30);
  });

  it("checks membership without promoting an entry", () => {
    const cache = new BoundedPreviewCache(2, 100);
    const first = resource("a", 20);
    const second = resource("b", 20);
    const third = resource("c", 20);
    cache.put("first", first);
    cache.put("second", second);

    expect(cache.has("first")).toBe(true);
    cache.put("third", third);

    expect(first.revoke).toHaveBeenCalledOnce();
    expect(second.revoke).not.toHaveBeenCalled();
  });

  it("leaves an oversized artifact under caller ownership", () => {
    const cache = new BoundedPreviewCache(2, 20);
    const oversized = resource("a", 21);

    expect(cache.put("oversized", oversized)).toBe(false);
    expect(oversized.revoke).not.toHaveBeenCalled();
    expect(cache.size).toBe(0);
  });

  it("keys every display-affecting value but never the private source path", () => {
    const first = request("a");
    const moved = { ...first, path: "/another-machine/frame.fits" };
    const changedStretch = { ...first, midtone: 0.3 };

    const firstKey = previewCacheKey(first, "stretch-v1");
    expect(previewCacheKey(moved, "stretch-v1")).toBe(firstKey);
    expect(previewCacheKey(changedStretch, "stretch-v1")).not.toBe(firstKey);
    expect(previewCacheKey(first, "stretch-v2")).not.toBe(firstKey);
    expect(firstKey).not.toContain("private-session");
  });

  it("rejects invalid bounds and non-finite cache identities", () => {
    expect(() => new BoundedPreviewCache(0, 10)).toThrow(RangeError);
    expect(() => new BoundedPreviewCache(1, Number.NaN)).toThrow(RangeError);
    expect(() =>
      previewCacheKey(
        { ...request("a"), whitePoint: Number.POSITIVE_INFINITY },
        "v1",
      ),
    ).toThrow(RangeError);
  });
});
