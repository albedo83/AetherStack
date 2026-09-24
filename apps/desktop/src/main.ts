import "./styles.css";

import { demoReviewModel } from "./demo-data.ts";
import type {
  FrameRole,
  ReviewFrame,
  ReviewState,
  ReviewViewModel,
} from "./model.ts";
import {
  estimateFitsPreviewTransform,
  requestFitsPreview,
  type EstimatedDisplayTransform,
  type PreviewResource,
} from "./preview-bridge.ts";
import { reorderReviewFrames, sortReviewFrames } from "./review-bridge.ts";
import { mountReviewScreen } from "./review-screen.ts";
import {
  selectAndImportSession,
  type ImportedFrame,
  type ImportedSession,
} from "./session-bridge.ts";

const root = document.querySelector<HTMLElement>("#app");
if (!root) throw new Error("AetherStack desktop root is missing");

const roleOrder: readonly FrameRole[] = ["light", "flat", "dark", "bias"];
const roleLabels: Readonly<Record<FrameRole, string>> = {
  bias: "Bias",
  dark: "Darks",
  flat: "Flats",
  light: "Lights",
};
const previewBounds = { maximumWidth: 1_600, maximumHeight: 1_200 } as const;

let model = demoReviewModel;
let importedSession: ImportedSession | null = null;
let sharedTransform: {
  readonly role: FrameRole;
  readonly value: EstimatedDisplayTransform;
} | null = null;
let previewResource: PreviewResource | null = null;
let previewTicket = 0;
let sortTicket = 0;
let blinkTimer: number | null = null;

const screen = mountReviewScreen(root, model, {
  onImportSession() {
    void importSession();
  },
  onSelectRole(role) {
    selectRole(role);
  },
  onSelectFrame(frameId) {
    selectFrame(frameId);
  },
  onSort(field, direction) {
    void applySort(field, direction);
  },
  onSetDecision(frameId, state, reason) {
    updateDecision(frameId, state, reason);
  },
  onClearDecision(frameId) {
    updateDecision(frameId, "undecided", null);
  },
  onUndo() {
    // Transactional undo is already implemented by aether-review and will be
    // exposed with the review-model adapter rather than duplicated here.
  },
  onSetPlaying(playing) {
    setPlaying(playing);
  },
  onRequestStep(direction) {
    step(direction);
  },
  onSetViewerScale(scale) {
    update({ ...model, viewerScale: scale });
  },
});

window.addEventListener("beforeunload", disposeRuntimeResources, {
  once: true,
});

async function importSession(): Promise<void> {
  const previousStatus = model.sessionStatus;
  update({
    ...model,
    sessionStatus: { tone: "busy", label: "Scanning FITS sources" },
  });
  try {
    const imported = await selectAndImportSession();
    if (!imported) {
      update({ ...model, sessionStatus: previousStatus });
      return;
    }
    installImportedSession(imported);
  } catch {
    update({
      ...model,
      sessionStatus: {
        tone: "error",
        label: "Import failed · open Diagnostics",
      },
    });
  }
}

function installImportedSession(session: ImportedSession): void {
  importedSession = session;
  stopBlinkTimer();
  releasePreview();
  sharedTransform = null;
  previewTicket += 1;
  sortTicket += 1;

  const roles = (["bias", "dark", "flat", "light"] as const).map((role) => ({
    role,
    label: roleLabels[role],
    count: session.frames.filter((frame) => frame.role === role).length,
    unresolved: 0,
  }));
  const activeRole =
    roleOrder.find((role) => roles.find((item) => item.role === role)?.count) ??
    "light";
  const frames = reviewFramesForRole(session, activeRole);
  const issueCount =
    session.classificationConflicts +
    session.recoverableFailures.length +
    session.unassignedSources.length;
  update({
    ...model,
    sessionName: session.name,
    sessionStatus:
      issueCount === 0
        ? {
            tone: "ready",
            label: `${session.frames.length} FITS verified`,
          }
        : {
            tone: "warning",
            label: `${issueCount} import issue${issueCount === 1 ? "" : "s"}`,
          },
    roles,
    activeRole,
    frames,
    selectedFrameId: frames[0]?.id ?? null,
    playing: false,
    sharedStretchLabel: "Reference stretch · resolving",
    preview: null,
  });
  void loadSelectedPreview();
}

function reviewFramesForRole(
  session: ImportedSession,
  role: FrameRole,
): readonly ReviewFrame[] {
  return session.frames
    .filter((frame) => frame.role === role)
    .map(importedReviewFrame);
}

function importedReviewFrame(frame: ImportedFrame): ReviewFrame {
  return {
    id: frame.id,
    label: frame.label,
    sourcePath: frame.path,
    exposureSeconds: frame.exposureSeconds,
    temperatureCelsius: frame.temperatureCelsius,
    classificationWarning: frame.classificationConflict
      ? "Header and directory frame types conflict; the directory role was applied"
      : null,
    state: "undecided",
    rejectionReason: null,
    metrics: {
      fwhmPixels: null,
      eccentricity: null,
      detectedStars: null,
      background: null,
      noise: null,
    },
  };
}

