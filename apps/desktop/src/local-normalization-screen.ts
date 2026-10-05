import type { LocalNormalizationSettings } from "./local-normalization-bridge.ts";
import type { LocalNormalizationViewModel, ReviewActions } from "./model.ts";

type NumberSettingKey = Exclude<
  keyof LocalNormalizationSettings,
  "saturationLevel"
>;

interface NumberControl {
  readonly key: NumberSettingKey;
  readonly label: string;
  readonly group:
    "background" | "stars" | "protection" | "grid" | "fit" | "surface";
  readonly min: number;
  readonly step: number | "any";
  readonly unit?: string;
}

const controls: readonly NumberControl[] = [
  {
    key: "backgroundClippingSigma",
    label: "Clipping sigma",
    group: "background",
    min: 0.1,
    step: 0.1,
    unit: "σ",
  },
  {
    key: "backgroundMaximumIterations",
    label: "Maximum passes",
    group: "background",
    min: 1,
    step: 1,
  },
  {
    key: "backgroundMinimumSamples",
    label: "Minimum samples",
    group: "background",
    min: 1,
    step: 1,
  },
  {
    key: "detectionSigma",
    label: "Detection threshold",
    group: "stars",
    min: 0.1,
    step: 0.1,
    unit: "σ",
  },
  {
    key: "measurementFloorSigma",
    label: "Measurement floor",
    group: "stars",
    min: 0.1,
    step: 0.1,
    unit: "σ",
  },
  {
    key: "measurementRadius",
    label: "Measurement radius",
    group: "stars",
    min: 1,
    step: 1,
    unit: "px",
  },
  {
    key: "minimumSeparation",
    label: "Minimum separation",
    group: "stars",
    min: 1,
    step: 1,
    unit: "px",
  },
  {
    key: "minimumMeasurementPixels",
    label: "Minimum footprint",
    group: "stars",
    min: 1,
    step: 1,
    unit: "px",
  },
  {
    key: "maximumCandidates",
    label: "Candidate ceiling",
    group: "stars",
    min: 1,
    step: 1,
  },
  {
    key: "protectionGrowthFactor",
    label: "Stellar growth",
    group: "protection",
    min: 0.05,
    step: 0.05,
    unit: "×",
  },
  {
    key: "saturatedGrowthFactor",
    label: "Saturated growth",
    group: "protection",
    min: 0.05,
    step: 0.05,
    unit: "×",
  },
  {
    key: "minimumProtectionRadius",
    label: "Minimum radius",
    group: "protection",
    min: 1,
    step: 1,
    unit: "px",
  },
  {
    key: "maximumProtectionRadius",
    label: "Maximum radius",
    group: "protection",
    min: 1,
    step: 1,
    unit: "px",
  },
  {
    key: "maximumProtectedSources",
    label: "Protected-source ceiling",
    group: "protection",
    min: 1,
    step: 1,
  },
  {
    key: "maximumProtectionPixelVisits",
    label: "Protection work ceiling",
    group: "protection",
    min: 1,
    step: 1,
  },
  {
    key: "cellWidth",
    label: "Cell width",
    group: "grid",
    min: 1,
    step: 1,
    unit: "px",
  },
  {
    key: "cellHeight",
    label: "Cell height",
    group: "grid",
    min: 1,
    step: 1,
    unit: "px",
  },
  {
    key: "maximumSamplesPerCell",
    label: "Samples per cell",
    group: "grid",
    min: 1,
    step: 1,
  },
  {
    key: "maximumCells",
    label: "Cell ceiling",
    group: "grid",
    min: 1,
    step: 1,
  },
  {
    key: "minimumFitSamples",
    label: "Minimum fit support",
    group: "fit",
    min: 2,
    step: 1,
  },
  {
    key: "maximumFitSamples",
    label: "Maximum fit support",
    group: "fit",
    min: 2,
    step: 1,
  },
  {
    key: "maximumPairwiseSlopes",
    label: "Pairwise-slope ceiling",
    group: "fit",
    min: 1,
    step: 1,
  },
  {
    key: "minimumAbsoluteScale",
    label: "Minimum absolute scale",
    group: "fit",
    min: 0,
    step: "any",
  },
  {
    key: "minimumControlPoints",
    label: "Minimum controls",
    group: "surface",
    min: 1,
    step: 1,
  },
  {
    key: "minimumSurfaceNeighbors",
    label: "Minimum neighbors",
    group: "surface",
    min: 1,
    step: 1,
  },
  {
    key: "maximumSurfaceNeighbors",
    label: "Maximum neighbors",
    group: "surface",
    min: 1,
    step: 1,
  },
  {
    key: "maximumSurfaceDistance",
    label: "Maximum distance",
    group: "surface",
    min: 1,
    step: 1,
    unit: "px",
  },
];

