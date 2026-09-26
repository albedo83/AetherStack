import "./styles.css";

import {
  cancelLightPlan,
  cancelMasterPlan,
  executeLightPlan,
  executeMasterPlan,
  previewMasterPlan,
  selectLightOutputDirectory,
  selectMasterOutputDirectory,
  type ExecutedCalibratedLightFrame,
  type LightExecutionProgress,
  type MasterExecutionProgress,
  type MasterPlanSettings,
} from "./calibration-bridge.ts";
import {
  bindCalibratedLightFrames,
  frameArtifactKey,
} from "./calibrated-review.ts";
import { demoReviewModel } from "./demo-data.ts";
import type {
  FitsStatistics,
  FrameRole,
  LightFrameView,
  ReviewFrame,
  ReviewRejectionReason,
  ReviewState,
  ReviewViewModel,
  WorkspaceView,
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
  adjacentPreviewFrames,
  PreviewPrefetchCoordinator,
} from "./preview-prefetch.ts";
import {
  inspectCfaFrameQuality,
  inspectRgbFrameQuality,
  type FrameQualityResult,
} from "./quality-bridge.ts";
import { runSerialBatch } from "./quality-batch.ts";
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
// One frame in each direction warms Blink without saturating the FITS worker pool.
const maximumAdjacentPrefetches = 2;

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
const previewPrefetch = new PreviewPrefetchCoordinator<PreviewResource>();
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
let qualityBatchTicket = 0;
let decisionSessionRevision = 0;
let decisionGeneration = 0;
let blinkTimer: number | null = null;
let masterPlanTicket = 0;
let masterExecutionTicket = 0;
let lightExecutionTicket = 0;

const screen = mountReviewScreen(root, model, {
  onSelectWorkspace(workspace) {
    selectWorkspace(workspace);
  },
  onUpdateCalibrationSettings(settings) {
    updateCalibrationSettings(settings);
  },
  onUpdateLightOutputMode(outputMode) {
    if (isActiveExecutionState(model.calibration.lightExecution.state)) return;
    update({
      ...model,
      calibration: {
        ...model.calibration,
        lightSettings: {
          ...model.calibration.lightSettings,
          outputMode,
        },
      },
    });
  },
  onRefreshMasterPlan() {
    void refreshMasterPlan();
  },
  onExecuteMasterPlan() {
    void executeMasters();
  },
  onCancelMasterPlan() {
    void cancelMasters();
  },
  onExecuteLightPlan() {
    void executeLights();
  },
  onCancelLightPlan() {
    void cancelLights();
  },
  onImportSession() {
    void importSession();
  },
  onSelectRole(role) {
    selectRole(role);
  },
  onSelectLightFrameView(view) {
    selectLightFrameView(view);
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
    if (!model.qualityBatchRunning) void measureQuality(frameId);
  },
  onMeasureAllQuality() {
    void measureAllQuality();
  },
});

window.addEventListener("beforeunload", disposeRuntimeResources, {
  once: true,
});

async function importSession(): Promise<void> {
  if (
    model.calibration.execution.state === "running" ||
    model.calibration.execution.state === "cancelling" ||
    isActiveExecutionState(model.calibration.lightExecution.state)
  ) {
    return;
  }
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
  qualityBatchTicket += 1;
  qualityCache.clear();
  qualityPending.clear();
  decisionSessionRevision += 1;
  decisionGeneration = 0;
  decisionCache.clear();
  masterPlanTicket += 1;
  masterExecutionTicket += 1;

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
    lightFrameView: "raw",
    frames,
    selectedFrameId: frames[0]?.id ?? null,
    playing: false,
    reviewSessionReady: session.frames.length > 0,
    canUndo: false,
    decisionPending: false,
    qualityBatchRunning: false,
    qualityBatchProgress: null,
    sharedStretchLabel: "Reference stretch · resolving",
    preview: null,
    statisticsPanel: closedStatisticsPanel(),
    calibration: {
      ...model.calibration,
      state: "loading",
      plan: null,
      message: "Building native calibration graph…",
      execution: {
        state: "idle",
        outputDirectory: null,
        progress: null,
        result: null,
        message: "Choose an output directory when the plan is ready",
      },
      lightExecution: {
        state: "idle",
        masterDirectory: null,
        outputDirectory: null,
        progress: null,
        result: null,
        message: "Build the reviewed masters before integrating Lights",
      },
    },
  });
  void loadSelectedPreview();
  void refreshMasterPlan();
}

