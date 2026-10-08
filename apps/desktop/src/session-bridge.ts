import { Channel, invoke } from "@tauri-apps/api/core";
import { open, save } from "@tauri-apps/plugin-dialog";

import type {
  BayerPattern,
  FrameRole,
  SessionDiagnostics,
  SessionDiagnosticItem,
} from "./model.ts";
import type { FrameQualityResult } from "./quality-bridge.ts";

export interface ImportedFrame {
  readonly id: string;
  readonly role: FrameRole;
  readonly label: string;
  readonly relativePath: string;
  readonly path: string;
  readonly exposureSeconds: number | null;
  readonly temperatureCelsius: number | null;
  readonly camera: string | null;
  readonly filter: string | null;
  readonly bayerPattern: BayerPattern | null;
  readonly axes: readonly number[];
  readonly fitsDiagnosticCount: number;
  readonly classificationConflict: boolean;
  readonly quality: FrameQualityResult | null;
}

export interface ImportedFailure {
  readonly relativePath: string;
  readonly code: string;
}

export interface SessionImportProgress {
  readonly stage: "discovering" | "analyzing" | "assembling" | "completed";
  readonly completedSources: number;
  readonly totalSources: number | null;
}

export interface ImportedSession {
  readonly name: string;
  readonly rootPath: string;
  readonly frames: readonly ImportedFrame[];
  readonly filesConsidered: number;
  readonly fingerprintedSourceBytes: number;
  readonly scanElapsedMilliseconds: number;
  readonly sourceAnalysisParallelism: number;
  readonly classificationConflicts: number;
  readonly recoverableFailures: readonly ImportedFailure[];
  readonly unassignedSources: readonly string[];
  readonly qualityEvidenceRestored: number;
  readonly qualityEvidenceMissing: number;
  readonly qualityEvidenceRejected: number;
  readonly qualityEvidenceRejections: readonly ImportedFailure[];
}

export interface ImportedSessionStatus {
  readonly tone: "ready" | "warning";
  readonly label: string;
}

export interface SessionDiagnosticsExport {
  readonly path: string;
  readonly reportSha256: string;
  readonly itemCount: number;
}

export interface SessionDiagnosticsInspection {
  readonly schemaVersion: number;
  readonly algorithmId: string;
  readonly manifestSha256: string;
  readonly reportSha256: string;
  readonly itemCount: number;
}

export interface QualityCacheMaintenancePreview {
  readonly algorithmId: string;
  readonly planSha256: string;
  readonly eligibleCount: number;
  readonly blockedCount: number;
  readonly totalFileBytes: number;
  readonly items: readonly {
    readonly source: string;
    readonly code: string;
    readonly cacheKey: string;
    readonly fileBytes: number;
    readonly fileSha256: string;
  }[];
  readonly blockedItems: readonly {
    readonly source: string;
    readonly code: string;
    readonly reason: string;
  }[];
}

export interface QualityCacheMaintenanceResult {
  readonly removedCount: number;
  readonly removedBytes: number;
  readonly skippedCount: number;
}

const MAX_SESSION_DIAGNOSTIC_ITEMS = 100;

/** Builds a path-safe, DOM-bounded list from native session evidence. */
export function importedSessionDiagnostics(
  session: ImportedSession,
): SessionDiagnostics {
  const items: SessionDiagnosticItem[] = [];
  const append = (item: SessionDiagnosticItem): void => {
    if (items.length < MAX_SESSION_DIAGNOSTIC_ITEMS) items.push(item);
  };
  for (const frame of session.frames) {
    if (frame.classificationConflict) {
      append({
        category: "classification",
        source: frame.relativePath,
        code: "classification_conflict",
      });
    }
  }
  for (const failure of session.recoverableFailures) {
    append({
      category: "fits",
      source: failure.relativePath,
      code: failure.code,
    });
  }
  for (const source of session.unassignedSources) {
    append({
      category: "grouping",
      source,
      code: "session_source_unassigned",
    });
  }
  for (const rejection of session.qualityEvidenceRejections) {
    append({
      category: "quality_cache",
      source: rejection.relativePath,
      code: rejection.code,
    });
  }
  const totalItems =
    session.classificationConflicts +
    session.recoverableFailures.length +
    session.unassignedSources.length +
    session.qualityEvidenceRejected;
  return {
    filesConsidered: session.filesConsidered,
    fingerprintedSourceBytes: session.fingerprintedSourceBytes,
    scanElapsedMilliseconds: session.scanElapsedMilliseconds,
    sourceAnalysisParallelism: session.sourceAnalysisParallelism,
    verifiedFrames: session.frames.length,
    classificationConflicts: session.classificationConflicts,
    recoverableFailures: session.recoverableFailures.length,
    unassignedSources: session.unassignedSources.length,
    qualityEvidenceRestored: session.qualityEvidenceRestored,
    qualityEvidenceMissing: session.qualityEvidenceMissing,
    qualityEvidenceRejected: session.qualityEvidenceRejected,
    items,
    omittedItems: Math.max(0, totalItems - items.length),
    exportState: "idle",
    exportMessage: "No redacted report exported",
    inspectionState: "idle",
    inspectionMessage: "No diagnostics report verified",
    maintenanceState: "idle",
    maintenanceMessage: "Rejected cache artifacts have not been inspected",
    maintenanceEligible: 0,
    maintenanceBlocked: 0,
    maintenanceBytes: 0,
    maintenancePlanSha256: null,
  };
}

