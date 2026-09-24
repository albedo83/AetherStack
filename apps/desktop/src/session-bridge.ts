import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";

import type { BayerPattern, FrameRole } from "./model.ts";

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