const groupLabels: Readonly<Record<NumberControl["group"], string>> = {
  background: "Robust background",
  stars: "Stellar measurement",
  protection: "Protected regions",
  grid: "Sampling grid",
  fit: "Cell fitting",
  surface: "Coefficient surface",
};

export interface LocalNormalizationPanel {
  readonly update: (model: LocalNormalizationViewModel) => void;
  readonly destroy: () => void;
}

type PanelActions = Pick<
  ReviewActions,
  | "onSelectLocalNormalizationSource"
  | "onSelectLocalNormalizationReference"
  | "onSelectLocalNormalizationOutput"
  | "onUpdateLocalNormalizationSettings"
  | "onUpdateLocalNormalizationMemoryLimit"
  | "onUpdateLocalNormalizationGroupId"
  | "onPreflightLocalNormalization"
  | "onExecuteLocalNormalization"
  | "onCancelLocalNormalization"
  | "onSelectLocalNormalizationPreview"
  | "onInspectLocalNormalizationStatistics"
>;

/** Returns the static semantic shell mounted inside the main workflow. */
export function localNormalizationMarkup(): string {
  return `
    <section class="normalization-workspace" aria-labelledby="normalization-heading" data-normalization-workspace hidden>
      <div class="workspace-heading normalization-heading">
        <div>
          <p class="eyebrow">Local normalization laboratory</p>
          <h2 id="normalization-heading">Match the sky, preserve the signal</h2>
          <p class="workspace-intro">Fit a guarded, spatially varying background and scale model in binary64. Stars and saturated structures are measured first, then excluded from every fit.</p>
        </div>
        <div class="workspace-heading__actions">
          <span class="normalization-readiness" data-localnorm-status role="status" aria-live="polite"></span>
          <button class="button button--primary" type="button" data-localnorm-action="execute">Normalize image</button>
          <button class="button button--danger" type="button" data-localnorm-action="cancel" hidden>Cancel</button>
        </div>
      </div>

      <div class="normalization-layout">
        <section class="normalization-console" aria-labelledby="normalization-inputs-heading">
          <div class="panel-heading panel-heading--compact">
            <div><p class="eyebrow">Sealed transaction</p><h3 id="normalization-inputs-heading">Image pair</h3></div>
            <span class="hardware-light" aria-hidden="true"></span>
          </div>
          <div class="normalization-paths">
            ${pathCard("source", "Source", "Calibrated image to correct", "Choose source")}
            <span class="normalization-paths__operator" aria-hidden="true">→</span>
            ${pathCard("reference", "Reference", "Stable target background", "Choose reference")}
            <span class="normalization-paths__operator" aria-hidden="true">→</span>
            ${pathCard("output", "Output", "Atomic create-new FITS", "Choose output")}
          </div>
          <div class="normalization-run-controls">
            <label class="control-field">
              <span>Group identity</span>
              <input class="instrument-input" data-localnorm-group type="text" maxlength="128" autocomplete="off" spellcheck="false" />
            </label>
            <label class="control-field">
              <span>Memory ceiling</span>
              <span class="instrument-stepper number-control"><input data-localnorm-memory type="number" min="256" step="256" inputmode="numeric" /><b>MiB</b></span>
            </label>
            <button class="button button--quiet" type="button" data-localnorm-action="preflight">Verify memory</button>
          </div>
          <div class="normalization-progress" data-localnorm-progress-panel data-state="idle">
            <div><span>Native f64 transaction</span><strong data-localnorm-progress-label>Waiting for inputs</strong></div>
            <progress data-localnorm-progress aria-label="Local normalization progress" max="1" hidden></progress>
            <p data-localnorm-message></p>
          </div>
          <dl class="normalization-memory-breakdown" data-localnorm-memory-breakdown hidden>
            ${evidence("Active images", "data-localnorm-memory-images", "—")}
            ${evidence("Sampling + fit", "data-localnorm-memory-fit", "—")}
            ${evidence("Diagnostics", "data-localnorm-memory-diagnostics", "—")}
            ${evidence("FITS I/O", "data-localnorm-memory-io", "—")}
          </dl>
        </section>

        <section class="normalization-instrument" aria-labelledby="normalization-profile-heading">
          <div class="normalization-profile">
            <div class="normalization-profile__dial" aria-hidden="true"><span>Q</span></div>
            <div>
              <p class="eyebrow">Quality-first profile</p>
              <h3 id="normalization-profile-heading">Conservative stellar protection</h3>
              <p>Robust per-cell affine fits, guarded interpolation and explicit work ceilings. Nothing is extrapolated through unsupported regions.</p>
            </div>
            <span class="instrument-label">F64 · deterministic</span>
          </div>

          <dl class="normalization-evidence" aria-label="Local normalization evidence">
            ${evidence("Image", "data-localnorm-dimensions", "Awaiting result")}
            ${evidence("Measured stars", "data-localnorm-stars", "—")}
            ${evidence("Protected pixels", "data-localnorm-protected", "—")}
            ${evidence("Control points", "data-localnorm-controls", "—")}
            ${evidence("Peak memory", "data-localnorm-peak-memory", "—")}
            ${evidence("Publication", "data-localnorm-publication", "Not started")}
          </dl>

          <section class="normalization-preview" aria-labelledby="normalization-preview-heading">
            <div class="normalization-preview__heading">
              <div><p class="eyebrow">Matched inspection</p><h4 id="normalization-preview-heading">Source · reference · result</h4></div>
              <span data-localnorm-stretch>Reference stretch · awaiting publication</span>
            </div>
            <div class="normalization-preview__tabs" role="tablist" aria-label="Local normalization comparison">
              <button type="button" role="tab" data-localnorm-preview="source" data-localnorm-action="preview-source" aria-selected="false" disabled>Source</button>
              <button type="button" role="tab" data-localnorm-preview="reference" data-localnorm-action="preview-reference" aria-selected="false" disabled>Reference</button>
              <button type="button" role="tab" data-localnorm-preview="output" data-localnorm-action="preview-output" aria-selected="true" disabled>Normalized</button>
            </div>
            <div class="normalization-overlay-bar">
              <label class="normalization-overlay-toggle"><input type="checkbox" data-localnorm-overlay checked /> Control map</label>
              <span data-localnorm-map-summary>Published controls appear here</span>
              <span class="normalization-map-key"><i></i> accepted <i></i> rejected</span>
            </div>
            <div class="normalization-preview__surface" data-localnorm-preview-surface data-state="idle">
              <img data-localnorm-preview-image alt="" hidden />
              <svg data-localnorm-control-overlay aria-label="Spatial normalization control map" hidden></svg>
              <div data-localnorm-preview-placeholder>The normalized result will appear after publication</div>
            </div>
            <div class="normalization-statistics" data-localnorm-statistics data-state="idle">
              <div><span>Exact primary-array statistics</span><strong data-localnorm-statistics-message>Select a published product</strong></div>
              <dl aria-label="Selected normalization product statistics">
                ${evidence("Usable", "data-localnorm-stat-usable", "—")}
                ${evidence("Minimum", "data-localnorm-stat-minimum", "—")}
                ${evidence("Maximum", "data-localnorm-stat-maximum", "—")}
                ${evidence("Mean", "data-localnorm-stat-mean", "—")}
                ${evidence("σ population", "data-localnorm-stat-deviation", "—")}
              </dl>
              <button class="button button--quiet" type="button" data-localnorm-action="statistics" disabled>Calculate exact statistics</button>
            </div>
          </section>

          <details class="advanced-settings normalization-advanced">
            <summary><span>Advanced scientific controls</span><small>28 plan-bound parameters</small></summary>
            <p class="control-note">Every value below is validated in Rust and sealed into the parameter and execution-plan digests.</p>
            <div class="normalization-control-groups">
              ${controlGroupsMarkup()}
            </div>
          </details>

          <div class="normalization-seal">
            <span>Plan seal</span>
            <code data-localnorm-plan>Created only after native source fingerprinting</code>
            <span>Parameter seal</span>
            <code data-localnorm-parameters>Advanced controls not yet executed</code>
          </div>
        </section>
      </div>
    </section>`;
}

