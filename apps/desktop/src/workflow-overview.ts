import type { ReviewViewModel, WorkspaceView } from "./model.ts";

type StageState = "complete" | "ready" | "waiting" | "attention" | "running";

interface WorkflowStage {
  readonly label: string;
  readonly description: string;
  readonly workspace: WorkspaceView;
  readonly state: StageState;
  readonly evidence: string;
}

interface PublishedArtifact {
  readonly kind: string;
  readonly path: string;
  readonly evidence: string;
  readonly workspace: WorkspaceView;
}

/** Static shell for the workflow control room and published-artifact ledger. */
export function workflowOverviewMarkup(): string {
  return `
    <section class="overview-workspace" aria-labelledby="run-heading" data-run-workspace hidden>
      <div class="workspace-heading overview-heading">
        <div><p class="eyebrow">Control room</p><h2 id="run-heading">Prepare a defensible processing run</h2></div>
        <span class="overview-heading__seal">Native evidence only</span>
      </div>
      <p class="workspace-intro">Every stage below is derived from the current native session. Open a stage to resolve its prerequisites or inspect the published evidence.</p>
      <div class="workflow-meter" role="status" aria-live="polite">
        <div><span data-workflow-summary>Waiting for session evidence</span><strong data-workflow-percent>0%</strong></div>
        <progress data-workflow-progress max="100" value="0" aria-label="Workflow completion"></progress>
      </div>
      <ol class="workflow-stage-list" data-workflow-stages></ol>
    </section>

    <section class="overview-workspace" aria-labelledby="results-heading" data-results-workspace hidden>
      <div class="workspace-heading overview-heading">
        <div><p class="eyebrow">Published evidence</p><h2 id="results-heading">Products and reproducibility ledger</h2></div>
        <span class="overview-heading__seal">Atomic artifacts</span>
      </div>
      <p class="workspace-intro">Only files confirmed by completed native transactions appear here. Paths are runtime evidence and are never synthesized by the interface.</p>
      <div class="results-summary" role="status" aria-live="polite">
        <strong data-results-count>0 artifacts</strong>
        <span data-results-summary>No native product has been published yet.</span>
      </div>
      <div class="results-ledger" data-results-ledger></div>
    </section>`;
}

/** Updates workflow readiness without making scientific decisions in the browser. */
export function renderWorkflowOverview(
  runWorkspace: HTMLElement,
  resultsWorkspace: HTMLElement,
  model: ReviewViewModel,
): void {
  const stages = deriveStages(model);
  const complete = stages.filter((stage) => stage.state === "complete").length;
  const percent = Math.round((complete / stages.length) * 100);
  text(
    runWorkspace,
    "[data-workflow-summary]",
    `${complete} of ${stages.length} native stages published`,
  );
  text(runWorkspace, "[data-workflow-percent]", `${percent}%`);
  const progress = required<HTMLProgressElement>(
    runWorkspace,
    "[data-workflow-progress]",
  );
  progress.value = percent;
  required<HTMLOListElement>(runWorkspace, "[data-workflow-stages]").innerHTML =
    stages.map(stageMarkup).join("");

  const artifacts = deriveArtifacts(model);
  text(
    resultsWorkspace,
    "[data-results-count]",
    `${artifacts.length} ${artifacts.length === 1 ? "artifact" : "artifacts"}`,
  );
  text(
    resultsWorkspace,
    "[data-results-summary]",
    artifacts.length === 0
      ? "No native product has been published yet."
      : "Every listed path was returned by a completed native transaction.",
  );
  required<HTMLElement>(resultsWorkspace, "[data-results-ledger]").innerHTML =
    artifacts.length === 0
      ? `<div class="results-empty"><span aria-hidden="true">◎</span><h3>No published products</h3><p>Complete calibration, registration, normalization, integration, or Drizzle to populate this ledger.</p></div>`
      : artifacts.map(artifactMarkup).join("");
}

