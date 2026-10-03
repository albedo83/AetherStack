import { invoke } from "@tauri-apps/api/core";

import type { ReviewFrame } from "./model.ts";
import type { ReviewDecisionUpdate } from "./review-bridge.ts";

export type FrameSelectionMetric =
  | "background"
  | "noise"
  | "signal_to_noise"
  | "detected_stars"
  | "usable_stars"
  | "fwhm_pixels"
  | "eccentricity";

export type FrameSelectionComparator = "less_than" | "greater_than";
export type MissingMetricPolicy = "retain" | "reject";

export type FrameSelectionThreshold =
  | { readonly kind: "scalar"; readonly value: number }
  | { readonly kind: "count"; readonly value: number };

export interface FrameSelectionRule {
  readonly metric: FrameSelectionMetric;
  readonly comparator: FrameSelectionComparator;
  readonly threshold: FrameSelectionThreshold;
  readonly missingPolicy: MissingMetricPolicy;
}

export type FrameSelectionValue =
  | { readonly kind: "scalar"; readonly value: number }
  | { readonly kind: "count"; readonly value: number };

export type FrameSelectionRuleState =
  "passed" | "failed" | "missing_retained" | "missing_rejected";

export interface FrameSelectionMetricEvidence {
  readonly measured: FrameSelectionValue | null;
  readonly state: FrameSelectionRuleState;
}

export interface FrameSelectionFrameResult {
  readonly frameId: string;
  readonly proposal: "retain" | "reject";
  readonly evidence: readonly FrameSelectionMetricEvidence[];
}

export interface FrameSelectionPlan {
  readonly schemaVersion: number;
  readonly algorithmId: string;
  readonly rules: readonly FrameSelectionRule[];
  readonly frames: readonly FrameSelectionFrameResult[];
  readonly planSha256: string;
}

/**
 * Requests an identity-bound, non-mutating automatic-selection preview.
 *
 * Rust resolves previously measured metrics by exact frame identity and
 * artifact path, validates every rule, restores native processing order, and
 * returns the canonical digest. This bridge contains no threshold evaluator
 * and cannot change a manual review decision.
 */
export function previewFrameSelection(
  frames: readonly ReviewFrame[],
  rules: readonly FrameSelectionRule[],
): Promise<FrameSelectionPlan> {
  return invoke<FrameSelectionPlan>("preview_frame_selection", {
    request: {
      frames: frames.map((frame) => ({
        frameId: frame.id,
        sourcePath: frame.sourcePath,
      })),
      rules,
    },
  });
}

/**
 * Confirms one already-previewed native plan as a single undoable transaction.
 * Rust rebuilds the plan from retained evidence and rejects a stale digest.
 * Existing explicit decisions remain authoritative.
 */
export function applyFrameSelection(
  frames: readonly ReviewFrame[],
  rules: readonly FrameSelectionRule[],
  planSha256: string,
): Promise<ReviewDecisionUpdate> {
  return invoke<ReviewDecisionUpdate>("apply_frame_selection", {
    request: {
      frames: frames.map((frame) => ({
        frameId: frame.id,
        sourcePath: frame.sourcePath,
      })),
      rules,
      planSha256,
    },
  });
}