/** Binds the isolated panel without giving it authority over other workspaces. */
export function mountLocalNormalizationPanel(
  root: HTMLElement,
  actions: PanelActions,
): LocalNormalizationPanel {
  const settingsInputs = Array.from(
    root.querySelectorAll<HTMLInputElement>("[data-localnorm-setting]"),
  );
  const saturationEnabled = required<HTMLInputElement>(
    root,
    "[data-localnorm-saturation-enabled]",
  );
  const saturationLevel = required<HTMLInputElement>(
    root,
    "[data-localnorm-saturation]",
  );
  const memory = required<HTMLInputElement>(root, "[data-localnorm-memory]");
  const groupId = required<HTMLInputElement>(root, "[data-localnorm-group]");
  const overlay = required<HTMLInputElement>(root, "[data-localnorm-overlay]");
  const execute = required<HTMLButtonElement>(
    root,
    '[data-localnorm-action="execute"]',
  );
  const cancel = required<HTMLButtonElement>(
    root,
    '[data-localnorm-action="cancel"]',
  );
  let current: LocalNormalizationViewModel;

  const onClick = (event: Event): void => {
    const target =
      event.target instanceof Element
        ? event.target.closest<HTMLElement>("[data-localnorm-action]")
        : null;
    switch (target?.dataset.localnormAction) {
      case "source":
        actions.onSelectLocalNormalizationSource();
        break;
      case "reference":
        actions.onSelectLocalNormalizationReference();
        break;
      case "output":
        actions.onSelectLocalNormalizationOutput();
        break;
      case "execute":
        actions.onExecuteLocalNormalization();
        break;
      case "preflight":
        actions.onPreflightLocalNormalization();
        break;
      case "cancel":
        actions.onCancelLocalNormalization();
        break;
      case "preview-source":
        actions.onSelectLocalNormalizationPreview("source");
        break;
      case "preview-reference":
        actions.onSelectLocalNormalizationPreview("reference");
        break;
      case "preview-output":
        actions.onSelectLocalNormalizationPreview("output");
        break;
      case "statistics":
        actions.onInspectLocalNormalizationStatistics();
        break;
    }
  };
  const onChange = (event: Event): void => {
    if (event.target === memory) {
      const mebibytes = memory.valueAsNumber;
      if (Number.isSafeInteger(mebibytes) && mebibytes >= 256) {
        actions.onUpdateLocalNormalizationMemoryLimit(
          mebibytes * 1_024 * 1_024,
        );
      }
      return;
    }
    if (event.target === groupId) {
      const value = groupId.value.trim();
      if (value.length > 0) actions.onUpdateLocalNormalizationGroupId(value);
      return;
    }
    if (event.target === overlay) {
      renderControlOverlay(root, current, overlay.checked);
      return;
    }
    if (event.target === saturationEnabled) {
      saturationLevel.disabled = !saturationEnabled.checked;
    }
    if (
      event.target === saturationEnabled ||
      event.target === saturationLevel ||
      (event.target instanceof Element &&
        event.target.matches("[data-localnorm-setting]"))
    ) {
      const settings = readSettings(
        settingsInputs,
        saturationEnabled,
        saturationLevel,
        current.settings,
      );
      if (settings) actions.onUpdateLocalNormalizationSettings(settings);
    }
  };
  root.addEventListener("click", onClick);
  root.addEventListener("change", onChange);

  return {
    update(model) {
      current = model;
      const busy = model.state === "running" || model.state === "cancelling";
      const preflight = required<HTMLButtonElement>(
        root,
        '[data-localnorm-action="preflight"]',
      );
      setPath(root, "source", model.sourcePath);
      setPath(root, "reference", model.referencePath);
      setPath(root, "output", model.outputPath);
      if (document.activeElement !== memory)
        memory.value = String(Math.round(model.memoryLimitBytes / 1_048_576));
      if (document.activeElement !== groupId) groupId.value = model.groupId;
      for (const input of settingsInputs) {
        const key = input.dataset.localnormSetting as
          NumberSettingKey | undefined;
        if (key && document.activeElement !== input)
          input.value = String(model.settings[key]);
        input.disabled = busy;
      }
      saturationEnabled.checked = model.settings.saturationLevel !== null;
      saturationEnabled.disabled = busy;
      saturationLevel.value = String(model.settings.saturationLevel ?? 65_535);
      saturationLevel.disabled = busy || !saturationEnabled.checked;
      memory.disabled = busy;
      groupId.disabled = busy;
      preflight.disabled =
        busy ||
        model.preflightState === "loading" ||
        !model.sourcePath ||
        !model.referencePath;
      preflight.textContent =
        model.preflightState === "loading" ? "Checking…" : "Verify memory";
      execute.hidden = busy;
      execute.disabled =
        !model.sourcePath ||
        !model.referencePath ||
        !model.outputPath ||
        model.groupId.trim().length === 0 ||
        model.preflight?.fitsMemoryLimit === false;
      cancel.hidden = !busy;
      cancel.disabled = model.state === "cancelling";
      text(root, "[data-localnorm-status]", statusLabel(model));
      text(root, "[data-localnorm-message]", model.message);
      const progress = required<HTMLProgressElement>(
        root,
        "[data-localnorm-progress]",
      );
      progress.hidden = !busy;
      if (model.progress?.totalUnits) {
        progress.max = model.progress.totalUnits;
        progress.value = model.progress.completedUnits;
      } else {
        progress.removeAttribute("value");
      }
      text(root, "[data-localnorm-progress-label]", progressLabel(model));
      required<HTMLElement>(
        root,
        "[data-localnorm-progress-panel]",
      ).dataset.state = model.state;
      renderEvidence(root, model);
      renderMemoryBreakdown(root, model);
      renderPreview(root, model);
      renderControlOverlay(root, model, overlay.checked);
      renderStatistics(root, model);
    },
    destroy() {
      root.removeEventListener("click", onClick);
      root.removeEventListener("change", onChange);
    },
  };
}

