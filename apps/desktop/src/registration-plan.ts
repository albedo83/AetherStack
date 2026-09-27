import type { RegistrationDiagnostic } from "./registration-bridge.ts";
import type {
  AcceptedRegistrationSolution,
  RegistrationFrameOption,
} from "./model.ts";

/**
 * Reconciles one pair diagnostic into the deterministic multi-frame evidence.
 * Rejected diagnostics remove stale acceptance for that source. Unknown and
 * reference identities are never retained, even if a stale UI response arrives.
 */
export function reconcileRegistrationSolutions(
  frames: readonly RegistrationFrameOption[],
  referenceFrameId: string,
  sourceFrameId: string,
  existing: readonly AcceptedRegistrationSolution[],
  diagnostic: RegistrationDiagnostic,
): readonly AcceptedRegistrationSolution[] {
  const order = new Map(frames.map((frame, index) => [frame.id, index]));
  const retained = existing.filter(
    (solution) =>
      solution.sourceFrameId !== sourceFrameId &&
      solution.sourceFrameId !== referenceFrameId &&
      order.has(solution.sourceFrameId),
  );
  if (diagnostic.confidence.accepted && diagnostic.acceptedPlan !== null) {
    retained.push({ sourceFrameId, diagnostic });
  }
  retained.sort(
    (left, right) =>
      (order.get(left.sourceFrameId) ?? Number.MAX_SAFE_INTEGER) -
      (order.get(right.sourceFrameId) ?? Number.MAX_SAFE_INTEGER),
  );
  return retained;
}
