export interface BatchProgress {
  readonly completed: number;
  readonly total: number;
}

export type BatchOutcome = "completed" | "cancelled";

/**
 * Processes immutable frame identities strictly one at a time.
 *
 * Cancellation is checked before and after every awaited item. Progress counts
 * processed items rather than successful scientific measurements; the caller
 * keeps each frame's ready or failed state. Processing failures propagate so a
 * transport or invariant bug cannot be mistaken for a completed batch.
 */
export async function runSerialBatch<T>(
  items: readonly T[],
  process: (item: T) => Promise<void>,
  onProgress: (progress: BatchProgress) => void,
  isCancelled: () => boolean,
): Promise<BatchOutcome> {
  const total = items.length;
  for (const [index, item] of items.entries()) {
    if (isCancelled()) return "cancelled";
    await process(item);
    if (isCancelled()) return "cancelled";
    onProgress({ completed: index + 1, total });
  }
  return "completed";
}