/**
 * Summarizes FITS import integrity and optional quality-cache provenance.
 * Missing evidence is expected on a first import; rejected evidence means a
 * present cache artifact failed verification and will be recomputed safely.
 */
export function importedSessionStatus(
  session: ImportedSession,
): ImportedSessionStatus {
  const issueCount =
    session.classificationConflicts +
    session.recoverableFailures.length +
    session.unassignedSources.length;
  const importSummary =
    issueCount > 0
      ? `${issueCount} import issue${issueCount === 1 ? "" : "s"}`
      : `${session.frames.length} FITS verified`;
  const cacheSummary = [
    `${session.qualityEvidenceRestored} restored`,
    `${session.qualityEvidenceMissing} missing`,
    `${session.qualityEvidenceRejected} rejected`,
  ].join(" · ");
  return {
    tone:
      issueCount > 0 || session.qualityEvidenceRejected > 0
        ? "warning"
        : "ready",
    label: `${importSummary} · quality cache ${cacheSummary}`,
  };
}

/**
 * Lets the operating system select one directory, then asks Rust to perform the
 * bounded, deterministic FITS scan. Cancelling the native dialog is a normal
 * outcome and never clears the current session.
 */
export async function selectAndImportSession(
  onProgress: (progress: SessionImportProgress) => void = () => {},
): Promise<ImportedSession | null> {
  const path = await open({
    directory: true,
    multiple: false,
    title: "Import an astrophotography session",
  });
  if (typeof path !== "string") return null;
  return importSessionPath(path, onProgress);
}

/** Imports one exact native path selected by a trusted desktop interaction. */
export async function importSessionPath(
  path: string,
  onProgress: (progress: SessionImportProgress) => void = () => {},
): Promise<ImportedSession> {
  const progress = new Channel<SessionImportProgress>();
  progress.onmessage = onProgress;
  return invoke<ImportedSession>("import_session_directory", {
    path,
    onProgress: progress,
  });
}

/** Requests cooperative cancellation of the active native directory import. */
export async function cancelSessionImport(): Promise<boolean> {
  return invoke<boolean>("cancel_session_import");
}

/** Requests a new destination, then asks Rust to seal and publish the report. */
export async function exportSessionDiagnostics(): Promise<SessionDiagnosticsExport | null> {
  const path = await save({
    title: "Export redacted session diagnostics",
    defaultPath: "aetherstack-session-diagnostics.json",
    filters: [{ name: "JSON diagnostics", extensions: ["json"] }],
  });
  if (!path) return null;
  return invoke<SessionDiagnosticsExport>("export_session_diagnostics", {
    path,
  });
}

/** Selects one report and asks Rust to validate its complete canonical form. */
export async function selectAndInspectSessionDiagnostics(): Promise<SessionDiagnosticsInspection | null> {
  const path = await open({
    directory: false,
    multiple: false,
    title: "Verify a session diagnostics report",
    filters: [{ name: "JSON diagnostics", extensions: ["json"] }],
  });
  if (typeof path !== "string") return null;
  return invoke<SessionDiagnosticsInspection>(
    "inspect_session_diagnostics_report",
    { path },
  );
}

/** Builds a sealed, read-only preview of exactly removable rejected entries. */
export async function previewQualityCacheMaintenance(): Promise<QualityCacheMaintenancePreview> {
  return invoke<QualityCacheMaintenancePreview>(
    "preview_quality_cache_maintenance",
  );
}

/** Applies only the sealed plan; native code revalidates every target. */
export async function applyQualityCacheMaintenance(
  planSha256: string,
): Promise<QualityCacheMaintenanceResult> {
  return invoke<QualityCacheMaintenanceResult>(
    "apply_quality_cache_maintenance",
    { request: { planSha256 } },
  );
}
