import type {
  LightCalibrationProduct,
  LightMasterAssociation,
  MasterPlanSettings,
  MasterProductPlan,
} from "./calibration-bridge.ts";
import type {
  ReviewActions,
  ReviewFrame,
  ReviewRejectionReason,
  ReviewState,
  ReviewViewModel,
  SortDirection,
  SortField,
} from "./model.ts";

export interface ReviewScreen {
  readonly update: (model: ReviewViewModel) => void;
  readonly destroy: () => void;
}

const rejectionReasons = [
  ["blur", "Blur"],
  ["trailing", "Trailing"],
  ["cloud", "Cloud / transparency"],
  ["intrusive_trail", "Satellite or aircraft trail"],
  ["gradient", "Gradient"],
  ["framing", "Framing"],
  ["saturation", "Saturation"],
] as const;

/**
 * Mounts the review workspace as a pure presenter. Scientific decisions,
 * sorting, and Blink advancement are requested through callbacks and must be
 * confirmed by a subsequent model update from the Rust backend.
 */
export function mountReviewScreen(
  root: HTMLElement,
  initialModel: ReviewViewModel,
  actions: ReviewActions,
): ReviewScreen {
  root.innerHTML = shellMarkup();

  const elements = {
    workspaceNavigation: requiredAll<HTMLElement>(root, "[data-workspace]"),
    framesWorkspace: required<HTMLElement>(root, "[data-frames-workspace]"),
    calibrationWorkspace: required<HTMLElement>(
      root,
      "[data-calibration-workspace]",
    ),
    calibrationStatus: required<HTMLElement>(root, "[data-calibration-status]"),
    calibrationProducts: required<HTMLElement>(
      root,
      "[data-calibration-products]",
    ),
    calibrationDigest: required<HTMLElement>(root, "[data-calibration-digest]"),
    pedestalPolicy: required<HTMLSelectElement>(root, "[data-pedestal-policy]"),
    exposureTolerance: required<HTMLInputElement>(
      root,
      "[data-exposure-tolerance]",
    ),
    temperatureTolerance: required<HTMLInputElement>(
      root,
      "[data-temperature-tolerance]",
    ),
    lightTemperatureTolerance: required<HTMLInputElement>(
      root,
      "[data-light-temperature-tolerance]",
    ),
    lightAssociations: required<HTMLElement>(root, "[data-light-associations]"),
    lightDigest: required<HTMLElement>(root, "[data-light-digest]"),
    lightOutputMode: required<HTMLSelectElement>(
      root,
      "[data-light-output-mode]",
    ),
    refreshMasterPlan: required<HTMLButtonElement>(
      root,
      '[data-action="refresh-master-plan"]',
    ),
    executeMasterPlan: required<HTMLButtonElement>(
      root,
      '[data-action="execute-master-plan"]',
    ),
    cancelMasterPlan: required<HTMLButtonElement>(
      root,
      '[data-action="cancel-master-plan"]',
    ),
    executeLightPlan: required<HTMLButtonElement>(
      root,
      '[data-action="execute-light-plan"]',
    ),
    cancelLightPlan: required<HTMLButtonElement>(
      root,
      '[data-action="cancel-light-plan"]',
    ),
    masterExecution: required<HTMLElement>(root, "[data-master-execution]"),
    masterExecutionMessage: required<HTMLElement>(
      root,
      "[data-master-execution-message]",
    ),
    masterExecutionProgress: required<HTMLProgressElement>(
      root,
      "[data-master-execution-progress]",
    ),
    masterExecutionOutput: required<HTMLElement>(
      root,
      "[data-master-execution-output]",
    ),
    lightExecution: required<HTMLElement>(root, "[data-light-execution]"),
    lightExecutionMessage: required<HTMLElement>(
      root,
      "[data-light-execution-message]",
    ),
    lightExecutionProgress: required<HTMLProgressElement>(
      root,
      "[data-light-execution-progress]",
    ),
    lightExecutionOutput: required<HTMLElement>(
      root,
      "[data-light-execution-output]",
    ),
    lightExecutionHeading: required<HTMLElement>(
      root,
      "[data-light-execution-heading]",
    ),
    sessionName: required<HTMLElement>(root, "[data-session-name]"),
    sessionStatus: required<HTMLElement>(root, "[data-session-status]"),
    sessionStatusLabel: required<HTMLElement>(
      root,
      "[data-session-status-label]",
    ),
    importSession: required<HTMLButtonElement>(root, "[data-import-session]"),
    undo: required<HTMLButtonElement>(root, '[data-action="undo"]'),
    roleTabs: required<HTMLElement>(root, "[data-role-tabs]"),
    roleHeading: required<HTMLElement>(root, "[data-role-heading]"),
    frameTable: required<HTMLTableElement>(root, "[data-frame-table]"),
    filter: required<HTMLButtonElement>(root, "[data-filter-frames]"),
    tableBody: required<HTMLTableSectionElement>(root, "[data-frame-rows]"),
    selectionCount: required<HTMLElement>(root, "[data-selection-count]"),
    framePosition: required<HTMLElement>(root, "[data-frame-position]"),
    selectedLabel: required<HTMLElement>(root, "[data-selected-label]"),
    preview: required<HTMLElement>(root, "[data-preview]"),
    previewImage: required<HTMLImageElement>(root, "[data-preview-image]"),
    previewPlaceholder: required<HTMLElement>(
      root,
      "[data-preview-placeholder]",
    ),
    previewBadges: required<HTMLElement>(root, "[data-preview-badges]"),
    previewTitle: required<HTMLElement>(root, "[data-preview-title]"),
    previewDescription: required<HTMLElement>(
      root,
      "[data-preview-description]",
    ),
    fitPreview: required<HTMLButtonElement>(root, '[data-action="viewer-fit"]'),
    actualPreview: required<HTMLButtonElement>(
      root,
      '[data-action="viewer-actual"]',
    ),
    statisticsButton: required<HTMLButtonElement>(
      root,
      '[data-action="open-statistics"]',
    ),
    qualityButton: required<HTMLButtonElement>(
      root,
      '[data-action="measure-quality"]',
    ),
    qualityBatchButton: required<HTMLButtonElement>(
      root,
      '[data-action="measure-all-quality"]',
    ),
    qualityBadge: required<HTMLElement>(root, "[data-quality-badge]"),
    cfaBadge: required<HTMLElement>(root, "[data-cfa-badge]"),
    state: required<HTMLElement>(root, "[data-review-state]"),
    fwhm: required<HTMLElement>(root, "[data-metric-fwhm]"),
    eccentricity: required<HTMLElement>(root, "[data-metric-eccentricity]"),
    stars: required<HTMLElement>(root, "[data-metric-stars]"),
    background: required<HTMLElement>(root, "[data-metric-background]"),
    noise: required<HTMLElement>(root, "[data-metric-noise]"),
    stretch: required<HTMLElement>(root, "[data-stretch-label]"),
    play: required<HTMLButtonElement>(root, '[data-action="toggle-play"]'),
    stepButtons: requiredAll<HTMLButtonElement>(root, "[data-step-action]"),
    decisionButtons: requiredAll<HTMLButtonElement>(
      root,
      "[data-decision-action]",
    ),
    clearDecision: required<HTMLButtonElement>(
      root,
      '[data-action="clear-decision"]',
    ),
    accept: required<HTMLButtonElement>(root, '[data-action="accept"]'),
    decisionControls: required<HTMLElement>(root, "[data-decision-controls]"),
    rejectDialog: required<HTMLElement>(root, "[data-reject-dialog]"),
    rejectFrame: required<HTMLElement>(root, "[data-reject-frame]"),
    statisticsDialog: required<HTMLElement>(root, "[data-statistics-dialog]"),
    statisticsFrame: required<HTMLElement>(root, "[data-statistics-frame]"),
    statisticsStatus: required<HTMLElement>(root, "[data-statistics-status]"),
    statisticsContent: required<HTMLElement>(root, "[data-statistics-content]"),
    statisticsAlgorithm: required<HTMLElement>(
      root,
      "[data-statistics-algorithm]",
    ),
    statisticsAxes: required<HTMLElement>(root, "[data-statistics-axes]"),
    statisticsFormat: required<HTMLElement>(root, "[data-statistics-format]"),
    statisticsHeader: required<HTMLElement>(root, "[data-statistics-header]"),
    statisticsUsable: required<HTMLElement>(root, "[data-statistics-usable]"),
    statisticsExcluded: required<HTMLElement>(
      root,
      "[data-statistics-excluded]",
    ),
    statisticsMinimum: required<HTMLElement>(root, "[data-statistics-minimum]"),
    statisticsMaximum: required<HTMLElement>(root, "[data-statistics-maximum]"),
    statisticsMean: required<HTMLElement>(root, "[data-statistics-mean]"),
    statisticsDeviation: required<HTMLElement>(
      root,
      "[data-statistics-deviation]",
    ),
    statisticsSampleDeviation: required<HTMLElement>(
      root,
      "[data-statistics-sample-deviation]",
    ),
    liveRegion: required<HTMLElement>(root, "[data-live-region]"),
  };

  let model = initialModel;
  let pendingRejectFrameId: string | null = null;
  const sortDirections = new Map<SortField, SortDirection>();

  const onClick = (event: MouseEvent): void => {
    const target = event.target instanceof Element ? event.target : null;
    const actionElement = target?.closest<HTMLElement>("[data-action]");
    if (!actionElement) return;
    const action = actionElement.dataset.action;

    if (action === "select-workspace") {
      const workspace = actionElement.dataset.workspace;
      if (workspace === "frames" || workspace === "calibration") {
        actions.onSelectWorkspace(workspace);
      }
      return;
    }
    if (action === "refresh-master-plan") {
      actions.onRefreshMasterPlan();
      return;
    }
    if (action === "execute-master-plan") {
      actions.onExecuteMasterPlan();
      return;
    }
    if (action === "cancel-master-plan") {
      actions.onCancelMasterPlan();
      return;
    }
    if (action === "execute-light-plan") {
      actions.onExecuteLightPlan();
      return;
    }
    if (action === "cancel-light-plan") {
      actions.onCancelLightPlan();
      return;
    }

    if (action === "select-role") {
      const role = actionElement.dataset.role;
      if (isFrameRole(role)) actions.onSelectRole(role);
      return;
    }
    if (action === "import-session") {
      actions.onImportSession();
      return;
    }
    if (action === "select-frame") {
      const frameId = actionElement.dataset.frameId;
      if (frameId) actions.onSelectFrame(frameId);
      return;
    }
    if (action === "sort") {
      const field = actionElement.dataset.sortField as SortField | undefined;
      if (!field) return;
      const previous = sortDirections.get(field) ?? "descending";
      const direction: SortDirection =
        previous === "ascending" ? "descending" : "ascending";
      sortDirections.set(field, direction);
      actionElement.setAttribute(
        "aria-label",
        `Sort ${humanize(field)} ${direction}`,
      );
      actions.onSort(field, direction);
      return;
    }
    if (action === "accept") {
      const frame = selectedFrame(model);
      if (frame) actions.onSetDecision(frame.id, "accepted", null);
      return;
    }
    if (action === "clear-decision") {
      const frame = selectedFrame(model);
      if (frame) actions.onClearDecision(frame.id);
      return;
    }
    if (action === "open-reject") {
      const frame = selectedFrame(model);
      if (frame) openRejectDialog(frame);
      return;
    }
    if (action === "cancel-reject") {
      closeRejectDialog();
      return;
    }
    if (action === "reject-reason") {
      const reason = actionElement.dataset.reason;
      if (pendingRejectFrameId && isReviewRejectionReason(reason)) {
        actions.onSetDecision(pendingRejectFrameId, "rejected", reason);
        closeRejectDialog();
      }
      return;
    }
    if (action === "undo") {
      actions.onUndo();
      return;
    }
    if (action === "toggle-play") {
      actions.onSetPlaying(!model.playing);
      return;
    }
    if (action === "previous") {
      actions.onRequestStep("backward");
      return;
    }
    if (action === "next") {
      actions.onRequestStep("forward");
      return;
    }
    if (action === "viewer-fit") {
      actions.onSetViewerScale("fit");
      return;
    }
    if (action === "viewer-actual") {
      actions.onSetViewerScale("actual");
      return;
    }
    if (action === "open-statistics") {
      const frame = selectedFrame(model);
      if (frame?.sourcePath) {
        actions.onOpenStatistics(frame.id);
        queueMicrotask(() => {
          required<HTMLButtonElement>(
            elements.statisticsDialog,
            '[data-action="close-statistics"]',
          ).focus();
        });
      }
      return;
    }
    if (action === "measure-quality") {
      const frame = selectedFrame(model);
      if (frame) actions.onMeasureQuality(frame.id);
      return;
    }
    if (action === "measure-all-quality") {
      actions.onMeasureAllQuality();
      return;
    }
    if (action === "close-statistics") {
      actions.onCloseStatistics();
      queueMicrotask(() => elements.statisticsButton.focus());
    }
  };

  const onChange = (event: Event): void => {
    const target = event.target;
    if (target === elements.lightOutputMode) {
      const mode = elements.lightOutputMode.value;
      if (mode === "calibrated_frames" || mode === "integrated") {
        actions.onUpdateLightOutputMode(mode);
      }
      return;
    }
    if (
      target !== elements.pedestalPolicy &&
      target !== elements.exposureTolerance &&
      target !== elements.temperatureTolerance &&
      target !== elements.lightTemperatureTolerance
    ) {
      return;
    }
    const settings = calibrationSettings(elements);
    if (settings) actions.onUpdateCalibrationSettings(settings);
  };

  const onKeyDown = (event: KeyboardEvent): void => {
    if (event.key === "Escape" && !elements.rejectDialog.hidden) {
      closeRejectDialog();
      return;
    }
    if (event.key === "Escape" && !elements.statisticsDialog.hidden) {
      actions.onCloseStatistics();
      queueMicrotask(() => elements.statisticsButton.focus());
      return;
    }
    const target = event.target instanceof Element ? event.target : null;
    if (!elements.rejectDialog.hidden || !elements.statisticsDialog.hidden) {
      return;
    }
    if (!event.repeat && !isTextEntryTarget(target)) {
      const frame = selectedFrame(model);
      const reviewAvailable = reviewCommandsAvailable(model);
      const key = event.key.toLocaleLowerCase("en-US");
      if (
        key === "z" &&
        (event.metaKey || event.ctrlKey) &&
        !event.altKey &&
        !event.shiftKey &&
        reviewAvailable &&
        model.canUndo
      ) {
        event.preventDefault();
        actions.onUndo();
        return;
      }
      if (!event.metaKey && !event.ctrlKey && !event.altKey && frame) {
        if (key === "a" && reviewAvailable && frame.state !== "accepted") {
          event.preventDefault();
          actions.onSetDecision(frame.id, "accepted", null);
          return;
        }
        if (key === "r" && reviewAvailable) {
          event.preventDefault();
          openRejectDialog(frame);
          return;
        }
        if (key === "c" && reviewAvailable && frame.state !== "undecided") {
          event.preventDefault();
          actions.onClearDecision(frame.id);
          return;
        }
      }
    }
    const row = target?.closest<HTMLElement>('[role="row"][data-frame-id]');
    if (row && (event.key === "ArrowDown" || event.key === "ArrowUp")) {
      event.preventDefault();
      const currentIndex = model.frames.findIndex(
        (frame) => frame.id === row.dataset.frameId,
      );
      if (currentIndex < 0) return;
      const delta = event.key === "ArrowDown" ? 1 : -1;
      const nextIndex = Math.min(
        model.frames.length - 1,
        Math.max(0, currentIndex + delta),
      );
      const next = model.frames[nextIndex];
      if (next) {
        actions.onSelectFrame(next.id);
        queueMicrotask(() => {
          root
            .querySelector<HTMLElement>(`[data-frame-id="${next.id}"]`)
            ?.focus();
        });
      }
      return;
    }

    const tab = target?.closest<HTMLElement>('[role="tab"][data-role]');
    if (!tab || (event.key !== "ArrowRight" && event.key !== "ArrowLeft")) {
      return;
    }
    event.preventDefault();
    const currentIndex = model.roles.findIndex(
      (role) => role.role === tab.dataset.role,
    );
    if (currentIndex < 0 || model.roles.length === 0) return;
    const delta = event.key === "ArrowRight" ? 1 : -1;
    const nextIndex =
      (currentIndex + delta + model.roles.length) % model.roles.length;
    const next = model.roles[nextIndex];
    if (next) {
      actions.onSelectRole(next.role);
      queueMicrotask(() => {
        root.querySelector<HTMLElement>(`[data-role="${next.role}"]`)?.focus();
      });
    }
  };

  const openRejectDialog = (frame: ReviewFrame): void => {
    pendingRejectFrameId = frame.id;
    elements.rejectFrame.textContent = frame.label;
    elements.rejectDialog.hidden = false;
    required<HTMLButtonElement>(
      elements.rejectDialog,
      '[data-action="reject-reason"]',
    ).focus();
  };

  const closeRejectDialog = (): void => {
    pendingRejectFrameId = null;
    elements.rejectDialog.hidden = true;
    required<HTMLButtonElement>(root, '[data-action="open-reject"]').focus();
  };

  const update = (nextModel: ReviewViewModel): void => {
    model = nextModel;
    elements.sessionName.textContent = model.sessionName;
    elements.sessionStatus.dataset.tone = model.sessionStatus.tone;
    elements.sessionStatusLabel.textContent = model.sessionStatus.label;
    const importing = model.sessionStatus.tone === "busy";
    const masterBusy =
      model.calibration.execution.state === "running" ||
      model.calibration.execution.state === "cancelling";
    const decisionBusy = model.decisionPending || importing || masterBusy;
    const decisionsAvailable = reviewCommandsAvailable(model);
    const framesActive = model.activeWorkspace === "frames";
    elements.framesWorkspace.hidden = !framesActive;
    elements.calibrationWorkspace.hidden = framesActive;
    for (const item of elements.workspaceNavigation) {
      const selected = item.dataset.workspace === model.activeWorkspace;
      item.toggleAttribute("aria-current", selected);
      item.classList.toggle("nav-item--active", selected);
    }
    renderCalibration(elements, model);
    elements.importSession.disabled = importing || masterBusy;
    elements.importSession.textContent = importing
      ? "Scanning FITS…"
      : "＋ Import session";
    renderRoles(elements.roleTabs, model);
    renderRows(elements.tableBody, model);
    const activeRole = model.roles.find(
      (role) => role.role === model.activeRole,
    );
    const activeRoleLabel = activeRole?.label ?? "Frames";
    const frame = selectedFrame(model);
    const position = frame
      ? model.frames.findIndex((candidate) => candidate.id === frame.id) + 1
      : 0;
    elements.roleHeading.textContent = activeRoleLabel;
    elements.frameTable.setAttribute(
      "aria-label",
      `${activeRoleLabel} review metrics`,
    );
    elements.filter.setAttribute(
      "aria-label",
      `Filter ${activeRoleLabel.toLocaleLowerCase("en-US")}`,
    );
    elements.filter.title = `Filter ${activeRoleLabel.toLocaleLowerCase("en-US")}`;
    elements.selectionCount.textContent = activeRole
      ? `${model.frames.length} shown · ${activeRole.count} total`
      : `${model.frames.length} shown`;
    elements.framePosition.textContent = `${position} / ${model.frames.length}`;
    elements.selectedLabel.textContent = frame?.label ?? "No frame selected";
    const resolvedPreview =
      frame && model.preview?.frameId === frame.id ? model.preview : null;
    elements.previewImage.hidden = resolvedPreview === null;
    elements.previewPlaceholder.hidden = resolvedPreview !== null;
    if (resolvedPreview && frame) {
      elements.previewImage.src = resolvedPreview.url;
      elements.previewImage.alt = `Display preview of ${frame.label}`;
    } else {
      elements.previewImage.removeAttribute("src");
      elements.previewImage.alt = "";
    }
    elements.previewBadges.hidden = frame === null;
    elements.previewTitle.textContent = frame
      ? frame.sourcePath
        ? "Rendering FITS preview"
        : "Native FITS renderer ready"
      : "No frame selected";
    elements.previewDescription.textContent = frame
      ? frame.sourcePath
        ? "Reducing pixels with the locked display stretch"
        : "Import a session to inspect its bounded pixel preview"
      : "Select a frame to inspect its display preview";
    elements.preview.setAttribute(
      "aria-label",
      frame
        ? `Display-only preview for ${frame.label}`
        : "No frame preview selected",
    );
    elements.preview.dataset.scale = model.viewerScale;
    elements.fitPreview.setAttribute(
      "aria-pressed",
      String(model.viewerScale === "fit"),
    );
    elements.actualPreview.setAttribute(
      "aria-pressed",
      String(model.viewerScale === "actual"),
    );
    elements.statisticsButton.disabled = !frame?.sourcePath;
    const qualityActionable =
      model.activeRole === "light" &&
      !model.qualityBatchRunning &&
      !!frame?.sourcePath &&
      !!frame.bayerPattern &&
      (frame.qualityState === "idle" || frame.qualityState === "error");
    elements.qualityButton.disabled = !qualityActionable;
    elements.qualityButton.textContent = qualityButtonLabel(frame);
    elements.qualityButton.title = frame?.qualityMessage ?? "No frame selected";
    const qualityCandidateCount = model.frames.filter(
      (candidate) =>
        candidate.sourcePath !== null &&
        candidate.bayerPattern !== null &&
        (candidate.qualityState === "idle" ||
          candidate.qualityState === "error"),
    ).length;
    const qualityMeasurementActive = model.frames.some(
      (candidate) => candidate.qualityState === "loading",
    );
    elements.qualityBatchButton.hidden = model.activeRole !== "light";
    elements.qualityBatchButton.disabled =
      model.qualityBatchRunning ||
      qualityMeasurementActive ||
      qualityCandidateCount === 0;
    elements.qualityBatchButton.textContent = model.qualityBatchRunning
      ? model.qualityBatchProgress
        ? `Analyzing ${model.qualityBatchProgress.completed} / ${model.qualityBatchProgress.total}`
        : "Measuring…"
      : qualityCandidateCount > 0
        ? `Measure all · ${qualityCandidateCount}`
        : "Quality complete";
    elements.qualityBatchButton.setAttribute(
      "aria-label",
      model.qualityBatchRunning
        ? model.qualityBatchProgress
          ? `Processed diagnostic quality for ${model.qualityBatchProgress.completed} of ${model.qualityBatchProgress.total} eligible light frames`
          : "Measuring diagnostic quality for all eligible light frames"
        : qualityMeasurementActive
          ? "Wait for the active diagnostic quality measurement"
          : qualityCandidateCount > 0
            ? `Measure diagnostic quality for ${qualityCandidateCount} eligible light frames`
            : "All eligible light frames have diagnostic quality measurements",
    );
    elements.cfaBadge.textContent = frame?.bayerPattern
      ? `RAW CFA · ${frame.bayerPattern.toUpperCase()}`
      : "LINEAR · UNRESOLVED";
    elements.qualityBadge.hidden = frame === null;
    elements.qualityBadge.dataset.state = frame?.qualityState ?? "unavailable";
    elements.qualityBadge.textContent = qualityBadgeLabel(frame);
    elements.qualityBadge.title = frame?.qualityMessage ?? "";
    renderStatisticsPanel(elements, model);
    elements.stretch.textContent = model.sharedStretchLabel;
    renderSelectedMetrics(elements, frame);
    elements.play.setAttribute("aria-pressed", String(model.playing));
    elements.play.disabled = model.frames.length < 2;
    for (const button of elements.stepButtons) {
      button.disabled = model.frames.length < 2;
    }
    for (const button of elements.decisionButtons) {
      button.disabled = frame === null || !decisionsAvailable;
    }
    elements.accept.disabled ||= frame?.state === "accepted";
    elements.clearDecision.disabled ||= frame?.state === "undecided";
    elements.undo.disabled = !model.canUndo || !decisionsAvailable;
    elements.undo.setAttribute(
      "aria-label",
      decisionBusy
        ? "Review decision transaction in progress"
        : model.canUndo
          ? "Undo last review decision"
          : "No review decision to undo",
    );
    elements.undo.title = model.canUndo
      ? "Undo the last native review transaction (⌘Z / Ctrl+Z)"
      : "No review decision to undo";
    elements.decisionControls.setAttribute("aria-busy", String(decisionBusy));
    elements.decisionControls.title = model.reviewSessionReady
      ? "Manual decisions are committed by the native review engine"
      : "Import a FITS session to enable manual review decisions";
    elements.play.setAttribute(
      "aria-label",
      model.playing ? "Pause Blink playback" : "Start Blink playback",
    );
    required<HTMLElement>(elements.play, "[data-play-icon]").textContent =
      model.playing ? "Ⅱ" : "▶";
    if (frame) {
      elements.liveRegion.textContent = `${frame.label}, ${stateLabel(frame.state)}, ${frame.qualityMessage}, frame ${position} of ${model.frames.length}`;
    }
  };

  const destroy = (): void => {
    root.removeEventListener("click", onClick);
    root.removeEventListener("change", onChange);
    root.removeEventListener("keydown", onKeyDown);
    root.replaceChildren();
  };

  root.addEventListener("click", onClick);
  root.addEventListener("change", onChange);
  root.addEventListener("keydown", onKeyDown);
  update(initialModel);

  return { update, destroy };
}

