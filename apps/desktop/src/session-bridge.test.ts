import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { afterEach, describe, expect, it, vi } from "vitest";

import { selectAndImportSession } from "./session-bridge.ts";

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
    };
    vi.mocked(invoke).mockResolvedValue(imported);

    await expect(selectAndImportSession()).resolves.toBe(imported);
    expect(invoke).toHaveBeenCalledWith("import_session_directory", {
      path: "/selected/session",
    });
  });
});
