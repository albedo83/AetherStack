import type {
  DefectBatchPreview,
  DefectBatchPreviewItem,
  DefectExecutionProgress,
  ExecutedCalibratedLightFrame,
} from "./calibration-bridge.ts";

type DefectFrameIdentity = Pick<
  ExecutedCalibratedLightFrame,
  "sourceFrameId" | "sourceIndex" | "groupId"
>;

export interface ReconciledDefectBatchItem<T> {
  readonly input: T;
  readonly destination: DefectBatchPreviewItem;
}

/**
 * Reconciles native destinations with the exact browser-visible artifact set.
 * Any missing, duplicate, foreign, or blocked identity closes execution.
 */
export function reconcileDefectBatchPreview<
  T extends { readonly frame: DefectFrameIdentity },
>(
  preview: DefectBatchPreview,
  inputs: readonly T[],
): readonly ReconciledDefectBatchItem<T>[] {
  if (
    !preview.ready ||
    preview.blockedItemCount !== 0 ||
    preview.itemCount !== preview.items.length ||
    preview.items.length !== inputs.length ||
    !/^[0-9a-f]{64}$/.test(preview.planSha256) ||
    !/^[0-9a-f]{64}$/.test(preview.parametersSha256)
  ) {
    throw new Error("The native correction plan is not executable");
  }
  const byId = new Map(inputs.map((input) => [input.frame.sourceFrameId, input]));
  if (byId.size !== inputs.length)
    throw new Error("The correction inputs contain duplicate identities");
  const destinations = new Set<string>();
  const seen = new Set<string>();
  return preview.items.map((destination) => {
    const input = byId.get(destination.sourceFrameId);
    if (
      !input ||
      seen.has(destination.sourceFrameId) ||
      destination.blockedByExistingOutput ||
      destination.groupId !== input.frame.groupId ||
      destination.sourceIndex !== input.frame.sourceIndex ||
      destination.correctedOutputPath === destination.mapOutputPath ||
      destinations.has(destination.correctedOutputPath) ||
      destinations.has(destination.mapOutputPath)
    ) {
      throw new Error("The native correction plan does not match reviewed artifacts");
    }
    seen.add(destination.sourceFrameId);
    destinations.add(destination.correctedOutputPath);
    destinations.add(destination.mapOutputPath);
    return { input, destination };
  });
}

/** Explains the exact native phase without implying whole-batch atomicity. */
export function defectBatchProgressMessage(
  itemIndex: number,
  itemCount: number,
  progress: DefectExecutionProgress,
): string {
  if (progress.state === "cancelled") return "Cancellation acknowledged safely";
  if (progress.state === "failed")
    return "Correction stopped before publication";
  const phase = [
    "Verifying sources and reserving bounded memory",
    "Master analysis complete · correcting the calibrated Light",
    "Correction complete · staging both FITS companions",
    "Private products staged · validating checksums and sources",
    "Validation complete · entering atomic publication",
    "Corrected Light and exact defect map published",
  ][Math.min(progress.completedUnits, 5)];
  return `Light ${itemIndex + 1}/${itemCount} · ${phase} · ${progress.completedUnits}/${progress.totalUnits ?? 5}`;
}