function isReviewRejectionReason(
  value: string | undefined,
): value is ReviewRejectionReason {
  return rejectionReasons.some(([code]) => code === value);
}

function reviewCommandsAvailable(model: ReviewViewModel): boolean {
  return (
    model.reviewSessionReady &&
    !model.decisionPending &&
    model.sessionStatus.tone !== "busy"
  );
}

function isTextEntryTarget(target: Element | null): boolean {
  return (
    target instanceof HTMLInputElement ||
    target instanceof HTMLTextAreaElement ||
    target instanceof HTMLSelectElement ||
    (target?.closest('[contenteditable="true"]') ?? null) !== null
  );
}

function qualityButtonLabel(frame: ReviewFrame | null): string {
  switch (frame?.qualityState) {
    case "loading":
      return "Measuring…";
    case "ready":
      return "Quality measured";
    case "error":
      return "Retry quality";
    case "idle":
    case "unavailable":
    case undefined:
      return "Measure quality";
  }
}

function qualityBadgeLabel(frame: ReviewFrame | null): string {
  switch (frame?.qualityState) {
    case "loading":
      return "QUALITY · MEASURING";
    case "ready":
      return "QUALITY · DIAGNOSTIC";
    case "error":
      return "QUALITY · FAILED";
    case "idle":
      return "QUALITY · NOT MEASURED";
    case "unavailable":
    case undefined:
      return "QUALITY · BLOCKED";
  }
}

