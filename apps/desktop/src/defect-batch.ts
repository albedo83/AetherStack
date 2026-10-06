import type {
  ActiveDefectBatch,
  DefectBatchPreview,
  DefectBatchPreviewItem,
  DefectCorrectionResult,
  DefectExecutionProgress,
  ExecutedCalibratedLightFrame,
} from "./calibration-bridge.ts";
import type { DefectBatchReport } from "./model.ts";

type DefectFrameIdentity = Pick<
  ExecutedCalibratedLightFrame,
  "sourceFrameId" | "sourceIndex" | "groupId"
>;

export interface ReconciledDefectBatchItem<T> {
  readonly input: T;
  readonly destination: DefectBatchPreviewItem;
}

/**
 * Proves that the browser resume cursor still describes the active native
 * batch. Every persisted aggregate is compared before another pair may run.
 */
export function reconcileActiveDefectBatch(
  report: DefectBatchReport,
  native: ActiveDefectBatch,
  expectedNextItemIndex: number,
): void {
  const counters = [
    report.totalItems,
    report.completedItems,
    report.requestedSamples,
    report.correctedSamples,
    report.insufficientSupportSamples,
    report.blockedBySourceMaskSamples,
    report.conflictingSamples,
    report.peakReservedBytes,
    native.nextItemIndex,
    native.totalItems,
    native.requestedSamples,
    native.correctedSamples,
    native.insufficientSupportSamples,
    native.blockedBySourceMaskSamples,
    native.hotSamples,
    native.coldSamples,
    native.defectiveSamples,
    native.conflictingSamples,
    native.peakReservedBytes,
    expectedNextItemIndex,
  ];
  if (
    counters.some((value) => !Number.isSafeInteger(value) || value < 0) ||
    !/^[0-9a-f]{64}$/.test(report.planSha256) ||
    !/^[0-9a-f]{64}$/.test(report.parametersSha256) ||
    report.planSha256 !== native.planSha256 ||
    report.parametersSha256 !== native.parametersSha256 ||
    report.completedItems !== expectedNextItemIndex ||
    native.nextItemIndex !== expectedNextItemIndex ||
    report.totalItems !== native.totalItems ||
    native.complete ||
    native.nextItemIndex >= native.totalItems ||
    report.requestedSamples !== native.requestedSamples ||
    report.requestedSamples !== native.defectiveSamples ||
    report.correctedSamples !== native.correctedSamples ||
    report.insufficientSupportSamples !== native.insufficientSupportSamples ||
    report.blockedBySourceMaskSamples !== native.blockedBySourceMaskSamples ||
    report.hotSamples !== native.hotSamples ||
    report.coldSamples !== native.coldSamples ||
    report.conflictingSamples !== native.conflictingSamples ||
    report.peakReservedBytes !== native.peakReservedBytes
  ) {
    throw new Error(
      "The native correction batch no longer matches the resume state",
    );
  }
}

/** Starts an inspectable report tied to the exact reviewed native plan. */
export function startDefectBatchReport(
  preview: DefectBatchPreview,
): DefectBatchReport {
  return {
    state: "running",
    planSha256: preview.planSha256,
    parametersSha256: preview.parametersSha256,
    totalItems: preview.itemCount,
    completedItems: 0,
    requestedSamples: 0,
    correctedSamples: 0,
    insufficientSupportSamples: 0,
    blockedBySourceMaskSamples: 0,
    hotSamples: 0,
    coldSamples: 0,
    conflictingSamples: 0,
    peakReservedBytes: 0,
  };
}

/** Adds native evidence without discarding the worst observed memory peak. */
export function appendDefectBatchResult(
  report: DefectBatchReport,
  result: DefectCorrectionResult,
): DefectBatchReport {
  const expectedCompleted = report.completedItems + 1;
  if (
    report.state !== "running" ||
    result.parametersSha256 !== report.parametersSha256 ||
    result.batchPlanSha256 !== report.planSha256 ||
    result.batchItemIndex !== report.completedItems ||
    result.batchCompletedItems !== expectedCompleted ||
    result.batchTotalItems !== report.totalItems ||
    result.batchComplete !== (expectedCompleted === report.totalItems)
  )
    throw new Error("Correction evidence does not belong to the sealed batch");
  return {
    ...report,
    completedItems: report.completedItems + 1,
    requestedSamples: report.requestedSamples + result.requestedSamples,
    correctedSamples: report.correctedSamples + result.correctedSamples,
    insufficientSupportSamples:
      report.insufficientSupportSamples + result.insufficientSupportSamples,
    blockedBySourceMaskSamples:
      report.blockedBySourceMaskSamples + result.blockedBySourceMaskSamples,
    hotSamples:
      report.hotSamples +
      result.darkDetection.hotSamples +
      result.flatDetection.hotSamples,
    coldSamples:
      report.coldSamples +
      result.darkDetection.coldSamples +
      result.flatDetection.coldSamples,
    conflictingSamples:
      report.conflictingSamples + result.mapSummary.conflictingSamples,
    peakReservedBytes: Math.max(report.peakReservedBytes, result.reservedBytes),
  };
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
  const byId = new Map(
    inputs.map((input) => [input.frame.sourceFrameId, input]),
  );
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
      throw new Error(
        "The native correction plan does not match reviewed artifacts",
      );
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