function renderMemoryBreakdown(
  root: HTMLElement,
  model: LocalNormalizationViewModel,
): void {
  const panel = required<HTMLElement>(
    root,
    "[data-localnorm-memory-breakdown]",
  );
  panel.hidden = model.preflight === null;
  const estimate = model.preflight;
  text(
    root,
    "[data-localnorm-memory-images]",
    estimate
      ? formatBytes(estimate.planeImagesBytes + estimate.applicationBandBytes)
      : "—",
  );
  text(
    root,
    "[data-localnorm-memory-fit]",
    estimate
      ? formatBytes(
          estimate.retainedSamplesBytes +
            estimate.qualityBytes +
            estimate.slopeBytes,
        )
      : "—",
  );
  text(
    root,
    "[data-localnorm-memory-diagnostics]",
    estimate ? formatBytes(estimate.diagnosticsBytes) : "—",
  );
  text(
    root,
    "[data-localnorm-memory-io]",
    estimate
      ? formatBytes(estimate.decodeStatusBytes + estimate.writerBufferBytes)
      : "—",
  );
}

function renderControlOverlay(
  root: HTMLElement,
  model: LocalNormalizationViewModel,
  enabled: boolean,
): void {
  const svg = required<SVGSVGElement>(root, "[data-localnorm-control-overlay]");
  const result = model.result;
  svg.replaceChildren();
  text(
    root,
    "[data-localnorm-map-summary]",
    result
      ? `${result.controlPoints.length.toLocaleString()} accepted · ${result.cellDiagnostics.filter((cell) => !cell.accepted).length.toLocaleString()} rejected`
      : "Published controls appear here",
  );
  const hidden = !enabled || !result || !model.preview;
  svg.toggleAttribute("hidden", hidden);
  if (hidden || !result) return;
  svg.setAttribute("viewBox", `0 0 ${result.width} ${result.height}`);
  svg.setAttribute("preserveAspectRatio", "xMidYMid meet");
  const largestResidual = Math.max(
    ...result.controlPoints.map((point) => point.medianAbsoluteResidual),
    Number.EPSILON,
  );
  const colors = ["#6fc4ff", "#72e5a0", "#ff718f"] as const;
  for (const cell of result.cellDiagnostics.filter((item) => !item.accepted)) {
    const rectangle = document.createElementNS(
      "http://www.w3.org/2000/svg",
      "rect",
    );
    rectangle.setAttribute("x", String(cell.x));
    rectangle.setAttribute("y", String(cell.y));
    rectangle.setAttribute("width", String(cell.width));
    rectangle.setAttribute("height", String(cell.height));
    rectangle.setAttribute("fill", "#ff365f");
    rectangle.setAttribute("fill-opacity", "0.22");
    rectangle.setAttribute("stroke", "#ff5c78");
    rectangle.setAttribute("stroke-width", "4");
    const title = document.createElementNS(
      "http://www.w3.org/2000/svg",
      "title",
    );
    title.textContent = `Rejected plane ${cell.plane + 1} · ${rejectionLabel(cell.rejectionCode)} · retained ${cell.retained}/${cell.eligible} · protected ${cell.protected}`;
    rectangle.append(title);
    svg.append(rectangle);
  }
  for (const point of result.controlPoints) {
    const circle = document.createElementNS(
      "http://www.w3.org/2000/svg",
      "circle",
    );
    const residualRatio = Math.min(
      1,
      point.medianAbsoluteResidual / largestResidual,
    );
    circle.setAttribute("cx", String(point.x));
    circle.setAttribute("cy", String(point.y));
    circle.setAttribute("r", String(10 + residualRatio * 18));
    circle.setAttribute(
      "fill",
      colors[point.plane % colors.length] ?? colors[0],
    );
    circle.setAttribute("fill-opacity", String(0.35 + residualRatio * 0.45));
    circle.setAttribute("stroke", "#f3f8ff");
    circle.setAttribute("stroke-width", "3");
    const title = document.createElementNS(
      "http://www.w3.org/2000/svg",
      "title",
    );
    title.textContent = `Plane ${point.plane + 1} · scale ${point.scale.toPrecision(6)} · offset ${point.offset.toPrecision(6)} · median residual ${point.medianAbsoluteResidual.toPrecision(4)}`;
    circle.append(title);
    svg.append(circle);
  }
}