function renderRoles(container: HTMLElement, model: ReviewViewModel): void {
  container.replaceChildren();
  for (const role of model.roles) {
    const button = document.createElement("button");
    const selected = role.role === model.activeRole;
    button.className = "role-tab";
    button.type = "button";
    button.role = "tab";
    button.dataset.action = "select-role";
    button.dataset.role = role.role;
    button.setAttribute("aria-selected", String(selected));
    button.tabIndex = selected ? 0 : -1;
    button.innerHTML = `<span class="role-tab__label"></span><span class="role-tab__count"></span>`;
    required<HTMLElement>(button, ".role-tab__label").textContent = role.label;
    required<HTMLElement>(button, ".role-tab__count").textContent = String(
      role.count,
    );
    if (role.unresolved > 0) {
      button.setAttribute(
        "aria-label",
        `${role.label}, ${role.count} frames, ${role.unresolved} unresolved`,
      );
    }
    container.append(button);
  }
}

function renderRows(
  container: HTMLTableSectionElement,
  model: ReviewViewModel,
): void {
  container.replaceChildren();
  if (model.frames.length === 0) {
    const emptyRow = document.createElement("tr");
    const emptyCell = textCell("No frames loaded for this type", "empty-row");
    emptyCell.colSpan = 7;
    emptyRow.append(emptyCell);
    container.append(emptyRow);
    return;
  }
  for (const frame of model.frames) {
    const row = document.createElement("tr");
    const selected = frame.id === model.selectedFrameId;
    row.role = "row";
    row.tabIndex = selected ? 0 : -1;
    row.dataset.action = "select-frame";
    row.dataset.frameId = frame.id;
    row.dataset.state = frame.state;
    row.setAttribute("aria-selected", String(selected));
    if (selected) row.classList.add("is-selected");

    const state = cell("td", "state-cell");
    state.innerHTML = `<span class="state-marker" aria-hidden="true"></span><span class="sr-only"></span>`;
    required<HTMLElement>(state, ".state-marker").textContent = stateSymbol(
      frame.state,
    );
    required<HTMLElement>(state, ".sr-only").textContent = stateLabel(
      frame.state,
    );
    const frameName = textCell(frame.label, "frame-name");
    if (frame.classificationWarning) {
      const warning = document.createElement("span");
      warning.className = "classification-warning";
      warning.title = frame.classificationWarning;
      warning.setAttribute("aria-label", frame.classificationWarning);
      warning.textContent = "!";
      frameName.append(" ", warning);
    }
    row.append(
      state,
      frameName,
      textCell(
        frame.exposureSeconds === null
          ? "—"
          : `${frame.exposureSeconds.toFixed(1)} s`,
        "numeric",
      ),
      textCell(formatTemperature(frame.temperatureCelsius), "numeric"),
      textCell(formatMetric(frame.metrics.fwhmPixels, 2), "numeric"),
      textCell(formatMetric(frame.metrics.eccentricity, 2), "numeric"),
      textCell(
        frame.metrics.detectedStars === null
          ? "—"
          : String(frame.metrics.detectedStars),
        "numeric",
      ),
    );
    container.append(row);
  }
}