function deriveStages(model: ReviewViewModel): readonly WorkflowStage[] {
  const lightCount =
    model.roles.find((role) => role.role === "light")?.count ?? 0;
  const masters = model.calibration.execution.result?.products.length ?? 0;
  const calibrated =
    model.calibration.lightExecution.result?.calibratedFrames.length ?? 0;
  const registered = model.registration.execution.result?.frames.length ?? 0;
  const integrated = model.registration.stack.result;
  const normalization = model.localNormalization.result;
  return [
    {
      label: "Review acquisition frames",
      description:
        "Classify FITS sources, inspect quality, and record explicit decisions.",
      workspace: "frames",
      state: model.reviewSessionReady
        ? "complete"
        : model.frames.length > 0
          ? "ready"
          : "waiting",
      evidence: model.reviewSessionReady
        ? `${lightCount} reviewed Lights in the session`
        : `${model.frames.length} frames available for review`,
    },
    {
      label: "Build calibration masters",
      description:
        "Publish immutable master Dark, Flat, and optional Bias products.",
      workspace: "calibration",
      state: executionState(
        model.calibration.execution.state,
        masters > 0,
        model.calibration.plan?.ready === true,
      ),
      evidence:
        masters > 0
          ? `${masters} master products published`
          : model.calibration.message,
    },
    {
      label: "Calibrate reviewed Lights",
      description:
        "Apply exact master associations and preserve per-frame provenance.",
      workspace: "calibration",
      state: executionState(
        model.calibration.lightExecution.state,
        calibrated > 0,
        masters > 0,
      ),
      evidence:
        calibrated > 0
          ? `${calibrated} calibrated frames published`
          : model.calibration.lightExecution.message,
    },
    {
      label: "Register the Light stack",
      description:
        "Solve reviewed geometry and publish aligned FITS frames atomically.",
      workspace: "registration",
      state: executionState(
        model.registration.execution.state,
        registered > 0,
        calibrated > 0,
      ),
      evidence:
        registered > 0
          ? `${registered} registered frames published`
          : model.registration.execution.message,
    },
    {
      label: "Normalize local background",
      description:
        "Optionally match large-scale background while preserving source signal.",
      workspace: "normalization",
      state: executionState(
        model.localNormalization.state,
        normalization !== null,
        registered > 0,
      ),
      evidence: normalization
        ? "Normalized FITS product published"
        : model.localNormalization.message,
    },
    {
      label: "Integrate registered frames",
      description:
        "Resolve a sealed estimator policy and publish science and evidence maps.",
      workspace: "registration",
      state: executionState(
        model.registration.stack.state,
        integrated !== null,
        registered > 0,
      ),
      evidence: integrated
        ? `${integrated.width} × ${integrated.height} × ${integrated.planes} science product`
        : model.registration.stack.message,
    },
  ];
}

function executionState(
  state: "idle" | "running" | "cancelling" | "completed" | "error",
  complete: boolean,
  ready: boolean,
): StageState {
  if (complete || state === "completed") return "complete";
  if (state === "running" || state === "cancelling") return "running";
  if (state === "error") return "attention";
  return ready ? "ready" : "waiting";
}