function rejectionLabel(code: string | null): string {
  return (code ?? "unknown rejection").replaceAll("_", " ");
}

function readSettings(
  inputs: readonly HTMLInputElement[],
  saturationEnabled: HTMLInputElement,
  saturation: HTMLInputElement,
  previous: LocalNormalizationSettings,
): LocalNormalizationSettings | null {
  const next = { ...previous } as Record<
    keyof LocalNormalizationSettings,
    number | null
  >;
  for (const input of inputs) {
    const key = input.dataset.localnormSetting as NumberSettingKey | undefined;
    if (!key || !input.validity.valid || !Number.isFinite(input.valueAsNumber))
      return null;
    next[key] = input.valueAsNumber;
  }
  if (saturationEnabled.checked) {
    if (
      !saturation.validity.valid ||
      !Number.isFinite(saturation.valueAsNumber)
    )
      return null;
    next.saturationLevel = saturation.valueAsNumber;
  } else {
    next.saturationLevel = null;
  }
  return next as unknown as LocalNormalizationSettings;
}

function renderEvidence(
  root: HTMLElement,
  model: LocalNormalizationViewModel,
): void {
  const result = model.result;
  text(
    root,
    "[data-localnorm-dimensions]",
    result
      ? `${result.width} × ${result.height} × ${result.planes}`
      : model.preflight
        ? `${model.preflight.width} × ${model.preflight.height} × ${model.preflight.planes}`
        : "Awaiting result",
  );
  text(
    root,
    "[data-localnorm-stars]",
    result ? result.measuredSources.toLocaleString() : "—",
  );
  text(
    root,
    "[data-localnorm-protected]",
    result ? result.protectedPixels.toLocaleString() : "—",
  );
  text(
    root,
    "[data-localnorm-controls]",
    result
      ? `${result.validControlPoints.toLocaleString()} valid · ${result.rejectedCells.toLocaleString()} rejected`
      : "—",
  );
  text(
    root,
    "[data-localnorm-peak-memory]",
    result
      ? formatBytes(result.peakReservedBytes)
      : model.preflight
        ? `${formatBytes(model.preflight.requiredBytes)} · ${model.preflight.fitsMemoryLimit ? "ready" : "insufficient"}`
        : "—",
  );
  text(
    root,
    "[data-localnorm-publication]",
    result
      ? `${result.samplesWritten.toLocaleString()} samples · ${formatBytes(result.bytesWritten)}`
      : "Not started",
  );
  text(
    root,
    "[data-localnorm-plan]",
    result?.planSha256 ?? "Created only after native source fingerprinting",
  );
  text(
    root,
    "[data-localnorm-parameters]",
    result?.parametersSha256 ?? "Advanced controls not yet executed",
  );
}