function selectRole(role: FrameRole): void {
  stopBlinkTimer();
  releasePreview();
  sharedTransform = null;
  previewTicket += 1;
  sortTicket += 1;
  if (!importedSession) {
    if (role === "light") {
      update({
        ...model,
        activeRole: role,
        frames: demoReviewModel.frames,
        selectedFrameId: demoReviewModel.selectedFrameId,
        preview: null,
      });
      return;
    }
    update({
      ...model,
      activeRole: role,
      frames: [],
      selectedFrameId: null,
      playing: false,
      preview: null,
    });
    return;
  }

  const frames = reviewFramesForRole(importedSession, role);
  update({
    ...model,
    activeRole: role,
    frames,
    selectedFrameId: frames[0]?.id ?? null,
    playing: false,
    sharedStretchLabel: "Reference stretch · resolving",
    preview: null,
  });
  void loadSelectedPreview();
}

function selectFrame(frameId: string): void {
  releasePreview();
  update({ ...model, selectedFrameId: frameId, preview: null });
  void loadSelectedPreview();
}

async function loadSelectedPreview(): Promise<void> {
  const frame = model.frames.find(
    (candidate) => candidate.id === model.selectedFrameId,
  );
  if (!frame?.sourcePath) return;

  const ticket = ++previewTicket;
  try {
    let transform =
      sharedTransform?.role === model.activeRole ? sharedTransform.value : null;
    if (!transform) {
      transform = await estimateFitsPreviewTransform({
        path: frame.sourcePath,
        plane: 0,
        ...previewBounds,
      });
      if (ticket !== previewTicket) return;
      sharedTransform = { role: model.activeRole, value: transform };
      update({
        ...model,
        sharedStretchLabel: "Reference stretch · locked",
      });
    }

    const resource = await requestFitsPreview({
      frameId: frame.id,
      path: frame.sourcePath,
      plane: 0,
      ...previewBounds,
      blackPoint: transform.blackPoint,
      whitePoint: transform.whitePoint,
      midtone: transform.midtone,
      transfer: { kind: "midtones" },
    });
    if (ticket !== previewTicket) {
      resource.revoke();
      return;
    }
    releasePreview();
    previewResource = resource;
    update({ ...model, preview: resource.preview });
  } catch {
    if (ticket !== previewTicket) return;
    update({
      ...model,
      sharedStretchLabel: "Preview unavailable · inspect Diagnostics",
      preview: null,
    });
  }
}

async function applySort(
  field: Parameters<typeof sortReviewFrames>[1],
  direction: Parameters<typeof sortReviewFrames>[2],
): Promise<void> {
  if (model.frames.length === 0) return;
  const frames = model.frames;
  const ticket = ++sortTicket;
  try {
    const identities = await sortReviewFrames(frames, field, direction);
    if (ticket !== sortTicket) return;
    const sorted = reorderReviewFrames(model.frames, identities);
    if (!sorted) return;
    update({ ...model, frames: sorted });
  } catch {
    if (ticket !== sortTicket) return;
    update({
      ...model,
      sessionStatus: {
        tone: "error",
        label: "Sort failed · processing order preserved",
      },
    });
  }
}

function setPlaying(playing: boolean): void {
  stopBlinkTimer();
  update({ ...model, playing });
  if (playing && model.frames.length > 1) {
    blinkTimer = window.setInterval(() => step("forward"), 900);
  }
}

function step(direction: "backward" | "forward"): void {
  const current = model.frames.findIndex(
    (frame) => frame.id === model.selectedFrameId,
  );
  if (current < 0 || model.frames.length < 2) return;
  const offset = direction === "forward" ? 1 : -1;
  const next = (current + offset + model.frames.length) % model.frames.length;
  const frame = model.frames[next];
  if (frame) selectFrame(frame.id);
}

function stopBlinkTimer(): void {
  if (blinkTimer !== null) {
    window.clearInterval(blinkTimer);
    blinkTimer = null;
  }
  if (model.playing) model = { ...model, playing: false };
}

function releasePreview(): void {
  previewResource?.revoke();
  previewResource = null;
}

function disposeRuntimeResources(): void {
  previewTicket += 1;
  sortTicket += 1;
  stopBlinkTimer();
  releasePreview();
}

function update(next: ReviewViewModel): void {
  model = next;
  screen.update(model);
}

function updateDecision(
  frameId: string,
  state: ReviewState,
  reason: string | null,
): void {
  update({
    ...model,
    frames: model.frames.map((frame) =>
      frame.id === frameId
        ? { ...frame, state, rejectionReason: reason }
        : frame,
    ),
  });
}
