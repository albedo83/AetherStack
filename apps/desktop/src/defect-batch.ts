import type {
  DefectExecutionProgress,
  ExecutedCalibratedLightFrame,
} from "./calibration-bridge.ts";

type DefectFrameIdentity = Pick<
  ExecutedCalibratedLightFrame,
  "sourceFrameId" | "sourceIndex" | "groupId"
>;

/** Keeps native publication order stable while prioritizing the review focus. */
export function orderDefectFrames<
  T extends { readonly frame: DefectFrameIdentity },
>(inputs: readonly T[], selectedFrameId: string | null): readonly T[] {
  const selected = inputs.find(
    (candidate) => candidate.frame.sourceFrameId === selectedFrameId,
  );
  if (!selected) return [...inputs];
  return [
    selected,
    ...inputs.filter(
      (candidate) => candidate.frame.sourceFrameId !== selectedFrameId,
    ),
  ];
}

/** Creates a deterministic collision-resistant product name inside one group. */
export function defectOutputPath(
  directory: string,
  frame: DefectFrameIdentity,
  suffix: "corrected" | "defects",
): string {
  const separator =
    directory.includes("\\") && !directory.includes("/") ? "\\" : "/";
  const root = directory.replace(/[\\/]$/, "");
  const group = frame.groupId.replace(/[^a-zA-Z0-9._-]/g, "-");
  const sequence = String(frame.sourceIndex + 1).padStart(4, "0");
  return `${root}${separator}${group}-${sequence}-${suffix}.fits`;
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