function deriveArtifacts(model: ReviewViewModel): readonly PublishedArtifact[] {
  const artifacts: PublishedArtifact[] = [];
  for (const product of model.calibration.execution.result?.products ?? []) {
    artifacts.push({
      kind: `Master ${product.kind}`,
      path: product.outputPath,
      evidence: product.groupId,
      workspace: "calibration",
    });
  }
  for (const frame of model.calibration.lightExecution.result
    ?.calibratedFrames ?? []) {
    artifacts.push({
      kind: "Calibrated Light",
      path: frame.outputPath,
      evidence: frame.sourceLabel,
      workspace: "calibration",
    });
    if (frame.rgbOutputPath)
      artifacts.push({
        kind: "Debayered linear RGB",
        path: frame.rgbOutputPath,
        evidence: frame.sourceLabel,
        workspace: "calibration",
      });
  }
  for (const frame of model.registration.execution.result?.frames ?? []) {
    artifacts.push({
      kind: "Registered Light",
      path: frame.outputPath,
      evidence: `${frame.samplesWritten.toLocaleString()} samples`,
      workspace: "registration",
    });
  }
  if (model.localNormalization.result) {
    artifacts.push({
      kind: "Locally normalized image",
      path: model.localNormalization.result.outputPath,
      evidence: shortSeal(model.localNormalization.result.planSha256),
      workspace: "normalization",
    });
  }
  const stack = model.registration.stack.result;
  if (stack) {
    artifacts.push({
      kind: "Integrated science",
      path: stack.outputPath,
      evidence: shortSeal(stack.reportSha256),
      workspace: "registration",
    });
    if (stack.lowRejectionMapPath)
      artifacts.push({
        kind: "Low rejection map",
        path: stack.lowRejectionMapPath,
        evidence: stack.estimator,
        workspace: "registration",
      });
    if (stack.highRejectionMapPath)
      artifacts.push({
        kind: "High rejection map",
        path: stack.highRejectionMapPath,
        evidence: stack.estimator,
        workspace: "registration",
      });
    if (stack.supportMapPath)
      artifacts.push({
        kind: "Integration support map",
        path: stack.supportMapPath,
        evidence: stack.estimator,
        workspace: "registration",
      });
    artifacts.push({
      kind: "Integration report",
      path: stack.reportPath,
      evidence: shortSeal(stack.reportSha256),
      workspace: "registration",
    });
  }
  const drizzle = model.registration.drizzle.result;
  if (drizzle) {
    artifacts.push({
      kind: "Drizzle science",
      path: drizzle.sciencePath,
      evidence: `${drizzle.sourceCount} sources`,
      workspace: "registration",
    });
    artifacts.push({
      kind: "Drizzle weight",
      path: drizzle.weightPath,
      evidence: shortSeal(drizzle.parametersSha256),
      workspace: "registration",
    });
    artifacts.push({
      kind: "Drizzle support",
      path: drizzle.supportPath,
      evidence: shortSeal(drizzle.parametersSha256),
      workspace: "registration",
    });
  }
  return artifacts;
}

function stageMarkup(stage: WorkflowStage): string {
  return `<li class="workflow-stage" data-state="${stage.state}">
    <span class="workflow-stage__lamp" aria-hidden="true"></span>
    <div><div class="workflow-stage__title"><h3>${escapeHtml(stage.label)}</h3><span>${stateLabel(stage.state)}</span></div><p>${escapeHtml(stage.description)}</p><output>${escapeHtml(stage.evidence)}</output></div>
    <button class="button button--quiet" type="button" data-action="select-workspace" data-workspace="${stage.workspace}" aria-label="Open ${escapeHtml(stage.label)}">Open</button>
  </li>`;
}

function artifactMarkup(artifact: PublishedArtifact): string {
  return `<article class="result-card">
    <div class="result-card__icon" aria-hidden="true">FITS</div>
    <div><p>${escapeHtml(artifact.kind)}</p><code title="${escapeHtml(artifact.path)}">${escapeHtml(artifact.path)}</code><small>${escapeHtml(artifact.evidence)}</small></div>
    <button class="button button--quiet" type="button" data-action="select-workspace" data-workspace="${artifact.workspace}" aria-label="Inspect ${escapeHtml(artifact.kind)}">Inspect</button>
  </article>`;
}

function stateLabel(state: StageState): string {
  if (state === "complete") return "Published";
  if (state === "ready") return "Ready";
  if (state === "running") return "Running";
  if (state === "attention") return "Attention";
  return "Waiting";
}

function shortSeal(value: string): string {
  return `SHA-256 ${value.slice(0, 12)}…`;
}

function escapeHtml(value: string): string {
  return value.replace(/[&<>"']/g, (character) => {
    const entities: Record<string, string> = {
      "&": "&amp;",
      "<": "&lt;",
      ">": "&gt;",
      '"': "&quot;",
      "'": "&#39;",
    };
    return entities[character] ?? character;
  });
}

function text(root: HTMLElement, selector: string, value: string): void {
  required<HTMLElement>(root, selector).textContent = value;
}

function required<T extends Element>(root: ParentNode, selector: string): T {
  const element = root.querySelector<T>(selector);
  if (!element)
    throw new Error(`Missing required workflow element: ${selector}`);
  return element;
}