function renderPreview(
  root: HTMLElement,
  model: LocalNormalizationViewModel,
): void {
  const available = model.result !== null;
  for (const button of root.querySelectorAll<HTMLButtonElement>(
    "[data-localnorm-preview]",
  )) {
    const selected = button.dataset.localnormPreview === model.previewView;
    button.disabled = !available;
    button.setAttribute("aria-selected", String(selected));
  }
  text(root, "[data-localnorm-stretch]", model.sharedStretchLabel);
  const surface = required<HTMLElement>(
    root,
    "[data-localnorm-preview-surface]",
  );
  const image = required<HTMLImageElement>(
    root,
    "[data-localnorm-preview-image]",
  );
  const placeholder = required<HTMLElement>(
    root,
    "[data-localnorm-preview-placeholder]",
  );
  surface.dataset.state = model.previewState;
  const preview = model.preview;
  image.hidden = preview === null;
  if (preview) {
    if (image.src !== preview.url) image.src = preview.url;
    image.alt = `${previewLabel(model.previewView)} local-normalization preview`;
  } else {
    image.removeAttribute("src");
    image.alt = "";
  }
  placeholder.hidden = preview !== null;
  placeholder.textContent = model.previewMessage;
}

function previewLabel(
  view: LocalNormalizationViewModel["previewView"],
): string {
  if (view === "source") return "Source";
  if (view === "reference") return "Reference";
  return "Normalized result";
}