function selectWorkspace(workspace: WorkspaceView): void {
  if (workspace === model.activeWorkspace) return;
  if (workspace !== "frames") setPlaying(false);
  update({ ...model, activeWorkspace: workspace });
  if (workspace === "frames" && model.preview === null) {
    void loadSelectedPreview();
  }
}

function updateCalibrationSettings(settings: MasterPlanSettings): void {
  if (
    model.calibration.execution.state === "running" ||
    model.calibration.execution.state === "cancelling" ||
    model.calibration.lightExecution.state === "running" ||
    model.calibration.lightExecution.state === "cancelling"
  ) {
    return;
  }
  update({
    ...model,
    calibration: {
      ...model.calibration,
      settings,
      state: importedSession ? "loading" : "idle",
      message: importedSession
        ? "Rebuilding native calibration graph…"
        : "Import a session to build a calibration graph",
      execution: {
        state: "idle",
        outputDirectory: null,
        progress: null,
        result: null,
        message: "Plan changed · choose an output directory after validation",
      },
      lightExecution: {
        state: "idle",
        masterDirectory: null,
        outputDirectory: null,
        progress: null,
        result: null,
        message: "Plan changed · rebuild masters before integrating Lights",
      },
    },
  });
  if (importedSession) void refreshMasterPlan();
}

async function executeMasters(): Promise<void> {
  const plan = model.calibration.plan;
  const execution = model.calibration.execution;
  if (
    !importedSession ||
    !plan?.ready ||
    execution.state === "running" ||
    execution.state === "cancelling" ||
    model.calibration.lightExecution.state === "running" ||
    model.calibration.lightExecution.state === "cancelling"
  ) {
    return;
  }
  const selectedSession = importedSession;
  const selectedPlanSha256 = plan.planSha256;
  const outputDirectory = await selectMasterOutputDirectory();
  if (!outputDirectory) return;
  // The native picker is asynchronous. Do not execute an obsolete plan if the
  // user imported another session or changed matching controls while it was
  // open; the refreshed plan must be reviewed first.
  if (
    importedSession !== selectedSession ||
    model.calibration.plan?.planSha256 !== selectedPlanSha256 ||
    model.calibration.execution.state === "running" ||
    model.calibration.execution.state === "cancelling" ||
    isActiveExecutionState(model.calibration.lightExecution.state)
  ) {
    return;
  }
  const ticket = ++masterExecutionTicket;
  update({
    ...model,
    calibration: {
      ...model.calibration,
      execution: {
        state: "running",
        outputDirectory,
        progress: null,
        result: null,
        message: "Preparing transactional master build…",
      },
    },
  });
  const onProgress = (progress: MasterExecutionProgress): void => {
    if (ticket !== masterExecutionTicket) return;
    const state = model.calibration.execution.state;
    if (state !== "running" && state !== "cancelling") return;
    update({
      ...model,
      calibration: {
        ...model.calibration,
        execution: {
          ...model.calibration.execution,
          state,
          progress,
          message: masterProgressMessage(progress),
        },
      },
    });
  };
  try {
    const result = await executeMasterPlan(
      outputDirectory,
      model.calibration.settings,
      model.calibration.buildSettings,
      plan,
      onProgress,
    );
    if (ticket !== masterExecutionTicket) return;
    update({
      ...model,
      calibration: {
        ...model.calibration,
        execution: {
          state: "completed",
          outputDirectory,
          progress: model.calibration.execution.progress,
          result,
          message: `${result.products.length} master${result.products.length === 1 ? "" : "s"} published · peak ${formatMemory(result.peakReservedBytes)}`,
        },
        lightExecution: {
          state: "idle",
          masterDirectory: outputDirectory,
          outputDirectory: null,
          progress: null,
          result: null,
          message:
            "Verified masters ready · choose an output directory for Lights",
        },
      },
    });
  } catch (error) {
    if (ticket !== masterExecutionTicket) return;
    const cancelled = nativeErrorCode(error) === "master_execution_cancelled";
    update({
      ...model,
      calibration: {
        ...model.calibration,
        execution: {
          ...model.calibration.execution,
          state: cancelled ? "idle" : "error",
          result: null,
          message: cancelled
            ? "Build cancelled · no partial product set published"
            : "Master build failed safely · no existing output was modified",
        },
      },
    });
  }
}

