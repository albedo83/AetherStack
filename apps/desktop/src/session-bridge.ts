import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";

import type { BayerPattern, FrameRole } from "./model.ts";
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
}

export interface ImportedSessionStatus {
  readonly tone: "ready" | "warning";
  readonly label: string;
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