function renderStatistics(
  root: HTMLElement,
  model: LocalNormalizationViewModel,
): void {
  const panel = required<HTMLElement>(root, "[data-localnorm-statistics]");
  panel.dataset.state = model.statisticsState;
  const button = required<HTMLButtonElement>(
    root,
    '[data-localnorm-action="statistics"]',
  );
  button.disabled =
    model.result === null || model.statisticsState === "loading";
  button.textContent =
    model.statisticsState === "loading"
      ? "Calculating…"
      : model.statisticsView === model.previewView && model.statistics
        ? "Recalculate exact statistics"
        : "Calculate exact statistics";
  text(root, "[data-localnorm-statistics-message]", model.statisticsMessage);
  const statistics =
    model.result !== null && model.statisticsView === model.previewView
      ? model.statistics
      : null;
  text(
    root,
    "[data-localnorm-stat-usable]",
    statistics
      ? `${statistics.usableSamples.toLocaleString()} / ${statistics.totalSamples.toLocaleString()}`
      : "—",
  );
  text(
    root,
    "[data-localnorm-stat-minimum]",
    statistics ? formatScientificNumber(statistics.minimum) : "—",
  );
  text(
    root,
    "[data-localnorm-stat-maximum]",
    statistics ? formatScientificNumber(statistics.maximum) : "—",
  );
  text(
    root,
    "[data-localnorm-stat-mean]",
    statistics ? formatScientificNumber(statistics.mean) : "—",
  );
  text(
    root,
    "[data-localnorm-stat-deviation]",
    statistics
      ? formatScientificNumber(statistics.populationStandardDeviation)
      : "—",
  );
}