function renderSelectedMetrics(
  elements: {
    state: HTMLElement;
    fwhm: HTMLElement;
    eccentricity: HTMLElement;
    stars: HTMLElement;
    background: HTMLElement;
    noise: HTMLElement;
  },
  frame: ReviewFrame | null,
): void {
  elements.state.dataset.state = frame?.state ?? "undecided";
  elements.state.textContent = frame ? stateLabel(frame.state) : "No selection";
  elements.fwhm.textContent = formatMetric(
    frame?.metrics.fwhmPixels ?? null,
    2,
  );
  elements.eccentricity.textContent = formatMetric(
    frame?.metrics.eccentricity ?? null,
    2,
  );
  elements.stars.textContent =
    frame?.metrics.detectedStars === null ||
    frame?.metrics.detectedStars === undefined
      ? "—"
      : String(frame.metrics.detectedStars);
  elements.background.textContent = formatMetric(
    frame?.metrics.background ?? null,
    1,
  );
  elements.noise.textContent = formatMetric(frame?.metrics.noise ?? null, 1);
}

function renderStatisticsPanel(
  elements: {
    statisticsDialog: HTMLElement;
    statisticsFrame: HTMLElement;
    statisticsStatus: HTMLElement;
    statisticsContent: HTMLElement;
    statisticsAlgorithm: HTMLElement;
    statisticsAxes: HTMLElement;
    statisticsFormat: HTMLElement;
    statisticsHeader: HTMLElement;
    statisticsUsable: HTMLElement;
    statisticsExcluded: HTMLElement;
    statisticsMinimum: HTMLElement;
    statisticsMaximum: HTMLElement;
    statisticsMean: HTMLElement;
    statisticsDeviation: HTMLElement;
    statisticsSampleDeviation: HTMLElement;
  },
  model: ReviewViewModel,
): void {
  const panel = model.statisticsPanel;
  const statistics = panel.statistics;
  elements.statisticsDialog.hidden = !panel.open;
  elements.statisticsFrame.textContent =
    panel.frameLabel ?? "No frame selected";
  elements.statisticsContent.hidden = panel.state !== "ready" || !statistics;
  elements.statisticsStatus.hidden = panel.state === "ready" && !!statistics;
  elements.statisticsStatus.dataset.state = panel.state;
  elements.statisticsStatus.textContent =
    panel.message ??
    (panel.state === "loading"
      ? "Reading the primary array in three deterministic passes…"
      : "Statistics are not available.");
  if (!statistics) return;

  elements.statisticsAlgorithm.textContent = statistics.algorithmId;
  elements.statisticsAxes.textContent = statistics.axes.join(" × ");
  elements.statisticsFormat.textContent = statistics.storedFormat;
  elements.statisticsHeader.textContent = statistics.headerConformant
    ? `Conformant · ${statistics.headerDiagnostics} diagnostics`
    : `Accepted with ${statistics.headerDiagnostics} diagnostics`;
  elements.statisticsUsable.textContent = `${formatCount(statistics.usableSamples)} / ${formatCount(statistics.totalSamples)}`;
  elements.statisticsExcluded.textContent = `${formatCount(statistics.undefinedSamples)} undefined · ${formatCount(statistics.nonFiniteSamples)} non-finite`;
  elements.statisticsMinimum.textContent = formatScientificValue(
    statistics.minimum,
  );
  elements.statisticsMaximum.textContent = formatScientificValue(
    statistics.maximum,
  );
  elements.statisticsMean.textContent = formatScientificValue(statistics.mean);
  elements.statisticsDeviation.textContent = formatScientificValue(
    statistics.populationStandardDeviation,
  );
  elements.statisticsSampleDeviation.textContent =
    statistics.sampleStandardDeviation === null
      ? "Undefined"
      : formatScientificValue(statistics.sampleStandardDeviation);
}

interface CalibrationElements {
  readonly calibrationStatus: HTMLElement;
  readonly calibrationProducts: HTMLElement;
  readonly calibrationDigest: HTMLElement;
  readonly pedestalPolicy: HTMLSelectElement;
  readonly exposureTolerance: HTMLInputElement;
  readonly temperatureTolerance: HTMLInputElement;
  readonly lightTemperatureTolerance: HTMLInputElement;
  readonly lightAssociations: HTMLElement;
  readonly lightDigest: HTMLElement;
  readonly lightOutputMode: HTMLSelectElement;
  readonly refreshMasterPlan: HTMLButtonElement;
  readonly executeMasterPlan: HTMLButtonElement;
  readonly cancelMasterPlan: HTMLButtonElement;
  readonly executeLightPlan: HTMLButtonElement;
  readonly cancelLightPlan: HTMLButtonElement;
  readonly masterExecution: HTMLElement;
  readonly masterExecutionMessage: HTMLElement;
  readonly masterExecutionProgress: HTMLProgressElement;
  readonly masterExecutionOutput: HTMLElement;
  readonly lightExecution: HTMLElement;
  readonly lightExecutionMessage: HTMLElement;
  readonly lightExecutionProgress: HTMLProgressElement;
  readonly lightExecutionOutput: HTMLElement;
  readonly lightExecutionHeading: HTMLElement;
}

