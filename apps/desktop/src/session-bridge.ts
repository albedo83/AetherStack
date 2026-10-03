import { invoke } from "@tauri-apps/api/core";
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

export interface ImportedSession {
  readonly name: string;
  readonly rootPath: string;
  readonly frames: readonly ImportedFrame[];
  readonly filesConsidered: number;
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
export async function selectAndImportSession(): Promise<ImportedSession | null> {
  const path = await open({
    directory: true,
    multiple: false,
    title: "Import an astrophotography session",
  });
  if (typeof path !== "string") return null;
  return invoke<ImportedSession>("import_session_directory", { path });
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
