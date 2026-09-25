import { invoke } from "@tauri-apps/api/core";

export type FlatPedestalPolicy =
  "require_matched_dark" | "require_bias" | "prefer_matched_dark_then_bias";

export interface MasterPlanSettings {
  readonly flatPedestalPolicy: FlatPedestalPolicy;
  readonly maximumExposureDeltaSeconds: number;
  readonly maximumTemperatureDeltaC: number;
}

export type MasterProductKind = "bias" | "dark" | "flat";
export type PedestalStatus =
  "not_applicable" | "matched_dark" | "bias" | "unresolved";

export interface MasterBinning {
  readonly x: number;
  readonly y: number;
}

export interface MasterPedestal {
  readonly status: PedestalStatus;
  readonly selectedGroupId: string | null;
  readonly exposureDeltaSeconds: number | null;
  readonly temperatureBasis: "sensor" | "set_point" | null;
  readonly temperatureDeltaCelsius: number | null;
  readonly blockingReason: string | null;
  readonly ambiguousGroupIds: readonly string[];
}

export interface MasterPedestalMismatch {
  readonly field: string;
  readonly reason: "missing" | "different" | "outside_tolerance";
}

export interface MasterPedestalCandidate {
  readonly groupId: string;
  readonly sourceKind: "dark" | "bias";
  readonly status: "compatible" | "rejected";
  readonly exposureDeltaSeconds: number | null;
  readonly temperatureBasis: "sensor" | "set_point" | null;
  readonly temperatureDeltaCelsius: number | null;
  readonly mismatches: readonly MasterPedestalMismatch[];
}

export interface MasterProductPlan {
  readonly groupId: string;
  readonly kind: MasterProductKind;
  readonly frameCount: number;
  readonly camera: string | null;
  readonly axes: readonly number[];
  readonly exposureSeconds: number | null;
  readonly sensorTemperatureCelsius: number | null;
  readonly setTemperatureCelsius: number | null;
  readonly gain: number | null;
  readonly offset: number | null;
  readonly binning: MasterBinning | null;
  readonly filter: string | null;
  readonly bayerPattern: string | null;
  readonly pedestal: MasterPedestal;
  readonly candidates: readonly MasterPedestalCandidate[];
}

export interface MasterPlanPreview {
  readonly schemaVersion: number;
  readonly manifestSha256: string;
  readonly planSha256: string;
  readonly ready: boolean;
  readonly products: readonly MasterProductPlan[];
}

/**
 * Asks the native planner to derive calibration products from the immutable
 * imported manifest. No source paths or browser-reconstructed groups cross the
 * command boundary.
 */
export function previewMasterPlan(
  settings: MasterPlanSettings,
): Promise<MasterPlanPreview> {
  return invoke<MasterPlanPreview>("preview_master_plan", {
    request: settings,
  });
}
