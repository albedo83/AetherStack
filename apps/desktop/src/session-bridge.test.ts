import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { afterEach, describe, expect, it, vi } from "vitest";

import {
  importedSessionStatus,
  selectAndImportSession,
  type ImportedSession,
} from "./session-bridge.ts";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));

afterEach(() => {
  vi.clearAllMocks();
});

describe("native session bridge", () => {
  it("does not scan when the native directory dialog is cancelled", async () => {
    vi.mocked(open).mockResolvedValue(null);

    await expect(selectAndImportSession()).resolves.toBeNull();
    expect(invoke).not.toHaveBeenCalled();
  });

  it("passes the exact selected directory to the bounded Rust scanner", async () => {
    vi.mocked(open).mockResolvedValue("/selected/session");
    const imported = {
      name: "session",
      rootPath: "/selected/session",
      frames: [],
      filesConsidered: 0,
      classificationConflicts: 0,
      recoverableFailures: [],
      unassignedSources: [],
      qualityEvidenceRestored: 0,
      qualityEvidenceMissing: 0,
      qualityEvidenceRejected: 0,
    };
    vi.mocked(invoke).mockResolvedValue(imported);

    await expect(selectAndImportSession()).resolves.toBe(imported);
    expect(invoke).toHaveBeenCalledWith("import_session_directory", {
      path: "/selected/session",
    });
  });

  it("reports normal cache misses without degrading a verified import", () => {
    const session = importedSessionFixture({
      qualityEvidenceRestored: 3,
      qualityEvidenceMissing: 7,
    });

    expect(importedSessionStatus(session)).toEqual({
      tone: "ready",
      label:
        "0 FITS verified · quality cache 3 restored · 7 missing · 0 rejected",
    });
  });

  it("warns about rejected cache evidence while preserving every count", () => {
    const session = importedSessionFixture({
      classificationConflicts: 1,
      qualityEvidenceRestored: 2,
      qualityEvidenceMissing: 4,
      qualityEvidenceRejected: 1,
    });

    expect(importedSessionStatus(session)).toEqual({
      tone: "warning",
      label:
        "1 import issue · quality cache 2 restored · 4 missing · 1 rejected",
    });
  });
});

function importedSessionFixture(
  overrides: Partial<ImportedSession> = {},
): ImportedSession {
  return {
    name: "session",
    rootPath: "/selected/session",
    frames: [],
    filesConsidered: 0,
    classificationConflicts: 0,
    recoverableFailures: [],
    unassignedSources: [],
    qualityEvidenceRestored: 0,
    qualityEvidenceMissing: 0,
    qualityEvidenceRejected: 0,
    ...overrides,
  };
}
