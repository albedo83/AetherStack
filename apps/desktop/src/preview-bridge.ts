import { invoke } from "@tauri-apps/api/core";

import type { FramePreview } from "./model.ts";

export type PreviewTransfer =
  | { readonly kind: "linear" }
  | { readonly kind: "midtones" }
  | { readonly kind: "asinh"; readonly softness: number };

export interface FitsPreviewRequest {
  readonly frameId: string;
  readonly path: string;
  readonly plane: number;
  readonly maximumWidth: number;
  readonly maximumHeight: number;
  readonly blackPoint: number;
  readonly whitePoint: number;
  readonly midtone: number;
  readonly transfer: PreviewTransfer;
}

export interface PreviewResource {
  readonly preview: FramePreview;
  /** Releases the browser-side PNG URL without affecting Rust cache state. */
  readonly revoke: () => void;
}

/**
 * Requests a bounded PNG from the Rust preview engine.
 *
 * The stable frame identity stays on the frontend side of the IPC boundary and
 * is attached only after the raw response completes. The Review presenter then
 * verifies that identity before displaying the image, so an older asynchronous
 * response cannot replace the currently selected frame during Blink playback.
 */
export async function requestFitsPreview(
  request: FitsPreviewRequest,
): Promise<PreviewResource> {
  const { frameId, ...nativeRequest } = request;
  const png = await invoke<ArrayBuffer>("render_fits_preview", {
    request: nativeRequest,
  });
  const url = URL.createObjectURL(new Blob([png], { type: "image/png" }));
  let revoked = false;
  return {
    preview: { frameId, url },
    revoke() {
      if (revoked) return;
      URL.revokeObjectURL(url);
      revoked = true;
    },
  };
}