function calibrationSettings(
  elements: Pick<
    CalibrationElements,
    | "pedestalPolicy"
    | "exposureTolerance"
    | "temperatureTolerance"
    | "lightTemperatureTolerance"
  >,
): MasterPlanSettings | null {
  const policy = elements.pedestalPolicy.value;
  if (
    policy !== "prefer_matched_dark_then_bias" &&
    policy !== "require_matched_dark" &&
    policy !== "require_bias"
  ) {
    return null;
  }
  const maximumExposureDeltaSeconds = elements.exposureTolerance.valueAsNumber;
  const maximumTemperatureDeltaC = elements.temperatureTolerance.valueAsNumber;
  const maximumLightDarkTemperatureDeltaC =
    elements.lightTemperatureTolerance.valueAsNumber;
  if (
    !Number.isFinite(maximumExposureDeltaSeconds) ||
    maximumExposureDeltaSeconds < 0 ||
    !Number.isFinite(maximumTemperatureDeltaC) ||
    maximumTemperatureDeltaC < 0 ||
    !Number.isFinite(maximumLightDarkTemperatureDeltaC) ||
    maximumLightDarkTemperatureDeltaC < 0
  ) {
    return null;
  }
  return {
    flatPedestalPolicy: policy,
    maximumExposureDeltaSeconds,
    maximumTemperatureDeltaC,
    maximumLightDarkTemperatureDeltaC,
  };
}

function renderCalibration(
  elements: CalibrationElements,
  model: ReviewViewModel,
): void {
  const calibration = model.calibration;
  elements.pedestalPolicy.value = calibration.settings.flatPedestalPolicy;
  elements.exposureTolerance.value = String(
    calibration.settings.maximumExposureDeltaSeconds,
  );
  elements.temperatureTolerance.value = String(
    calibration.settings.maximumTemperatureDeltaC,
  );
  elements.lightTemperatureTolerance.value = String(
    calibration.settings.maximumLightDarkTemperatureDeltaC,
  );
  elements.calibrationStatus.dataset.state = calibration.state;
  elements.calibrationStatus.textContent = calibration.message;
  const execution = calibration.execution;
  const masterBusy =
    execution.state === "running" || execution.state === "cancelling";
  const lightExecution = calibration.lightExecution;
  const lightBusy =
    lightExecution.state === "running" || lightExecution.state === "cancelling";
  const executionBusy = masterBusy || lightBusy;
  elements.refreshMasterPlan.disabled =
    calibration.state === "loading" ||
    !model.reviewSessionReady ||
    executionBusy;
  elements.pedestalPolicy.disabled = executionBusy;
  elements.exposureTolerance.disabled = executionBusy;
  elements.temperatureTolerance.disabled = executionBusy;
  elements.lightTemperatureTolerance.disabled = executionBusy;
  elements.lightOutputMode.value = calibration.lightSettings.outputMode;
  elements.lightOutputMode.disabled = executionBusy;
  elements.executeLightPlan.textContent =
    calibration.lightSettings.outputMode === "calibrated_frames"
      ? "Calibrate frames"
      : "Calibrate & integrate";
  elements.lightExecutionHeading.textContent =
    calibration.lightSettings.outputMode === "calibrated_frames"
      ? "Lossless calibrated frames"
      : "Calibrate + strict integration";
  elements.executeMasterPlan.disabled =
    !calibration.plan?.ready || executionBusy || !model.reviewSessionReady;
  elements.executeMasterPlan.hidden = masterBusy;
  elements.cancelMasterPlan.hidden = !masterBusy;
  elements.cancelMasterPlan.disabled = execution.state === "cancelling";
  const lightPlan = calibration.plan?.lightPlan;
  elements.executeLightPlan.disabled =
    !lightPlan?.ready ||
    lightPlan.products.length === 0 ||
    !lightExecution.masterDirectory ||
    executionBusy ||
    !model.reviewSessionReady;
  elements.executeLightPlan.hidden = lightBusy;
  elements.cancelLightPlan.hidden = !lightBusy;
  elements.cancelLightPlan.disabled = lightExecution.state === "cancelling";
  renderMasterExecution(elements, model);
  renderLightExecution(elements, model);
  const plan = calibration.plan;
  elements.calibrationDigest.textContent = plan
    ? `PLAN ${plan.planSha256.slice(0, 12)}`
    : "PLAN —";
  elements.calibrationDigest.title = plan?.planSha256 ?? "No native plan yet";

  const products = plan?.products ?? [];
  const nodes = products.map(masterProductCard);
  if (nodes.length === 0) {
    const empty = document.createElement("div");
    empty.className = "master-empty";
    const aperture = document.createElement("span");
    aperture.className = "master-empty__aperture";
    aperture.setAttribute("aria-hidden", "true");
    const title = document.createElement("strong");
    title.textContent =
      calibration.state === "loading"
        ? "Resolving calibration groups"
        : "No master groups available";
    const detail = document.createElement("span");
    detail.textContent = model.reviewSessionReady
      ? "Check imported metadata and grouping diagnostics"
      : "Import a FITS session to build the native dependency graph";
    empty.append(aperture, title, detail);
    nodes.push(empty);
  }
  elements.calibrationProducts.replaceChildren(...nodes);
  renderLightAssociations(elements, plan);
}

function renderLightAssociations(
  elements: Pick<CalibrationElements, "lightAssociations" | "lightDigest">,
  plan: ReviewViewModel["calibration"]["plan"],
): void {
  const lightPlan = plan?.lightPlan ?? null;
  elements.lightDigest.textContent = lightPlan
    ? `LIGHT ${lightPlan.planSha256.slice(0, 12)}`
    : "LIGHT —";
  elements.lightDigest.title =
    lightPlan?.planSha256 ?? "Light associations wait for a ready master plan";

  if (!lightPlan) {
    const blocked = document.createElement("div");
    blocked.className = "light-matrix__empty";
    const title = document.createElement("strong");
    title.textContent = "Light associations are gated";
    const detail = document.createElement("span");
    detail.textContent =
      "Resolve every Flat pedestal before matching Lights to generated masters.";
    blocked.append(title, detail);
    elements.lightAssociations.replaceChildren(blocked);
    return;
  }

  if (lightPlan.products.length === 0) {
    const empty = document.createElement("div");
    empty.className = "light-matrix__empty";
    const title = document.createElement("strong");
    title.textContent = "No Light groups in this session";
    const detail = document.createElement("span");
    detail.textContent =
      "The master plan remains valid and can be built independently.";
    empty.append(title, detail);
    elements.lightAssociations.replaceChildren(empty);
    return;
  }

  const table = document.createElement("table");
  table.className = "light-matrix";
  table.setAttribute("aria-label", "Light calibration associations");
  const head = document.createElement("thead");
  const heading = document.createElement("tr");
  for (const label of [
    "Light group",
    "Dark master",
    "Flat master",
    "State",
    "Evidence",
  ]) {
    const cell = document.createElement("th");
    cell.scope = "col";
    cell.textContent = label;
    heading.append(cell);
  }
  head.append(heading);
  const body = document.createElement("tbody");
  for (const product of lightPlan.products)
    body.append(lightAssociationRow(product));
  table.append(head, body);
  elements.lightAssociations.replaceChildren(table);
}

function lightAssociationRow(
  product: LightCalibrationProduct,
): HTMLTableRowElement {
  const row = document.createElement("tr");
  row.dataset.state =
    product.dark.status === "matched" && product.flat.status === "matched"
      ? "ready"
      : "blocked";
  const identity = document.createElement("th");
  identity.scope = "row";
  identity.textContent = product.groupId;
  identity.title = product.groupId;
  row.append(
    identity,
    lightAssociationCell(product.dark),
    lightAssociationCell(product.flat),
  );
  const state = document.createElement("td");
  const badge = document.createElement("span");
  badge.className = "association-state";
  badge.textContent = row.dataset.state === "ready" ? "Ready" : "Blocked";
  state.append(badge);

  const evidence = document.createElement("td");
  const details = document.createElement("details");
  details.className = "candidate-evidence";
  const summary = document.createElement("summary");
  const compatible = product.candidates.filter(
    (candidate) => candidate.status === "compatible",
  ).length;
  summary.textContent =
    product.candidates.length === 0
      ? "No master candidates"
      : `${compatible}/${product.candidates.length} compatible`;
  const list = document.createElement("ul");
  for (const candidate of product.candidates) {
    const item = document.createElement("li");
    const reasons = candidate.mismatches
      .map(
        (mismatch) =>
          `${humanize(mismatch.field)}: ${humanize(mismatch.reason)}`,
      )
      .join(", ");
    item.textContent = `${candidate.kind.toUpperCase()} · ${candidate.groupId} · ${
      candidate.status === "compatible" ? "compatible" : reasons
    }`;
    list.append(item);
  }
  details.append(summary, list);
  evidence.append(details);
  row.append(state, evidence);
  return row;
}

