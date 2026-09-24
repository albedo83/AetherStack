import "./styles.css";

import { demoReviewModel } from "./demo-data.ts";
import type {
  FitsStatistics,
  FrameRole,
  ReviewFrame,
  ReviewRejectionReason,
  ReviewState,
  ReviewViewModel,
} from "./model.ts";
import {
  estimateFitsPreviewTransform,
  requestFitsPreview,
  type EstimatedDisplayTransform,
  type FitsPreviewRequest,
  type PreviewResource,
} from "./preview-bridge.ts";
import { BoundedPreviewCache, previewCacheKey } from "./preview-cache.ts";
import {
  inspectCfaFrameQuality,
  type FrameQualityResult,
} from "./quality-bridge.ts";
import {
  applyReviewDecision,
  reorderReviewFrames,
  sortReviewFrames,
  undoReviewDecision,
  type ReviewDecisionUpdate,
} from "./review-bridge.ts";
import { mountReviewScreen } from "./review-screen.ts";
import {
  selectAndImportSession,
  type ImportedFrame,
  type ImportedSession,
} from "./session-bridge.ts";
import { inspectFitsStatistics } from "./statistics-bridge.ts";

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
const maximumCachedPreviews = 5;
const maximumCachedPreviewBytes = 32 * 1_024 * 1_024;

let model = demoReviewModel;
let importedSession: ImportedSession | null = null;
let sharedTransform: {
  readonly role: FrameRole;
  readonly value: EstimatedDisplayTransform;
} | null = null;
const previewCache = new BoundedPreviewCache(
  maximumCachedPreviews,
  maximumCachedPreviewBytes,
);
let ephemeralPreviewResource: PreviewResource | null = null;
let previewTicket = 0;
let sortTicket = 0;
let statisticsTicket = 0;
const statisticsCache = new Map<string, FitsStatistics>();
const qualityCache = new Map<string, FrameQualityResult>();
const qualityPending = new Set<string>();
const decisionCache = new Map<
  string,
  Pick<ReviewFrame, "state" | "rejectionReason">