async function cancelMasters(): Promise<void> {
  if (model.calibration.execution.state !== "running") return;
  update({
    ...model,
    calibration: {
      ...model.calibration,
      execution: {
        ...model.calibration.execution,
        state: "cancelling",
        message: "Cancellation requested · finishing the current bounded unit…",
      },
    },
  });
  try {
    await cancelMasterPlan();
  } catch {
    update({
      ...model,
      calibration: {
        ...model.calibration,
        execution: {
          ...model.calibration.execution,
          state: "error",
          message: "Cancellation request failed · native task state is unknown",
        },
      },
    });
  }
}

async function executeLights(): Promise<void> {
  const plan = model.calibration.plan;
  const lightPlan = plan?.lightPlan;
  const execution = model.calibration.lightExecution;
  if (
    !importedSession ||
    !plan?.ready ||
    !lightPlan?.ready ||
    lightPlan.products.length === 0 ||
    !execution.masterDirectory ||
    execution.state === "running" ||
    execution.state === "cancelling" ||
    model.calibration.execution.state === "running" ||
    model.calibration.execution.state === "cancelling"
  ) {
    return;
  }
  const selectedSession = importedSession;
  const selectedMasterPlanSha256 = plan.planSha256;
  const selectedLightPlanSha256 = lightPlan.planSha256;
  const masterDirectory = execution.masterDirectory;
  const outputDirectory = await selectLightOutputDirectory();
  if (!outputDirectory) return;
  if (
    importedSession !== selectedSession ||
    model.calibration.plan?.planSha256 !== selectedMasterPlanSha256 ||
    model.calibration.plan?.lightPlan?.planSha256 !== selectedLightPlanSha256
  ) {
    return;
  }
  const ticket = ++lightExecutionTicket;
  const outputMode = model.calibration.lightSettings.outputMode;
  const leavingCalibratedView = model.lightFrameView === "calibrated";
  const rawFrames = leavingCalibratedView
    ? reviewFramesForRole(selectedSession, "light")
    : model.frames;
  if (leavingCalibratedView) {
    stopBlinkTimer();
    clearPreviewResources();
    sharedTransform = null;
    previewTicket += 1;
    sortTicket += 1;
    statisticsTicket += 1;
    qualitySessionRevision += 1;
    qualityBatchTicket += 1;
    qualityPending.clear();
  }
  update({
    ...model,
    lightFrameView: leavingCalibratedView ? "raw" : model.lightFrameView,
    frames: rawFrames,
    selectedFrameId: leavingCalibratedView
      ? (rawFrames[0]?.id ?? null)
      : model.selectedFrameId,
    playing: leavingCalibratedView ? false : model.playing,
    qualityBatchRunning: leavingCalibratedView
      ? false
      : model.qualityBatchRunning,
    qualityBatchProgress: leavingCalibratedView
      ? null
      : model.qualityBatchProgress,
    sharedStretchLabel: leavingCalibratedView
      ? "Reference stretch · resolving"
      : model.sharedStretchLabel,
    preview: leavingCalibratedView ? null : model.preview,
    statisticsPanel: leavingCalibratedView
      ? closedStatisticsPanel()
      : model.statisticsPanel,
    calibration: {
      ...model.calibration,
      lightExecution: {
        state: "running",
        masterDirectory,
        outputDirectory,
        progress: null,
        result: null,
        message:
          outputMode === "calibrated_frames"
            ? "Preparing lossless calibrated-frame export…"
            : "Preparing calibrated integration…",
      },
    },
  });
  const onProgress = (progress: LightExecutionProgress): void => {
    if (ticket !== lightExecutionTicket) return;
    const state = model.calibration.lightExecution.state;
    if (state !== "running" && state !== "cancelling") return;
    update({
      ...model,
      calibration: {
        ...model.calibration,
        lightExecution: {
          ...model.calibration.lightExecution,
          state,
          progress,
          message: lightProgressMessage(progress),
        },
      },
    });
  };
  try {
    const result = await executeLightPlan(
      masterDirectory,
      outputDirectory,
      model.calibration.settings,
      model.calibration.lightSettings,
      plan,
      lightPlan,
      onProgress,
    );
    if (ticket !== lightExecutionTicket) return;
    update({
      ...model,
      calibration: {
        ...model.calibration,
        lightExecution: {
          state: "completed",
          masterDirectory,
          outputDirectory,
          progress: model.calibration.lightExecution.progress,
          result,
          message:
            result.outputMode === "calibrated_frames"
              ? `${result.calibratedFrames.length} calibrated frame${result.calibratedFrames.length === 1 ? "" : "s"} published · peak ${formatMemory(result.peakReservedBytes)}`
              : `${result.products.length} integrated product${result.products.length === 1 ? "" : "s"} published · peak ${formatMemory(result.peakReservedBytes)}`,
        },
      },
    });
    if (result.outputMode === "calibrated_frames") {
      selectLightFrameView("calibrated");
    }
  } catch (error) {
    if (ticket !== lightExecutionTicket) return;
    const cancelled = nativeErrorCode(error) === "light_execution_cancelled";
    update({
      ...model,
      calibration: {
        ...model.calibration,
        lightExecution: {
          ...model.calibration.lightExecution,
          state: cancelled ? "idle" : "error",
          result: null,
          message: cancelled
            ? "Light run cancelled · no partial product set published"
            : "Light calibration failed safely · no existing output was modified",
        },
      },
    });
  }
}

