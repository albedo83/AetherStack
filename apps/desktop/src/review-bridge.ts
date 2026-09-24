import { invoke } from "@tauri-apps/api/core";

import type {
  ReviewFrame,
  ReviewRejectionReason,
  SortDirection,
  SortField,
} from "./model.ts";

export type ReviewDecisionAction =
  | { readonly kind: "accept" }
  | { readonly kind: "reject"; readonly reason: ReviewRejectionReason }
  | { readonly kind: "clear" };

export interface ReviewDecisionEntry {
  readonly frameId: string;
  readonly state: "undecided" | "accepted" | "rejected";
  readonly rejectionReason: ReviewRejectionReason | null;
}

export interface ReviewDecisionUpdate {
  readonly generation: number;
  readonly canUndo: boolean;
  readonly changes: readonly ReviewDecisionEntry[];
}

/** Applies one explicit decision through the native transaction engine. */
export function applyReviewDecision(
  frameId: string,
  action: ReviewDecisionAction,
): Promise<ReviewDecisionUpdate> {
  return invoke<ReviewDecisionUpdate>("apply_review_decision", {
    request: { frameId, action },
  });
}

/** Reverts the latest native decision transaction for the imported session. */
export function undoReviewDecision(): Promise<ReviewDecisionUpdate> {
  return invoke<ReviewDecisionUpdate>("undo_review_decision");
}

/**
 * Requests a view-only deterministic order from the Rust review model.
 * Processing order, decisions, and metrics remain unchanged.
 */
export function sortReviewFrames(
  frames: readonly ReviewFrame[],
  field: SortField,
  direction: SortDirection,
): Promise<readonly string[]> {
  return invoke<readonly string[]>("sort_review_frames", {
    request: {
      frames: frames.map((frame) => ({
        id: frame.id,
        label: frame.label,
        fwhmPixels: frame.metrics.fwhmPixels,
        eccentricity: frame.metrics.eccentricity,
        detectedStars: frame.metrics.detectedStars,
        background: frame.metrics.background,
        noise: frame.metrics.noise,
      })),
      field,
      direction,
    },
  });
}

/**
 * Applies a native view order to the latest frame objects.
 *
 * The native request can overlap a manual review decision. Resolving returned
 * identities against the current model prevents the older request snapshot
 * from replacing a newer decision. Invalid, duplicate, or incomplete identity
 * sets are rejected without partially reordering the table.
 */
export function reorderReviewFrames(
  frames: readonly ReviewFrame[],
  identities: readonly string[],
): readonly ReviewFrame[] | null {
  if (identities.length !== frames.length) return null;
  const byIdentity = new Map(frames.map((frame) => [frame.id, frame]));
  if (byIdentity.size !== frames.length) return null;

  const seen = new Set<string>();
  const ordered: ReviewFrame[] = [];
  for (const identity of identities) {
    const frame = byIdentity.get(identity);
    if (!frame || seen.has(identity)) return null;
    seen.add(identity);
    ordered.push(frame);
  }
  return ordered;
}