function lightAssociationCell(
  association: LightMasterAssociation,
): HTMLTableCellElement {
  const output = document.createElement("td");
  const name = document.createElement("strong");
  name.textContent =
    association.selectedGroupId ?? associationBlockingLabel(association);
  name.title = association.selectedGroupId ?? "Unresolved association";
  output.append(name);
  if (association.temperatureDeltaCelsius !== null) {
    const temperature = document.createElement("small");
    temperature.textContent = `${association.temperatureDeltaCelsius.toFixed(2)} °C · ${
      association.temperatureBasis === "sensor" ? "sensor" : "set point"
    }`;
    output.append(temperature);
  }
  return output;
}

function associationBlockingLabel(association: LightMasterAssociation): string {
  if (association.ambiguousGroupIds.length > 0) {
    return `Ambiguous · ${association.ambiguousGroupIds.length} candidates`;
  }
  if (association.missingFields.length > 0) {
    return `Missing · ${association.missingFields.map(humanize).join(", ")}`;
  }
  return association.blockingReason === "no_compatible_candidate"
    ? "No compatible master"
    : "Unresolved";
}

function renderMasterExecution(
  elements: Pick<
    CalibrationElements,
    | "masterExecution"
    | "masterExecutionMessage"
    | "masterExecutionProgress"
    | "masterExecutionOutput"
  >,
  model: ReviewViewModel,
): void {
  const execution = model.calibration.execution;
  elements.masterExecution.dataset.state = execution.state;
  elements.masterExecutionMessage.textContent = execution.message;
  elements.masterExecutionOutput.textContent = execution.outputDirectory
    ? execution.outputDirectory
    : "Output directory selected at run time";
  elements.masterExecutionOutput.title = execution.outputDirectory ?? "";
  const progress = execution.progress;
  if (progress?.totalUnits) {
    elements.masterExecutionProgress.max = progress.totalUnits;
    elements.masterExecutionProgress.value = progress.completedUnits;
  } else {
    elements.masterExecutionProgress.removeAttribute("value");
    elements.masterExecutionProgress.max = 1;
  }
  elements.masterExecutionProgress.hidden =
    execution.state !== "running" && execution.state !== "cancelling";
}

function renderLightExecution(
  elements: Pick<
    CalibrationElements,
    | "lightExecution"
    | "lightExecutionMessage"
    | "lightExecutionProgress"
    | "lightExecutionOutput"
  >,
  model: ReviewViewModel,
): void {
  const execution = model.calibration.lightExecution;
  elements.lightExecution.dataset.state = execution.state;
  elements.lightExecutionMessage.textContent = execution.message;
  elements.lightExecutionOutput.textContent = execution.outputDirectory
    ? execution.outputDirectory
    : execution.masterDirectory
      ? "Output directory selected at run time"
      : "Build masters to unlock Light execution";
  elements.lightExecutionOutput.title = execution.outputDirectory ?? "";
  const progress = execution.progress;
  if (progress?.totalUnits) {
    elements.lightExecutionProgress.max = progress.totalUnits;
    elements.lightExecutionProgress.value = progress.completedUnits;
  } else {
    elements.lightExecutionProgress.removeAttribute("value");
    elements.lightExecutionProgress.max = 1;
  }
  elements.lightExecutionProgress.hidden =
    execution.state !== "running" && execution.state !== "cancelling";
}

function masterProductCard(product: MasterProductPlan): HTMLElement {
  const card = document.createElement("article");
  card.className = "master-card";
  card.dataset.kind = product.kind;
  card.dataset.status = product.pedestal.status;

  const header = document.createElement("header");
  const identity = document.createElement("div");
  const role = document.createElement("span");
  role.className = "master-card__role";
  role.textContent = masterKindLabel(product.kind);
  const name = document.createElement("h4");
  name.textContent = product.groupId;
  identity.append(role, name);
  const count = document.createElement("span");
  count.className = "master-card__count";
  count.textContent = `${formatCount(product.frameCount)} frame${product.frameCount === 1 ? "" : "s"}`;
  header.append(identity, count);

  const metadata = document.createElement("dl");
  metadata.className = "master-card__metadata";
  appendCompactMetric(metadata, "Camera", product.camera ?? "Unresolved");
  appendCompactMetric(metadata, "Axes", product.axes.join(" × "));
  appendCompactMetric(
    metadata,
    "Exposure",
    product.exposureSeconds === null ? "—" : `${product.exposureSeconds} s`,
  );
  appendCompactMetric(
    metadata,
    "Temperature",
    product.sensorTemperatureCelsius === null
      ? "—"
      : `${product.sensorTemperatureCelsius.toFixed(1)} °C`,
  );
  appendCompactMetric(
    metadata,
    "Gain / offset",
    `${product.gain ?? "—"} / ${product.offset ?? "—"}`,
  );
  appendCompactMetric(
    metadata,
    "Sampling",
    `${product.binning ? `${product.binning.x}×${product.binning.y}` : "—"} · ${product.bayerPattern ?? "Mono / unknown"}`,
  );

  const dependency = document.createElement("div");
  dependency.className = "master-card__dependency";
  const dependencyLight = document.createElement("span");
  dependencyLight.className = "dependency-light";
  dependencyLight.setAttribute("aria-hidden", "true");
  const dependencyText = document.createElement("div");
  const dependencyTitle = document.createElement("strong");
  dependencyTitle.textContent = pedestalTitle(product);
  const dependencyDetail = document.createElement("span");
  dependencyDetail.textContent = pedestalDetail(product);
  dependencyText.append(dependencyTitle, dependencyDetail);
  dependency.append(dependencyLight, dependencyText);

  card.append(header, metadata, dependency);
  if (product.kind === "flat" && product.candidates.length > 0) {
    card.append(candidateDisclosure(product));
  }
  return card;
}

function appendCompactMetric(
  container: HTMLDListElement,
  label: string,
  value: string,
): void {
  const item = document.createElement("div");
  const term = document.createElement("dt");
  term.textContent = label;
  const description = document.createElement("dd");
  description.textContent = value;
  item.append(term, description);
  container.append(item);
}

function candidateDisclosure(product: MasterProductPlan): HTMLDetailsElement {
  const details = document.createElement("details");
  details.className = "candidate-evidence";
  const summary = document.createElement("summary");
  const compatible = product.candidates.filter(
    (candidate) => candidate.status === "compatible",
  ).length;
  summary.textContent = `${product.candidates.length} pedestal candidate${product.candidates.length === 1 ? "" : "s"} · ${compatible} compatible`;
  const list = document.createElement("ul");
  for (const candidate of product.candidates) {
    const item = document.createElement("li");
    item.dataset.status = candidate.status;
    const title = document.createElement("strong");
    title.textContent = `${candidate.sourceKind.toUpperCase()} · ${candidate.groupId}`;
    const reason = document.createElement("span");
    reason.textContent =
      candidate.status === "compatible"
        ? `Compatible · Δt ${candidate.exposureDeltaSeconds ?? 0} s · ΔT ${candidate.temperatureDeltaCelsius ?? 0} °C`
        : candidate.mismatches
            .map(
              (mismatch) =>
                `${humanize(mismatch.field)} ${humanize(mismatch.reason)}`,
            )
            .join(" · ");
    item.append(title, reason);
    list.append(item);
  }
  details.append(summary, list);
  return details;
}

function masterKindLabel(kind: MasterProductPlan["kind"]): string {
  switch (kind) {
    case "bias":
      return "Master Bias";
    case "dark":
      return "Master Dark";
    case "flat":
      return "Master Flat";
  }
}

function pedestalTitle(product: MasterProductPlan): string {
  switch (product.pedestal.status) {
    case "not_applicable":
      return "Direct strict-mean integration";
    case "matched_dark":
      return "Matched dark selected";
    case "bias":
      return "True bias selected";
    case "unresolved":
      return "Pedestal unresolved";
  }
}

function pedestalDetail(product: MasterProductPlan): string {
  const pedestal = product.pedestal;
  if (pedestal.status === "not_applicable") {
    return "No pedestal subtraction is applied to this source role";
  }
  if (pedestal.status === "unresolved") {
    const ambiguity = pedestal.ambiguousGroupIds.length
      ? ` · ${pedestal.ambiguousGroupIds.join(", ")}`
      : "";
    return `${humanize(pedestal.blockingReason ?? "manual decision required")}${ambiguity}`;
  }
  return `${pedestal.selectedGroupId ?? "Unknown group"} · Δt ${pedestal.exposureDeltaSeconds ?? 0} s · ΔT ${pedestal.temperatureDeltaCelsius ?? 0} °C`;
}

function selectedFrame(model: ReviewViewModel): ReviewFrame | null {
  return (
    model.frames.find((frame) => frame.id === model.selectedFrameId) ?? null
  );
}

function stateLabel(state: ReviewState): string {
  switch (state) {
    case "accepted":
      return "Accepted";
    case "rejected":
      return "Rejected";
    case "undecided":
      return "Undecided";
  }
}

function stateSymbol(state: ReviewState): string {
  switch (state) {
    case "accepted":
      return "✓";
    case "rejected":
      return "×";
    case "undecided":
      return "·";
  }
}

