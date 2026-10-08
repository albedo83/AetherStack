import type { ReviewViewModel, WorkspaceView } from "./model.ts";

type StageState = "complete" | "ready" | "waiting" | "attention" | "running";

interface WorkflowStage {
  readonly label: string;
  readonly description: string;
  readonly workspace: WorkspaceView;
  /** Optional stages remain visible but never block a complete science run. */
  readonly required: boolean;
  readonly state: StageState;
  readonly evidence: string;
}

interface PublishedArtifact {
  readonly kind: string;
  readonly path: string;
  readonly evidence: string;
  readonly workspace: WorkspaceView;
}

interface WorkflowGuidance {
  readonly step: string;
  readonly title: string;
  readonly description: string;
  readonly actionLabel: string;
  readonly action: "import-session" | "select-workspace";
  readonly workspace?: WorkspaceView;
  readonly tone: "next" | "busy" | "attention" | "complete";
}

/** Persistent, plain-language orientation for first-time and expert users. */
export function workflowCoachMarkup(): string {
  return `<aside class="workflow-coach" data-workflow-coach data-tone="next" aria-labelledby="workflow-coach-title">
    <span class="workflow-coach__beacon" aria-hidden="true">?</span>
    <div class="workflow-coach__copy">
      <p><span data-workflow-coach-step>Getting started</span> · Recommended next action</p>
      <h2 id="workflow-coach-title" data-workflow-coach-title>Import an imaging session</h2>
      <span data-workflow-coach-description>Select the folder that contains your Lights and calibration frames.</span>
    </div>
    <button class="button button--primary" type="button" data-workflow-coach-action data-action="import-session">Import session</button>
  </aside>`;
}

/**
 * Explains the next useful action from native evidence. The coach never makes
 * a scientific choice: it only routes the user to the workspace that owns it.
 */
