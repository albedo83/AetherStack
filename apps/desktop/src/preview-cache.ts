import type {
  FitsPreviewRequest,
  PreviewResource,
  PreviewTransfer,
} from "./preview-bridge.ts";

/** Versioned identity for browser-side display artifacts. */
export const PREVIEW_CACHE_SCHEMA = "aether-desktop-preview-cache-v1";

/**
 * Small least-recently-used cache for browser object URLs.
 *
 * Both entry count and encoded PNG bytes are hard bounds for application-owned
 * object URLs. The entry limit also bounds the decoded raster surfaces still
 * addressable through those URLs; WebView-internal caches remain outside
 * application control. Eviction revokes each URL exactly once through the
 * resource's idempotent owner.
 */
export class BoundedPreviewCache {
  readonly #maximumEntries: number;
  readonly #maximumEncodedBytes: number;
  readonly #entries = new Map<string, PreviewResource>();
  #encodedBytes = 0;

  constructor(maximumEntries: number, maximumEncodedBytes: number) {
    if (!positiveSafeInteger(maximumEntries)) {
      throw new RangeError(
        "preview cache entry limit must be a positive integer",
      );
    }
    if (!positiveSafeInteger(maximumEncodedBytes)) {
      throw new RangeError(
        "preview cache byte limit must be a positive integer",
      );
    }
    this.#maximumEntries = maximumEntries;
    this.#maximumEncodedBytes = maximumEncodedBytes;
  }

  get size(): number {
    return this.#entries.size;
  }

  get encodedBytes(): number {
    return this.#encodedBytes;
  }

  /** Tests membership without changing least-recently-used order. */
  has(key: string): boolean {
    return this.#entries.has(key);
  }

  /** Returns and promotes one artifact, or `undefined` on a cache miss. */
  get(key: string): PreviewResource | undefined {
    const resource = this.#entries.get(key);
    if (!resource) return undefined;
    this.#entries.delete(key);
    this.#entries.set(key, resource);
    return resource;
  }

  /**
   * Takes ownership of an artifact when it fits both bounds.
   *
   * A `false` result leaves ownership with the caller and never revokes the
   * supplied resource. Replacing a key or evicting an older entry revokes the
   * cache-owned resource immediately.
   */
  put(key: string, resource: PreviewResource): boolean {
    if (key.length === 0) throw new RangeError("preview cache key is empty");
    if (!positiveSafeInteger(resource.byteLength)) {
      throw new RangeError("preview resource byte length must be positive");
    }
    if (resource.byteLength > this.#maximumEncodedBytes) return false;

    const previous = this.#entries.get(key);
    if (previous === resource) {
      this.#entries.delete(key);
      this.#entries.set(key, resource);
      return true;
    }
    if (previous) {
      this.#entries.delete(key);
      this.#encodedBytes -= previous.byteLength;
      previous.revoke();
    }

    while (
      this.#entries.size >= this.#maximumEntries ||
      this.#encodedBytes + resource.byteLength > this.#maximumEncodedBytes
    ) {
      this.#evictOldest();
    }
    this.#entries.set(key, resource);
    this.#encodedBytes += resource.byteLength;
    return true;
  }

  /** Revokes and removes every cache-owned object URL. */
  clear(): void {
    for (const resource of this.#entries.values()) resource.revoke();
    this.#entries.clear();
    this.#encodedBytes = 0;
  }

  #evictOldest(): void {
    const key = this.#entries.keys().next().value;
    if (key === undefined) {
      throw new Error("preview cache accounting is inconsistent");
    }
    const resource = this.#entries.get(key);
    if (!resource) {
      throw new Error("preview cache entry disappeared during eviction");
    }
    this.#entries.delete(key);
    this.#encodedBytes -= resource.byteLength;
    resource.revoke();
  }
}

/**
 * Builds a path-free cache identity for one deterministic display artifact.
 *
 * The content-derived frame identity replaces the machine-specific source path.
 * Every display-affecting request field and the resolved transform algorithm are
 * included, so a changed stretch can never reuse pixels rendered under an older
 * transform.
 */
export function previewCacheKey(
  request: FitsPreviewRequest,
  transformAlgorithmId: string,
): string {
  if (transformAlgorithmId.length === 0) {
    throw new RangeError("preview transform algorithm identity is empty");
  }
  const contentIdentity =
    request.content.kind === "scalar"
      ? [request.content.kind, request.content.plane]
      : [request.content.kind];
  for (const value of [
    ...(request.content.kind === "scalar" ? [request.content.plane] : []),
    request.maximumWidth,
    request.maximumHeight,
    request.blackPoint,
    request.whitePoint,
    request.midtone,
  ]) {
    if (!Number.isFinite(value)) {
      throw new RangeError(
        "preview cache identity contains a non-finite number",
      );
    }
  }
  return JSON.stringify([
    PREVIEW_CACHE_SCHEMA,
    request.frameId,
    contentIdentity,
    request.maximumWidth,
    request.maximumHeight,
    canonicalZero(request.blackPoint),
    canonicalZero(request.whitePoint),
    canonicalZero(request.midtone),
    transferIdentity(request.transfer),
    transformAlgorithmId,
  ]);
}

function transferIdentity(transfer: PreviewTransfer): readonly unknown[] {
  switch (transfer.kind) {
    case "linear":
    case "midtones":
      return [transfer.kind];
    case "asinh":
      if (!Number.isFinite(transfer.softness)) {
        throw new RangeError("preview transfer softness is not finite");
      }
      return [transfer.kind, canonicalZero(transfer.softness)];
  }
}

function positiveSafeInteger(value: number): boolean {
  return Number.isSafeInteger(value) && value > 0;
}

function canonicalZero(value: number): number {
  return Object.is(value, -0) ? 0 : value;
}
