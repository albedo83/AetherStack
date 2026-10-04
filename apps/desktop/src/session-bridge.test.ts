import { invoke } from "@tauri-apps/api/core";
import { open, save } from "@tauri-apps/plugin-dialog";
import { afterEach, describe, expect, it, vi } from "vitest";

import {
  applyQualityCacheMaintenance,
  importedSessionDiagnostics,
  importedSessionStatus,
  previewQualityCacheMaintenance,
  exportSessionDiagnostics,
  selectAndInspectSessionDiagnostics,
  selectAndImportSession,
  type ImportedSession,
} from "./session-bridge.ts";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: vi.fn(),
  save: vi.fn(),
}));

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
      fingerprintedSourceBytes: 0,
      scanElapsedMilliseconds: 0,
      classificationConflicts: 0,
      recoverableFailures: [],
      unassignedSources: [],
      qualityEvidenceRestored: 0,
      qualityEvidenceMissing: 0,
      qualityEvidenceRejected: 0,
      qualityEvidenceRejections: [],
    };
    vi.mocked(invoke).mockResolvedValue(imported);

    await expect(selectAndImportSession()).resolves.toBe(imported);
    expect(invoke).toHaveBeenCalledWith("import_session_directory", {
      path: "/selected/session",
    });
  });

  it("exports only after a JSON destination is selected", async () => {
    vi.mocked(save).mockResolvedValue("/reports/session.json");
    const exported = {
      path: "/reports/session.json",
      reportSha256: "a".repeat(64),
      itemCount: 4,
    };
    vi.mocked(invoke).mockResolvedValue(exported);

    await expect(exportSessionDiagnostics()).resolves.toBe(exported);
    expect(invoke).toHaveBeenCalledWith("export_session_diagnostics", {
      path: "/reports/session.json",
    });
  });

  it("does not invoke Rust when diagnostics export is cancelled", async () => {
    vi.mocked(save).mockResolvedValue(null);

    await expect(exportSessionDiagnostics()).resolves.toBeNull();
    expect(invoke).not.toHaveBeenCalled();
  });

  it("passes only the selected report path to native verification", async () => {
    vi.mocked(open).mockResolvedValue("/reports/session.json");
    const inspection = {
      schemaVersion: 1,
      algorithmId: "aetherstack-session-diagnostics-v1",
      manifestSha256: "b".repeat(64),
      reportSha256: "a".repeat(64),
      itemCount: 4,
    };
    vi.mocked(invoke).mockResolvedValue(inspection);

    await expect(selectAndInspectSessionDiagnostics()).resolves.toBe(
      inspection,
    );
    expect(invoke).toHaveBeenCalledWith("inspect_session_diagnostics_report", {
      path: "/reports/session.json",
    });
  });

  it("requests cache maintenance preview without browser-supplied targets", async () => {
    const preview = {
      algorithmId: "quality-cache-maintenance-preview-v1",
      planSha256: "c".repeat(64),
      eligibleCount: 1,
      blockedCount: 0,
      totalFileBytes: 4_096,
      items: [],
      blockedItems: [],
    };
    vi.mocked(invoke).mockResolvedValue(preview);

    await expect(previewQualityCacheMaintenance()).resolves.toBe(preview);
    expect(invoke).toHaveBeenCalledWith("preview_quality_cache_maintenance");
  });

  it("applies cache maintenance using only the sealed plan digest", async () => {
    const result = { removedCount: 1, removedBytes: 4_096, skippedCount: 0 };
    vi.mocked(invoke).mockResolvedValue(result);

    await expect(applyQualityCacheMaintenance("c".repeat(64))).resolves.toBe(
      result,
    );
    expect(invoke).toHaveBeenCalledWith("apply_quality_cache_maintenance", {
      request: { planSha256: "c".repeat(64) },
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

  it("builds bounded relative-source evidence with stable categories", () => {
    const frame = {
      id: "a".repeat(64),
      role: "light" as const,
      label: "light.fits",
      relativePath: "LIGHTS/light.fits",
      path: "/private/session/LIGHTS/light.fits",
      exposureSeconds: 60,
      temperatureCelsius: -5,
      camera: "ASI294MC Pro",
      filter: null,
      bayerPattern: "rggb" as const,
      axes: [4144, 2822],
      fitsDiagnosticCount: 1,
      classificationConflict: true,
      quality: null,
    };
    const session = importedSessionFixture({
      frames: [frame],
      classificationConflicts: 1,
      recoverableFailures: [
        { relativePath: "DARKS/dark.fits", code: "fits_header_invalid" },
      ],
      unassignedSources: ["FLATS/flat.fits"],
      qualityEvidenceRejected: 1,
      qualityEvidenceRejections: [
        {
          relativePath: "LIGHTS/cache.fits",
          code: "quality_cache_artifact_invalid",
        },
      ],
    });

    const diagnostics = importedSessionDiagnostics(session);

    expect(diagnostics.items).toEqual([
      {
        category: "classification",
        source: "LIGHTS/light.fits",
        code: "classification_conflict",
      },
      {
        category: "fits",
        source: "DARKS/dark.fits",
        code: "fits_header_invalid",
      },
      {
        category: "grouping",
        source: "FLATS/flat.fits",
        code: "session_source_unassigned",
      },
      {
        category: "quality_cache",
        source: "LIGHTS/cache.fits",
        code: "quality_cache_artifact_invalid",
      },
    ]);
    expect(JSON.stringify(diagnostics)).not.toContain("/private/session");
  });

  it("bounds source evidence independently from aggregate diagnostics", () => {
    const recoverableFailures = Array.from({ length: 101 }, (_, index) => ({
      relativePath: `LIGHTS/frame-${index}.fits`,
      code: "fits_header_invalid",
    }));
    const diagnostics = importedSessionDiagnostics(
      importedSessionFixture({ recoverableFailures }),
    );

    expect(diagnostics.recoverableFailures).toBe(101);
    expect(diagnostics.items).toHaveLength(100);
    expect(diagnostics.omittedItems).toBe(1);
    expect(diagnostics.items.at(-1)?.source).toBe("LIGHTS/frame-99.fits");
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
    fingerprintedSourceBytes: 0,
    scanElapsedMilliseconds: 0,
    classificationConflicts: 0,
    recoverableFailures: [],
    unassignedSources: [],
    qualityEvidenceRestored: 0,
    qualityEvidenceMissing: 0,
    qualityEvidenceRejected: 0,
    qualityEvidenceRejections: [],
    ...overrides,
  };
}