async function cancelLights(): Promise<void> {
  if (model.calibration.lightExecution.state !== "running") return;
  update({
    ...model,
    calibration: {
      ...model.calibration,
      lightExecution: {
        ...model.calibration.lightExecution,
        state: "cancelling",
        message: "Cancellation requested · finishing the current bounded unit…",
      },
    },
  });
  try {
    await cancelLightPlan();
  } catch {
    update({
      ...model,
      calibration: {
        ...model.calibration,
        lightExecution: {
          ...model.calibration.lightExecution,
          state: "error",
          message: "Cancellation request failed · native task state is unknown",
        },
      },
    });
  }
}

function lightProgressMessage(progress: LightExecutionProgress): string {
  const product = progress.productIndex + 1;
  const source =
    progress.sourceIndex !== null && progress.sourceCount !== null
      ? ` · frame ${progress.sourceIndex + 1}/${progress.sourceCount}`
      : "";
  const units = progress.totalUnits
    ? ` · ${progress.completedUnits}/${progress.totalUnits}`
    : "";
  return `Light ${product}/${progress.productCount} · ${progress.groupId}${source} · ${progress.stage}${units}`;
}

function masterProgressMessage(progress: MasterExecutionProgress): string {
  const product = progress.productIndex + 1;
  const units = progress.totalUnits
    ? ` · ${progress.completedUnits}/${progress.totalUnits}`
    : "";
  return `Master ${product}/${progress.productCount} · ${progress.groupId} · ${progress.stage}${units}`;
}

function formatMemory(bytes: number): string {
  return `${(bytes / (1_024 * 1_024)).toFixed(1)} MiB`;
}

function isActiveExecutionState(
  state: ReviewViewModel["calibration"]["execution"]["state"],
): boolean {
  return state === "running" || state === "cancelling";
}

