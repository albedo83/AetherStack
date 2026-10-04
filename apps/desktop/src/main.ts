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
  RegisteredStackProductView,
  ReviewFrame,
  ReviewRejectionReason,
  ReviewState,
  ReviewViewModel,
  WorkspaceView,
} from "./model.ts";
import {
  estimateFitsPreviewTransform,
  inspectRejectionHistogram,
  inspectStackPixel,
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
import { buildQualityWeightPreflight } from "./quality-weight.ts";
import {
  cancelRegisteredStack,
  cancelRegisteredStackSourceVerification,
  cancelRegistrationPlan,
  diagnoseFitsRegistration,
  executeRegistrationPlan,
  executeRegisteredStack,
  inspectRegisteredStackReport,
  previewRegisteredWeights,
  previewRegistrationPlan,
  selectRegistrationOutputDirectory,
  selectRegisteredStackReport,
  selectRegisteredStackSourceDirectory,
  selectRegisteredStackOutput,
  verifyRegisteredStackSources,
  type RegistrationExecutionProgress,
  type RegisteredStackIntegrationSettings,
  type RegisteredStackProgress,
  type RegisteredStackSourceVerificationProgress,
} from "./registration-bridge.ts";
import { reconcileRegistrationSolutions } from "./registration-plan.ts";
import { bindRegisteredReviewFrames } from "./registered-review.ts";
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
  applyFrameSelection,
  previewFrameSelection,
  type FrameSelectionRule,
} from "./selection-bridge.ts";
import {
  applyQualityCacheMaintenance,
  cancelSessionImport,
  exportSessionDiagnostics,
  importedSessionDiagnostics,
  importedSessionStatus,
  previewQualityCacheMaintenance,
  selectAndInspectSessionDiagnostics,
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
let sessionImportPhase: "idle" | "running" | "cancelling" = "idle";

function sessionImportWasCancelled(): boolean {
  return sessionImportPhase === "cancelling";
}
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
const qualityOrigins = new Map<string, "measured" | "restored">();
const qualityPending = new Set<string>();
const decisionCache = new Map<
  string,
  Pick<ReviewFrame, "state" | "rejectionReason">
>();
let qualitySessionRevision = 0;
let qualityBatchTicket = 0;
let frameSelectionTicket = 0;
let decisionSessionRevision = 0;
let decisionGeneration = 0;
let blinkTimer: number | null = null;
let masterPlanTicket = 0;
let masterExecutionTicket = 0;
let lightExecutionTicket = 0;
let registrationTicket = 0;
let registrationExecutionTicket = 0;
let registeredStackTicket = 0;
let registeredStackPreviewTicket = 0;
let stackPixelTicket = 0;
let stackReportTicket = 0;
let stackSourceVerificationTicket = 0;
let registrationPreviewTicket = 0;
let registrationBlinkTimer: number | null = null;
let registrationSharedTransform: EstimatedDisplayTransform | null = null;
const registrationPreviewCache = new BoundedPreviewCache(
  maximumCachedPreviews,
  maximumCachedPreviewBytes,
);
const registrationPreviewPrefetch =
  new PreviewPrefetchCoordinator<PreviewResource>();
let ephemeralRegistrationPreview: PreviewResource | null = null;
let registeredStackSciencePreviewResource: PreviewResource | null = null;
let registeredStackDiagnosticPreviewResource: PreviewResource | null = null;

const screen = mountReviewScreen(root, model, {
  onSelectWorkspace(workspace) {
    selectWorkspace(workspace);
  },
  onSelectRegistrationReference(frameId) {
    selectRegistrationFrame("reference", frameId);
  },
  onSelectRegistrationSource(frameId) {
    selectRegistrationFrame("source", frameId);
  },
  onAnalyzeRegistration() {
    void analyzeRegistration();
  },
  onExecuteRegistration() {
    void executeRegistration();
  },
  onCancelRegistration() {
    void cancelRegistration();
  },
  onExecuteRegisteredStack() {
    void executeStack();
  },
  onCancelRegisteredStack() {
    void cancelStack();
  },
  onUpdateRegisteredStackSettings(settings) {
    if (
      model.registration.stack.state === "running" ||
      model.registration.stack.state === "cancelling"
    ) {
      return;
    }
    registeredStackTicket += 1;
    registeredStackPreviewTicket += 1;
    stackPixelTicket += 1;
    stackReportTicket += 1;
    stackSourceVerificationTicket += 1;
    clearRegisteredStackPreviewResources();
    update({
      ...model,
      registration: {
        ...model.registration,
        stack: {
          ...idleRegisteredStack("Advanced integration settings updated"),
          settings,
        },
      },
    });
  },
  onSelectRegisteredStackProduct(product) {
    selectRegisteredStackProduct(product);
  },
  onSetRegisteredStackOverlayOpacity(opacity) {
    if (!Number.isFinite(opacity) || opacity < 0 || opacity > 1) return;
    update({
      ...model,
      registration: {
        ...model.registration,
        stack: { ...model.registration.stack, overlayOpacity: opacity },
      },
    });
  },
  onInspectRegisteredStackPixel(x, y) {
    void inspectRegisteredStackPixel(x, y);
  },
  onInspectRegisteredStackReport() {
    void inspectStackReport();
  },
  onOpenRegisteredStackReport() {
    void openStackReport();
  },
  onReturnToActiveStack() {
    returnToActiveStack();
  },
  onVerifyRegisteredStackSources() {
    void verifyStackReportSources();
  },
  onCancelRegisteredStackSourceVerification() {
    void cancelStackReportSourceVerification();
  },
  onSelectRegisteredFrame(frameId) {
    selectRegisteredFrame(frameId);
  },
  onSetRegisteredPlaying(playing) {
    setRegisteredPlaying(playing);
  },
  onStepRegisteredFrame(direction) {
    stepRegisteredFrame(direction);
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
  onExportDiagnostics() {
    void exportDiagnostics();
  },
  onInspectDiagnosticsReport() {
    void inspectDiagnosticsReport();
  },
  onPreviewQualityCacheMaintenance() {
    void previewCacheMaintenance();
  },
  onApplyQualityCacheMaintenance() {
    void applyCacheMaintenance();
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
  onUpdateFrameSelectionRules(rules) {
    frameSelectionTicket += 1;
    update({
      ...model,
      frameSelection: {
        state: "idle",
        rules,
        plan: null,
        message: "Rules changed · preview the native recommendations again",
      },
    });
  },
  onPreviewFrameSelection() {
    void previewAutomaticSelection();
  },
  onApplyFrameSelection() {
    void applyAutomaticSelection();
  },
});

window.addEventListener("beforeunload", disposeRuntimeResources, {
  once: true,
});

async function importSession(): Promise<void> {
  if (sessionImportPhase === "running") {
    sessionImportPhase = "cancelling";
    update({
      ...model,
      sessionStatus: { tone: "busy", label: "Cancelling FITS import" },
    });
    try {
      const requested = await cancelSessionImport();
      if (!requested) {
        sessionImportPhase = "running";
        update({
          ...model,
          sessionStatus: { tone: "busy", label: "Scanning FITS sources" },
        });
      }
    } catch {
      sessionImportPhase = "idle";
      update({
        ...model,
        sessionStatus: { tone: "error", label: "Import cancellation failed" },
      });
    }
    return;
  }
  if (sessionImportPhase === "cancelling") return;
  if (
    model.calibration.execution.state === "running" ||
    model.calibration.execution.state === "cancelling" ||
    isActiveExecutionState(model.calibration.lightExecution.state) ||
    isRegistrationWorkActive()
  ) {
    return;
  }
  const previousStatus = model.sessionStatus;
  sessionImportPhase = "running";
  update({
    ...model,
    sessionStatus: { tone: "busy", label: "Scanning FITS sources" },
  });
  try {
    const imported = await selectAndImportSession((progress) => {
      if (sessionImportPhase !== "running") return;
      const label =
        progress.stage === "discovering"
          ? "Discovering FITS sources"
          : progress.stage === "analyzing"
            ? `Analyzing ${progress.completedSources} / ${progress.totalSources ?? "?"} FITS`
            : progress.stage === "assembling"
              ? "Assembling session manifest"
              : "Finalizing imported session";
      update({ ...model, sessionStatus: { tone: "busy", label } });
    });
    if (!imported) {
      update({ ...model, sessionStatus: previousStatus });
      return;
    }
    installImportedSession(imported);
  } catch {
    update(
      sessionImportWasCancelled()
        ? { ...model, sessionStatus: previousStatus }
        : {
            ...model,
            sessionStatus: {
              tone: "error",
              label: "Import failed · open Diagnostics",
            },
          },
    );
  } finally {
    sessionImportPhase = "idle";
  }
}

async function exportDiagnostics(): Promise<void> {
  if (
    !importedSession ||
    model.sessionDiagnostics.exportState === "exporting"
  ) {
    return;
  }
  update({
    ...model,
    sessionDiagnostics: {
      ...model.sessionDiagnostics,
      exportState: "exporting",
      exportMessage: "Sealing redacted native diagnostics…",
    },
  });
  try {
    const exported = await exportSessionDiagnostics();
    if (!exported) {
      update({
        ...model,
        sessionDiagnostics: {
          ...model.sessionDiagnostics,
          exportState: "idle",
          exportMessage: "Diagnostics export cancelled",
        },
      });
      return;
    }
    update({
      ...model,
      sessionDiagnostics: {
        ...model.sessionDiagnostics,
        exportState: "ready",
        exportMessage: `${exported.itemCount.toLocaleString("en-US")} redacted items · sha256 ${exported.reportSha256.slice(0, 12)}…`,
      },
    });
  } catch {
    update({
      ...model,
      sessionDiagnostics: {
        ...model.sessionDiagnostics,
        exportState: "error",
        exportMessage: "Export failed · the destination was not modified",
      },
    });
  }
}

async function inspectDiagnosticsReport(): Promise<void> {
  if (model.sessionDiagnostics.inspectionState === "inspecting") return;
  update({
    ...model,
    sessionDiagnostics: {
      ...model.sessionDiagnostics,
      inspectionState: "inspecting",
      inspectionMessage: "Validating the complete native report…",
    },
  });
  try {
    const inspected = await selectAndInspectSessionDiagnostics();
    if (!inspected) {
      update({
        ...model,
        sessionDiagnostics: {
          ...model.sessionDiagnostics,
          inspectionState: "idle",
          inspectionMessage: "Diagnostics verification cancelled",
        },
      });
      return;
    }
    update({
      ...model,
      sessionDiagnostics: {
        ...model.sessionDiagnostics,
        inspectionState: "ready",
        inspectionMessage: `Verified schema ${inspected.schemaVersion} · ${inspected.itemCount.toLocaleString("en-US")} items · sha256 ${inspected.reportSha256.slice(0, 12)}…`,
      },
    });
  } catch {
    update({
      ...model,
      sessionDiagnostics: {
        ...model.sessionDiagnostics,
        inspectionState: "error",
        inspectionMessage: "Verification failed · report not trusted",
      },
    });
  }
}

async function previewCacheMaintenance(): Promise<void> {
  if (
    !importedSession ||
    model.sessionDiagnostics.maintenanceState === "inspecting"
  ) {
    return;
  }
  update({
    ...model,
    sessionDiagnostics: {
      ...model.sessionDiagnostics,
      maintenanceState: "inspecting",
      maintenanceMessage: "Fingerprinting rejected cache artifacts…",
    },
  });
  try {
    const preview = await previewQualityCacheMaintenance();
    const eligible = preview.eligibleCount.toLocaleString("en-US");
    const blocked = preview.blockedCount.toLocaleString("en-US");
    update({
      ...model,
      sessionDiagnostics: {
        ...model.sessionDiagnostics,
        maintenanceState: "ready",
        maintenanceMessage: `${eligible} removable · ${blocked} blocked · preview only`,
        maintenanceEligible: preview.eligibleCount,
        maintenanceBlocked: preview.blockedCount,
        maintenanceBytes: preview.totalFileBytes,
        maintenancePlanSha256: preview.planSha256,
      },
    });
  } catch {
    update({
      ...model,
      sessionDiagnostics: {
        ...model.sessionDiagnostics,
        maintenanceState: "error",
        maintenanceMessage: "Cache inspection failed · nothing was changed",
        maintenanceEligible: 0,
        maintenanceBlocked: 0,
        maintenanceBytes: 0,
        maintenancePlanSha256: null,
      },
    });
  }
}

async function applyCacheMaintenance(): Promise<void> {
  const planSha256 = model.sessionDiagnostics.maintenancePlanSha256;
  if (
    !importedSession ||
    !planSha256 ||
    model.sessionDiagnostics.maintenanceState !== "ready"
  ) {
    return;
  }
  update({
    ...model,
    sessionDiagnostics: {
      ...model.sessionDiagnostics,
      maintenanceState: "applying",
      maintenanceMessage: "Revalidating and removing inspected artifacts…",
    },
  });
  try {
    const result = await applyQualityCacheMaintenance(planSha256);
    update({
      ...model,
      sessionDiagnostics: {
        ...model.sessionDiagnostics,
        maintenanceState: "idle",
        maintenanceMessage: `${result.removedCount.toLocaleString("en-US")} removed · ${formatByteCount(result.removedBytes)} · ${result.skippedCount.toLocaleString("en-US")} retained`,
        maintenanceEligible: 0,
        maintenanceBlocked: result.skippedCount,
        maintenanceBytes: 0,
        maintenancePlanSha256: null,
      },
    });
  } catch {
    update({
      ...model,
      sessionDiagnostics: {
        ...model.sessionDiagnostics,
        maintenanceState: "error",
        maintenanceMessage: "Removal refused or interrupted · preview again",
        maintenancePlanSha256: null,
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
  frameSelectionTicket += 1;
  qualityCache.clear();
  qualityOrigins.clear();
  for (const frame of session.frames) {
    if (frame.quality) {
      const artifactKey = frameArtifactKey(frame.id, frame.path);
      qualityCache.set(artifactKey, frame.quality);
      qualityOrigins.set(artifactKey, "restored");
    }
  }
  qualityPending.clear();
  decisionSessionRevision += 1;
  decisionGeneration = 0;
  decisionCache.clear();
  masterPlanTicket += 1;
  masterExecutionTicket += 1;
  lightExecutionTicket += 1;
  registrationExecutionTicket += 1;
  registeredStackTicket += 1;
  registrationTicket += 1;
  clearRegistrationPreviewResources();

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
  const registrationFrames = registrationFramesForSession(session);
  update({
    ...model,
    sessionName: session.name,
    sessionStatus: importedSessionStatus(session),
    sessionDiagnostics: importedSessionDiagnostics(session),
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
    frameSelection: resetFrameSelection(model.frameSelection.rules),
    sharedStretchLabel: "Reference stretch · resolving",
    preview: null,
    statisticsPanel: closedStatisticsPanel(),
    registration: {
      state: "idle",
      frames: registrationFrames,
      referenceFrameId: registrationFrames[0]?.id ?? null,
      sourceFrameId: registrationFrames[1]?.id ?? null,
      diagnostic: null,
      solutions: [],
      planState: "idle",
      plan: null,
      execution: {
        state: "idle",
        outputDirectory: null,
        progress: null,
        result: null,
        message: "Export calibrated Lights to unlock registration",
      },
      stack: idleRegisteredStack(),
      resultReview: idleRegistrationResultReview(),
      message:
        registrationFrames.length >= 2
          ? "Choose a Light pair, then run the native geometric solver"
          : "At least two Light frames are required for registration",
    },
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

function registrationFramesForSession(
  session: ImportedSession,
): ReviewViewModel["registration"]["frames"] {
  return session.frames
    .filter(
      (frame) =>
        frame.role === "light" &&
        decisionCache.get(frame.id)?.state !== "rejected",
    )
    .map((frame) => ({
      id: frame.id,
      label: frame.label,
      sourcePath: frame.path,
    }));
}

function selectWorkspace(workspace: WorkspaceView): void {
  if (workspace === model.activeWorkspace) return;
  if (workspace !== "frames") setPlaying(false);
  if (workspace !== "registration") setRegisteredPlaying(false);
  update({ ...model, activeWorkspace: workspace });
  if (workspace === "frames" && model.preview === null) {
    void loadSelectedPreview();
  }
}

function selectRegistrationFrame(
  role: "reference" | "source",
  frameId: string,
): void {
  if (!model.registration.frames.some((frame) => frame.id === frameId)) return;
  registrationTicket += 1;
  if (role === "reference") clearRegistrationPreviewResources();
  update({
    ...model,
    registration: {
      ...model.registration,
      state: "idle",
      referenceFrameId:
        role === "reference" ? frameId : model.registration.referenceFrameId,
      sourceFrameId:
        role === "source" ? frameId : model.registration.sourceFrameId,
      diagnostic: null,
      solutions: role === "reference" ? [] : model.registration.solutions,
      planState: role === "reference" ? "idle" : model.registration.planState,
      plan: role === "reference" ? null : model.registration.plan,
      execution:
        role === "reference"
          ? idleRegistrationExecution("Reference changed · rebuild the plan")
          : model.registration.execution,
      stack:
        role === "reference"
          ? idleRegisteredStack(
              "Reference changed · integrate a fresh registered set",
            )
          : model.registration.stack,
      resultReview:
        role === "reference"
          ? idleRegistrationResultReview(
              "Reference changed · register a fresh frame set",
            )
          : model.registration.resultReview,
      message: "Pair changed · run the native geometric solver",
    },
  });
}

async function analyzeRegistration(): Promise<void> {
  const reference = model.registration.frames.find(
    (frame) => frame.id === model.registration.referenceFrameId,
  );
  const source = model.registration.frames.find(
    (frame) => frame.id === model.registration.sourceFrameId,
  );
  if (
    !reference?.sourcePath ||
    !source?.sourcePath ||
    reference.id === source.id ||
    model.registration.state === "running"
  ) {
    return;
  }

  const ticket = ++registrationTicket;
  clearRegistrationPreviewResources();
  const referenceId = reference.id;
  const sourceId = source.id;
  update({
    ...model,
    registration: {
      ...model.registration,
      state: "running",
      diagnostic: null,
      planState: "idle",
      plan: null,
      execution: idleRegistrationExecution(
        "Geometry changed · reseal before registration",
      ),
      stack: idleRegisteredStack(
        "Geometry changed · publish and integrate a fresh registered set",
      ),
      resultReview: idleRegistrationResultReview(
        "Geometry changed · register a fresh frame set",
      ),
      message: "Detecting stars and testing deterministic geometry…",
    },
  });
  try {
    const diagnostic = await diagnoseFitsRegistration({
      sourcePath: source.sourcePath,
      referencePath: reference.sourcePath,
    });
    if (
      ticket !== registrationTicket ||
      model.registration.referenceFrameId !== referenceId ||
      model.registration.sourceFrameId !== sourceId
    ) {
      return;
    }
    const accepted = diagnostic.confidence.accepted;
    const solutions = reconcileRegistrationSolutions(
      model.registration.frames,
      referenceId,
      sourceId,
      model.registration.solutions,
      diagnostic,
    );
    const nextPending = accepted
      ? model.registration.frames.find(
          (frame) =>
            frame.id !== referenceId &&
            frame.id !== sourceId &&
            !solutions.some((solution) => solution.sourceFrameId === frame.id),
        )
      : null;
    update({
      ...model,
      registration: {
        ...model.registration,
        state: accepted ? "accepted" : "rejected",
        diagnostic,
        solutions,
        planState: "idle",
        plan: null,
        message: accepted
          ? nextPending
            ? `Geometry accepted · ${solutions.length}/${model.registration.frames.length - 1} transforms reviewed`
            : "All source transforms accepted · multi-frame plan is ready for final review"
          : "Geometry rejected by the confidence gate",
      },
    });
    if (!nextPending) {
      void rebuildRegistrationPlan(referenceId, solutions);
    }
  } catch {
    if (ticket !== registrationTicket) return;
    update({
      ...model,
      registration: {
        ...model.registration,
        state: "error",
        diagnostic: null,
        message: "Registration diagnostic failed · inspect native diagnostics",
      },
    });
  }
}

async function rebuildRegistrationPlan(
  referenceFrameId: string,
  solutions: ReviewViewModel["registration"]["solutions"],
): Promise<void> {
  const required = model.registration.frames.filter(
    (frame) => frame.id !== referenceFrameId,
  );
  if (
    required.length === 0 ||
    required.some(
      (frame) =>
        !solutions.some((solution) => solution.sourceFrameId === frame.id),
    )
  ) {
    return;
  }
  const ticket = ++registrationTicket;
  update({
    ...model,
    registration: {
      ...model.registration,
      planState: "building",
      plan: null,
      message:
        "Rebuilding every accepted pair in Rust and sealing the canonical plan…",
    },
  });
  try {
    const plan = await previewRegistrationPlan({
      referenceFrameId,
      sourceFrameIds: required.map((frame) => frame.id),
    });
    if (
      ticket !== registrationTicket ||
      model.registration.referenceFrameId !== referenceFrameId
    ) {
      return;
    }
    update({
      ...model,
      registration: {
        ...model.registration,
        planState: "ready",
        plan,
        message: `Canonical plan sealed · ${plan.frames.length} frames · digest ${plan.planSha256.slice(0, 12)}…`,
      },
    });
  } catch {
    if (ticket !== registrationTicket) return;
    update({
      ...model,
      registration: {
        ...model.registration,
        planState: "error",
        plan: null,
        message:
          "Native plan reconstruction failed · pair evidence was not trusted",
      },
    });
  }
}

function idleRegistrationExecution(
  message: string,
): ReviewViewModel["registration"]["execution"] {
  return {
    state: "idle",
    outputDirectory: null,
    progress: null,
    result: null,
    message,
  };
}

function idleRegisteredStack(
  message = "Register the reviewed Lights to unlock integration",
): ReviewViewModel["registration"]["stack"] {
  return {
    state: "idle",
    outputPath: null,
    progress: null,
    result: null,
    previewState: "idle",
    preview: null,
    sciencePreview: null,
    selectedProduct: "science",
    overlayOpacity: 0.65,
    histogramState: "idle",
    histogram: null,
    pixelInspectionState: "idle",
    pixelInspection: null,
    weightPreflight: null,
    reportInspectionState: "idle",
    reportInspection: null,
    reportInspectionPath: null,
    sourceVerificationState: "idle",
    sourceVerification: null,
    sourceVerificationProgress: null,
    settings: defaultRegisteredStackSettings(),
    message,
  };
}

function defaultRegisteredStackSettings(): RegisteredStackIntegrationSettings {
  return {
    estimator: "strict_mean",
    weightReferenceFrameId: null,
    lowFraction: 0.1,
    highFraction: 0.1,
    minimumRetainedSamples: 3,
    generateRejectionMaps: false,
  };
}

function idleRegistrationResultReview(
  message = "Registered pixels will appear here after atomic publication",
): ReviewViewModel["registration"]["resultReview"] {
  return {
    frames: [],
    selectedFrameId: null,
    state: "idle",
    preview: null,
    playing: false,
    message,
    sharedStretchLabel: "Registered stretch · awaiting pixels",
  };
}

async function executeRegistration(): Promise<void> {
  const plan = model.registration.plan;
  const calibration = model.calibration.lightExecution.result;
  const execution = model.registration.execution;
  if (
    model.registration.planState !== "ready" ||
    !plan ||
    !calibration ||
    calibration.outputMode !== "calibrated_frames" ||
    execution.state === "running" ||
    execution.state === "cancelling" ||
    isActiveExecutionState(model.calibration.execution.state) ||
    isActiveExecutionState(model.calibration.lightExecution.state)
  ) {
    return;
  }
  const artifacts = plan.frames.map((planned) => {
    const calibrated = calibration.calibratedFrames.find(
      (frame) => frame.sourceFrameId === planned.frameId,
    );
    return calibrated
      ? {
          frameId: planned.frameId,
          path: calibrated.rgbOutputPath ?? calibrated.outputPath,
        }
      : null;
  });
  if (artifacts.some((artifact) => artifact === null)) {
    update({
      ...model,
      registration: {
        ...model.registration,
        execution: {
          ...execution,
          state: "error",
          message:
            "The calibrated artifact set does not match every sealed Light identity",
        },
      },
    });
    return;
  }
  const selectedPlanDigest = plan.planSha256;
  const outputDirectory = await selectRegistrationOutputDirectory();
  if (
    !outputDirectory ||
    model.registration.plan?.planSha256 !== selectedPlanDigest
  ) {
    return;
  }
  const ticket = ++registrationExecutionTicket;
  clearRegistrationPreviewResources();
  update({
    ...model,
    registration: {
      ...model.registration,
      execution: {
        state: "running",
        outputDirectory,
        progress: null,
        result: null,
        message: "Preparing the atomic registered-frame transaction…",
      },
      stack: idleRegisteredStack(
        "Registration is publishing the source set required for integration",
      ),
      resultReview: idleRegistrationResultReview(
        "Registration is publishing the complete sealed frame set…",
      ),
    },
  });
  const onProgress = (progress: RegistrationExecutionProgress): void => {
    if (ticket !== registrationExecutionTicket) return;
    const state = model.registration.execution.state;
    if (state !== "running" && state !== "cancelling") return;
    const units = progress.totalUnits
      ? ` · ${progress.completedUnits}/${progress.totalUnits}`
      : "";
    update({
      ...model,
      registration: {
        ...model.registration,
        execution: {
          ...model.registration.execution,
          state,
          progress,
          message: `Frame ${progress.frameIndex + 1}/${progress.frameCount} · ${progress.stage}${units}`,
        },
      },
    });
  };
  try {
    const result = await executeRegistrationPlan(
      outputDirectory,
      {
        referenceFrameId: plan.referenceFrameId,
        sourceFrameIds: plan.frames
          .filter((frame) => !frame.reference)
          .map((frame) => frame.frameId),
      },
      plan.planSha256,
      artifacts.filter((artifact) => artifact !== null),
      {
        bandHeight: 128,
        memoryLimitBytes: model.calibration.lightSettings.memoryLimitBytes,
      },
      onProgress,
    );
    if (ticket !== registrationExecutionTicket) return;
    const registeredFrames = bindRegisteredReviewFrames(
      model.registration.frames,
      calibration.calibratedFrames,
      result.frames,
    );
    update({
      ...model,
      registration: {
        ...model.registration,
        execution: {
          state: "completed",
          outputDirectory,
          progress: model.registration.execution.progress,
          result,
          message: `${result.frames.length} registered frames published atomically · peak ${formatMemory(result.peakReservedBytes)}`,
        },
        stack: idleRegisteredStack(
          "Registered set verified · ready to integrate the common crop",
        ),
        resultReview: registeredFrames
          ? {
              frames: registeredFrames,
              selectedFrameId: registeredFrames[0]?.id ?? null,
              state: "loading",
              preview: null,
              playing: false,
              message: "Resolving one shared display stretch…",
              sharedStretchLabel: "Registered stretch · resolving",
            }
          : {
              ...idleRegistrationResultReview(),
              state: "error",
              message:
                "Published frame identities could not be bound for result review",
            },
      },
    });
    if (registeredFrames) void loadSelectedRegisteredPreview();
  } catch (error) {
    if (ticket !== registrationExecutionTicket) return;
    const cancelled =
      nativeErrorCode(error) === "registration_execution_cancelled";
    update({
      ...model,
      registration: {
        ...model.registration,
        execution: {
          ...model.registration.execution,
          state: cancelled ? "idle" : "error",
          result: null,
          message: cancelled
            ? "Registration cancelled · no partial frame set published"
            : "Registration failed safely · no existing output was modified",
        },
      },
    });
  }
}

async function cancelRegistration(): Promise<void> {
  if (model.registration.execution.state !== "running") return;
  update({
    ...model,
    registration: {
      ...model.registration,
      execution: {
        ...model.registration.execution,
        state: "cancelling",
        message: "Cancellation requested · finishing the current bounded band…",
      },
    },
  });
  try {
    await cancelRegistrationPlan();
  } catch {
    update({
      ...model,
      registration: {
        ...model.registration,
        execution: {
          ...model.registration.execution,
          state: "error",
          message: "Cancellation request failed · native task state is unknown",
        },
      },
    });
  }
}

async function executeStack(): Promise<void> {
  const plan = model.registration.plan;
  const registered = model.registration.execution.result;
  const stack = model.registration.stack;
  const settings = stack.settings;
  const weightPreflight = buildQualityWeightPreflight(
    plan,
    model.activeRole === "light" && model.lightFrameView === "calibrated"
      ? model.frames
      : [],
    settings.weightReferenceFrameId,
  );
  if (
    model.registration.planState !== "ready" ||
    !plan ||
    !registered ||
    registered.planSha256 !== plan.planSha256 ||
    stack.state === "running" ||
    stack.state === "cancelling" ||
    isActiveExecutionState(model.registration.execution.state) ||
    isActiveExecutionState(model.calibration.execution.state) ||
    isActiveExecutionState(model.calibration.lightExecution.state) ||
    (settings.estimator === "weighted_mean" && !weightPreflight.ready)
  ) {
    return;
  }
  let nativeWeightDigest: string | null = null;
  let nativeWeightTicket: number | null = null;
  if (settings.estimator === "weighted_mean") {
    const referenceFrameId = weightPreflight.referenceFrameId;
    if (!referenceFrameId) return;
    const preflightTicket = ++registeredStackTicket;
    nativeWeightTicket = preflightTicket;
    update({
      ...model,
      registration: {
        ...model.registration,
        stack: {
          ...model.registration.stack,
          state: "idle",
          weightPreflight: null,
          message: "Recomputing canonical weight evidence in Rust…",
        },
      },
    });
    try {
      const nativePreflight = await previewRegisteredWeights(
        plan.planSha256,
        plan.frames.map((frame) => frame.frameId),
        referenceFrameId,
        weightPreflight.evidence,
      );
      const expectedIds = new Set(plan.frames.map((frame) => frame.frameId));
      const returnedIds = new Set(
        nativePreflight.weights.map((weight) => weight.frameId),
      );
      if (
        preflightTicket !== registeredStackTicket ||
        model.registration.plan?.planSha256 !== plan.planSha256
      ) {
        return;
      }
      if (
        nativePreflight.planSha256 !== plan.planSha256 ||
        nativePreflight.referenceFrameId !== referenceFrameId ||
        nativePreflight.parametersSha256.length !== 64 ||
        returnedIds.size !== expectedIds.size ||
        [...expectedIds].some((frameId) => !returnedIds.has(frameId))
      ) {
        throw new Error(
          "Native weight evidence does not match the reviewed plan",
        );
      }
      nativeWeightDigest = nativePreflight.parametersSha256;
      update({
        ...model,
        registration: {
          ...model.registration,
          stack: {
            ...model.registration.stack,
            state: "idle",
            weightPreflight: nativePreflight,
            message: `Native weights sealed · SHA-256 ${nativeWeightDigest}`,
          },
        },
      });
    } catch {
      if (preflightTicket !== registeredStackTicket) return;
      update({
        ...model,
        registration: {
          ...model.registration,
          stack: {
            ...model.registration.stack,
            state: "error",
            weightPreflight: null,
            message:
              "Native weight preflight failed · integration was not started",
          },
        },
      });
      return;
    }
  }
  const outputPath = await selectRegisteredStackOutput();
  if (
    !outputPath ||
    model.registration.plan?.planSha256 !== plan.planSha256 ||
    model.registration.execution.result?.planSha256 !== plan.planSha256 ||
    (nativeWeightTicket !== null &&
      nativeWeightTicket !== registeredStackTicket)
  ) {
    return;
  }
  const ticket = ++registeredStackTicket;
  stackReportTicket += 1;
  stackSourceVerificationTicket += 1;
  update({
    ...model,
    registration: {
      ...model.registration,
      stack: {
        state: "running",
        outputPath,
        progress: null,
        result: null,
        previewState: "idle",
        preview: null,
        sciencePreview: null,
        selectedProduct: "science",
        overlayOpacity: stack.overlayOpacity,
        histogramState: "idle",
        histogram: null,
        pixelInspectionState: "idle",
        pixelInspection: null,
        weightPreflight: model.registration.stack.weightPreflight,
        reportInspectionState: "idle",
        reportInspection: null,
        reportInspectionPath: null,
        sourceVerificationState: "idle",
        sourceVerification: null,
        sourceVerificationProgress: null,
        settings,
        message:
          settings.estimator === "strict_mean"
            ? "Integrating the sealed common crop with strict F64 mean…"
            : settings.estimator === "weighted_mean"
              ? "Integrating with identity-bound balanced PSF weights…"
              : "Integrating with deterministic percentile rejection…",
      },
    },
  });
  const onProgress = (progress: RegisteredStackProgress): void => {
    if (ticket !== registeredStackTicket) return;
    const state = model.registration.stack.state;
    if (state !== "running" && state !== "cancelling") return;
    const units = progress.totalUnits
      ? ` · ${progress.completedUnits}/${progress.totalUnits}`
      : "";
    update({
      ...model,
      registration: {
        ...model.registration,
        stack: {
          ...model.registration.stack,
          state,
          progress,
          message: `Registered common crop · ${progress.stage}${units}`,
        },
      },
    });
  };
  try {
    const result = await executeRegisteredStack(
      outputPath,
      {
        referenceFrameId: plan.referenceFrameId,
        sourceFrameIds: plan.frames
          .filter((frame) => !frame.reference)
          .map((frame) => frame.frameId),
      },
      plan.planSha256,
      registered.frames.map((frame) => ({
        frameId: frame.frameId,
        path: frame.outputPath,
      })),
      settings.estimator === "weighted_mean" ? weightPreflight.evidence : [],
      settings.estimator === "weighted_mean"
        ? weightPreflight.referenceFrameId
        : null,
      {
        bandHeight: 128,
        memoryLimitBytes: model.calibration.lightSettings.memoryLimitBytes,
        integration: settings,
      },
      onProgress,
    );
    if (ticket !== registeredStackTicket) return;
    update({
      ...model,
      registration: {
        ...model.registration,
        stack: {
          state: "completed",
          outputPath,
          progress: model.registration.stack.progress,
          result,
          previewState: "loading",
          preview: null,
          sciencePreview: null,
          selectedProduct: "science",
          overlayOpacity: model.registration.stack.overlayOpacity,
          histogramState: "idle",
          histogram: null,
          pixelInspectionState: "idle",
          pixelInspection: null,
          weightPreflight: model.registration.stack.weightPreflight,
          reportInspectionState: "idle",
          reportInspection: null,
          reportInspectionPath: result.reportPath,
          sourceVerificationState: "idle",
          sourceVerification: null,
          sourceVerificationProgress: null,
          settings,
          message: `${result.width} × ${result.height} × ${result.planes} integrated atomically · ${result.estimator}${nativeWeightDigest ? ` · weights ${nativeWeightDigest.slice(0, 12)}…` : ""} · peak ${formatMemory(result.peakReservedBytes)}`,
        },
      },
    });
    void loadRegisteredStackPreview(
      registeredStackPreviewSourceFromResult(result),
      "science",
    );
  } catch (error) {
    if (ticket !== registeredStackTicket) return;
    const cancelled = nativeErrorCode(error) === "registered_stack_cancelled";
    update({
      ...model,
      registration: {
        ...model.registration,
        stack: {
          ...model.registration.stack,
          state: cancelled ? "idle" : "error",
          result: null,
          weightPreflight: null,
          message: cancelled
            ? "Integration cancelled · no partial stack published"
            : "Integration failed safely · no existing output was modified",
        },
      },
    });
  }
}

async function cancelStack(): Promise<void> {
  if (model.registration.stack.state !== "running") return;
  update({
    ...model,
    registration: {
      ...model.registration,
      stack: {
        ...model.registration.stack,
        state: "cancelling",
        message: "Cancellation requested · finishing the current bounded band…",
      },
    },
  });
  try {
    await cancelRegisteredStack();
  } catch {
    update({
      ...model,
      registration: {
        ...model.registration,
        stack: {
          ...model.registration.stack,
          state: "error",
          message: "Cancellation request failed · native task state is unknown",
        },
      },
    });
  }
}

async function loadRegisteredStackPreview(
  source: RegisteredStackPreviewSource,
  product: RegisteredStackProductView,
): Promise<void> {
  const ticket = ++registeredStackPreviewTicket;
  let scienceResource: PreviewResource | null = null;
  let diagnosticResource: PreviewResource | null = null;
  try {
    scienceResource = await requestRegisteredStackProductPreview(
      source,
      "science",
    );
    if (product !== "science") {
      diagnosticResource = await requestRegisteredStackProductPreview(
        source,
        product,
      );
    }
    if (
      ticket !== registeredStackPreviewTicket ||
      registeredStackPreviewSource(model.registration.stack)?.identity !==
        source.identity ||
      model.registration.stack.selectedProduct !== product
    ) {
      scienceResource.revoke();
      diagnosticResource?.revoke();
      return;
    }
    clearRegisteredStackPreviewResources();
    registeredStackSciencePreviewResource = scienceResource;
    registeredStackDiagnosticPreviewResource = diagnosticResource;
    const selectedPreview =
      diagnosticResource?.preview ?? scienceResource.preview;
    update({
      ...model,
      registration: {
        ...model.registration,
        stack: {
          ...model.registration.stack,
          previewState: "ready",
          preview: selectedPreview,
          sciencePreview: scienceResource.preview,
          histogramState: product === "science" ? "idle" : "loading",
          histogram: null,
        },
      },
    });
    if (product !== "science") {
      void loadRegisteredStackHistogram(source, product, ticket);
    }
  } catch {
    scienceResource?.revoke();
    diagnosticResource?.revoke();
    if (ticket !== registeredStackPreviewTicket) return;
    update({
      ...model,
      registration: {
        ...model.registration,
        stack: {
          ...model.registration.stack,
          previewState: "error",
          preview: null,
          sciencePreview: null,
          histogramState: "idle",
          histogram: null,
        },
      },
    });
  }
}

async function loadRegisteredStackHistogram(
  source: RegisteredStackPreviewSource,
  product: Exclude<RegisteredStackProductView, "science">,
  ticket: number,
): Promise<void> {
  const path = registeredStackProductPath(source, product);
  if (!path) return;
  try {
    const histogram = await inspectRejectionHistogram(path);
    if (
      ticket !== registeredStackPreviewTicket ||
      registeredStackPreviewSource(model.registration.stack)?.identity !==
        source.identity ||
      model.registration.stack.selectedProduct !== product
    ) {
      return;
    }
    update({
      ...model,
      registration: {
        ...model.registration,
        stack: {
          ...model.registration.stack,
          histogramState: "ready",
          histogram,
        },
      },
    });
  } catch {
    if (ticket !== registeredStackPreviewTicket) return;
    update({
      ...model,
      registration: {
        ...model.registration,
        stack: {
          ...model.registration.stack,
          histogramState: "error",
          histogram: null,
        },
      },
    });
  }
}

async function inspectRegisteredStackPixel(
  x: number,
  y: number,
): Promise<void> {
  const result = model.registration.stack.result;
  if (
    !result ||
    model.registration.stack.state !== "completed" ||
    !Number.isInteger(x) ||
    !Number.isInteger(y) ||
    x < 0 ||
    y < 0 ||
    x >= result.width ||
    y >= result.height
  ) {
    return;
  }
  const ticket = ++stackPixelTicket;
  update({
    ...model,
    registration: {
      ...model.registration,
      stack: {
        ...model.registration.stack,
        pixelInspectionState: "loading",
        pixelInspection: null,
      },
    },
  });
  try {
    const inspection = await inspectStackPixel({
      sciencePath: result.outputPath,
      lowRejectionPath: result.lowRejectionMapPath,
      highRejectionPath: result.highRejectionMapPath,
      x,
      y,
    });
    if (
      ticket !== stackPixelTicket ||
      model.registration.stack.result?.outputPath !== result.outputPath
    ) {
      return;
    }
    update({
      ...model,
      registration: {
        ...model.registration,
        stack: {
          ...model.registration.stack,
          pixelInspectionState: "ready",
          pixelInspection: inspection,
        },
      },
    });
  } catch {
    if (ticket !== stackPixelTicket) return;
    update({
      ...model,
      registration: {
        ...model.registration,
        stack: {
          ...model.registration.stack,
          pixelInspectionState: "error",
          pixelInspection: null,
        },
      },
    });
  }
}

async function openStackReport(): Promise<void> {
  const path = await selectRegisteredStackReport();
  if (!path) return;
  await inspectStackReport(path);
}

async function inspectStackReport(selectedPath?: string): Promise<void> {
  const result = model.registration.stack.result;
  const path =
    selectedPath ??
    result?.reportPath ??
    model.registration.stack.reportInspectionPath;
  if (!path) return;
  const activeResult = result?.reportPath === path ? result : null;
  const ticket = ++stackReportTicket;
  stackSourceVerificationTicket += 1;
  if (activeResult === null) {
    registeredStackPreviewTicket += 1;
    stackPixelTicket += 1;
    clearRegisteredStackPreviewResources();
  }
  update({
    ...model,
    registration: {
      ...model.registration,
      stack: {
        ...model.registration.stack,
        reportInspectionState: "loading",
        reportInspection: null,
        reportInspectionPath: path,
        sourceVerificationState: "idle",
        sourceVerification: null,
        sourceVerificationProgress: null,
        previewState:
          activeResult === null
            ? "idle"
            : model.registration.stack.previewState,
        preview:
          activeResult === null ? null : model.registration.stack.preview,
        sciencePreview:
          activeResult === null
            ? null
            : model.registration.stack.sciencePreview,
        selectedProduct:
          activeResult === null
            ? "science"
            : model.registration.stack.selectedProduct,
        histogramState:
          activeResult === null
            ? "idle"
            : model.registration.stack.histogramState,
        histogram:
          activeResult === null ? null : model.registration.stack.histogram,
        pixelInspectionState:
          activeResult === null
            ? "idle"
            : model.registration.stack.pixelInspectionState,
        pixelInspection:
          activeResult === null
            ? null
            : model.registration.stack.pixelInspection,
      },
    },
  });
  try {
    const inspection = await inspectRegisteredStackReport(path);
    if (
      ticket !== stackReportTicket ||
      model.registration.stack.reportInspectionPath !== path
    ) {
      return;
    }
    if (
      activeResult !== null &&
      (inspection.reportSha256 !== activeResult.reportSha256 ||
        inspection.planSha256 !== activeResult.planSha256)
    ) {
      throw new Error("Validated report does not match the active stack");
    }
    update({
      ...model,
      registration: {
        ...model.registration,
        stack: {
          ...model.registration.stack,
          reportInspectionState: "ready",
          reportInspection: inspection,
          reportInspectionPath: path,
          sourceVerificationState: "idle",
          sourceVerification: null,
          sourceVerificationProgress: null,
        },
      },
    });
    if (activeResult === null) {
      selectRegisteredStackProduct("science");
    }
  } catch {
    if (ticket !== stackReportTicket) return;
    update({
      ...model,
      registration: {
        ...model.registration,
        stack: {
          ...model.registration.stack,
          reportInspectionState: "error",
          reportInspection: null,
        },
      },
    });
  }
}

async function verifyStackReportSources(): Promise<void> {
  const stack = model.registration.stack;
  const reportPath = stack.reportInspectionPath;
  const report = stack.reportInspection;
  if (reportPath === null || report === null) return;
  const ticket = ++stackSourceVerificationTicket;
  const sourceDirectory = await selectRegisteredStackSourceDirectory();
  if (
    sourceDirectory === null ||
    ticket !== stackSourceVerificationTicket ||
    model.registration.stack.reportInspectionPath !== reportPath ||
    model.registration.stack.reportInspection?.reportSha256 !==
      report.reportSha256
  ) {
    return;
  }
  update({
    ...model,
    registration: {
      ...model.registration,
      stack: {
        ...model.registration.stack,
        sourceVerificationState: "loading",
        sourceVerification: null,
        sourceVerificationProgress: null,
      },
    },
  });
  const onProgress = (
    progress: RegisteredStackSourceVerificationProgress,
  ): void => {
    if (
      ticket !== stackSourceVerificationTicket ||
      model.registration.stack.reportInspectionPath !== reportPath
    ) {
      return;
    }
    update({
      ...model,
      registration: {
        ...model.registration,
        stack: {
          ...model.registration.stack,
          sourceVerificationProgress: progress,
        },
      },
    });
  };
  try {
    const verification = await verifyRegisteredStackSources(
      reportPath,
      sourceDirectory,
      onProgress,
    );
    if (
      ticket !== stackSourceVerificationTicket ||
      model.registration.stack.reportInspectionPath !== reportPath ||
      model.registration.stack.reportInspection?.reportSha256 !==
        report.reportSha256
    ) {
      return;
    }
    if (verification.reportSha256 !== report.reportSha256) {
      throw new Error(
        "Source verification does not match the inspected report",
      );
    }
    update({
      ...model,
      registration: {
        ...model.registration,
        stack: {
          ...model.registration.stack,
          sourceVerificationState: "ready",
          sourceVerification: verification,
        },
      },
    });
  } catch (error) {
    if (ticket !== stackSourceVerificationTicket) return;
    const cancelled =
      nativeErrorCode(error) ===
      "registered_stack_source_verification_cancelled";
    update({
      ...model,
      registration: {
        ...model.registration,
        stack: {
          ...model.registration.stack,
          sourceVerificationState: cancelled ? "idle" : "error",
          sourceVerification: null,
          sourceVerificationProgress: null,
        },
      },
    });
  }
}

async function cancelStackReportSourceVerification(): Promise<void> {
  if (model.registration.stack.sourceVerificationState !== "loading") return;
  update({
    ...model,
    registration: {
      ...model.registration,
      stack: {
        ...model.registration.stack,
        sourceVerificationState: "cancelling",
      },
    },
  });
  try {
    await cancelRegisteredStackSourceVerification();
  } catch {
    update({
      ...model,
      registration: {
        ...model.registration,
        stack: {
          ...model.registration.stack,
          sourceVerificationState: "error",
          sourceVerificationProgress: null,
        },
      },
    });
  }
}

function returnToActiveStack(): void {
  const stack = model.registration.stack;
  const result = stack.result;
  registeredStackPreviewTicket += 1;
  stackPixelTicket += 1;
  stackReportTicket += 1;
  stackSourceVerificationTicket += 1;
  clearRegisteredStackPreviewResources();
  update({
    ...model,
    registration: {
      ...model.registration,
      stack: {
        ...stack,
        previewState: result === null ? "idle" : "loading",
        preview: null,
        sciencePreview: null,
        selectedProduct: "science",
        histogramState: "idle",
        histogram: null,
        pixelInspectionState: "idle",
        pixelInspection: null,
        reportInspectionState: "idle",
        reportInspection: null,
        reportInspectionPath: result?.reportPath ?? null,
        sourceVerificationState: "idle",
        sourceVerification: null,
        sourceVerificationProgress: null,
      },
    },
  });
  if (result !== null && stack.state === "completed") {
    void loadRegisteredStackPreview(
      registeredStackPreviewSourceFromResult(result),
      "science",
    );
  }
}

async function requestRegisteredStackProductPreview(
  source: RegisteredStackPreviewSource,
  product: RegisteredStackProductView,
): Promise<PreviewResource> {
  const path = registeredStackProductPath(source, product);
  if (!path) throw new Error("registered stack product is unavailable");
  const content =
    product === "science" && source.planes === 3
      ? ({ kind: "rgb" } as const)
      : ({ kind: "scalar", plane: 0 } as const);
  const transform = await estimateFitsPreviewTransform({
    path,
    content,
    ...previewBounds,
  });
  return requestFitsPreview({
    frameId: `${source.identity}:${product}`,
    path,
    content,
    ...previewBounds,
    blackPoint: transform.blackPoint,
    whitePoint: transform.whitePoint,
    midtone: transform.midtone,
    transfer: { kind: "midtones" },
    palette:
      product === "rejection_low"
        ? "rejection_low"
        : product === "rejection_high"
          ? "rejection_high"
          : "grayscale",
  });
}

function registeredStackProductPath(
  source: RegisteredStackPreviewSource,
  product: RegisteredStackProductView,
): string | null {
  switch (product) {
    case "science":
      return source.sciencePath;
    case "rejection_low":
      return source.lowRejectionPath;
    case "rejection_high":
      return source.highRejectionPath;
  }
}

interface RegisteredStackPreviewSource {
  readonly identity: string;
  readonly planes: number;
  readonly sciencePath: string;
  readonly lowRejectionPath: string | null;
  readonly highRejectionPath: string | null;
}

function registeredStackPreviewSourceFromResult(
  result: NonNullable<ReviewViewModel["registration"]["stack"]["result"]>,
): RegisteredStackPreviewSource {
  return {
    identity: `${result.planSha256}:registered-stack`,
    planes: result.planes,
    sciencePath: result.outputPath,
    lowRejectionPath: result.lowRejectionMapPath,
    highRejectionPath: result.highRejectionMapPath,
  };
}

function registeredStackPreviewSource(
  stack: ReviewViewModel["registration"]["stack"],
): RegisteredStackPreviewSource | null {
  const report = stack.reportInspection;
  const reportIsExternal =
    report !== null &&
    (stack.result === null ||
      stack.reportInspectionPath !== stack.result.reportPath);
  if (
    stack.result !== null &&
    stack.state === "completed" &&
    !reportIsExternal
  ) {
    return registeredStackPreviewSourceFromResult(stack.result);
  }
  if (report === null) return null;
  const verifiedPath = (role: RegisteredStackProductView): string | null =>
    report.products.find(
      (product) => product.role === role && product.status === "verified",
    )?.path ?? null;
  const sciencePath = verifiedPath("science");
  if (sciencePath === null) return null;
  return {
    identity: `${report.reportSha256}:reported-stack`,
    planes: report.planes,
    sciencePath,
    lowRejectionPath: verifiedPath("rejection_low"),
    highRejectionPath: verifiedPath("rejection_high"),
  };
}

function selectRegisteredStackProduct(
  product: RegisteredStackProductView,
): void {
  const stack = model.registration.stack;
  const source = registeredStackPreviewSource(stack);
  if (!source || !registeredStackProductPath(source, product)) return;
  registeredStackPreviewTicket += 1;
  stackPixelTicket += 1;
  clearRegisteredStackPreviewResources();
  update({
    ...model,
    registration: {
      ...model.registration,
      stack: {
        ...stack,
        selectedProduct: product,
        previewState: "loading",
        preview: null,
        sciencePreview: null,
        histogramState: product === "science" ? "idle" : "loading",
        histogram: null,
        pixelInspectionState: "idle",
        pixelInspection: null,
      },
    },
  });
  void loadRegisteredStackPreview(source, product);
}

function selectRegisteredFrame(frameId: string): void {
  const review = model.registration.resultReview;
  if (!review.frames.some((frame) => frame.id === frameId)) return;
  registrationPreviewTicket += 1;
  update({
    ...model,
    registration: {
      ...model.registration,
      resultReview: {
        ...review,
        selectedFrameId: frameId,
        state: "loading",
        preview: null,
        message: "Rendering registered pixels with the locked stretch…",
      },
    },
  });
  void loadSelectedRegisteredPreview();
}

function setRegisteredPlaying(playing: boolean): void {
  stopRegistrationBlinkTimer();
  const review = model.registration.resultReview;
  const enabled = playing && review.frames.length > 1;
  update({
    ...model,
    registration: {
      ...model.registration,
      resultReview: { ...review, playing: enabled },
    },
  });
  if (enabled) {
    registrationBlinkTimer = window.setInterval(
      () => stepRegisteredFrame("forward"),
      900,
    );
  }
}

function stepRegisteredFrame(direction: "backward" | "forward"): void {
  const review = model.registration.resultReview;
  const current = review.frames.findIndex(
    (frame) => frame.id === review.selectedFrameId,
  );
  if (current < 0 || review.frames.length < 2) return;
  const offset = direction === "forward" ? 1 : -1;
  const next = (current + offset + review.frames.length) % review.frames.length;
  const frame = review.frames[next];
  if (frame) selectRegisteredFrame(frame.id);
}

async function loadSelectedRegisteredPreview(): Promise<void> {
  const review = model.registration.resultReview;
  const frame = review.frames.find(
    (candidate) => candidate.id === review.selectedFrameId,
  );
  const planDigest = model.registration.execution.result?.planSha256;
  if (!frame || !planDigest) return;

  const ticket = ++registrationPreviewTicket;
  try {
    let transform = registrationSharedTransform;
    if (!transform) {
      transform = await estimateFitsPreviewTransform({
        path: frame.outputPath,
        content: frame.previewContent,
        ...previewBounds,
      });
      if (ticket !== registrationPreviewTicket) return;
      registrationSharedTransform = transform;
    }
    const request = registeredPreviewRequest(frame, planDigest, transform);
    const cacheKey = previewCacheKey(request, transform.algorithmId);
    let cached = registrationPreviewCache.get(cacheKey);
    const pending = registrationPreviewPrefetch.pending(cacheKey);
    if (!cached && pending) {
      await pending;
      if (ticket !== registrationPreviewTicket) return;
      cached = registrationPreviewCache.get(cacheKey);
    }
    if (!cached) {
      const resource = await requestFitsPreview(request);
      if (ticket !== registrationPreviewTicket) {
        resource.revoke();
        return;
      }
      releaseEphemeralRegistrationPreview();
      if (!registrationPreviewCache.put(cacheKey, resource)) {
        ephemeralRegistrationPreview = resource;
      }
      cached = resource;
    } else {
      releaseEphemeralRegistrationPreview();
    }
    if (ticket !== registrationPreviewTicket) return;
    update({
      ...model,
      registration: {
        ...model.registration,
        resultReview: {
          ...model.registration.resultReview,
          state: "ready",
          preview: cached.preview,
          message: "Published registered pixels · shared stretch locked",
          sharedStretchLabel: "Registered stretch · locked",
        },
      },
    });
    scheduleRegisteredPreviewPrefetch(transform, planDigest);
  } catch {
    if (ticket !== registrationPreviewTicket) return;
    update({
      ...model,
      registration: {
        ...model.registration,
        resultReview: {
          ...model.registration.resultReview,
          state: "error",
          preview: null,
          message: "Registered FITS preview could not be rendered",
          sharedStretchLabel: "Registered stretch · unavailable",
        },
      },
    });
  }
}

function registeredPreviewRequest(
  frame: ReviewViewModel["registration"]["resultReview"]["frames"][number],
  planDigest: string,
  transform: EstimatedDisplayTransform,
): FitsPreviewRequest {
  return {
    frameId: `${planDigest}:registered:${frame.id}`,
    path: frame.outputPath,
    content: frame.previewContent,
    ...previewBounds,
    blackPoint: transform.blackPoint,
    whitePoint: transform.whitePoint,
    midtone: transform.midtone,
    transfer: { kind: "midtones" },
  };
}

function scheduleRegisteredPreviewPrefetch(
  transform: EstimatedDisplayTransform,
  planDigest: string,
): void {
  const review = model.registration.resultReview;
  const selected = review.frames.findIndex(
    (frame) => frame.id === review.selectedFrameId,
  );
  if (selected < 0 || review.frames.length < 2) return;
  const candidates = [1, -1]
    .map((offset) =>
      review.frames.at(
        (selected + offset + review.frames.length) % review.frames.length,
      ),
    )
    .filter((frame) => frame !== undefined);
  for (const frame of candidates) {
    const request = registeredPreviewRequest(frame, planDigest, transform);
    const cacheKey = previewCacheKey(request, transform.algorithmId);
    if (registrationPreviewCache.has(cacheKey)) continue;
    void registrationPreviewPrefetch.schedule(
      cacheKey,
      () => requestFitsPreview(request),
      (resource) => registrationPreviewCache.put(cacheKey, resource),
    );
  }
}

function updateCalibrationSettings(settings: MasterPlanSettings): void {
  if (
    model.calibration.execution.state === "running" ||
    model.calibration.execution.state === "cancelling" ||
    model.calibration.lightExecution.state === "running" ||
    model.calibration.lightExecution.state === "cancelling" ||
    isRegistrationWorkActive()
  ) {
    return;
  }
  clearRegistrationPreviewResources();
  update({
    ...model,
    registration: {
      ...model.registration,
      execution: idleRegistrationExecution(
        "Calibration changed · export fresh Light artifacts",
      ),
      stack: idleRegisteredStack(
        "Calibration changed · publish and integrate fresh registered artifacts",
      ),
      resultReview: idleRegistrationResultReview(
        "Calibration changed · register a fresh frame set",
      ),
    },
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
    model.calibration.lightExecution.state === "cancelling" ||
    isRegistrationWorkActive()
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
    isActiveExecutionState(model.calibration.lightExecution.state) ||
    isRegistrationWorkActive()
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
    model.calibration.execution.state === "cancelling" ||
    isRegistrationWorkActive()
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
    model.calibration.plan?.lightPlan?.planSha256 !== selectedLightPlanSha256 ||
    isRegistrationWorkActive()
  ) {
    return;
  }
  const ticket = ++lightExecutionTicket;
  clearRegistrationPreviewResources();
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
    registration: {
      ...model.registration,
      execution: idleRegistrationExecution(
        "Light calibration is running · wait for the complete artifact set",
      ),
      stack: idleRegisteredStack(
        "Light calibration is running · registered integration reset",
      ),
      resultReview: idleRegistrationResultReview(
        "Light calibration is running · registered review reset",
      ),
    },
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

function isRegistrationWorkActive(): boolean {
  return (
    isActiveExecutionState(model.registration.execution.state) ||
    isActiveExecutionState(model.registration.stack.state)
  );
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
    model.calibration.lightExecution.state === "cancelling" ||
    isRegistrationWorkActive()
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
  const qualityOrigin = qualityOrigins.get(artifactKey) ?? null;
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
      ? qualityOrigin === "restored"
        ? `Verified cache · ${qualityResultMessage(cachedQuality)}`
        : qualityResultMessage(cachedQuality)
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
    qualityOrigin,
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
    signalToNoise: null,
    fwhmPixels: null,
    eccentricity: null,
    detectedStars: null,
    usableStars: null,
    background: null,
    noise: null,
  };
}

function qualityMetrics(result: FrameQualityResult): ReviewFrame["metrics"] {
  return {
    signalToNoise: result.signalToNoise,
    fwhmPixels: result.fwhmPixels,
    eccentricity: result.eccentricity,
    detectedStars: result.detectedStars,
    usableStars: result.usableStars,
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
        frameSelection: resetFrameSelection(model.frameSelection.rules),
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
    applyQualityResult(
      frame.id,
      cached,
      qualityOrigins.get(artifactKey) ?? "measured",
    );
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
        ? await inspectRgbFrameQuality(frame.id, frame.sourcePath)
        : await inspectCfaFrameQuality(
            frame.id,
            frame.sourcePath,
            frame.bayerPattern!,
          );
    if (sessionRevision !== qualitySessionRevision) return;
    qualityCache.set(artifactKey, result);
    qualityOrigins.set(artifactKey, "measured");
    applyQualityResult(frame.id, result, "measured");
  } catch {
    if (sessionRevision !== qualitySessionRevision) return;
    updateQualityFrame(frame.id, {
      qualityState: "error",
      qualityMessage: "Strict quality diagnostics could not be completed",
      qualityProfileId: null,
      qualityOrigin: null,
      metrics: emptyQualityMetrics(),
    });
  } finally {
    qualityPending.delete(artifactKey);
  }
}

function applyQualityResult(
  frameId: string,
  result: FrameQualityResult,
  origin: "measured" | "restored",
): void {
  updateQualityFrame(frameId, {
    qualityState: "ready",
    qualityMessage:
      origin === "restored"
        ? `Verified cache · ${qualityResultMessage(result)}`
        : qualityResultMessage(result),
    qualityProfileId: result.profileId,
    qualityOrigin: origin,
    metrics: qualityMetrics(result),
  });
}

function updateQualityFrame(
  frameId: string,
  patch: Partial<
    Pick<
      ReviewFrame,
      | "qualityState"
      | "qualityMessage"
      | "qualityProfileId"
      | "qualityOrigin"
      | "metrics"
    >
  >,
): void {
  if (!model.frames.some((frame) => frame.id === frameId)) return;
  frameSelectionTicket += 1;
  update({
    ...model,
    frames: model.frames.map((frame) =>
      frame.id === frameId ? { ...frame, ...patch } : frame,
    ),
    frameSelection: resetFrameSelection(model.frameSelection.rules),
  });
}

async function previewAutomaticSelection(): Promise<void> {
  if (
    model.activeRole !== "light" ||
    model.frameSelection.state === "previewing"
  ) {
    return;
  }
  const frames = model.frames.filter(
    (frame) => frame.sourcePath !== null && frame.qualityState === "ready",
  );
  if (frames.length !== model.frames.length || frames.length === 0) {
    update({
      ...model,
      frameSelection: {
        ...model.frameSelection,
        state: "error",
        plan: null,
        message:
          "Measure quality for every shown Light before previewing rules",
      },
    });
    return;
  }

  const ticket = ++frameSelectionTicket;
  const rules = model.frameSelection.rules;
  update({
    ...model,
    frameSelection: {
      ...model.frameSelection,
      state: "previewing",
      plan: null,
      message: "Rust is evaluating every rule against native evidence…",
    },
  });
  try {
    const plan = await previewFrameSelection(frames, rules);
    if (ticket !== frameSelectionTicket) return;
    const rejected = plan.frames.filter(
      (frame) => frame.proposal === "reject",
    ).length;
    update({
      ...model,
      frameSelection: {
        state: "ready",
        rules,
        plan,
        message: `${plan.frames.length - rejected} retained · ${rejected} proposed rejects · no decisions changed`,
      },
    });
  } catch {
    if (ticket !== frameSelectionTicket) return;
    update({
      ...model,
      frameSelection: {
        ...model.frameSelection,
        state: "error",
        plan: null,
        message:
          "Native selection preview could not validate the current evidence",
      },
    });
  }
}

async function applyAutomaticSelection(): Promise<void> {
  const plan = model.frameSelection.plan;
  if (
    !importedSession ||
    !plan ||
    model.frameSelection.state !== "ready" ||
    model.decisionPending ||
    isRegistrationWorkActive()
  ) {
    return;
  }
  const frames = model.frames.filter(
    (frame) => frame.sourcePath !== null && frame.qualityState === "ready",
  );
  if (frames.length !== model.frames.length || frames.length === 0) return;
  const rules = model.frameSelection.rules;
  const applied = await runDecisionTransaction(() =>
    applyFrameSelection(frames, rules, plan.planSha256),
  );
  if (!applied) return;
  frameSelectionTicket += 1;
  update({
    ...model,
    frameSelection: {
      state: "idle",
      rules,
      plan: null,
      message:
        "Recommendations applied to undecided Lights · one undo restores the batch",
    },
  });
}

function resetFrameSelection(
  rules: readonly FrameSelectionRule[],
): ReviewViewModel["frameSelection"] {
  return {
    state: "idle",
    rules,
    plan: null,
    message: "Measure every Light, then preview automatic recommendations",
  };
}

function selectRole(role: FrameRole): void {
  qualityBatchTicket += 1;
  qualitySessionRevision += 1;
  frameSelectionTicket += 1;
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
        frameSelection: resetFrameSelection(model.frameSelection.rules),
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
      frameSelection: resetFrameSelection(model.frameSelection.rules),
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
    frameSelection: resetFrameSelection(model.frameSelection.rules),
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
  frameSelectionTicket += 1;
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
    frameSelection: resetFrameSelection(model.frameSelection.rules),
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

function stopRegistrationBlinkTimer(): void {
  if (registrationBlinkTimer !== null) {
    window.clearInterval(registrationBlinkTimer);
    registrationBlinkTimer = null;
  }
  if (model.registration.resultReview.playing) {
    model = {
      ...model,
      registration: {
        ...model.registration,
        resultReview: {
          ...model.registration.resultReview,
          playing: false,
        },
      },
    };
  }
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

function releaseEphemeralRegistrationPreview(): void {
  ephemeralRegistrationPreview?.revoke();
  ephemeralRegistrationPreview = null;
}

function clearRegisteredStackPreviewResources(): void {
  registeredStackSciencePreviewResource?.revoke();
  registeredStackDiagnosticPreviewResource?.revoke();
  registeredStackSciencePreviewResource = null;
  registeredStackDiagnosticPreviewResource = null;
}

function clearRegistrationPreviewResources(): void {
  registrationPreviewTicket += 1;
  registeredStackPreviewTicket += 1;
  stackPixelTicket += 1;
  stackReportTicket += 1;
  stackSourceVerificationTicket += 1;
  registrationSharedTransform = null;
  registrationPreviewPrefetch.cancel();
  releaseEphemeralRegistrationPreview();
  registrationPreviewCache.clear();
  clearRegisteredStackPreviewResources();
  stopRegistrationBlinkTimer();
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
  if (isActiveExecutionState(model.registration.execution.state)) {
    void cancelRegistrationPlan();
  }
  if (isActiveExecutionState(model.registration.stack.state)) {
    void cancelRegisteredStack();
  }
  stopBlinkTimer();
  clearPreviewResources();
  clearRegistrationPreviewResources();
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
  if (!importedSession || model.decisionPending || isRegistrationWorkActive())
    return;
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
  if (!importedSession || model.decisionPending || isRegistrationWorkActive())
    return;
  await runDecisionTransaction(() =>
    applyReviewDecision(frameId, { kind: "clear" }),
  );
}

async function undoDecision(): Promise<void> {
  if (
    !importedSession ||
    model.decisionPending ||
    !model.canUndo ||
    isRegistrationWorkActive()
  )
    return;
  await runDecisionTransaction(undoReviewDecision);
}

async function runDecisionTransaction(
  transaction: () => Promise<ReviewDecisionUpdate>,
): Promise<boolean> {
  const sessionRevision = decisionSessionRevision;
  update({ ...model, decisionPending: true });
  try {
    const result = await transaction();
    if (sessionRevision !== decisionSessionRevision) return false;
    applyDecisionUpdate(result);
    return true;
  } catch {
    if (sessionRevision !== decisionSessionRevision) return false;
    update({
      ...model,
      decisionPending: false,
      sessionStatus: {
        tone: "error",
        label: "Review transaction failed · state preserved",
      },
    });
    return false;
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
  const registrationFrames = importedSession
    ? registrationFramesForSession(importedSession)
    : model.registration.frames;
  const registrationMembershipChanged =
    registrationFrames.length !== model.registration.frames.length ||
    registrationFrames.some(
      (frame, index) => frame.id !== model.registration.frames[index]?.id,
    );
  if (registrationMembershipChanged) {
    registrationTicket += 1;
    registrationExecutionTicket += 1;
    registeredStackTicket += 1;
    stackReportTicket += 1;
    stackSourceVerificationTicket += 1;
    clearRegistrationPreviewResources();
  }
  update({
    ...model,
    canUndo: result.canUndo,
    decisionPending: false,
    frames: model.frames.map((frame) => {
      const decision = decisionCache.get(frame.id);
      return decision ? { ...frame, ...decision } : frame;
    }),
    registration: registrationMembershipChanged
      ? {
          ...model.registration,
          state: "idle",
          frames: registrationFrames,
          referenceFrameId: registrationFrames[0]?.id ?? null,
          sourceFrameId: registrationFrames[1]?.id ?? null,
          diagnostic: null,
          solutions: [],
          planState: "idle",
          plan: null,
          execution: idleRegistrationExecution(
            "Review membership changed · rebuild the registration plan",
          ),
          stack: idleRegisteredStack(
            "Review membership changed · publish and integrate a fresh registered set",
          ),
          resultReview: idleRegistrationResultReview(
            "Review membership changed · register a fresh frame set",
          ),
          message:
            registrationFrames.length >= 2
              ? "Rejected Lights excluded · rebuild native geometry"
              : "At least two non-rejected Lights are required",
        }
      : model.registration,
  });
}

function formatByteCount(bytes: number): string {
  if (bytes < 1_024) return `${bytes} B`;
  if (bytes < 1_024 * 1_024) return `${(bytes / 1_024).toFixed(1)} KiB`;
  return `${(bytes / (1_024 * 1_024)).toFixed(1)} MiB`;
}
