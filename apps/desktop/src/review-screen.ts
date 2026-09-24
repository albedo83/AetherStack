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
    if (action === "close-statistics") {
      actions.onCloseStatistics();
      queueMicrotask(() => elements.statisticsButton.focus());
    }
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
    const decisionBusy = model.decisionPending || importing;
    const decisionsAvailable = reviewCommandsAvailable(model);
    elements.importSession.disabled = importing;
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
      !!frame?.sourcePath &&
      !!frame.bayerPattern &&
      (frame.qualityState === "idle" || frame.qualityState === "error");
    elements.qualityButton.disabled = !qualityActionable;
    elements.qualityButton.textContent = qualityButtonLabel(frame);
    elements.qualityButton.title = frame?.qualityMessage ?? "No frame selected";
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
    root.removeEventListener("keydown", onKeyDown);
    root.replaceChildren();
  };

  root.addEventListener("click", onClick);
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

function humanize(field: SortField): string {
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
          ${navigationItem("calibration", "Calibration", "◫", false)}
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

        <section class="frames-workspace" aria-labelledby="frames-heading">
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
                <button class="icon-button" type="button" data-filter-frames aria-label="Frame filtering is not available in this build" title="Frame filtering is not connected yet" disabled>⌕</button>
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
  current: boolean,
): string {
  if (current) {
    return `<span class="nav-item" data-workspace="${id}" aria-current="page">
      <span class="nav-item__icon" aria-hidden="true">${icon}</span><span>${label}</span>
    </span>`;
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
