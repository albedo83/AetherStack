import { invoke } from "@tauri-apps/api/core";

import type { FitsStatistics } from "./model.ts";

/**
 * Calculates deterministic three-pass moments without materializing the full
 * FITS primary array in browser or Rust memory.
 */
export function inspectFitsStatistics(path: string): Promise<FitsStatistics> {
  return invoke<FitsStatistics>("inspect_fits_statistics", {
    request: { path },
  });
}
