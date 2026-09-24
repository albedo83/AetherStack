/** Minimal ownership contract for an asynchronously produced display artifact. */
export interface RevocableArtifact {
  revoke(): void;
}

interface PendingPrefetch {
  readonly token: symbol;
  readonly completion: Promise<void>;
}

/**
 * Coordinates speculative preview work without allowing it to escape a session.
 *
 * Callers retain the scientific policy: this helper only coalesces equal work,
 * invalidates older generations, and guarantees that an unconsumed artifact is
 * revoked exactly once. Loader failures are intentionally contained because a
 * prefetch miss must never turn into a user-visible review failure.
 */
export class PreviewPrefetchCoordinator<T extends RevocableArtifact> {
  readonly #pending = new Map<string, PendingPrefetch>();
  #generation = 0;

  get pendingCount(): number {
    return this.#pending.size;
  }

  /** Returns the completion of matching speculative work, when one exists. */
  pending(key: string): Promise<void> | null {
    return this.#pending.get(key)?.completion ?? null;
  }

  /**
   * Starts or joins one keyed prefetch.
   *
   * `consume` returns true only when it has accepted ownership of the artifact.
   * Returning false or throwing leaves cleanup to the coordinator.
   */
  schedule(
    key: string,
    load: () => Promise<T>,
    consume: (artifact: T) => boolean,
  ): Promise<void> {
    if (key.length === 0) throw new RangeError("prefetch key is empty");
    const existing = this.#pending.get(key);
    if (existing) return existing.completion;

    const generation = this.#generation;
    const token = Symbol(key);
    const completion = Promise.resolve()
      .then(load)
      .then((artifact) => {
        const current =
          generation === this.#generation &&
          this.#pending.get(key)?.token === token;
        if (!current) {
          artifact.revoke();
          return;
        }

        let consumed = false;
        try {
          consumed = consume(artifact);
        } finally {
          if (!consumed) artifact.revoke();
        }
      })
      .catch(() => {
        // Speculative work is best-effort; the foreground loader reports errors.
      })
      .finally(() => {
        if (this.#pending.get(key)?.token === token) {
          this.#pending.delete(key);
        }
      });

    this.#pending.set(key, { token, completion });
    return completion;
  }

  /** Invalidates all work; late artifacts are revoked when their loads settle. */
  cancel(): void {
    this.#generation += 1;
    this.#pending.clear();
  }
}

/**
 * Chooses a symmetric, deterministic neighbourhood around the selected frame.
 *
 * Forward frames are emitted first to favor Blink playback, followed by the
 * matching backward frame. Wrapping never duplicates a frame or the selection.
 */
export function adjacentPreviewFrames<T extends { readonly id: string }>(
  frames: readonly T[],
  selectedId: string,
  maximum: number,
): readonly T[] {
  if (!Number.isSafeInteger(maximum) || maximum < 0) {
    throw new RangeError(
      "adjacent preview limit must be a non-negative integer",
    );
  }
  const selectedIndex = frames.findIndex((frame) => frame.id === selectedId);
  if (selectedIndex < 0 || maximum === 0 || frames.length < 2) return [];

  const selected = frames[selectedIndex];
  if (!selected) return [];
  const result: T[] = [];
  const seen = new Set<string>([selected.id]);
  for (let distance = 1; result.length < maximum; distance += 1) {
    if (distance >= frames.length) break;
    for (const offset of [distance, -distance]) {
      const index = (selectedIndex + offset + frames.length) % frames.length;
      const frame = frames[index];
      if (!frame || seen.has(frame.id)) continue;
      seen.add(frame.id);
      result.push(frame);
      if (result.length === maximum) break;
    }
  }
  return result;
}