function nativeErrorCode(error: unknown): string | null {
  if (typeof error !== "object" || error === null || !("code" in error)) {
    return null;
  }
  return typeof error.code === "string" ? error.code : null;
}

async function refreshMasterPlan(): Promise<void> {
  if (
    !importedSession ||
    model.calibration.execution.state === "running" ||
    model.calibration.execution.state === "cancelling" ||
    model.calibration.lightExecution.state === "running" ||
    model.calibration.lightExecution.state === "cancelling"
  ) {
    return;
  }
  const ticket = ++masterPlanTicket;
  const settings = model.calibration.settings;
  update({
    ...model,
    calibration: {
      ...model.calibration,
      state: "loading",
      message: "Resolving Bias, Darks and Flats…",
      execution: {
        state: "idle",
        outputDirectory: null,
        progress: null,
        result: null,
        message: "Waiting for a validated native plan",
      },
    },
  });
  try {
    const plan = await previewMasterPlan(settings);
    if (ticket !== masterPlanTicket) return;
    const unresolved = plan.products.filter(
      (product) => product.pedestal.status === "unresolved",
    ).length;
    const unresolvedLights =
      plan.lightPlan?.products.filter(
        (product) =>
          product.dark.status === "unresolved" ||
          product.flat.status === "unresolved",
      ).length ?? 0;
    const lightCount = plan.lightPlan?.products.length ?? 0;
    update({
      ...model,
      calibration: {
        ...model.calibration,
        state: "ready",
        plan,
        message: !plan.ready
          ? `${unresolved} flat group${unresolved === 1 ? "" : "s"} require attention`
          : unresolvedLights > 0
            ? `${unresolvedLights} Light group${unresolvedLights === 1 ? "" : "s"} require master attention`
            : `${plan.products.length} master groups · ${lightCount} Light group${lightCount === 1 ? "" : "s"} ready`,
      },
    });
  } catch {
    if (ticket !== masterPlanTicket) return;
    update({
      ...model,
      calibration: {
        ...model.calibration,
        state: "error",
        plan: null,
        message: "Calibration planning failed · imported session preserved",
      },
    });
  }
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
  return reviewFrameFromImported(frame, frame.path, frame.label);
}