function formatScientificNumber(value: number): string {
  const magnitude = Math.abs(value);
  if ((magnitude !== 0 && magnitude < 0.001) || magnitude >= 1_000_000) {
    return value.toExponential(6);
  }
  return value.toLocaleString("en-US", { maximumFractionDigits: 6 });
}

function setPath(root: HTMLElement, kind: string, path: string | null): void {
  const value = required<HTMLElement>(root, `[data-localnorm-path="${kind}"]`);
  value.textContent = path
    ? (path.split(/[\\/]/).pop() ?? path)
    : "Not selected";
  value.title = path ?? "";
  value.dataset.ready = String(path !== null);
}

function statusLabel(model: LocalNormalizationViewModel): string {
  if (model.state === "running") return "Processing";
  if (model.state === "cancelling") return "Cancelling";
  if (model.state === "completed") return "Published";
  if (model.state === "error") return "Needs attention";
  return model.sourcePath && model.referencePath && model.outputPath
    ? "Ready"
    : "Configure";
}

function progressLabel(model: LocalNormalizationViewModel): string {
  const progress = model.progress;
  if (!progress)
    return model.state === "completed"
      ? "Atomic publication complete"
      : "Waiting for inputs";
  return progress.totalUnits === null
    ? progress.state
    : `${progress.completedUnits} / ${progress.totalUnits} units`;
}

function controlGroupsMarkup(): string {
  return (Object.keys(groupLabels) as NumberControl["group"][])
    .map(
      (group) =>
        `<fieldset class="normalization-control-group"><legend>${groupLabels[group]}</legend><div>${controls
          .filter((control) => control.group === group)
          .map(numberControlMarkup)
          .join(
            "",
          )}${group === "stars" ? saturationMarkup() : ""}</div></fieldset>`,
    )
    .join("");
}

function numberControlMarkup(control: NumberControl): string {
  return `<label class="normalization-control"><span>${control.label}</span><span class="instrument-stepper number-control"><input data-localnorm-setting="${control.key}" type="number" min="${control.min}" step="${control.step}" inputmode="decimal" /><b>${control.unit ?? ""}</b></span></label>`;
}

function saturationMarkup(): string {
  return `<div class="normalization-control normalization-control--saturation"><label><input data-localnorm-saturation-enabled type="checkbox" /> <span>Known saturation level</span></label><span class="instrument-stepper number-control"><input data-localnorm-saturation type="number" min="0" step="1" inputmode="decimal" aria-label="Known saturation level in DN" disabled /><b>DN</b></span></div>`;
}

function pathCard(
  kind: string,
  label: string,
  description: string,
  button: string,
): string {
  return `<article class="normalization-path"><div><span>${label}</span><strong data-localnorm-path="${kind}" data-ready="false">Not selected</strong><small>${description}</small></div><button class="button button--quiet" type="button" data-localnorm-action="${kind}">${button}</button></article>`;
}

function evidence(label: string, attribute: string, value: string): string {
  return `<div><dt>${label}</dt><dd ${attribute}>${value}</dd></div>`;
}

function formatBytes(bytes: number): string {
  if (bytes < 1_048_576) return `${(bytes / 1_024).toFixed(1)} KiB`;
  if (bytes < 1_073_741_824) return `${(bytes / 1_048_576).toFixed(1)} MiB`;
  return `${(bytes / 1_073_741_824).toFixed(2)} GiB`;
}

function text(root: HTMLElement, selector: string, value: string): void {
  required<HTMLElement>(root, selector).textContent = value;
}

function required<T extends Element>(root: ParentNode, selector: string): T {
  const element = root.querySelector<T>(selector);
  if (!element)
    throw new Error(`Missing local-normalization element: ${selector}`);
  return element;
}