function formatMetric(value: number | null, precision: number): string {
  return value === null ? "—" : value.toFixed(precision);
}

function formatTemperature(value: number | null): string {
  return value === null ? "—" : `${value.toFixed(1)} °C`;
}

function formatCount(value: number): string {
  return value.toLocaleString("en-US");
}

function formatScientificValue(value: number): string {
  const magnitude = Math.abs(value);
  if (magnitude !== 0 && (magnitude >= 1_000_000 || magnitude < 0.001)) {
    return value.toExponential(6);
  }
  return value.toLocaleString("en-US", { maximumFractionDigits: 6 });
}

function humanize(field: string): string {
  return field.replaceAll("_", " ");
}

function isFrameRole(
  value: string | undefined,
): value is ReviewViewModel["activeRole"] {
  return (
    value === "bias" ||
    value === "dark" ||
    value === "flat" ||
    value === "light"
  );
}

function textCell(text: string, className: string): HTMLTableCellElement {
  const output = cell("td", className);
  output.textContent = text;
  return output;
}

function cell(tagName: "td", className: string): HTMLTableCellElement {
  const output = document.createElement(tagName);
  output.className = className;
  return output;
}

function required<T extends Element>(root: ParentNode, selector: string): T {
  const element = root.querySelector<T>(selector);
  if (!element) throw new Error(`Missing required review element: ${selector}`);
  return element;
}

function requiredAll<T extends Element>(
  root: ParentNode,
  selector: string,
): readonly T[] {
  const elements = [...root.querySelectorAll<T>(selector)];
  if (elements.length === 0) {
    throw new Error(`Missing required review elements: ${selector}`);
  }
  return elements;
}