function reviewFrameFromImported(
  frame: ImportedFrame,
  sourcePath: string,
  label: string,
  previewContent: ReviewFrame["previewContent"] = {
    kind: "scalar",
    plane: 0,
  },
): ReviewFrame {
  const artifactKey = frameArtifactKey(frame.id, sourcePath);
  const cachedQuality = qualityCache.get(artifactKey);
  const cachedDecision = decisionCache.get(frame.id);
  const qualityAvailable =
    frame.role === "light" &&
    (previewContent.kind === "rgb" || frame.bayerPattern !== null);
  const qualityState = cachedQuality
    ? "ready"
    : qualityPending.has(artifactKey)
      ? "loading"
      : qualityAvailable
        ? "idle"
        : "unavailable";
  return {
    id: frame.id,
    label,
    sourcePath,
    previewContent,
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
          ? previewContent.kind === "rgb"
            ? "Ready for linked linear-luminance diagnostics"
            : "Ready for phase-neutral CFA diagnostics"
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

function calibratedReviewFrames(
  session: ImportedSession,
  calibrated: readonly ExecutedCalibratedLightFrame[],
): readonly ReviewFrame[] | null {
  const bound = bindCalibratedLightFrames(session.frames, calibrated);
  if (!bound) return null;
  return bound.map(({ source, artifact }) =>
    reviewFrameFromImported(
      source,
      artifact.rgbOutputPath ?? artifact.outputPath,
      artifact.sourceLabel,
      artifact.rgbOutputPath ? { kind: "rgb" } : { kind: "scalar", plane: 0 },
    ),
  );
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

function qualityCandidate(frame: ReviewFrame): boolean {
  return (
    frame.sourcePath !== null &&
    (frame.previewContent.kind === "rgb" || frame.bayerPattern !== null) &&
    (frame.qualityState === "idle" || frame.qualityState === "error")
  );
}

async function measureAllQuality(): Promise<void> {
  if (
    !importedSession ||
    model.activeRole !== "light" ||
    model.qualityBatchRunning ||
    qualityPending.size > 0
  ) {
    return;
  }
  const frameIds = model.frames
    .filter(qualityCandidate)
    .map((frame) => frame.id);
  if (frameIds.length === 0) return;

  const ticket = ++qualityBatchTicket;
  update({
    ...model,
    qualityBatchRunning: true,
    qualityBatchProgress: { completed: 0, total: frameIds.length },
  });
  try {
    await runSerialBatch(
      frameIds,
      measureQuality,
      (qualityBatchProgress) => update({ ...model, qualityBatchProgress }),
      () => ticket !== qualityBatchTicket,
    );
  } finally {
    if (ticket === qualityBatchTicket) {
      update({
        ...model,
        qualityBatchRunning: false,
        qualityBatchProgress: null,
      });
    }
  }
}

async function measureQuality(frameId: string): Promise<void> {
  const frame = model.frames.find((candidate) => candidate.id === frameId);
  const artifactKey = frame?.sourcePath
    ? frameArtifactKey(frame.id, frame.sourcePath)
    : null;
  if (
    !frame?.sourcePath ||
    (frame.previewContent.kind === "scalar" && !frame.bayerPattern) ||
    model.activeRole !== "light" ||
    !artifactKey ||
    qualityPending.has(artifactKey)
  ) {
    return;
  }

  const cached = qualityCache.get(artifactKey);
  if (cached) {
    applyQualityResult(frame.id, cached);
    return;
  }

  const sessionRevision = qualitySessionRevision;
  qualityPending.add(artifactKey);
  updateQualityFrame(frame.id, {
    qualityState: "loading",
    qualityMessage: "Measuring immutable linear pixels…",
  });
  try {
    const result =
      frame.previewContent.kind === "rgb"
        ? await inspectRgbFrameQuality(frame.sourcePath)
        : await inspectCfaFrameQuality(frame.sourcePath, frame.bayerPattern!);
    if (sessionRevision !== qualitySessionRevision) return;
    qualityCache.set(artifactKey, result);
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
    qualityPending.delete(artifactKey);
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
  qualityBatchTicket += 1;
  qualitySessionRevision += 1;
  qualityPending.clear();
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
        lightFrameView: "raw",
        frames: demoReviewModel.frames,
        selectedFrameId: demoReviewModel.selectedFrameId,
        preview: null,
        qualityBatchRunning: false,
        qualityBatchProgress: null,
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
      qualityBatchRunning: false,
      qualityBatchProgress: null,
      statisticsPanel: closedStatisticsPanel(),
    });
    return;
  }

  const calibrated =
    model.calibration.lightExecution.result?.calibratedFrames ?? [];
  const requestedLightFrameView =
    role === "light" &&
    model.lightFrameView === "calibrated" &&
    calibrated.length > 0
      ? "calibrated"
      : "raw";
  const calibratedFrames =
    role === "light" && requestedLightFrameView === "calibrated"
      ? calibratedReviewFrames(importedSession, calibrated)
      : null;
  const lightFrameView =
    requestedLightFrameView === "calibrated" && calibratedFrames
      ? "calibrated"
      : "raw";
  const frames = calibratedFrames ?? reviewFramesForRole(importedSession, role);
  update({
    ...model,
    activeRole: role,
    lightFrameView,
    frames,
    selectedFrameId: frames[0]?.id ?? null,
    playing: false,
    qualityBatchRunning: false,
    qualityBatchProgress: null,
    sharedStretchLabel: "Reference stretch · resolving",
    preview: null,
    statisticsPanel: closedStatisticsPanel(),
  });
  void loadSelectedPreview();
}

function selectLightFrameView(view: LightFrameView): void {
  if (!importedSession) return;
  const calibrated =
    model.calibration.lightExecution.result?.calibratedFrames ?? [];
  if (view === "calibrated" && calibrated.length === 0) return;
  if (view === model.lightFrameView && model.activeWorkspace === "frames") {
    return;
  }

  stopBlinkTimer();
  clearPreviewResources();
  sharedTransform = null;
  previewTicket += 1;
  sortTicket += 1;
  statisticsTicket += 1;
  qualitySessionRevision += 1;
  qualityBatchTicket += 1;
  qualityPending.clear();
  const frames =
    view === "calibrated"
      ? calibratedReviewFrames(importedSession, calibrated)
      : reviewFramesForRole(importedSession, "light");
  if (!frames) {
    update({
      ...model,
      calibration: {
        ...model.calibration,
        lightExecution: {
          ...model.calibration.lightExecution,
          message:
            "Calibrated frames published, but their source identities could not be verified for Blink",
        },
      },
    });
    return;
  }
  update({
    ...model,
    activeWorkspace: "frames",
    activeRole: "light",
    lightFrameView: view,
    frames,
    selectedFrameId: frames[0]?.id ?? null,
    playing: false,
    qualityBatchRunning: false,
    qualityBatchProgress: null,
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

  const artifactKey = frameArtifactKey(frame.id, frame.sourcePath);
  const cached = statisticsCache.get(artifactKey);
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
    statisticsCache.set(artifactKey, statistics);
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
        content: frame.previewContent,
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
      content: frame.previewContent,
      ...previewBounds,
      blackPoint: transform.blackPoint,
      whitePoint: transform.whitePoint,
      midtone: transform.midtone,
      transfer: { kind: "midtones" },
    };
    const cacheKey = previewCacheKey(request, transform.algorithmId);
    let cached = previewCache.get(cacheKey);
    const pendingPrefetch = previewPrefetch.pending(cacheKey);
    if (!cached && pendingPrefetch) {
      await pendingPrefetch;
      if (ticket !== previewTicket) return;
      cached = previewCache.get(cacheKey);
    }
    if (cached) {
      if (ticket !== previewTicket) return;
      releaseEphemeralPreview();
      update({ ...model, preview: cached.preview });
      scheduleAdjacentPreviewPrefetch(transform);
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
    scheduleAdjacentPreviewPrefetch(transform);
  } catch {
    if (ticket !== previewTicket) return;
    update({
      ...model,
      sharedStretchLabel: "Preview unavailable · inspect Diagnostics",
      preview: null,
    });
  }
}

function scheduleAdjacentPreviewPrefetch(
  transform: EstimatedDisplayTransform,
): void {
  const selectedId = model.selectedFrameId;
  if (!selectedId) return;
  for (const frame of adjacentPreviewFrames(
    model.frames,
    selectedId,
    maximumAdjacentPrefetches,
  )) {
    if (!frame.sourcePath) continue;
    const request: FitsPreviewRequest = {
      frameId: frame.id,
      path: frame.sourcePath,
      content: frame.previewContent,
      ...previewBounds,
      blackPoint: transform.blackPoint,
      whitePoint: transform.whitePoint,
      midtone: transform.midtone,
      transfer: { kind: "midtones" },
    };
    const cacheKey = previewCacheKey(request, transform.algorithmId);
    if (previewCache.has(cacheKey)) continue;
    void previewPrefetch.schedule(
      cacheKey,
      () => requestFitsPreview(request),
      (resource) => previewCache.put(cacheKey, resource),
    );
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
  previewPrefetch.cancel();
  releaseEphemeralPreview();
  previewCache.clear();
}

function disposeRuntimeResources(): void {
  previewTicket += 1;
  sortTicket += 1;
  statisticsTicket += 1;
  qualitySessionRevision += 1;
  qualityBatchTicket += 1;
  decisionSessionRevision += 1;
  masterPlanTicket += 1;
  masterExecutionTicket += 1;
  lightExecutionTicket += 1;
  if (
    model.calibration.execution.state === "running" ||
    model.calibration.execution.state === "cancelling"
  ) {
    void cancelMasterPlan();
  }
  if (
    model.calibration.lightExecution.state === "running" ||
    model.calibration.lightExecution.state === "cancelling"
  ) {
    void cancelLightPlan();
  }
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