export function renderWorkflowCoach(
  coach: HTMLElement,
  model: ReviewViewModel,
): void {
  const guidance = deriveGuidance(model);
  coach.dataset.tone = guidance.tone;
  text(coach, "[data-workflow-coach-step]", guidance.step);
  text(coach, "[data-workflow-coach-title]", guidance.title);
  text(coach, "[data-workflow-coach-description]", guidance.description);
  const action = required<HTMLButtonElement>(
    coach,
    "[data-workflow-coach-action]",
  );
  action.textContent = guidance.actionLabel;
  action.dataset.action = guidance.action;
  if (guidance.workspace) action.dataset.workspace = guidance.workspace;
  else delete action.dataset.workspace;
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
      <p class="results-reveal-status" data-results-reveal-status role="status" aria-live="polite"></p>
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
  const requiredStages = stages.filter((stage) => stage.required);
  const complete = requiredStages.filter(
    (stage) => stage.state === "complete",
  ).length;
  const optionalComplete = stages.filter(
    (stage) => !stage.required && stage.state === "complete",
  ).length;
  const percent = Math.round((complete / requiredStages.length) * 100);
  text(
    runWorkspace,
    "[data-workflow-summary]",
    `${complete} of ${requiredStages.length} required stages published${optionalComplete > 0 ? ` · ${optionalComplete} optional stage published` : ""}`,
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
      required: true,
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
      required: true,
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
      required: true,
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
      required: true,
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
      required: false,
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
      required: true,
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

function deriveGuidance(model: ReviewViewModel): WorkflowGuidance {
  const sessionFrameCount = model.roles.reduce(
    (total, role) => total + role.count,
    0,
  );
  if (sessionFrameCount === 0) {
    if (model.sessionStatus.tone === "busy") {
      return guidance(
        "Step 1 of 5",
        "Importing and verifying FITS files",
        "AetherStack is reading headers, fingerprinting sources, and building a trustworthy session. You can follow progress in Frames.",
        "View import progress",
        "select-workspace",
        "busy",
        "frames",
      );
    }
    return guidance(
      "Step 1 of 5",
      "Import your imaging session",
      "Select one folder containing Lights, Darks, Flats, and optional Bias frames. Your source files remain read-only.",
      "Import session",
      "import-session",
      "next",
    );
  }

  if (!model.reviewSessionReady) {
    return guidance(
      "Review before processing",
      "Resolve the frame review",
      "Inspect uncertain frames and confirm which Lights should participate before creating scientific products.",
      "Review frames",
      "select-workspace",
      "attention",
      "frames",
    );
  }

  const masters = model.calibration.execution.result?.products.length ?? 0;
  const masterState = model.calibration.execution.state;
  if (masterState === "running" || masterState === "cancelling") {
    return guidance(
      "Step 2 of 5",
      "Calibration masters are being built",
      "Keep this application open. Progress and any recoverable issue are shown in Calibration.",
      "View master progress",
      "select-workspace",
      "busy",
      "calibration",
    );
  }
  if (masterState === "error") {
    return guidance(
      "Step 2 needs attention",
      "Resolve the calibration issue",
      "Open Calibration for the exact failure evidence. Existing source files and previously published products are unchanged.",
      "Open Calibration",
      "select-workspace",
      "attention",
      "calibration",
    );
  }
  if (masters === 0) {
    return guidance(
      "Step 2 of 5",
      "Build the calibration masters",
      "Review the proposed Dark, Flat, and optional Bias associations, then choose an empty output folder.",
      "Open Calibration",
      "select-workspace",
      "next",
      "calibration",
    );
  }

  const calibrated =
    model.calibration.lightExecution.result?.calibratedFrames.length ?? 0;
  const lightState = model.calibration.lightExecution.state;
  if (lightState === "running" || lightState === "cancelling") {
    return guidance(
      "Step 3 of 5",
      "Lights are being calibrated",
      "AetherStack is applying the reviewed master associations without modifying the original exposures.",
      "View calibration progress",
      "select-workspace",
      "busy",
      "calibration",
    );
  }
  if (lightState === "error") {
    return guidance(
      "Step 3 needs attention",
      "Resolve the Light calibration issue",
      "Open Calibration to inspect the failed source and exact native diagnostic before trying again.",
      "Open Calibration",
      "select-workspace",
      "attention",
      "calibration",
    );
  }
  if (calibrated === 0) {
    return guidance(
      "Step 3 of 5",
      "Calibrate the accepted Lights",
      "Create calibrated linear images from the masters. Color CFA data can also produce debayered RGB previews for Blink.",
      "Open Light calibration",
      "select-workspace",
      "next",
      "calibration",
    );
  }

  const registered = model.registration.execution.result?.frames.length ?? 0;
  const registrationState = model.registration.execution.state;
  if (registrationState === "running" || registrationState === "cancelling") {
    return guidance(
      "Step 4 of 5",
      "Lights are being registered",
      "The native solver is aligning the accepted frames and validating their common image area.",
      "View registration progress",
      "select-workspace",
      "busy",
      "registration",
    );
  }
  if (registrationState === "error") {
    return guidance(
      "Step 4 needs attention",
      "Resolve the registration issue",
      "Open Registration to inspect star matches, residuals, overlap, and the suggested corrective action.",
      "Open Registration",
      "select-workspace",
      "attention",
      "registration",
    );
  }
  if (registered === 0) {
    return guidance(
      "Step 4 of 5",
      "Register the calibrated Lights",
      "Analyze a representative pair, review the geometry evidence, then publish the complete aligned set.",
      "Open Registration",
      "select-workspace",
      "next",
      "registration",
    );
  }

  const stackState = model.registration.stack.state;
  if (stackState === "running" || stackState === "cancelling") {
    return guidance(
      "Step 5 of 5",
      "The registered stack is being integrated",
      "Keep the application open while AetherStack publishes the science image and requested evidence maps atomically.",
      "View integration progress",
      "select-workspace",
      "busy",
      "registration",
    );
  }
  if (stackState === "error") {
    return guidance(
      "Step 5 needs attention",
      "Resolve the integration issue",
      "Open Registration to inspect the estimator, source evidence, output destination, and failure diagnostic.",
      "Open Integration",
      "select-workspace",
      "attention",
      "registration",
    );
  }
  if (!model.registration.stack.result) {
    return guidance(
      "Step 5 of 5",
      "Integrate the registered Lights",
      "Choose Automatic for a documented safe policy, or open Advanced for explicit rejection controls. Local normalization remains optional.",
      "Open Integration",
      "select-workspace",
      "next",
      "registration",
    );
  }

  return guidance(
    "Processing complete",
    "Your integrated science product is ready",
    "Review every published FITS product and its reproducibility evidence, then reveal the files in the system file manager.",
    "View Results",
    "select-workspace",
    "complete",
    "results",
  );
}

function guidance(
  step: string,
  title: string,
  description: string,
  actionLabel: string,
  action: WorkflowGuidance["action"],
  tone: WorkflowGuidance["tone"],
  workspace?: WorkspaceView,
): WorkflowGuidance {
  return { step, title, description, actionLabel, action, tone, workspace };
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
    <div><div class="workflow-stage__title"><h3>${escapeHtml(stage.label)}</h3><span>${stage.required ? stateLabel(stage.state) : optionalStateLabel(stage.state)}</span></div><p>${escapeHtml(stage.description)}</p><output>${escapeHtml(stage.evidence)}</output></div>
    <button class="button button--quiet" type="button" data-action="select-workspace" data-workspace="${stage.workspace}" aria-label="Open ${escapeHtml(stage.label)}">Open</button>
  </li>`;
}

function optionalStateLabel(state: StageState): string {
  if (state === "complete") return "Optional · complete";
  if (state === "running") return "Optional · running";
  if (state === "attention") return "Optional · attention";
  return "Optional";
}

function artifactMarkup(artifact: PublishedArtifact): string {
  return `<article class="result-card">
    <div class="result-card__icon" aria-hidden="true">FITS</div>
    <div><p>${escapeHtml(artifact.kind)}</p><code title="${escapeHtml(artifact.path)}">${escapeHtml(artifact.path)}</code><small>${escapeHtml(artifact.evidence)}</small></div>
    <div class="result-card__actions">
      <button class="button button--quiet" type="button" data-action="select-workspace" data-workspace="${artifact.workspace}" aria-label="Open ${escapeHtml(artifact.kind)} workspace">Workspace</button>
      <button class="button button--primary" type="button" data-action="reveal-artifact" data-artifact-path="${escapeHtml(artifact.path)}" data-artifact-kind="${escapeHtml(artifact.kind)}" aria-label="Reveal ${escapeHtml(artifact.kind)} in the file manager">Show file</button>
    </div>
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