function shellMarkup(): string {
  const reasonButtons = rejectionReasons
    .map(
      ([code, label]) =>
        `<button class="reason-button" type="button" data-action="reject-reason" data-reason="${code}">${label}</button>`,
    )
    .join("");
  return `
    <div class="app-shell">
      <aside class="sidebar" aria-label="Main navigation">
        <div class="brand" aria-label="AetherStack home">
          <span class="brand__mark" aria-hidden="true">A</span>
          <span class="brand__name">AetherStack</span>
        </div>
        <nav class="primary-nav" aria-label="Workflow">
          ${navigationItem("frames", "Frames", "▦", true)}
          ${navigationItem("calibration", "Calibration", "◫", true)}
          ${navigationItem("pipeline", "Pipeline", "⌁", false)}
          ${navigationItem("run", "Run", "▷", false)}
          ${navigationItem("results", "Results", "◉", false)}
        </nav>
        <button class="nav-item sidebar__settings" type="button" aria-label="Settings, not available in this build" title="Settings workspace is not connected yet" disabled>
          <span class="nav-item__icon" aria-hidden="true">⚙</span>
          <span>Settings</span>
        </button>
      </aside>

      <main class="workspace" id="workspace">
        <header class="topbar">
          <div>
            <p class="eyebrow">Active session</p>
            <h1 data-session-name></h1>
          </div>
          <div class="topbar__actions">
            <span class="health-chip" data-session-status data-tone="ready"><span aria-hidden="true">●</span><span data-session-status-label>Demo ready</span></span>
            <button class="button button--quiet" type="button" title="Diagnostics workspace is not connected yet" disabled>Diagnostics</button>
            <button class="button button--primary" type="button" title="Review planning is not connected yet" disabled>Review plan</button>
          </div>
        </header>

        <section class="frames-workspace" aria-labelledby="frames-heading" data-frames-workspace>
          <div class="workspace-heading">
            <div>
              <p class="eyebrow">Frames</p>
              <h2 id="frames-heading">Review acquisition data</h2>
            </div>
            <div class="workspace-heading__actions">
              <button class="icon-button" type="button" data-action="undo" aria-label="No review decision to undo" aria-keyshortcuts="Meta+Z Control+Z" title="No review decision to undo" disabled>↶</button>
              <button class="button button--quiet" type="button" data-action="import-session" data-import-session>＋ Import session</button>
            </div>
          </div>

          <div class="role-tabs" role="tablist" aria-label="Frame types" data-role-tabs></div>

          <div class="review-layout">
            <section class="frame-browser" aria-labelledby="light-table-heading">
              <div class="panel-heading">
                <div>
                  <h3 id="light-table-heading" data-role-heading></h3>
                  <p><span data-selection-count></span> · metrics are diagnostic</p>
                </div>
                <div class="panel-heading__actions">
                  <button class="tool-button" type="button" data-action="measure-all-quality" aria-label="All eligible light frames have diagnostic quality measurements" aria-live="polite" aria-atomic="true" disabled>Quality complete</button>
                  <button class="icon-button" type="button" data-filter-frames aria-label="Frame filtering is not available in this build" title="Frame filtering is not connected yet" disabled>⌕</button>
                </div>
              </div>
              <div class="table-scroll" tabindex="0" aria-label="Scrollable frame table">
                <table class="frame-table" data-frame-table aria-label="Frame review metrics">
                  <thead>
                    <tr>
                      <th scope="col"><span class="sr-only">Review state</span></th>
                      ${sortableHeading("Frame", "label")}
                      <th scope="col" class="numeric">Exposure</th>
                      <th scope="col" class="numeric">Temp.</th>
                      ${sortableHeading("FWHM", "fwhm_major", true)}
                      ${sortableHeading("Ecc.", "eccentricity", true)}
                      ${sortableHeading("Stars", "detected_stars", true)}
                    </tr>
                  </thead>
                  <tbody data-frame-rows></tbody>
                </table>
              </div>
              <div class="table-legend" aria-label="Review state legend">
                <span data-state="accepted"><b aria-hidden="true">✓</b> Accepted</span>
                <span data-state="rejected"><b aria-hidden="true">×</b> Rejected</span>
                <span data-state="undecided"><b aria-hidden="true">·</b> Undecided</span>
              </div>
            </section>

            <section class="viewer" aria-labelledby="viewer-heading">
              <div class="viewer__topbar">
                <div>
                  <p class="eyebrow">Display-only preview</p>
                  <h3 id="viewer-heading" data-selected-label></h3>
                </div>
                <div class="viewer__tools" aria-label="Viewer controls">
                  <button class="tool-button" type="button" data-action="viewer-fit" aria-label="Fit preview to viewer" aria-pressed="true">Fit</button>
                  <button class="tool-button" type="button" data-action="viewer-actual" aria-label="Show preview pixels at one hundred percent" aria-pressed="false">1:1</button>
                  <button class="tool-button" type="button" data-action="measure-quality" aria-label="Measure diagnostic frame quality">Measure quality</button>
                  <button class="tool-button" type="button" data-action="open-statistics" aria-label="Inspect exact FITS statistics">Statistics</button>
                  <button class="tool-button" type="button" aria-label="Clipping overlay is not available in this build" title="Clipping requires rejection maps" disabled>Clipping</button>
                </div>
              </div>

              <div class="preview-frame">
                <div class="preview-surface" role="group" data-preview>
                  <img class="preview-image" data-preview-image alt="" hidden />
                  <div class="preview-placeholder" data-preview-placeholder>
                    <span class="preview-placeholder__aperture" aria-hidden="true"></span>
                    <strong data-preview-title></strong>
                    <span data-preview-description></span>
                  </div>
                </div>
                <div class="preview-badges" data-preview-badges>
                  <span class="viewer-chip" data-cfa-badge></span>
                  <span class="viewer-chip viewer-chip--quality" data-quality-badge></span>
                  <span class="viewer-chip viewer-chip--lock" data-stretch-label></span>
                </div>
                <dl class="metric-strip" aria-label="Selected frame metrics">
                  ${metric("FWHM", "data-metric-fwhm", "px")}
                  ${metric("Eccentricity", "data-metric-eccentricity", "")}
                  ${metric("Stars", "data-metric-stars", "")}
                  ${metric("Background", "data-metric-background", "DN")}
                  ${metric("Noise", "data-metric-noise", "DN")}
                </dl>
              </div>

              <div class="review-controls">
                <div class="blink-controls" aria-label="Blink playback">
                  <button class="transport-button" type="button" data-action="previous" data-step-action aria-label="Previous frame">‹</button>
                  <button class="transport-button transport-button--play" type="button" data-action="toggle-play" aria-label="Start Blink playback" aria-pressed="false"><span data-play-icon aria-hidden="true">▶</span></button>
                  <button class="transport-button" type="button" data-action="next" data-step-action aria-label="Next frame">›</button>
                  <span class="frame-position" data-frame-position></span>
                </div>
                <div class="decision-controls" aria-label="Frame decision" data-decision-controls aria-busy="false">
                  <span class="state-chip" data-review-state data-state="undecided">Undecided</span>
                  <button class="button button--quiet" type="button" data-action="clear-decision" data-decision-action aria-keyshortcuts="C" title="Clear decision (C)">Clear</button>
                  <button class="button button--danger" type="button" data-action="open-reject" data-decision-action aria-keyshortcuts="R" title="Reject with a reason (R)">Reject</button>
                  <button class="button button--success" type="button" data-action="accept" data-decision-action aria-keyshortcuts="A" title="Accept selected frame (A)">Accept</button>
                </div>
              </div>
            </section>
          </div>
        </section>

        <section class="calibration-workspace" aria-labelledby="calibration-heading" data-calibration-workspace hidden>
          <div class="workspace-heading calibration-heading">
            <div>
              <p class="eyebrow">Calibration laboratory</p>
              <h2 id="calibration-heading">Build exact master frames</h2>
              <p class="workspace-intro">Bias, Darks, Flats and Lights remain separate. The native planner binds every Flat to one pedestal, then every Light to one exact Dark and one normalized Flat.</p>
            </div>
            <div class="workspace-heading__actions">
              <span class="calibration-readiness" data-calibration-status role="status" aria-live="polite"></span>
              <button class="button button--quiet" type="button" data-action="refresh-master-plan">↻ Rebuild plan</button>
              <button class="button button--primary" type="button" data-action="execute-master-plan">Build masters</button>
              <button class="button button--danger" type="button" data-action="cancel-master-plan" hidden>Cancel build</button>
            </div>
          </div>

          <div class="calibration-layout">
            <aside class="calibration-console" aria-labelledby="matching-heading">
              <div class="panel-heading panel-heading--compact">
                <div>
                  <p class="eyebrow">Matching controls</p>
                  <h3 id="matching-heading">Flat pedestal</h3>
                </div>
                <span class="hardware-light" aria-hidden="true"></span>
              </div>
              <label class="control-field">
                <span>Selection policy</span>
                <select data-pedestal-policy>
                  <option value="prefer_matched_dark_then_bias">Prefer matched dark, then bias</option>
                  <option value="require_matched_dark">Require matched dark</option>
                  <option value="require_bias">Require true bias</option>
                </select>
              </label>
              <div class="control-pair">
                <label class="control-field">
                  <span>Exposure tolerance</span>
                  <span class="number-control"><input data-exposure-tolerance type="number" min="0" step="0.01" inputmode="decimal" /><b>s</b></span>
                </label>
                <label class="control-field">
                  <span>Temperature tolerance</span>
                  <span class="number-control"><input data-temperature-tolerance type="number" min="0" step="0.1" inputmode="decimal" /><b>°C</b></span>
                </label>
              </div>
              <label class="control-field">
                <span>Light ↔ Dark temperature tolerance</span>
                <span class="number-control"><input data-light-temperature-tolerance type="number" min="0" step="0.1" inputmode="decimal" /><b>°C</b></span>
              </label>
              <p class="control-note">Exact camera, axes, gain, offset, binning and CFA phase are always required. Filter is not used to match darks or biases.</p>
              <details class="advanced-settings">
                <summary>Advanced matching evidence</summary>
                <p>Every compatible and rejected candidate is retained below with stable machine-readable reasons.</p>
              </details>
              <section class="master-execution" data-master-execution data-state="idle" aria-labelledby="master-execution-heading">
                <div>
                  <p class="eyebrow">Transactional build</p>
                  <h4 id="master-execution-heading">Native execution</h4>
                </div>
                <p data-master-execution-message></p>
                <progress data-master-execution-progress aria-label="Master build progress" hidden></progress>
                <code data-master-execution-output></code>
              </section>
            </aside>

            <section class="master-rack" aria-labelledby="master-rack-heading">
              <div class="panel-heading">
                <div>
                  <p class="eyebrow">Native dependency graph</p>
                  <h3 id="master-rack-heading">Planned masters</h3>
                </div>
                <code class="plan-digest" data-calibration-digest></code>
              </div>
              <div class="master-product-list" data-calibration-products></div>
              <section class="light-association-panel" aria-labelledby="light-association-heading">
                <div class="light-association-panel__heading">
                  <div>
                    <p class="eyebrow">Execution gate</p>
                    <h3 id="light-association-heading">Light calibration matrix</h3>
                  </div>
                  <div class="light-association-panel__actions">
                    <code class="plan-digest" data-light-digest></code>
                    <label class="light-output-mode">
                      <span>Output</span>
                      <select data-light-output-mode aria-label="Light output mode">
                        <option value="calibrated_frames">Calibrated frames</option>
                        <option value="integrated">Integrated group</option>
                      </select>
                    </label>
                    <button class="button button--primary" type="button" data-action="execute-light-plan">Calibrate &amp; integrate</button>
                    <button class="button button--danger" type="button" data-action="cancel-light-plan" hidden>Cancel Lights</button>
                  </div>
                </div>
                <div class="light-matrix-scroll" data-light-associations></div>
                <section class="light-execution" data-light-execution data-state="idle" aria-labelledby="light-execution-heading">
                  <div>
                    <p class="eyebrow">Atomic Light run</p>
                    <h4 id="light-execution-heading" data-light-execution-heading>Calibrate + strict integration</h4>
                  </div>
                  <p data-light-execution-message></p>
                  <progress data-light-execution-progress aria-label="Light calibration progress" hidden></progress>
                  <code data-light-execution-output></code>
                </section>
              </section>
            </section>
          </div>
        </section>
      </main>
    </div>

    <div class="dialog-backdrop" role="presentation" data-reject-dialog hidden>
      <section class="reason-dialog" role="dialog" aria-modal="true" aria-labelledby="reject-title" aria-describedby="reject-description">
        <p class="eyebrow">Manual review</p>
        <h2 id="reject-title">Why reject this frame?</h2>
        <p id="reject-description">The reason is stored with <strong data-reject-frame></strong> and remains visible in the run plan.</p>
        <div class="reason-grid">${reasonButtons}</div>
        <button class="button button--quiet dialog-cancel" type="button" data-action="cancel-reject">Cancel</button>
      </section>
    </div>
    <div class="dialog-backdrop" role="presentation" data-statistics-dialog hidden>
      <section class="statistics-dialog" role="dialog" aria-modal="true" aria-labelledby="statistics-title" aria-describedby="statistics-description">
        <div class="statistics-dialog__heading">
          <div>
            <p class="eyebrow">Exact FITS statistics</p>
            <h2 id="statistics-title" data-statistics-frame></h2>
          </div>
          <button class="icon-button" type="button" data-action="close-statistics" aria-label="Close FITS statistics">×</button>
        </div>
        <p id="statistics-description">The complete primary array is decoded in canonical order with fixed-size buffers. These moments are diagnostic and never alter scientific pixels.</p>
        <p class="statistics-status" data-statistics-status></p>
        <dl class="statistics-grid" data-statistics-content hidden>
          ${statistic("Algorithm", "data-statistics-algorithm")}
          ${statistic("Axes", "data-statistics-axes")}
          ${statistic("Stored format", "data-statistics-format")}
          ${statistic("Header", "data-statistics-header")}
          ${statistic("Usable samples", "data-statistics-usable")}
          ${statistic("Excluded samples", "data-statistics-excluded")}
          ${statistic("Minimum", "data-statistics-minimum")}
          ${statistic("Maximum", "data-statistics-maximum")}
          ${statistic("Arithmetic mean", "data-statistics-mean")}
          ${statistic("Population σ", "data-statistics-deviation")}
          ${statistic("Sample σ", "data-statistics-sample-deviation")}
        </dl>
      </section>
    </div>
    <p class="sr-only" aria-live="polite" aria-atomic="true" data-live-region></p>
  `;
}

function navigationItem(
  id: string,
  label: string,
  icon: string,
  enabled: boolean,
): string {
  if (enabled) {
    return `<button class="nav-item" type="button" data-action="select-workspace" data-workspace="${id}">
      <span class="nav-item__icon" aria-hidden="true">${icon}</span><span>${label}</span>
    </button>`;
  }
  return `<button class="nav-item" type="button" data-workspace="${id}" aria-label="${label}, not available in this build" title="${label} workspace is not connected yet" disabled>
    <span class="nav-item__icon" aria-hidden="true">${icon}</span><span>${label}</span>
  </button>`;
}

function sortableHeading(
  label: string,
  field: SortField,
  numeric = false,
): string {
  return `<th scope="col"${numeric ? ' class="numeric"' : ""}>
    <button class="sort-button" type="button" data-action="sort" data-sort-field="${field}" aria-label="Sort ${label} ascending">${label}<span aria-hidden="true">↕</span></button>
  </th>`;
}

function metric(label: string, attribute: string, unit: string): string {
  return `<div><dt>${label}</dt><dd><span ${attribute}>—</span>${unit ? ` <small>${unit}</small>` : ""}</dd></div>`;
}

function statistic(label: string, attribute: string): string {
  return `<div><dt>${label}</dt><dd ${attribute}>—</dd></div>`;
}
