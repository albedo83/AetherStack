import { invoke } from "@tauri-apps/api/core";

import type { FramePreview } from "./model.ts";

export type PreviewTransfer =
  | { readonly kind: "linear" }
  | { readonly kind: "midtones" }
  | { readonly kind: "asinh"; readonly softness: number };

/** Explicit primary-array interpretation; RGB always means planes R, G, B. */
export type FitsPreviewContent =
  | { readonly kind: "scalar"; readonly plane: number }
  | { readonly kind: "rgb" };

export interface FitsPreviewRequest {
  readonly frameId: string;
  readonly path: string;
  readonly content: FitsPreviewContent;
  readonly maximumWidth: number;
  readonly maximumHeight: number;
  readonly blackPoint: number;
  readonly whitePoint: number;
  readonly midtone: number;
  readonly transfer: PreviewTransfer;
}

export interface FitsPreviewEstimateRequest {
  readonly path: string;
  readonly content: FitsPreviewContent;
  readonly maximumWidth: number;
  readonly maximumHeight: number;
}

export interface EstimatedDisplayTransform {
  readonly algorithmId: string;
  readonly blackPoint: number;
  readonly whitePoint: number;
  readonly midtone: number;
  readonly finiteSamples: number;
  readonly median: number;
  readonly scaledMad: number;
  readonly highQuantile: number;
}

export interface PreviewResource {
  readonly preview: FramePreview;
  /** Exact encoded PNG payload retained by the browser object URL. */
  readonly byteLength: number;
  /** Releases the browser-side PNG URL without affecting Rust cache state. */
  readonly revoke: () => void;
}

/** Resolves one auditable reference stretch in Rust for shared Blink display. */
export function estimateFitsPreviewTransform(
  request: FitsPreviewEstimateRequest,
): Promise<EstimatedDisplayTransform> {
  return invoke<EstimatedDisplayTransform>("estimate_fits_preview_transform", {
    request,
  });
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
    byteLength: png.byteLength,
    revoke() {
      if (revoked) return;
      URL.revokeObjectURL(url);
      revoked = true;
    },
  };
}