>();
let qualitySessionRevision = 0;
let decisionSessionRevision = 0;
let decisionGeneration = 0;
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
    void setDecision(frameId, state, reason);
  },
  onClearDecision(frameId) {
    void clearDecision(frameId);
  },
  onUndo() {
    void undoDecision();
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
  onOpenStatistics(frameId) {
    void openStatistics(frameId);
  },
  onCloseStatistics() {
    closeStatistics();
  },
  onMeasureQuality(frameId) {
    void measureQuality(frameId);
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
  clearPreviewResources();
  sharedTransform = null;
  previewTicket += 1;
  sortTicket += 1;
  statisticsTicket += 1;
  statisticsCache.clear();
  qualitySessionRevision += 1;
  qualityCache.clear();
  qualityPending.clear();
  decisionSessionRevision += 1;
  decisionGeneration = 0;
  decisionCache.clear();

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
    reviewSessionReady: session.frames.length > 0,
    canUndo: false,
    decisionPending: false,
    sharedStretchLabel: "Reference stretch · resolving",
    preview: null,
    statisticsPanel: closedStatisticsPanel(),
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
  const cachedQuality = qualityCache.get(frame.id);
  const cachedDecision = decisionCache.get(frame.id);
  const qualityAvailable =
    frame.role === "light" && frame.bayerPattern !== null;
  const qualityState = cachedQuality
    ? "ready"
    : qualityPending.has(frame.id)
      ? "loading"
      : qualityAvailable
        ? "idle"
        : "unavailable";
  return {
    id: frame.id,
    label: frame.label,
    sourcePath: frame.path,
    exposureSeconds: frame.exposureSeconds,
    temperatureCelsius: frame.temperatureCelsius,
    classificationWarning: frame.classificationConflict
      ? "Header and directory frame types conflict; the directory role was applied"
      : null,
    bayerPattern: frame.bayerPattern,
    qualityState,
    qualityMessage: cachedQuality
      ? qualityResultMessage(cachedQuality)
      : qualityState === "loading"
        ? "Measuring immutable linear pixels…"
        : qualityAvailable
          ? "Ready for phase-neutral CFA diagnostics"
          : frame.role === "light"
            ? "Blocked · no supported CFA phase"
            : "Quality metrics apply to light frames",
    qualityProfileId: cachedQuality?.profileId ?? null,
    state: cachedDecision?.state ?? "undecided",
    rejectionReason: cachedDecision?.rejectionReason ?? null,
    metrics: cachedQuality
      ? qualityMetrics(cachedQuality)
      : emptyQualityMetrics(),
  };
}

function emptyQualityMetrics(): ReviewFrame["metrics"] {
  return {
    fwhmPixels: null,
    eccentricity: null,
    detectedStars: null,
    background: null,
    noise: null,
  };
}

function qualityMetrics(result: FrameQualityResult): ReviewFrame["metrics"] {
  return {
    fwhmPixels: result.fwhmPixels,
    eccentricity: result.eccentricity,
    detectedStars: result.detectedStars,
    background: result.background,
    noise: result.noise,
  };
}

function qualityResultMessage(result: FrameQualityResult): string {
  return `${result.usableStars.toLocaleString("en-US")} measured stars · ${result.interpretation} · ${result.detectionPlaneAlgorithmId} + ${result.starAlgorithmId} · saturation unclassified · diagnostic only`;
}

async function measureQuality(frameId: string): Promise<void> {
  const frame = model.frames.find((candidate) => candidate.id === frameId);
  if (
    !frame?.sourcePath ||
    !frame.bayerPattern ||
    model.activeRole !== "light" ||
    qualityPending.has(frame.id)
  ) {
    return;
  }

  const cached = qualityCache.get(frame.id);
  if (cached) {
    applyQualityResult(frame.id, cached);
    return;
  }

  const sessionRevision = qualitySessionRevision;
  qualityPending.add(frame.id);
  updateQualityFrame(frame.id, {
    qualityState: "loading",
    qualityMessage: "Measuring immutable linear pixels…",
  });
  try {
    const result = await inspectCfaFrameQuality(
      frame.sourcePath,
      frame.bayerPattern,
    );
    if (sessionRevision !== qualitySessionRevision) return;
    qualityCache.set(frame.id, result);
    applyQualityResult(frame.id, result);
  } catch {
    if (sessionRevision !== qualitySessionRevision) return;
    updateQualityFrame(frame.id, {
      qualityState: "error",
      qualityMessage: "Strict quality diagnostics could not be completed",
      qualityProfileId: null,
      metrics: emptyQualityMetrics(),
    });
  } finally {
    qualityPending.delete(frame.id);
  }
}

function applyQualityResult(frameId: string, result: FrameQualityResult): void {
  updateQualityFrame(frameId, {
    qualityState: "ready",
    qualityMessage: qualityResultMessage(result),
    qualityProfileId: result.profileId,
    metrics: qualityMetrics(result),
  });
}

function updateQualityFrame(
  frameId: string,
  patch: Partial<
    Pick<
      ReviewFrame,
      "qualityState" | "qualityMessage" | "qualityProfileId" | "metrics"
    >
  >,
): void {
  if (!model.frames.some((frame) => frame.id === frameId)) return;
  update({
    ...model,
    frames: model.frames.map((frame) =>
      frame.id === frameId ? { ...frame, ...patch } : frame,
    ),
  });
}

function selectRole(role: FrameRole): void {
  stopBlinkTimer();
  clearPreviewResources();
  sharedTransform = null;
  previewTicket += 1;
  sortTicket += 1;
  statisticsTicket += 1;
  if (!importedSession) {
    if (role === "light") {
      update({
        ...model,
        activeRole: role,
        frames: demoReviewModel.frames,
        selectedFrameId: demoReviewModel.selectedFrameId,
        preview: null,
        statisticsPanel: closedStatisticsPanel(),
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
      statisticsPanel: closedStatisticsPanel(),
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
    statisticsPanel: closedStatisticsPanel(),
  });
  void loadSelectedPreview();
}

function selectFrame(frameId: string): void {
  statisticsTicket += 1;
  releaseEphemeralPreview();
  update({
    ...model,
    selectedFrameId: frameId,
    preview: null,
    statisticsPanel: closedStatisticsPanel(),
  });
  void loadSelectedPreview();
}

async function openStatistics(frameId: string): Promise<void> {
  const frame = model.frames.find((candidate) => candidate.id === frameId);
  if (!frame?.sourcePath) return;

  const cached = statisticsCache.get(frame.id);
  if (cached) {
    update({
      ...model,
      statisticsPanel: {
        open: true,
        frameId: frame.id,
        frameLabel: frame.label,
        state: "ready",
        statistics: cached,
        message: null,
      },
    });
    return;
  }

  const ticket = ++statisticsTicket;
  update({
    ...model,
    statisticsPanel: {
      open: true,
      frameId: frame.id,
      frameLabel: frame.label,
      state: "loading",
      statistics: null,
      message: "Reading the primary array in three deterministic passes…",
    },
  });
  try {
    const statistics = await inspectFitsStatistics(frame.sourcePath);
    if (ticket !== statisticsTicket || model.selectedFrameId !== frame.id)
      return;
    statisticsCache.set(frame.id, statistics);
    update({
      ...model,
      statisticsPanel: {
        open: true,
        frameId: frame.id,
        frameLabel: frame.label,
        state: "ready",
        statistics,
        message: null,
      },
    });
  } catch {
    if (ticket !== statisticsTicket || model.selectedFrameId !== frame.id)
      return;
    update({
      ...model,
      statisticsPanel: {
        open: true,
        frameId: frame.id,
        frameLabel: frame.label,
        state: "error",
        statistics: null,
        message:
          "Exact FITS statistics could not be calculated for this frame.",
      },
    });
  }
}

function closeStatistics(): void {
  statisticsTicket += 1;
  update({ ...model, statisticsPanel: closedStatisticsPanel() });
}

function closedStatisticsPanel(): ReviewViewModel["statisticsPanel"] {
  return {
    open: false,
    frameId: null,
    frameLabel: null,
    state: "idle",
    statistics: null,
    message: null,
  };
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

    const request: FitsPreviewRequest = {
      frameId: frame.id,
      path: frame.sourcePath,
      plane: 0,
      ...previewBounds,
      blackPoint: transform.blackPoint,
      whitePoint: transform.whitePoint,
      midtone: transform.midtone,
      transfer: { kind: "midtones" },
    };
    const cacheKey = previewCacheKey(request, transform.algorithmId);
    const cached = previewCache.get(cacheKey);
    if (cached) {
      if (ticket !== previewTicket) return;
      releaseEphemeralPreview();
      update({ ...model, preview: cached.preview });
      return;
    }

    const resource = await requestFitsPreview(request);
    if (ticket !== previewTicket) {
      resource.revoke();
      return;
    }
    releaseEphemeralPreview();
    if (!previewCache.put(cacheKey, resource)) {
      ephemeralPreviewResource = resource;
    }
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

function releaseEphemeralPreview(): void {
  ephemeralPreviewResource?.revoke();
  ephemeralPreviewResource = null;
}

function clearPreviewResources(): void {
  releaseEphemeralPreview();
  previewCache.clear();
}

function disposeRuntimeResources(): void {
  previewTicket += 1;
  sortTicket += 1;
  statisticsTicket += 1;
  qualitySessionRevision += 1;
  decisionSessionRevision += 1;
  stopBlinkTimer();
  clearPreviewResources();
}

function update(next: ReviewViewModel): void {
  model = next;
  screen.update(model);
}

async function setDecision(
  frameId: string,
  state: Exclude<ReviewState, "undecided">,
  reason: ReviewRejectionReason | null,
): Promise<void> {
  if (!importedSession || model.decisionPending) return;
  if (state === "accepted") {
    await runDecisionTransaction(() =>
      applyReviewDecision(frameId, { kind: "accept" }),
    );
    return;
  }
  if (reason === null) return;
  await runDecisionTransaction(() =>
    applyReviewDecision(frameId, { kind: "reject", reason }),
  );
}

async function clearDecision(frameId: string): Promise<void> {
  if (!importedSession || model.decisionPending) return;
  await runDecisionTransaction(() =>
    applyReviewDecision(frameId, { kind: "clear" }),
  );
}

async function undoDecision(): Promise<void> {
  if (!importedSession || model.decisionPending || !model.canUndo) return;
  await runDecisionTransaction(undoReviewDecision);
}

async function runDecisionTransaction(
  transaction: () => Promise<ReviewDecisionUpdate>,
): Promise<void> {
  const sessionRevision = decisionSessionRevision;
  update({ ...model, decisionPending: true });
  try {
    const result = await transaction();
    if (sessionRevision !== decisionSessionRevision) return;
    applyDecisionUpdate(result);
  } catch {
    if (sessionRevision !== decisionSessionRevision) return;
    update({
      ...model,
      decisionPending: false,
      sessionStatus: {
        tone: "error",
        label: "Review transaction failed · state preserved",
      },
    });
  }
}

function applyDecisionUpdate(result: ReviewDecisionUpdate): void {
  if (result.generation < decisionGeneration) {
    update({ ...model, decisionPending: false });
    return;
  }
  decisionGeneration = result.generation;
  for (const change of result.changes) {
    decisionCache.set(change.frameId, {
      state: change.state,
      rejectionReason: change.rejectionReason,
    });
  }
  update({
    ...model,
    canUndo: result.canUndo,
    decisionPending: false,
    frames: model.frames.map((frame) => {
      const decision = decisionCache.get(frame.id);
      return decision ? { ...frame, ...decision } : frame;
    }),
  });
}
