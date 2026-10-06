import {
  fireEvent,
  getByRole,
  getByText,
  queryByText,
  within,
} from "@testing-library/dom";
import axe from "axe-core";
import { describe, expect, it, vi } from "vitest";

import { demoReviewModel } from "./demo-data.ts";
import type { ReviewActions, ReviewViewModel } from "./model.ts";
import { mountReviewScreen } from "./review-screen.ts";

function fixture(model: ReviewViewModel = demoReviewModel) {
  const root = document.createElement("div");
  root.id = "root";
  document.body.append(root);
  const actions: ReviewActions = {
    onSelectWorkspace: vi.fn(),
    onSelectLocalNormalizationSource: vi.fn(),
    onSelectLocalNormalizationReference: vi.fn(),
    onSelectLocalNormalizationOutput: vi.fn(),
    onUpdateLocalNormalizationSettings: vi.fn(),
    onUpdateLocalNormalizationMemoryLimit: vi.fn(),
    onUpdateLocalNormalizationGroupId: vi.fn(),
    onExecuteLocalNormalization: vi.fn(),
    onPreflightLocalNormalization: vi.fn(),
    onCancelLocalNormalization: vi.fn(),
    onSelectLocalNormalizationPreview: vi.fn(),
    onInspectLocalNormalizationStatistics: vi.fn(),
    onSelectRegistrationReference: vi.fn(),
    onSelectRegistrationSource: vi.fn(),
    onSelectRegistrationGeometryModel: vi.fn(),
    onAnalyzeRegistration: vi.fn(),
    onExecuteRegistration: vi.fn(),
    onCancelRegistration: vi.fn(),
    onExecuteRegisteredStack: vi.fn(),
    onCancelRegisteredStack: vi.fn(),
    onExecuteDrizzle: vi.fn(),
    onCancelDrizzle: vi.fn(),
    onUpdateDrizzleSettings: vi.fn(),
    onSelectDrizzleProduct: vi.fn(),
    onInspectDrizzleStatistics: vi.fn(),
    onInspectDrizzlePixel: vi.fn(),
    onSelectDrizzleWeighting: vi.fn(),
    onUpdateRegisteredStackSettings: vi.fn(),
    onSelectRegisteredStackProduct: vi.fn(),
    onSetRegisteredStackOverlayOpacity: vi.fn(),
    onInspectRegisteredStackPixel: vi.fn(),
    onInspectRegisteredStackReport: vi.fn(),
    onOpenRegisteredStackReport: vi.fn(),
    onReturnToActiveStack: vi.fn(),
    onVerifyRegisteredStackSources: vi.fn(),
    onCancelRegisteredStackSourceVerification: vi.fn(),
    onSelectRegisteredFrame: vi.fn(),
    onSetRegisteredPlaying: vi.fn(),
    onStepRegisteredFrame: vi.fn(),
    onUpdateCalibrationSettings: vi.fn(),
    onUpdateLightOutputMode: vi.fn(),
    onRefreshMasterPlan: vi.fn(),
    onExecuteMasterPlan: vi.fn(),
    onCancelMasterPlan: vi.fn(),
    onExecuteLightPlan: vi.fn(),
    onCancelLightPlan: vi.fn(),
    onUpdateDefectCorrectionSettings: vi.fn(),
    onExecuteDefectCorrection: vi.fn(),
    onExecuteAllDefectCorrections: vi.fn(),
    onExportDefectBatchReport: vi.fn(),
    onCancelDefectCorrection: vi.fn(),
    onSelectDefectPreview: vi.fn(),
    onImportSession: vi.fn(),
    onExportDiagnostics: vi.fn(),
    onInspectDiagnosticsReport: vi.fn(),
    onPreviewQualityCacheMaintenance: vi.fn(),
    onApplyQualityCacheMaintenance: vi.fn(),
    onSelectRole: vi.fn(),
    onSelectLightFrameView: vi.fn(),
    onSelectFrame: vi.fn(),
    onSort: vi.fn(),
    onSetDecision: vi.fn(),
    onClearDecision: vi.fn(),
    onUndo: vi.fn(),
    onSetPlaying: vi.fn(),
    onRequestStep: vi.fn(),
    onSetViewerScale: vi.fn(),
    onOpenStatistics: vi.fn(),
    onCloseStatistics: vi.fn(),
    onMeasureQuality: vi.fn(),
    onMeasureAllQuality: vi.fn(),
    onUpdateFrameSelectionRules: vi.fn(),
    onPreviewFrameSelection: vi.fn(),
    onApplyFrameSelection: vi.fn(),
  };
  const controller = mountReviewScreen(root, model, actions);
  return { root, actions, controller };
}

describe("frame review workspace", () => {
  it("presents a clear local-normalization transaction and native file choices", () => {
    const { root, actions, controller } = fixture({
      ...demoReviewModel,
      activeWorkspace: "normalization",
    });
    const workspace = root.querySelector<HTMLElement>(
      "[data-normalization-workspace]",
    );
    expect(workspace?.hidden).toBe(false);
    expect(
      getByRole(workspace!, "heading", {
        name: "Match the sky, preserve the signal",
      }),
    ).toBeTruthy();
    fireEvent.click(getByRole(workspace!, "button", { name: "Choose source" }));
    fireEvent.click(
      getByRole(workspace!, "button", { name: "Choose reference" }),
    );
    fireEvent.click(getByRole(workspace!, "button", { name: "Choose output" }));
    expect(actions.onSelectLocalNormalizationSource).toHaveBeenCalledOnce();
    expect(actions.onSelectLocalNormalizationReference).toHaveBeenCalledOnce();
    expect(actions.onSelectLocalNormalizationOutput).toHaveBeenCalledOnce();

    const execute = getByRole(workspace!, "button", {
      name: "Normalize image",
    }) as HTMLButtonElement;
    expect(execute.disabled).toBe(true);
    controller.update({
      ...demoReviewModel,
      activeWorkspace: "normalization",
      localNormalization: {
        ...demoReviewModel.localNormalization,
        sourcePath: "/session/source.fits",
        referencePath: "/session/reference.fits",
        outputPath: "/session/normalized.fits",
      },
    });
    expect(execute.disabled).toBe(false);
    fireEvent.click(getByRole(workspace!, "button", { name: "Verify memory" }));
    expect(actions.onPreflightLocalNormalization).toHaveBeenCalledOnce();
    controller.update({
      ...demoReviewModel,
      activeWorkspace: "normalization",
      localNormalization: {
        ...demoReviewModel.localNormalization,
        sourcePath: "/session/source.fits",
        referencePath: "/session/reference.fits",
        outputPath: "/session/normalized.fits",
        preflightState: "ready",
        preflight: {
          width: 4_144,
          height: 2_822,
          planes: 1,
          requiredBytes: 768_000_000,
          memoryLimitBytes: 2_147_483_648,
          headroomBytes: 1_379_483_648,
          fitsMemoryLimit: true,
          planeImagesBytes: 256_000_000,
          applicationBandBytes: 8_000_000,
          decodeStatusBytes: 32_000_000,
          retainedSamplesBytes: 400_000_000,
          diagnosticsBytes: 1_000_000,
          qualityBytes: 63_000_000,
          slopeBytes: 7_934_464,
          writerBufferBytes: 65_536,
        },
      },
    });
    expect(
      workspace!.querySelector("[data-localnorm-memory-images]")?.textContent,
    ).toContain("251.8 MiB");
    expect(
      workspace!.querySelector("[data-localnorm-memory-fit]")?.textContent,
    ).toContain("449.1 MiB");
    fireEvent.click(execute);
    expect(actions.onExecuteLocalNormalization).toHaveBeenCalledOnce();

    controller.update({
      ...demoReviewModel,
      activeWorkspace: "normalization",
      localNormalization: {
        ...demoReviewModel.localNormalization,
        sourcePath: "/session/source.fits",
        referencePath: "/session/reference.fits",
        outputPath: "/session/normalized.fits",
        preflightState: "ready",
        preflight: {
          width: 4_144,
          height: 2_822,
          planes: 1,
          requiredBytes: 440_485_472,
          memoryLimitBytes: 268_435_456,
          headroomBytes: 0,
          fitsMemoryLimit: false,
          planeImagesBytes: 234_000_000,
          applicationBandBytes: 6_300_000,
          decodeStatusBytes: 23_400_000,
          retainedSamplesBytes: 145_000_000,
          diagnosticsBytes: 300_000,
          qualityBytes: 23_000_000,
          slopeBytes: 8_419_936,
          writerBufferBytes: 65_536,
        },
      },
    });
    expect(execute.disabled).toBe(true);
    fireEvent.click(getByRole(workspace!, "button", { name: "Use 512 MiB" }));
    expect(actions.onUpdateLocalNormalizationMemoryLimit).toHaveBeenCalledWith(
      536_870_912,
    );
  });

  it("routes the Normalize workflow navigation", () => {
    const { root, actions } = fixture();
    fireEvent.click(getByRole(root, "button", { name: "Normalize" }));
    expect(actions.onSelectWorkspace).toHaveBeenCalledWith("normalization");
  });

  it("exposes all plan-bound controls and renders scientific evidence", () => {
    const { root, actions, controller } = fixture({
      ...demoReviewModel,
      activeWorkspace: "normalization",
    });
    const workspace = root.querySelector<HTMLElement>(
      "[data-normalization-workspace]",
    )!;
    expect(
      workspace.querySelectorAll<HTMLInputElement>("[data-localnorm-setting]"),
    ).toHaveLength(27);
    const detection = workspace.querySelector<HTMLInputElement>(
      '[data-localnorm-setting="detectionSigma"]',
    )!;
    detection.value = "7.5";
    fireEvent.change(detection);
    expect(actions.onUpdateLocalNormalizationSettings).toHaveBeenCalledWith(
      expect.objectContaining({ detectionSigma: 7.5 }),
    );

    controller.update({
      ...demoReviewModel,
      activeWorkspace: "normalization",
      localNormalization: {
        ...demoReviewModel.localNormalization,
        state: "completed",
        result: {
          planSha256: "a".repeat(64),
          parametersSha256: "b".repeat(64),
          outputPath: "/session/normalized.fits",
          width: 4_144,
          height: 2_822,
          planes: 3,
          memoryLimitBytes: 2_147_483_648,
          peakReservedBytes: 1_073_741_824,
          samplesWritten: 35_087_124,
          substitutedSamples: 0,
          bytesWritten: 280_700_000,
          transformedSamples: 35_087_124,
          inheritedMaskedSamples: 0,
          nonFiniteInputSamples: 0,
          unsupportedSurfaceSamples: 0,
          nonFiniteResultSamples: 0,
          measuredSources: 12_000,
          protectedPixels: 900_000,
          validControlPoints: 759,
          rejectedCells: 4,
          controlPoints: [
            {
              plane: 0,
              x: 64,
              y: 96,
              scale: 1.002,
              offset: -12.5,
              medianAbsoluteResidual: 0.8,
            },
          ],
          cellDiagnostics: [],
        },
        message: "Published atomically",
      },
    });
    expect(workspace.querySelector("[data-localnorm-stars]")?.textContent).toBe(
      "12,000",
    );
    expect(workspace.querySelector("[data-localnorm-plan]")?.textContent).toBe(
      "a".repeat(64),
    );
    expect(
      workspace.querySelector("[data-localnorm-publication]")?.textContent,
    ).toContain("35,087,124 samples");
    const sourceTab = getByRole(workspace, "tab", { name: "Source" });
    expect(sourceTab.hasAttribute("disabled")).toBe(false);
    fireEvent.click(sourceTab);
    expect(actions.onSelectLocalNormalizationPreview).toHaveBeenCalledWith(
      "source",
    );
    controller.update({
      ...demoReviewModel,
      activeWorkspace: "normalization",
      localNormalization: {
        ...demoReviewModel.localNormalization,
        state: "completed",
        result: {
          planSha256: "a".repeat(64),
          parametersSha256: "b".repeat(64),
          outputPath: "/session/normalized.fits",
          width: 4_144,
          height: 2_822,
          planes: 3,
          memoryLimitBytes: 2_147_483_648,
          peakReservedBytes: 1_073_741_824,
          samplesWritten: 35_087_124,
          substitutedSamples: 0,
          bytesWritten: 280_700_000,
          transformedSamples: 35_087_124,
          inheritedMaskedSamples: 0,
          nonFiniteInputSamples: 0,
          unsupportedSurfaceSamples: 0,
          nonFiniteResultSamples: 0,
          measuredSources: 12_000,
          protectedPixels: 900_000,
          validControlPoints: 759,
          rejectedCells: 4,
          controlPoints: [
            {
              plane: 0,
              x: 64,
              y: 96,
              scale: 1.002,
              offset: -12.5,
              medianAbsoluteResidual: 0.8,
            },
          ],
          cellDiagnostics: [
            {
              plane: 0,
              x: 128,
              y: 128,
              width: 128,
              height: 128,
              protected: 16_384,
              sourceMasked: 0,
              referenceMasked: 0,
              nonFinite: 0,
              eligible: 0,
              retained: 0,
              accepted: false,
              rejectionCode: "sample_count_outside_bounds",
            },
          ],
        },
        previewState: "ready",
        previewView: "source",
        preview: { frameId: "source-preview", url: "blob:source-preview" },
        previewMessage: "Source rendered from native FITS pixels",
        sharedStretchLabel: "Shared reference stretch · robust-v1",
        message: "Published atomically",
      },
    });
    const image = workspace.querySelector<HTMLImageElement>(
      "[data-localnorm-preview-image]",
    )!;
    expect(image.hidden).toBe(false);
    expect(image.alt).toBe("Source local-normalization preview");
    const control = workspace.querySelector<SVGCircleElement>(
      "[data-localnorm-control-overlay] circle",
    );
    expect(control?.getAttribute("cx")).toBe("64");
    expect(control?.querySelector("title")?.textContent).toContain(
      "median residual 0.8000",
    );
    const rejectedCell = workspace.querySelector<SVGRectElement>(
      "[data-localnorm-control-overlay] rect",
    );
    expect(rejectedCell?.getAttribute("x")).toBe("128");
    expect(rejectedCell?.querySelector("title")?.textContent).toContain(
      "sample count outside bounds",
    );
    expect(
      workspace.querySelector("[data-localnorm-map-summary]")?.textContent,
    ).toBe("1 accepted · 1 rejected");
    expect(sourceTab.getAttribute("aria-selected")).toBe("true");
    fireEvent.click(
      getByRole(workspace, "button", { name: "Calculate exact statistics" }),
    );
    expect(
      actions.onInspectLocalNormalizationStatistics,
    ).toHaveBeenCalledOnce();
    controller.update({
      ...demoReviewModel,
      activeWorkspace: "normalization",
      localNormalization: {
        ...demoReviewModel.localNormalization,
        state: "completed",
        result: {
          planSha256: "a".repeat(64),
          parametersSha256: "b".repeat(64),
          outputPath: "/session/normalized.fits",
          width: 4_144,
          height: 2_822,
          planes: 3,
          memoryLimitBytes: 2_147_483_648,
          peakReservedBytes: 1_073_741_824,
          samplesWritten: 35_087_124,
          substitutedSamples: 0,
          bytesWritten: 280_700_000,
          transformedSamples: 35_087_124,
          inheritedMaskedSamples: 0,
          nonFiniteInputSamples: 0,
          unsupportedSurfaceSamples: 0,
          nonFiniteResultSamples: 0,
          measuredSources: 12_000,
          protectedPixels: 900_000,
          validControlPoints: 759,
          rejectedCells: 4,
          controlPoints: [],
          cellDiagnostics: [],
        },
        previewView: "source",
        statisticsState: "ready",
        statisticsView: "source",
        statistics: {
          algorithmId: "fits-primary-statistics-f64-v1",
          axes: [4_144, 2_822, 3],
          storedFormat: "f64",
          headerConformant: true,
          headerDiagnostics: 0,
          totalSamples: 35_087_124,
          usableSamples: 35_087_120,
          undefinedSamples: 4,
          nonFiniteSamples: 0,
          minimum: -12.5,
          maximum: 65_535,
          mean: 1_234.5,
          populationStandardDeviation: 42.25,
          sampleStandardDeviation: 42.250001,
        },
        statisticsMessage: "Source · fits-primary-statistics-f64-v1",
        message: "Published atomically",
      },
    });
    expect(
      workspace.querySelector("[data-localnorm-stat-mean]")?.textContent,
    ).toBe("1,234.5");
    expect(
      workspace.querySelector("[data-localnorm-stat-usable]")?.textContent,
    ).toContain("35,087,120");
  });

  it("offers an explicit projective geometry choice", () => {
    const { root, actions } = fixture({
      ...demoReviewModel,
      activeWorkspace: "registration",
    });
    const affine = getByRole(root, "button", { name: /Affine/ });
    const projective = getByRole(root, "button", { name: /Projective/ });

    expect(affine.getAttribute("aria-pressed")).toBe("true");
    expect(projective.getAttribute("aria-pressed")).toBe("false");
    fireEvent.click(projective);
    expect(actions.onSelectRegistrationGeometryModel).toHaveBeenCalledWith(
      "projective",
    );
  });

  it("opens the advanced review plan from the primary top-bar action", async () => {
    const { root, actions } = fixture();
    const panel = root.querySelector<HTMLDetailsElement>(
      "[data-selection-panel]",
    );
    expect(panel?.open).toBe(false);

    fireEvent.click(getByRole(root, "button", { name: "Review plan" }));
    await Promise.resolve();

    expect(actions.onSelectWorkspace).toHaveBeenCalledWith("frames");
    expect(panel?.open).toBe(true);
  });

  it("edits typed quality gates and requests a native preview only when evidence is ready", () => {
    const frames = demoReviewModel.frames.map((frame, index) => ({
      ...frame,
      sourcePath: `/session/LIGHTS/light-${index}.fits`,
      qualityState: "ready" as const,
    }));
    const { root, actions } = fixture({ ...demoReviewModel, frames });
    const panel = root.querySelector<HTMLDetailsElement>(
      "[data-selection-panel]",
    );
    const threshold = root.querySelector<HTMLInputElement>(
      '[data-selection-rule][data-rule-index="0"] [data-selection-threshold]',
    );
    expect(panel).not.toBeNull();
    expect(threshold).not.toBeNull();
    if (!panel || !threshold) return;
    panel.open = true;

    fireEvent.change(threshold, { target: { value: "3.75" } });
    expect(actions.onUpdateFrameSelectionRules).toHaveBeenCalledWith(
      expect.arrayContaining([
        expect.objectContaining({
          metric: "fwhm_pixels",
          threshold: { kind: "scalar", value: 3.75 },
        }),
      ]),
    );

    const preview = getByRole(root, "button", {
      name: "Preview recommendations",
    });
    expect(preview.hasAttribute("disabled")).toBe(false);
    fireEvent.click(preview);
    expect(actions.onPreviewFrameSelection).toHaveBeenCalledOnce();
  });

  it("adds and removes unique quality gates without allowing an empty rule set", () => {
    const { root, actions } = fixture();

    fireEvent.click(getByRole(root, "button", { name: "＋ Add quality gate" }));
    expect(actions.onUpdateFrameSelectionRules).toHaveBeenLastCalledWith([
      ...demoReviewModel.frameSelection.rules,
      {
        metric: "signal_to_noise",
        comparator: "greater_than",
        threshold: { kind: "scalar", value: 10 },
        missingPolicy: "reject",
      },
    ]);

    fireEvent.click(getByRole(root, "button", { name: "Remove rule 2" }));
    expect(actions.onUpdateFrameSelectionRules).toHaveBeenLastCalledWith([
      demoReviewModel.frameSelection.rules[0],
      demoReviewModel.frameSelection.rules[2],
    ]);

    const singleRule = {
      ...demoReviewModel,
      frameSelection: {
        ...demoReviewModel.frameSelection,
        rules: demoReviewModel.frameSelection.rules.slice(0, 1),
      },
    };
    const only = fixture(singleRule);
    expect(
      getByRole(only.root, "button", { name: "Remove rule 1" }).hasAttribute(
        "disabled",
      ),
    ).toBe(true);
  });

  it("disables metrics already used by another quality gate", () => {
    const { root } = fixture();
    const secondMetric = root.querySelector<HTMLSelectElement>(
      '[data-selection-rule][data-rule-index="1"] [data-selection-metric]',
    );
    const fwhm = secondMetric?.querySelector<HTMLOptionElement>(
      'option[value="fwhm_pixels"]',
    );
    const eccentricity = secondMetric?.querySelector<HTMLOptionElement>(
      'option[value="eccentricity"]',
    );

    expect(fwhm?.disabled).toBe(true);
    expect(eccentricity?.disabled).toBe(false);
  });

  it("shows canonical selection totals and per-frame proposals without changing decisions", () => {
    const first = demoReviewModel.frames[0];
    const second = demoReviewModel.frames[1];
    expect(first).toBeDefined();
    expect(second).toBeDefined();
    if (!first || !second) return;
    const plan = {
      schemaVersion: 1,
      algorithmId: "frame-selection-rules-v1",
      rules: demoReviewModel.frameSelection.rules,
      frames: [
        {
          frameId: first.id,
          proposal: "retain" as const,
          evidence: demoReviewModel.frameSelection.rules.map(() => ({
            measured: null,
            state: "passed" as const,
          })),
        },
        {
          frameId: second.id,
          proposal: "reject" as const,
          evidence: demoReviewModel.frameSelection.rules.map((_, index) => ({
            measured:
              index === 0 ? ({ kind: "scalar", value: 5.84 } as const) : null,
            state: index === 0 ? ("failed" as const) : ("passed" as const),
          })),
        },
      ],
      planSha256: "d".repeat(64),
    };
    const { root } = fixture({
      ...demoReviewModel,
      frameSelection: {
        ...demoReviewModel.frameSelection,
        state: "ready",
        plan,
        message: "1 retained · 1 proposed reject · no decisions changed",
      },
    });

    expect(root.textContent).toContain("AUTO KEEP");
    expect(root.textContent).toContain("AUTO REJECT · FWHM");
    const rejectedProposal = root.querySelector<HTMLElement>(
      `[data-frame-id="${second.id}"] .selection-proposal`,
    );
    expect(rejectedProposal?.title).toBe(
      "Automatic recommendation: reject. Failed quality gates: FWHM.",
    );
    const evidence = root.querySelector<HTMLElement>(
      "[data-selection-evidence]",
    );
    expect(evidence?.hidden).toBe(false);
    expect(evidence?.textContent).toContain(second.label);
    expect(evidence?.textContent).toContain("FWHM");
    expect(evidence?.textContent).toContain("5.84 < 4.5");
    expect(evidence?.textContent).toContain("Fail");
    expect(root.textContent).toContain("1 retained · 1 proposed reject");
    expect(root.textContent).toContain("d".repeat(64));
    expect(first.state).toBe("accepted");
    expect(second.state).toBe("undecided");
  });

  it("requires explicit confirmation before applying recommendations to undecided frames", () => {
    const first = demoReviewModel.frames[0];
    const second = demoReviewModel.frames[1];
    expect(first).toBeDefined();
    expect(second).toBeDefined();
    if (!first || !second) return;
    const { root, actions } = fixture({
      ...demoReviewModel,
      frameSelection: {
        ...demoReviewModel.frameSelection,
        state: "ready",
        plan: {
          schemaVersion: 1,
          algorithmId: "frame-selection-rules-v1",
          rules: demoReviewModel.frameSelection.rules,
          frames: [
            { frameId: first.id, proposal: "retain", evidence: [] },
            { frameId: second.id, proposal: "reject", evidence: [] },
          ],
          planSha256: "d".repeat(64),
        },
        message: "Ready to confirm",
      },
    });

    fireEvent.click(getByRole(root, "button", { name: "Apply 1 undecided" }));
    const dialog = getByRole(root, "dialog", {
      name: "Apply the native recommendations?",
    });
    expect(dialog.textContent).toContain("1 undecided Lights");
    expect(dialog.textContent).toContain("manual decisions stay untouched");
    expect(actions.onApplyFrameSelection).not.toHaveBeenCalled();

    fireEvent.click(
      getByRole(dialog, "button", { name: "Apply reviewed plan" }),
    );
    expect(actions.onApplyFrameSelection).toHaveBeenCalledOnce();
  });

  it("requests a native session import from the primary workspace action", () => {
    const { root, actions } = fixture();

    fireEvent.click(getByRole(root, "button", { name: "＋ Import session" }));

    expect(actions.onImportSession).toHaveBeenCalledOnce();
  });

  it("turns the busy import action into an explicit cancellation", () => {
    const { root, actions, controller } = fixture({
      ...demoReviewModel,
      sessionStatus: { tone: "busy", label: "Scanning FITS sources" },
      sessionImportProgress: {
        stage: "analyzing",
        completed: 32,
        total: 135,
      },
    });

    const button = getByRole(root, "button", { name: "× Cancel import" });
    const progress = getByRole<HTMLProgressElement>(root, "progressbar", {
      name: "Session import progress",
    });
    expect(button.hasAttribute("disabled")).toBe(false);
    expect(button.classList.contains("button--danger")).toBe(true);
    expect(progress.value).toBe(32);
    expect(progress.max).toBe(135);
    expect(progress.getAttribute("aria-valuetext")).toBe(
      "32 of 135 FITS sources analyzed",
    );
    expect(root.textContent).toContain("Scanning FITS sources");
    fireEvent.click(button);
    expect(actions.onImportSession).toHaveBeenCalledOnce();

    controller.update({
      ...demoReviewModel,
      sessionStatus: { tone: "busy", label: "Cancelling FITS import" },
      sessionImportProgress: null,
    });
    expect(
      getByRole(root, "button", { name: "Cancelling…" }).hasAttribute(
        "disabled",
      ),
    ).toBe(true);
    expect(progress.hasAttribute("value")).toBe(false);
  });

  it("presents FITS and quality-cache diagnostics with explicit severity", () => {
    const { root, actions } = fixture({
      ...demoReviewModel,
      reviewSessionReady: true,
      sessionDiagnostics: {
        filesConsidered: 42,
        fingerprintedSourceBytes: 3_158_611_200,
        scanElapsedMilliseconds: 2_500,
        sourceAnalysisParallelism: 8,
        verifiedFrames: 40,
        classificationConflicts: 1,
        recoverableFailures: 2,
        unassignedSources: 3,
        qualityEvidenceRestored: 18,
        qualityEvidenceMissing: 4,
        qualityEvidenceRejected: 1,
        items: [
          {
            category: "quality_cache",
            source: "LIGHTS/light_0042.fits",
            code: "quality_cache_artifact_invalid",
          },
        ],
        omittedItems: 2,
        exportState: "idle",
        exportMessage: "No redacted report exported",
        inspectionState: "idle",
        inspectionMessage: "No diagnostics report verified",
        maintenanceState: "ready",
        maintenanceMessage: "1 removable · 0 blocked · preview only",
        maintenanceEligible: 1,
        maintenanceBlocked: 0,
        maintenanceBytes: 4_096,
        maintenancePlanSha256: "a".repeat(64),
      },
    });

    const trigger = getByRole(root, "button", { name: "Diagnostics" });
    fireEvent.click(trigger);
    const dialog = getByRole(root, "dialog", { name: "Import diagnostics" });
    expect(dialog.textContent).toContain("7 items require attention");
    expect(dialog.textContent).toContain("Sources considered42");
    expect(dialog.textContent).toContain("Fingerprinted bytes2.9 GiB");
    expect(dialog.textContent).toContain("Native scan time2.50 s");
    expect(dialog.textContent).toContain("Import worker limit8");
    expect(dialog.textContent).toContain("Verified frames40");
    expect(dialog.textContent).toContain("Restored18");
    expect(dialog.textContent).toContain("Missing4");
    expect(dialog.textContent).toContain("Rejected1");
    expect(dialog.textContent).toContain("never enter a selection plan");
    expect(dialog.textContent).toContain("LIGHTS/light_0042.fits");
    expect(dialog.textContent).toContain("quality_cache_artifact_invalid");
    expect(dialog.textContent).toContain("2 additional items omitted");
    expect(dialog.textContent).toContain("1 removable · 4.0 KiB");
    fireEvent.click(getByRole(dialog, "button", { name: "Preview cleanup" }));
    expect(actions.onPreviewQualityCacheMaintenance).toHaveBeenCalledOnce();
    fireEvent.click(
      getByRole(dialog, "button", { name: "Remove inspected files" }),
    );
    const confirmation = getByRole(root, "dialog", {
      name: "Remove only the inspected artifacts?",
    });
    expect(confirmation.textContent).toContain("1 rejected artifact · 4.0 KiB");
    expect(confirmation.textContent).toContain(`Plan sha256 ${"a".repeat(64)}`);
    expect(confirmation.textContent).toContain(
      "Original FITS files are never targeted",
    );
    fireEvent.click(getByRole(confirmation, "button", { name: "Keep files" }));
    expect(actions.onApplyQualityCacheMaintenance).not.toHaveBeenCalled();
    fireEvent.click(
      getByRole(dialog, "button", { name: "Remove inspected files" }),
    );
    fireEvent.click(
      getByRole(
        getByRole(root, "dialog", {
          name: "Remove only the inspected artifacts?",
        }),
        "button",
        { name: "Remove inspected files" },
      ),
    );
    expect(actions.onApplyQualityCacheMaintenance).toHaveBeenCalledOnce();
    fireEvent.click(
      getByRole(dialog, "button", { name: "Export redacted JSON" }),
    );
    expect(actions.onExportDiagnostics).toHaveBeenCalledOnce();
    fireEvent.click(getByRole(dialog, "button", { name: "Verify report" }));
    expect(actions.onInspectDiagnosticsReport).toHaveBeenCalledOnce();

    fireEvent.keyDown(root, { key: "Escape" });
    expect(
      root.querySelector<HTMLElement>("[data-diagnostics-dialog]")?.hidden,
    ).toBe(true);
  });

  it("filters bounded diagnostic evidence without changing native totals", () => {
    const { root } = fixture({
      ...demoReviewModel,
      sessionDiagnostics: {
        ...demoReviewModel.sessionDiagnostics,
        items: [
          {
            category: "fits",
            source: "LIGHTS/light_0001.fits",
            code: "fits_truncated_data",
          },
          {
            category: "quality_cache",
            source: "LIGHTS/light_0042.fits",
            code: "quality_cache_artifact_invalid",
          },
          {
            category: "classification",
            source: "DARKS/dark_0007.fits",
            code: "classification_conflict",
          },
        ],
      },
    });

    fireEvent.click(getByRole(root, "button", { name: "Diagnostics" }));
    const dialog = getByRole(root, "dialog", { name: "Import diagnostics" });
    expect(dialog.textContent).toContain("3 of 3 displayed issues");

    const cacheFilter = getByRole(dialog, "button", {
      name: "Quality cache",
    });
    fireEvent.click(cacheFilter);
    expect(cacheFilter.getAttribute("aria-pressed")).toBe("true");
    expect(dialog.textContent).toContain("1 of 3 displayed issues");
    expect(dialog.textContent).toContain("light_0042.fits");
    expect(dialog.textContent).not.toContain("light_0001.fits");

    fireEvent.click(getByRole(dialog, "button", { name: "All" }));
    fireEvent.input(
      getByRole(dialog, "searchbox", {
        name: "Search diagnostic evidence",
      }),
      { target: { value: "dark_0007" } },
    );
    expect(dialog.textContent).toContain("1 of 3 displayed issues");
    expect(dialog.textContent).toContain("DARKS/dark_0007.fits");
    expect(dialog.textContent).not.toContain("LIGHTS/light_0042.fits");

    fireEvent.input(
      getByRole(dialog, "searchbox", {
        name: "Search diagnostic evidence",
      }),
      { target: { value: "not-present" } },
    );
    expect(dialog.textContent).toContain(
      "No issue evidence matches the current filter.",
    );
    expect(dialog.textContent).toContain("0 of 3 displayed issues");
  });

  it("keeps classification overrides visibly and accessibly explained", () => {
    const first = demoReviewModel.frames[0];
    expect(first).toBeDefined();
    if (!first) return;
    const explanation =
      "Header and directory frame types conflict; the directory role was applied";
    const { root } = fixture({
      ...demoReviewModel,
      frames: [
        { ...first, classificationWarning: explanation },
        ...demoReviewModel.frames.slice(1),
      ],
    });

    const warning = root.querySelector<HTMLElement>(".classification-warning");
    expect(warning?.getAttribute("aria-label")).toBe(explanation);
    expect(warning?.title).toBe(explanation);
  });

  it("keeps all four acquisition roles visibly separate", () => {
    const { root } = fixture();
    const tabs = getByRole(root, "tablist", { name: "Frame types" });

    expect(within(tabs).getAllByRole("tab")).toHaveLength(4);
    expect(getByRole(tabs, "tab", { name: /Bias/ }).textContent).toContain("0");
    expect(getByRole(tabs, "tab", { name: /Darks/ }).textContent).toContain(
      "204",
    );
    expect(getByRole(tabs, "tab", { name: /Flats/ }).textContent).toContain(
      "26",
    );
    expect(
      getByRole(tabs, "tab", { name: /Lights/ }).getAttribute("aria-selected"),
    ).toBe("true");
  });

  it("switches between raw and identity-bound calibrated Light pixels", () => {
    const calibratedFrame = {
      groupId: "light-uvir",
      sourceIndex: 0,
      sourceFrameId: demoReviewModel.frames[0]?.id ?? "a".repeat(64),
      sourceLabel: "light_0001.fits",
      sourceSha256: "f".repeat(64),
      outputPath: "/runtime/calibrated-light-000000.fits",
      rgbOutputPath: null,
      totalSamples: 8,
      usableSamples: 8,
      maskedSamples: 0,
      nonFiniteSamples: 0,
      minimum: 1,
      maximum: 8,
      mean: 4.5,
      populationStandardDeviation: 2.29,
      samplesWritten: 8,
      substitutedSamples: 0,
      bytesWritten: 5760,
      tilesProcessed: 1,
      tilesReused: 0,
    };
    const { root, actions } = fixture({
      ...demoReviewModel,
      calibration: {
        ...demoReviewModel.calibration,
        lightExecution: {
          ...demoReviewModel.calibration.lightExecution,
          result: {
            manifestSha256: "a".repeat(64),
            masterPlanSha256: "b".repeat(64),
            lightPlanSha256: "c".repeat(64),
            memoryLimitBytes: 1_024,
            peakReservedBytes: 512,
            outputMode: "calibrated_frames",
            products: [],
            calibratedFrames: [calibratedFrame],
          },
        },
      },
    });

    const calibrated = getByRole(root, "button", {
      name: "Calibrated · 1",
    });
    expect(calibrated.hasAttribute("disabled")).toBe(false);
    fireEvent.click(calibrated);
    expect(actions.onSelectLightFrameView).toHaveBeenCalledWith("calibrated");

    const calibratedView = fixture({
      ...demoReviewModel,
      lightFrameView: "calibrated",
      frames: demoReviewModel.frames.slice(0, 1),
      selectedFrameId: demoReviewModel.frames[0]?.id ?? null,
      calibration: {
        ...demoReviewModel.calibration,
        lightExecution: {
          ...demoReviewModel.calibration.lightExecution,
          result: {
            manifestSha256: "a".repeat(64),
            masterPlanSha256: "b".repeat(64),
            lightPlanSha256: "c".repeat(64),
            memoryLimitBytes: 1_024,
            peakReservedBytes: 512,
            outputMode: "calibrated_frames",
            products: [],
            calibratedFrames: [calibratedFrame],
          },
        },
      },
    });
    expect(calibratedView.root.textContent).toContain("Calibrated Lights");
    expect(calibratedView.root.textContent).toContain("CALIBRATED CFA · RGGB");
    expect(
      getByRole(calibratedView.root, "button", {
        name: "Calibrated · 1",
      }).getAttribute("aria-pressed"),
    ).toBe("true");

    const rgbView = fixture({
      ...demoReviewModel,
      lightFrameView: "calibrated",
      frames: [
        {
          ...demoReviewModel.frames[0]!,
          previewContent: { kind: "rgb" },
        },
      ],
      selectedFrameId: demoReviewModel.frames[0]?.id ?? null,
    });
    expect(rgbView.root.textContent).toContain("CALIBRATED RGB · LINEAR");
  });

  it("opens the calibration laboratory and presents separate master roles", () => {
    const ready = { ...demoReviewModel, reviewSessionReady: true };
    const { root, actions, controller } = fixture(ready);

    fireEvent.click(getByRole(root, "button", { name: "Calibration" }));
    expect(actions.onSelectWorkspace).toHaveBeenCalledWith("calibration");
    controller.update({ ...ready, activeWorkspace: "calibration" });

    expect(
      getByRole(root, "heading", { name: "Build exact master frames" }),
    ).not.toBeNull();
    expect(root.textContent).toContain("Master Dark");
    expect(root.textContent).toContain("Master Flat");
    expect(root.textContent).toContain("Matched dark selected");
    expect(root.textContent).toContain("Light calibration matrix");
    expect(root.textContent).toContain("light-uvir-2s-g120-o30");
    expect(root.textContent).toContain("dark-2s-g120-o30");
    expect(root.textContent).toContain("1 Light group ready");
  });

  it("presents explicit detector controls only after native artifacts exist", () => {
    const sourceFrameId = demoReviewModel.frames[0]?.id ?? "f".repeat(64);
    const ready = {
      ...demoReviewModel,
      activeWorkspace: "calibration" as const,
      reviewSessionReady: true,
      selectedFrameId: sourceFrameId,
      calibration: {
        ...demoReviewModel.calibration,
        execution: {
          ...demoReviewModel.calibration.execution,
          result: {
            manifestSha256: "a".repeat(64),
            planSha256: "b".repeat(64),
            memoryLimitBytes: 1_073_741_824,
            peakReservedBytes: 256_000_000,
            products: [
              {
                groupId: "dark-2s-g120-o30",
                kind: "dark" as const,
                outputPath: "/masters/dark.fits",
                totalSamples: 8,
                usableSamples: 8,
                maskedSamples: 0,
                nonFiniteSamples: 0,
                minimum: 1,
                maximum: 8,
                mean: 4,
                populationStandardDeviation: 1,
                samplesWritten: 8,
                substitutedSamples: 0,
                bytesWritten: 5_760,
                normalization: null,
              },
              {
                groupId: "flat-uvir-2s-g120-o30",
                kind: "flat" as const,
                outputPath: "/masters/flat.fits",
                totalSamples: 8,
                usableSamples: 8,
                maskedSamples: 0,
                nonFiniteSamples: 0,
                minimum: 0.9,
                maximum: 1.1,
                mean: 1,
                populationStandardDeviation: 0.01,
                samplesWritten: 8,
                substitutedSamples: 0,
                bytesWritten: 5_760,
                normalization: 12_345,
              },
            ],
          },
        },
        lightExecution: {
          ...demoReviewModel.calibration.lightExecution,
          result: {
            manifestSha256: "a".repeat(64),
            masterPlanSha256: "b".repeat(64),
            lightPlanSha256: "c".repeat(64),
            memoryLimitBytes: 1_073_741_824,
            peakReservedBytes: 256_000_000,
            outputMode: "calibrated_frames" as const,
            products: [],
            calibratedFrames: [
              {
                groupId: "light-uvir-2s-g120-o30",
                sourceIndex: 0,
                sourceFrameId,
                sourceLabel: "light_0001.fits",
                sourceSha256: "d".repeat(64),
                outputPath: "/lights/light-0001.fits",
                rgbOutputPath: null,
                totalSamples: 8,
                usableSamples: 8,
                maskedSamples: 0,
                nonFiniteSamples: 0,
                minimum: 1,
                maximum: 8,
                mean: 4,
                populationStandardDeviation: 1,
                samplesWritten: 8,
                substitutedSamples: 0,
                bytesWritten: 5_760,
                tilesProcessed: 1,
                tilesReused: 0,
              },
            ],
          },
        },
      },
    };
    const { root, actions, controller } = fixture(ready);

    expect(
      getByRole<HTMLButtonElement>(root, "button", {
        name: "Correct selected Light",
      }).disabled,
    ).toBe(false);
    expect(root.textContent).toContain("Advanced · experimental");
    fireEvent.click(getByRole(root, "button", { name: "Mono · 1 px" }));
    expect(actions.onUpdateDefectCorrectionSettings).toHaveBeenCalledWith({
      ...ready.calibration.defectCorrection.settings,
      darkDetection: {
        ...ready.calibration.defectCorrection.settings.darkDetection,
        stride: 1,
      },
      flatDetection: {
        ...ready.calibration.defectCorrection.settings.flatDetection,
        stride: 1,
      },
      correctionStride: 1,
    });
    fireEvent.click(
      getByRole(root, "button", { name: "Correct selected Light" }),
    );
    expect(actions.onExecuteDefectCorrection).toHaveBeenCalledOnce();
    fireEvent.click(
      getByRole(root, "button", {
        name: /Correct all \d+ eligible Lights?/,
      }),
    );
    expect(actions.onExecuteAllDefectCorrections).toHaveBeenCalledOnce();

    controller.update({
      ...ready,
      calibration: {
        ...ready.calibration,
        defectCorrection: {
          ...ready.calibration.defectCorrection,
          state: "running",
          progress: {
            sequence: 3,
            stage: "strict-defect-correction",
            state: "running",
            completedUnits: 2,
            totalUnits: 5,
            code: null,
          },
          batchReport: {
            state: "running",
            planSha256: "a".repeat(64),
            parametersSha256: "e".repeat(64),
            totalItems: 4,
            completedItems: 1,
            requestedSamples: 31,
            correctedSamples: 30,
            insufficientSupportSamples: 1,
            blockedBySourceMaskSamples: 0,
            hotSamples: 3,
            coldSamples: 4,
            conflictingSamples: 1,
            peakReservedBytes: 234_020_736,
          },
          message: "Correction complete · staging both FITS companions · 2/5",
        },
      },
    });
    const correctionProgress = getByRole<HTMLProgressElement>(
      root,
      "progressbar",
      {
        name: "Detector correction progress",
      },
    );
    expect(correctionProgress.value).toBe(7);
    expect(correctionProgress.max).toBe(20);

    controller.update({
      ...ready,
      calibration: {
        ...ready.calibration,
        defectCorrection: {
          ...ready.calibration.defectCorrection,
          state: "completed",
          sourceFrameId,
          result: {
            correctedOutputPath: "/corrected/light.fits",
            mapOutputPath: "/corrected/light-map.fits",
            parametersSha256: "e".repeat(64),
            batchPlanSha256: "a".repeat(64),
            batchItemIndex: 0,
            batchCompletedItems: 1,
            batchTotalItems: 2,
            batchComplete: false,
            reservedBytes: 234_020_736,
            requestedSamples: 31,
            correctedSamples: 30,
            insufficientSupportSamples: 1,
            blockedBySourceMaskSamples: 0,
            correctedSamplesWritten: 8,
            correctedSubstitutedSamples: 0,
            correctedBytesWritten: 5_760,
            mapSamplesWritten: 8,
            mapSubstitutedSamples: 0,
            mapBytesWritten: 5_760,
            darkDetection: {
              examinedSamples: 8,
              insufficientSupportSamples: 0,
              unavailableCentreSamples: 0,
              hotSamples: 2,
              coldSamples: 1,
            },
            flatDetection: {
              examinedSamples: 8,
              insufficientSupportSamples: 0,
              unavailableCentreSamples: 0,
              hotSamples: 1,
              coldSamples: 3,
            },
            mapSummary: {
              defectiveSamples: 6,
              hotSamples: 3,
              coldSamples: 4,
              conflictingSamples: 1,
            },
          },
          batchReport: {
            state: "completed",
            planSha256: "a".repeat(64),
            parametersSha256: "e".repeat(64),
            totalItems: 2,
            completedItems: 2,
            requestedSamples: 62,
            correctedSamples: 60,
            insufficientSupportSamples: 2,
            blockedBySourceMaskSamples: 0,
            hotSamples: 6,
            coldSamples: 8,
            conflictingSamples: 2,
            peakReservedBytes: 234_020_736,
          },
          previewState: "ready",
          previewView: "after",
          preview: {
            frameId: `defect-after-${sourceFrameId}`,
            url: "blob:after",
          },
          previewMessage: "Corrected output · shared native stretch",
          message: "30/31 detector samples corrected",
        },
      },
    });
    const before = getByRole<HTMLButtonElement>(root, "tab", {
      name: "Before",
    });
    expect(before.disabled).toBe(false);
    expect(before.tabIndex).toBe(-1);
    fireEvent.click(before);
    expect(actions.onSelectDefectPreview).toHaveBeenCalledWith("before");
    const map = getByRole<HTMLButtonElement>(root, "tab", { name: "Map" });
    expect(map.disabled).toBe(false);
    fireEvent.click(map);
    expect(actions.onSelectDefectPreview).toHaveBeenCalledWith("map");
    expect(
      root.querySelector<HTMLImageElement>("[data-defect-preview-image]")?.src,
    ).toContain("blob:after");
    expect(
      root.querySelector("[data-defect-batch-report]")?.textContent,
    ).toContain("2/2 pairs");
    expect(
      root.querySelector<HTMLElement>("[data-defect-batch-report]")?.title,
    ).toContain("parameters SHA-256");
    const exportReport = getByRole<HTMLButtonElement>(root, "button", {
      name: "Export verified report",
    });
    expect(exportReport.disabled).toBe(false);
    fireEvent.click(exportReport);
    expect(actions.onExportDefectBatchReport).toHaveBeenCalledOnce();
  });

  it("presents an accepted registration plan with exact common crop evidence", () => {
    const frames = demoReviewModel.frames.slice(0, 2).map((frame, index) => ({
      id: frame.id,
      label: frame.label,
      sourcePath: `/session/light-${index}.fits`,
    }));
    const registration = {
      geometryModel: "affine" as const,
      state: "accepted" as const,
      frames,
      referenceFrameId: frames[0]?.id ?? null,
      sourceFrameId: frames[1]?.id ?? null,
      message: "Geometry accepted · exact full-resolution plan is available",
      solutions: [],
      planState: "idle" as const,
      plan: null,
      execution: demoReviewModel.registration.execution,
      stack: demoReviewModel.registration.stack,
      drizzle: demoReviewModel.registration.drizzle,
      resultReview: demoReviewModel.registration.resultReview,
      diagnostic: {
        schemaVersion: 3,
        profileId: "registration-v1",
        diagnosticOnly: true,
        source: {
          contentSha256: "a".repeat(64),
          sourceWidth: 4144,
          sourceHeight: 2822,
          detectedStars: 842,
          medianFwhmSourcePixels: 3.2,
          medianEccentricity: 0.4,
          registrationFeatures: 320,
        },
        reference: {
          contentSha256: "b".repeat(64),
          sourceWidth: 4144,
          sourceHeight: 2822,
          detectedStars: 861,
          medianFwhmSourcePixels: 3.1,
          medianEccentricity: 0.39,
          registrationFeatures: 330,
        },
        matching: {
          retainedHypotheses: 120,
          geometricCandidates: 8,
          truncated: false,
        },
        consensus: {
          scale: 0.9999898,
          rotationRadians: 0.00046228,
          reflected: false,
          inlierHypotheses: 91,
          inlierFeaturePairs: 302,
          rmsResidualDetectionPixels: 0.12,
          maximumResidualDetectionPixels: 0.4,
        },
        projectiveAdequacy: {
          selectionApplied: false as const,
          matchCount: 302,
          transformCoefficientsDetectionPixels: [
            [0.9999898, -0.00046228, -0.46],
            [0.00046228, 0.9999898, 0.095],
            [0.000001, -0.000002, 1],
          ] as const,
          similarityRmsResidualDetectionPixels: 0.12,
          similarityMaximumResidualDetectionPixels: 0.4,
          projectiveRmsResidualDetectionPixels: 0.08,
          projectiveMaximumResidualDetectionPixels: 0.29,
          rmsImprovementDetectionPixels: 0.04,
          relativeRmsImprovement: 1 / 3,
          maximumModelSeparationDetectionPixels: 0.21,
          rankSeparationRatio: 0.015,
          crossValidation: {
            foldCount: 5,
            projectiveBetterFolds: 1,
            similarityRmsResidualDetectionPixels: 0.13,
            similarityMaximumResidualDetectionPixels: 0.43,
            projectiveRmsResidualDetectionPixels: 0.14,
            projectiveMaximumResidualDetectionPixels: 0.48,
            rmsImprovementDetectionPixels: -0.01,
            relativeRmsImprovement: -0.077,
            minimumRankSeparationRatio: 0.014,
          },
          recommendation: {
            recommended: false,
            transformCoefficientsSourcePixels: [
              [0.9999898, -0.00046228, -0.92],
              [0.00046228, 0.9999898, 0.19],
              [0.0000005, -0.000001, 1],
            ] as const,
            minimumMatches: 20,
            minimumValidationFolds: 5,
            minimumProjectiveBetterFolds: 5,
            minimumRmsImprovementDetectionPixels: 0.05,
            minimumRelativeRmsImprovement: 0.1,
            minimumRankSeparationRatio: 0.01,
            maximumProjectiveRmsDetectionPixels: 1,
            minimumModelSeparationDetectionPixels: 0.25,
            supportSufficient: true,
            validationFoldsSufficient: true,
            foldWinsSufficient: false,
            absoluteGainSufficient: false,
            relativeGainSufficient: false,
            rankSeparationSufficient: true,
            projectiveRmsAcceptable: true,
            worstResidualNotIncreased: false,
            modelSeparationSufficient: false,
          },
        },
        confidence: {
          accepted: true,
          inlierRatio: 0.91,
          winnerSupportMargin: 0.72,
          sourceAxisSpanFraction: [0.8, 0.7] as const,
          referenceAxisSpanFraction: [0.81, 0.71] as const,
          rejections: [],
        },
        acceptedPlan: {
          footprintAlgorithmId: "lanczos3-common-footprint-v1",
          transformCoefficientsSourcePixels: [
            0.9999898, -0.00046228, 0.00046228, 0.9999898, -0.92, 0.19,
          ] as const,
          referenceWidth: 4144,
          referenceHeight: 2822,
          coveredPixels: 11_659_258,
          autocrop: { x: 2, y: 5, width: 4137, height: 2815 },
        },
      },
    };
    const ready = {
      ...demoReviewModel,
      activeWorkspace: "registration" as const,
      reviewSessionReady: true,
      registration: {
        ...registration,
        solutions: [
          {
            sourceFrameId: frames[1]?.id ?? "",
            diagnostic: registration.diagnostic,
          },
        ],
        planState: "ready" as const,
        plan: {
          schemaVersion: 1,
          planSha256: "f".repeat(64),
          referenceFrameId: frames[0]?.id ?? "",
          referenceWidth: 4144,
          referenceHeight: 2822,
          coveredPixels: 11_650_000,
          autocrop: { x: 3, y: 6, width: 4135, height: 2813 },
          frames: frames.map((frame, index) => ({
            frameId: frame.id,
            sourceWidth: 4144,
            sourceHeight: 2822,
            transformCoefficientsSourcePixels: [1, 0, 0, 1, 0, 0] as const,
            reference: index === 0,
          })),
        },
      },
    };
    const { root, actions, controller } = fixture(ready);

    expect(
      getByRole(root, "heading", {
        name: "Solve geometry before moving pixels",
      }),
    ).not.toBeNull();
    expect(root.textContent).toContain("4135 × 2813 px · origin 3, 6");
    expect(root.textContent).toContain("canonical all-frame crop");
    expect(root.textContent).toContain("302");
    expect(root.textContent).toContain("0.240");
    expect(root.textContent).toContain("lanczos3-common-footprint-v1");
    expect(root.textContent).toContain(
      "Diagnostic projective · detection pixels · not selected",
    );
    expect(root.textContent).toContain("gain 0.0400 px");
    expect(root.textContent).toContain("held-out gain -0.0100 px");
    expect(root.textContent).toContain("fold wins 1/5");
    expect(root.textContent).toContain("Advisory: not recommended");
    expect(root.textContent).toContain("absolute gain");
    expect(root.textContent).toContain("field separation");
    expect(root.textContent).toContain("2 frames sealed");
    expect(root.textContent).toContain(`SHA-256 ${"f".repeat(64)}`);
    expect(root.textContent).toContain("Reference");
    expect(root.textContent).toContain("Accepted");
    expect(
      getByRole(root, "button", { name: "Register all frames" }).hasAttribute(
        "disabled",
      ),
    ).toBe(true);
    controller.update({
      ...ready,
      calibration: {
        ...ready.calibration,
        lightExecution: {
          ...ready.calibration.lightExecution,
          state: "completed",
          result: {
            manifestSha256: "a".repeat(64),
            masterPlanSha256: "b".repeat(64),
            lightPlanSha256: "c".repeat(64),
            memoryLimitBytes: 1_073_741_824,
            peakReservedBytes: 1_024,
            outputMode: "calibrated_frames",
            products: [],
            calibratedFrames: frames.map((frame, index) => ({
              groupId: "light-uvir",
              sourceIndex: index,
              sourceFrameId: frame.id,
              sourceLabel: frame.label,
              sourceSha256: String(index + 1).repeat(64),
              outputPath: `/calibrated/${index}.fits`,
              rgbOutputPath: `/rgb/${index}.fits`,
              totalSamples: 1,
              usableSamples: 1,
              maskedSamples: 0,
              nonFiniteSamples: 0,
              minimum: 1,
              maximum: 1,
              mean: 1,
              populationStandardDeviation: 0,
              samplesWritten: 1,
              substitutedSamples: 0,
              bytesWritten: 2_880,
              tilesProcessed: 1,
              tilesReused: 0,
            })),
          },
        },
      },
    });
    const execute = getByRole(root, "button", {
      name: "Register all frames",
    });
    expect(execute.hasAttribute("disabled")).toBe(false);
    fireEvent.click(execute);
    expect(actions.onExecuteRegistration).toHaveBeenCalledOnce();
    const drizzle = getByRole(root, "button", { name: "Build Drizzle set" });
    expect(drizzle.hasAttribute("disabled")).toBe(false);
    fireEvent.click(drizzle);
    expect(actions.onExecuteDrizzle).toHaveBeenCalledOnce();
    fireEvent.click(getByRole(root, "button", { name: "3×" }));
    expect(actions.onUpdateDrizzleSettings).toHaveBeenLastCalledWith(
      expect.objectContaining({ scale: 3 }),
    );
    const dropShrink = root.querySelector<HTMLInputElement>(
      "[data-drizzle-drop-shrink]",
    );
    expect(dropShrink).not.toBeNull();
    fireEvent.input(dropShrink!, { target: { value: "0.65" } });
    expect(actions.onUpdateDrizzleSettings).toHaveBeenLastCalledWith(
      expect.objectContaining({ dropShrink: 0.65 }),
    );
    fireEvent.click(getByRole(root, "button", { name: "Balanced PSF" }));
    expect(actions.onSelectDrizzleWeighting).toHaveBeenCalledWith(
      "balanced_psf",
    );
    fireEvent.click(getByRole(root, "button", { name: "Analyze geometry" }));
    expect(actions.onAnalyzeRegistration).toHaveBeenCalledOnce();
  });

  it("keeps registration disabled until two distinct native Light paths exist", () => {
    const { root, actions } = fixture({
      ...demoReviewModel,
      activeWorkspace: "registration",
    });

    expect(
      getByRole(root, "button", { name: "Analyze geometry" }).hasAttribute(
        "disabled",
      ),
    ).toBe(true);
    const source = getByRole<HTMLSelectElement>(root, "combobox", {
      name: "Source frame",
    });
    fireEvent.change(source, {
      target: { value: demoReviewModel.registration.frames[0]?.id },
    });
    expect(actions.onSelectRegistrationSource).toHaveBeenCalledWith(
      demoReviewModel.registration.frames[0]?.id,
    );
  });

  it("presents published registered pixels as an accessible Blink sequence", () => {
    const frameA = demoReviewModel.registration.frames[0]!;
    const frameB = demoReviewModel.registration.frames[1]!;
    const planSha256 = "9".repeat(64);
    const ready = {
      ...demoReviewModel,
      activeWorkspace: "registration" as const,
      registration: {
        ...demoReviewModel.registration,
        planState: "ready" as const,
        plan: {
          schemaVersion: 1,
          planSha256,
          referenceFrameId: frameA.id,
          referenceWidth: 4144,
          referenceHeight: 2822,
          coveredPixels: 11_000_000,
          autocrop: { x: 8, y: 6, width: 4128, height: 2810 },
          frames: [
            {
              frameId: frameA.id,
              sourceWidth: 4144,
              sourceHeight: 2822,
              transformCoefficientsSourcePixels: [1, 0, 0, 1, 0, 0] as const,
              reference: true,
            },
            {
              frameId: frameB.id,
              sourceWidth: 4144,
              sourceHeight: 2822,
              transformCoefficientsSourcePixels: [1, 0, 0, 1, 1, 1] as const,
              reference: false,
            },
          ],
        },
        execution: {
          ...demoReviewModel.registration.execution,
          state: "completed" as const,
          outputDirectory: "/registered",
          result: {
            planSha256,
            memoryLimitBytes: 1_073_741_824,
            peakReservedBytes: 4_096,
            frames: [],
          },
        },
        stack: {
          ...demoReviewModel.registration.stack,
          state: "completed" as const,
          outputPath: "/results/integrated.fits",
          progress: null,
          result: {
            planSha256,
            outputPath: "/results/integrated.fits",
            width: 4128,
            height: 2810,
            planes: 3,
            samplesWritten: 34_798_080,
            substitutedSamples: 0,
            bytesWritten: 278_400_000,
            memoryLimitBytes: 1_073_741_824,
            peakReservedBytes: 18_000_000,
            estimator: "registered-crop-mean-v1",
            lowRejectionMapPath: "/results/integrated-rejection-low.fits",
            highRejectionMapPath: "/results/integrated-rejection-high.fits",
            rejectionMapSamplesWritten: 34_798_080,
            reportPath: "/results/integrated-integration-report.json",
            reportSha256: "d".repeat(64),
          },
          previewState: "ready" as const,
          preview: {
            frameId: `${planSha256}:registered-stack:science`,
            url: "blob:registered-stack",
          },
          message: "4128 × 2810 × 3 integrated atomically",
        },
        resultReview: {
          frames: [
            {
              id: frameA.id,
              label: frameA.label,
              outputPath: "/registered/a.fits",
              previewContent: { kind: "rgb" as const },
            },
            {
              id: frameB.id,
              label: frameB.label,
              outputPath: "/registered/b.fits",
              previewContent: { kind: "rgb" as const },
            },
          ],
          selectedFrameId: frameA.id,
          state: "ready" as const,
          preview: {
            frameId: `${planSha256}:registered:${frameA.id}`,
            url: "blob:registered-a",
          },
          playing: false,
          message: "Published registered pixels · shared stretch locked",
          sharedStretchLabel: "Registered stretch · locked",
        },
      },
    };
    const { root, actions, controller } = fixture(ready);

    expect(
      getByRole(root, "heading", { name: "Registered Blink" }),
    ).not.toBeNull();
    expect(
      getByRole<HTMLImageElement>(root, "img", {
        name: `Registered preview of ${frameA.label}`,
      }).src,
    ).toContain("blob:registered-a");
    expect(root.textContent).toContain("Registered stretch · locked");
    expect(
      getByRole<HTMLImageElement>(root, "img", {
        name: "Integrated registered common-crop preview",
      }).src,
    ).toContain("blob:registered-stack");
    expect(root.textContent).toContain("Integration report");
    expect(root.textContent).toContain("d".repeat(64));
    fireEvent.click(getByRole(root, "button", { name: "Open prior report" }));
    expect(actions.onOpenRegisteredStackReport).toHaveBeenCalledOnce();
    fireEvent.click(getByRole(root, "button", { name: "Verify report" }));
    expect(actions.onInspectRegisteredStackReport).toHaveBeenCalledOnce();
    controller.update({
      ...ready,
      registration: {
        ...ready.registration,
        stack: {
          ...ready.registration.stack,
          reportInspectionState: "ready",
          reportInspection: {
            schemaVersion: 2,
            reportSha256: "d".repeat(64),
            planSha256,
            geometryModel: "affine",
            manifestSha256: "e".repeat(64),
            estimator: "strict_mean",
            width: 4_128,
            height: 2_810,
            planes: 3,
            sourceCount: 2,
            productCount: 3,
            weighted: false,
            allProductsVerified: true,
            sources: [
              {
                frameId: frameA.id,
                fileName: "registered-a.fits",
                byteLength: 278_992_800,
                sha256: "9".repeat(64),
              },
            ],
            products: [
              {
                role: "science",
                fileName: "integrated.fits",
                path: "/stack/integrated.fits",
                bytesWritten: 278_992_800,
                status: "verified",
              },
            ],
          },
        },
      },
    });
    expect(root.textContent).toContain(
      "2 sources · 3 products · strict mean · affine geometry · schema 2",
    );
    expect(root.textContent).toContain("all FITS verified");
    expect(root.textContent).toContain("Science · integrated.fits");
    expect(root.textContent).toContain("266.1 MiB · FITS verified");
    expect(root.textContent).toContain("registered-a.fits · 266.1 MiB");
    expect(root.textContent).toContain("sha256 999999999999…");
    expect(getByRole(root, "button", { name: "Verify again" })).not.toBeNull();
    fireEvent.click(
      getByRole(root, "button", { name: "Next registered frame" }),
    );
    expect(actions.onStepRegisteredFrame).toHaveBeenCalledWith("forward");
    fireEvent.click(
      getByRole(root, "button", { name: "Start registered Blink" }),
    );
    expect(actions.onSetRegisteredPlaying).toHaveBeenCalledWith(true);
    fireEvent.change(
      getByRole<HTMLSelectElement>(root, "combobox", {
        name: "Registered frame",
      }),
      { target: { value: frameB.id } },
    );
    expect(actions.onSelectRegisteredFrame).toHaveBeenCalledWith(frameB.id);
    fireEvent.click(getByRole(root, "button", { name: "Integrate crop" }));
    expect(actions.onExecuteRegisteredStack).toHaveBeenCalledOnce();
    vi.mocked(actions.onExecuteRegisteredStack).mockClear();
    controller.update({
      ...ready,
      registration: {
        ...ready.registration,
        geometryModel: "projective",
        plan: {
          ...ready.registration.plan,
          schemaVersion: 2,
          geometryModel: "projective",
          frames: ready.registration.plan.frames.map((frame) => ({
            ...frame,
            transformCoefficientsSourcePixels: null,
            projectiveTransformCoefficientsSourcePixels: [
              [1, 0, 0],
              [0, 1, 0],
              [0.00001, -0.00001, 1],
            ] as const,
          })),
        },
      },
    });
    const projectiveIntegration = getByRole<HTMLButtonElement>(root, "button", {
      name: "Integrate crop",
    });
    expect(projectiveIntegration.disabled).toBe(false);
    fireEvent.click(projectiveIntegration);
    expect(actions.onExecuteRegisteredStack).toHaveBeenCalledOnce();
    fireEvent.click(getByRole(root, "tab", { name: "Low reject" }));
    expect(actions.onSelectRegisteredStackProduct).toHaveBeenCalledWith(
      "rejection_low",
    );

    controller.update({
      ...ready,
      registration: {
        ...ready.registration,
        stack: {
          ...ready.registration.stack,
          selectedProduct: "rejection_low",
          sciencePreview: {
            frameId: `${planSha256}:registered-stack:science`,
            url: "blob:registered-stack",
          },
          preview: {
            frameId: `${planSha256}:registered-stack:rejection_low`,
            url: "blob:registered-stack-low",
          },
          histogramState: "ready",
          histogram: {
            algorithmId: "rejection-count-histogram-v1",
            totalSamples: 1_000,
            zeroSamples: 925,
            rejectedSamples: 75,
            maximumRejectedCount: 3,
            bins: [
              { rejectedCount: 0, samples: 925 },
              { rejectedCount: 1, samples: 60 },
              { rejectedCount: 2, samples: 12 },
              { rejectedCount: 3, samples: 3 },
            ],
          },
        },
      },
    });
    expect(
      getByRole<HTMLImageElement>(root, "img", {
        name: "Integrated science preview beneath the rejection overlay",
      }).src,
    ).toContain("blob:registered-stack");
    expect(
      getByRole<HTMLImageElement>(root, "img", {
        name: "Low-tail rejection map preview",
      }).style.opacity,
    ).toBe("0.65");
    const opacity = getByRole<HTMLInputElement>(root, "slider", {
      name: "Rejection map opacity over science",
    });
    fireEvent.input(opacity, { target: { value: "42" } });
    expect(actions.onSetRegisteredStackOverlayOpacity).toHaveBeenCalledWith(
      0.42,
    );
    expect(root.textContent).toContain(
      "75 affected samples · 7.500% · maximum 3",
    );
    expect(root.textContent).toContain("60");
    fireEvent.change(getByRole(root, "spinbutton", { name: "X" }), {
      target: { value: "12" },
    });
    fireEvent.change(getByRole(root, "spinbutton", { name: "Y" }), {
      target: { value: "34" },
    });
    fireEvent.click(getByRole(root, "button", { name: "Inspect pixel" }));
    expect(actions.onInspectRegisteredStackPixel).toHaveBeenCalledWith(12, 34);

    controller.update({
      ...ready,
      registration: {
        ...ready.registration,
        stack: {
          ...ready.registration.stack,
          pixelInspectionState: "ready",
          pixelInspection: {
            x: 12,
            y: 34,
            scienceValues: [1024.5, 998.25, 1101.75],
            lowRejectionCounts: [0, 1, 0],
            highRejectionCounts: [2, 0, 1],
          },
        },
      },
    });
    expect(root.textContent).toContain("x 12 · y 34");
    expect(root.textContent).toContain("low 0 / 1 / 0");
  });

  it("keeps robust integration controls explicit and model-driven", () => {
    const advanced: ReviewViewModel = {
      ...demoReviewModel,
      activeWorkspace: "registration",
      registration: {
        ...demoReviewModel.registration,
        stack: {
          ...demoReviewModel.registration.stack,
          settings: {
            estimator: "percentile_clipped",
            weightReferenceFrameId: null,
            lowFraction: 0.12,
            highFraction: 0.08,
            lowSigma: 4,
            highSigma: 3,
            maximumIterations: 8,
            minimumRetainedSamples: 5,
            generateRejectionMaps: false,
          },
        },
      },
    };
    const { root, actions, controller } = fixture(advanced);
    fireEvent.click(getByText(root, "Advanced integration"));
    const estimator = getByRole<HTMLSelectElement>(root, "combobox", {
      name: "Estimator",
    });
    const maps = getByRole<HTMLInputElement>(root, "checkbox", {
      name: /Publish rejection evidence/,
    });

    expect(estimator.value).toBe("percentile_clipped");
    expect(estimator.classList.contains("instrument-select")).toBe(true);
    expect(maps.disabled).toBe(false);
    fireEvent.click(maps);

    expect(actions.onUpdateRegisteredStackSettings).toHaveBeenCalledWith({
      estimator: "percentile_clipped",
      weightReferenceFrameId: null,
      lowFraction: 0.12,
      highFraction: 0.08,
      lowSigma: 4,
      highSigma: 3,
      maximumIterations: 8,
      minimumRetainedSamples: 5,
      generateRejectionMaps: true,
    });

    fireEvent.click(
      getByRole(root, "button", { name: "Increase low-tail fraction" }),
    );
    expect(actions.onUpdateRegisteredStackSettings).toHaveBeenLastCalledWith({
      estimator: "percentile_clipped",
      weightReferenceFrameId: null,
      lowFraction: 0.13,
      highFraction: 0.08,
      lowSigma: 4,
      highSigma: 3,
      maximumIterations: 8,
      minimumRetainedSamples: 5,
      generateRejectionMaps: true,
    });

    fireEvent.change(estimator, { target: { value: "median" } });
    expect(actions.onUpdateRegisteredStackSettings).toHaveBeenLastCalledWith({
      estimator: "median",
      weightReferenceFrameId: null,
      lowFraction: 0.13,
      highFraction: 0.08,
      lowSigma: 4,
      highSigma: 3,
      maximumIterations: 8,
      minimumRetainedSamples: 5,
      generateRejectionMaps: false,
    });

    controller.update({
      ...advanced,
      registration: {
        ...advanced.registration,
        stack: {
          ...advanced.registration.stack,
          settings: {
            ...advanced.registration.stack.settings,
            estimator: "median",
          },
        },
      },
    });
    expect(
      root
        .querySelector<HTMLInputElement>("[data-registered-stack-low-fraction]")
        ?.closest<HTMLElement>(".control-field")?.hidden,
    ).toBe(true);
    expect(root.textContent).toContain("EXACT F64 MEDIAN");

    controller.update({
      ...advanced,
      registration: {
        ...advanced.registration,
        stack: {
          ...advanced.registration.stack,
          settings: {
            ...advanced.registration.stack.settings,
            estimator: "sigma_clipped",
          },
        },
      },
    });
    expect(
      getByRole<HTMLInputElement>(root, "spinbutton", { name: "Low sigma" })
        .disabled,
    ).toBe(false);
    const hiddenLowFraction = root.querySelector<HTMLInputElement>(
      "[data-registered-stack-low-fraction]",
    );
    expect(hiddenLowFraction?.disabled).toBe(true);
    expect(
      hiddenLowFraction?.closest<HTMLElement>(".control-field")?.hidden,
    ).toBe(true);
    expect(root.textContent).toContain("ITERATIVE SIGMA F64");
    fireEvent.click(getByRole(root, "button", { name: "Increase low sigma" }));
    expect(actions.onUpdateRegisteredStackSettings).toHaveBeenLastCalledWith({
      estimator: "sigma_clipped",
      weightReferenceFrameId: null,
      lowFraction: 0.12,
      highFraction: 0.08,
      lowSigma: 4.1,
      highSigma: 3,
      maximumIterations: 8,
      minimumRetainedSamples: 5,
      generateRejectionMaps: false,
    });

    controller.update({
      ...advanced,
      registration: {
        ...advanced.registration,
        stack: {
          ...advanced.registration.stack,
          settings: {
            ...advanced.registration.stack.settings,
            estimator: "winsorized_sigma_clipped",
          },
        },
      },
    });
    expect(root.textContent).toContain("WINSORIZED SIGMA F64");
    expect(
      getByRole<HTMLInputElement>(root, "spinbutton", { name: "Low sigma" })
        .disabled,
    ).toBe(false);
    fireEvent.click(getByRole(root, "button", { name: "Increase high sigma" }));
    expect(actions.onUpdateRegisteredStackSettings).toHaveBeenLastCalledWith({
      estimator: "winsorized_sigma_clipped",
      weightReferenceFrameId: null,
      lowFraction: 0.12,
      highFraction: 0.08,
      lowSigma: 4,
      highSigma: 3.1,
      maximumIterations: 8,
      minimumRetainedSamples: 5,
      generateRejectionMaps: false,
    });
  });

  it("presents a natively verified report without claiming an active stack", () => {
    const external: ReviewViewModel = {
      ...demoReviewModel,
      activeWorkspace: "registration",
      registration: {
        ...demoReviewModel.registration,
        stack: {
          ...demoReviewModel.registration.stack,
          reportInspectionPath: "/archive/m31-integration-report.json",
          reportInspectionState: "ready",
          reportInspection: {
            schemaVersion: 1,
            reportSha256: "f".repeat(64),
            planSha256: "a".repeat(64),
            manifestSha256: "b".repeat(64),
            estimator: "weighted_mean",
            width: 4_144,
            height: 2_822,
            planes: 3,
            sourceCount: 18,
            productCount: 1,
            weighted: true,
            allProductsVerified: false,
            sources: [
              {
                frameId: "c".repeat(64),
                fileName: "m31-registered-001.fits",
                byteLength: 278_992_800,
                sha256: "d".repeat(64),
              },
              {
                frameId: "e".repeat(64),
                fileName: "m31-registered-002.fits",
                byteLength: 278_992_800,
                sha256: "1".repeat(64),
              },
            ],
            products: [
              {
                role: "science",
                fileName: "m31.fits",
                path: "/archive/m31.fits",
                bytesWritten: 278_992_800,
                status: "missing",
              },
            ],
          },
        },
      },
    };
    const { root, actions, controller } = fixture(external);

    expect(root.textContent).toContain("/archive/m31-integration-report.json");
    expect(root.textContent).toContain(
      "18 sources · 1 product · balanced PSF weight · legacy geometry unspecified · schema 1 · product evidence incomplete",
    );
    expect(root.textContent).toContain("Science · m31.fits");
    expect(root.textContent).toContain("FITS missing");
    expect(root.textContent).toContain("m31-registered-001.fits · 266.1 MiB");
    fireEvent.click(getByText(root, "Source evidence"));
    fireEvent.click(
      getByRole(root, "button", { name: "Verify source folder" }),
    );
    expect(actions.onVerifyRegisteredStackSources).toHaveBeenCalledOnce();
    controller.update({
      ...external,
      registration: {
        ...external.registration,
        stack: {
          ...external.registration.stack,
          sourceVerificationState: "loading",
          sourceVerificationProgress: {
            sequence: 4,
            state: "running",
            completedSources: 4,
            totalSources: 18,
            currentFileName: "m31-registered-004.fits",
            completedBytes: 25 * 1_024 * 1_024,
            totalBytes: 100 * 1_024 * 1_024,
            currentFileBytes: 5 * 1_024 * 1_024,
            currentFileTotalBytes: 20 * 1_024 * 1_024,
          },
        },
      },
    });
    expect(root.textContent).toContain(
      "Hashing archived sources · 4/18 · 25.0 MiB/100.0 MiB · m31-registered-004.fits 25%",
    );
    const sourceProgress = getByRole<HTMLProgressElement>(root, "progressbar", {
      name: "Archived source verification progress",
    });
    expect(sourceProgress.value).toBe(25 * 1_024 * 1_024);
    expect(sourceProgress.max).toBe(100 * 1_024 * 1_024);
    fireEvent.click(getByRole(root, "button", { name: "Cancel hashing" }));
    expect(
      actions.onCancelRegisteredStackSourceVerification,
    ).toHaveBeenCalledOnce();
    expect(root.textContent).toContain("f".repeat(64));
    expect(root.textContent).toContain(
      "Published registered artifacts required",
    );
    fireEvent.click(
      getByRole(root, "button", { name: "Close archived report" }),
    );
    expect(actions.onReturnToActiveStack).toHaveBeenCalledOnce();
    fireEvent.click(getByRole(root, "button", { name: "Verify again" }));
    expect(actions.onInspectRegisteredStackReport).toHaveBeenCalledOnce();
    expect(
      getByRole<HTMLButtonElement>(root, "tab", { name: "Science" }).disabled,
    ).toBe(true);
    controller.update({
      ...external,
      registration: {
        ...external.registration,
        stack: {
          ...external.registration.stack,
          selectedProduct: "science",
          previewState: "ready",
          preview: {
            frameId: `${"f".repeat(64)}:reported-stack:science`,
            url: "blob:reopened-stack",
          },
          sciencePreview: {
            frameId: `${"f".repeat(64)}:reported-stack:science`,
            url: "blob:reopened-stack",
          },
          sourceVerificationState: "ready",
          sourceVerification: {
            reportSha256: "f".repeat(64),
            sourceDirectory: "/archive/registered",
            allSourcesVerified: false,
            sources: [
              {
                frameId: "c".repeat(64),
                fileName: "m31-registered-001.fits",
                path: "/archive/registered/m31-registered-001.fits",
                byteLength: 278_992_800,
                status: "verified",
              },
              {
                frameId: "e".repeat(64),
                fileName: "m31-registered-002.fits",
                path: "/archive/registered/m31-registered-002.fits",
                byteLength: 278_992_800,
                status: "fingerprint_mismatch",
              },
            ],
          },
          reportInspection: {
            ...external.registration.stack.reportInspection!,
            allProductsVerified: true,
            products: [
              {
                ...external.registration.stack.reportInspection!.products[0]!,
                status: "verified",
              },
            ],
          },
        },
      },
    });
    expect(
      getByRole<HTMLImageElement>(root, "img", {
        name: "Integrated registered common-crop preview",
      }).src,
    ).toContain("blob:reopened-stack");
    expect(root.textContent).toContain(
      "Source evidence mismatch · /archive/registered",
    );
    expect(root.textContent).toContain(
      "2 sources · 1 verified · 1 issue · 0 pending",
    );
    expect(root.textContent).toContain("SHA-256 verified");
    expect(root.textContent).toContain("SHA-256 mismatch");
    fireEvent.click(getByRole(root, "button", { name: "Issues" }));
    expect(queryByText(root, /m31-registered-001\.fits/)).toBeNull();
    expect(getByText(root, /m31-registered-002\.fits/)).toBeTruthy();
    fireEvent.click(getByRole(root, "button", { name: "Verified" }));
    expect(getByText(root, /m31-registered-001\.fits/)).toBeTruthy();
    expect(queryByText(root, /m31-registered-002\.fits/)).toBeNull();
    fireEvent.click(getByRole(root, "button", { name: "Pending" }));
    expect(root.textContent).toContain("No sources match the current filters.");
    fireEvent.click(getByRole(root, "button", { name: "All" }));
    const sourceSearch = getByRole<HTMLInputElement>(root, "textbox", {
      name: "Search source filenames",
    });
    fireEvent.input(sourceSearch, { target: { value: "002.FITS" } });
    expect(queryByText(root, /m31-registered-001\.fits/)).toBeNull();
    expect(getByText(root, /m31-registered-002\.fits/)).toBeTruthy();
    expect(root.textContent).toContain("1 shown");
    fireEvent.click(getByRole(root, "button", { name: "Clear source search" }));
    expect(getByText(root, /m31-registered-001\.fits/)).toBeTruthy();
    expect(getByText(root, /m31-registered-002\.fits/)).toBeTruthy();
    expect(document.activeElement).toBe(sourceSearch);
    const science = getByRole<HTMLButtonElement>(root, "tab", {
      name: "Science",
    });
    expect(science.disabled).toBe(false);
    fireEvent.click(science);
    expect(actions.onSelectRegisteredStackProduct).toHaveBeenCalledWith(
      "science",
    );
  });

  it("bounds large source evidence sets without limiting search", () => {
    const sources = Array.from({ length: 251 }, (_, index) => ({
      frameId: index.toString(16).padStart(64, "0"),
      fileName: `source-${index.toString().padStart(4, "0")}.fits`,
      byteLength: 23_397_120,
      sha256: "a".repeat(64),
    }));
    const model: ReviewViewModel = {
      ...demoReviewModel,
      activeWorkspace: "registration",
      registration: {
        ...demoReviewModel.registration,
        stack: {
          ...demoReviewModel.registration.stack,
          reportInspectionPath: "/archive/large-integration-report.json",
          reportInspectionState: "ready",
          reportInspection: {
            schemaVersion: 1,
            reportSha256: "f".repeat(64),
            planSha256: "b".repeat(64),
            manifestSha256: "c".repeat(64),
            estimator: "strict_mean",
            width: 4_144,
            height: 2_822,
            planes: 1,
            sourceCount: sources.length,
            productCount: 0,
            weighted: false,
            allProductsVerified: true,
            sources,
            products: [],
          },
        },
      },
    };
    const { root } = fixture(model);

    fireEvent.click(getByText(root, "Source evidence"));
    const evidence = getByRole(root, "list", {
      name: "Sealed source evidence",
    });
    expect(within(evidence).getAllByRole("listitem")).toHaveLength(250);
    expect(root.textContent).toContain("250 shown · 251 matching");
    fireEvent.click(getByRole(root, "button", { name: "Show next 1" }));
    expect(within(evidence).getAllByRole("listitem")).toHaveLength(251);
    expect(queryByText(root, /251 matching/)).toBeNull();

    const search = getByRole<HTMLInputElement>(root, "textbox", {
      name: "Search source filenames",
    });
    fireEvent.input(search, { target: { value: "source-0250" } });
    expect(within(evidence).getAllByRole("listitem")).toHaveLength(1);
    expect(root.textContent).toContain("source-0250.fits");
    expect(queryByText(root, /source-0000\.fits/)).toBeNull();
  });

  it("preflights every calibrated Light before enabling weighted integration", () => {
    const frameA = demoReviewModel.frames[0]!;
    const frameB = demoReviewModel.frames[1]!;
    const planSha256 = "8".repeat(64);
    const weighted: ReviewViewModel = {
      ...demoReviewModel,
      activeWorkspace: "registration",
      activeRole: "light",
      lightFrameView: "calibrated",
      frames: [frameA, frameB],
      registration: {
        ...demoReviewModel.registration,
        planState: "ready",
        plan: {
          schemaVersion: 1,
          planSha256,
          referenceFrameId: frameA.id,
          referenceWidth: 4144,
          referenceHeight: 2822,
          coveredPixels: 11_000_000,
          autocrop: { x: 0, y: 0, width: 4144, height: 2822 },
          frames: [
            {
              frameId: frameA.id,
              sourceWidth: 4144,
              sourceHeight: 2822,
              transformCoefficientsSourcePixels: [1, 0, 0, 0, 1, 0],
              reference: true,
            },
            {
              frameId: frameB.id,
              sourceWidth: 4144,
              sourceHeight: 2822,
              transformCoefficientsSourcePixels: [1, 0, 0, 0, 1, 0],
              reference: false,
            },
          ],
        },
        execution: {
          ...demoReviewModel.registration.execution,
          state: "completed",
          result: {
            planSha256,
            memoryLimitBytes: 1_073_741_824,
            peakReservedBytes: 4_096,
            frames: [],
          },
        },
        stack: {
          ...demoReviewModel.registration.stack,
          weightPreflight: {
            schemaVersion: 1,
            planSha256,
            algorithmId: "balanced-psf-weight-v1",
            parametersSha256: "a".repeat(64),
            referenceFrameId: frameA.id,
            weights: [
              { frameId: frameA.id, weight: 1 },
              { frameId: frameB.id, weight: 0.8125 },
            ],
          },
          settings: {
            ...demoReviewModel.registration.stack.settings,
            estimator: "weighted_mean",
          },
        },
      },
    };
    const { root, actions, controller } = fixture(weighted);
    fireEvent.click(getByText(root, "Advanced integration"));

    const table = getByRole(root, "table");
    expect(table.textContent).toContain(`${frameA.label} · reference`);
    expect(table.textContent).toContain(frameB.label);
    expect(root.textContent).toContain("2 / 2 frames sealed natively");
    expect(root.textContent).toContain("balanced-psf-weight-v1");
    expect(root.textContent).toContain("a".repeat(64));
    expect(
      getByRole<HTMLButtonElement>(root, "button", { name: "Integrate crop" })
        .disabled,
    ).toBe(false);
    fireEvent.change(
      getByRole<HTMLSelectElement>(root, "combobox", {
        name: "Weight reference",
      }),
      { target: { value: frameB.id } },
    );
    expect(actions.onUpdateRegisteredStackSettings).toHaveBeenCalledWith({
      ...weighted.registration.stack.settings,
      weightReferenceFrameId: frameB.id,
    });

    controller.update({ ...weighted, lightFrameView: "raw" });
    expect(root.textContent).toContain("0 / 2 frames have valid metrics");
    expect(root.textContent).not.toContain("balanced-psf-weight-v1");
    expect(
      getByRole<HTMLButtonElement>(root, "button", { name: "Integrate crop" })
        .disabled,
    ).toBe(true);
  });

  it("keeps ambiguous Light associations visibly blocked", () => {
    const lightPlan = demoReviewModel.calibration.plan?.lightPlan;
    if (!lightPlan) throw new Error("demo Light plan is missing");
    const source = lightPlan.products[0];
    if (!source) throw new Error("demo Light product is missing");
    const blocked = {
      ...demoReviewModel,
      activeWorkspace: "calibration" as const,
      calibration: {
        ...demoReviewModel.calibration,
        plan: {
          ...demoReviewModel.calibration.plan!,
          lightPlan: {
            ...lightPlan,
            ready: false,
            products: [
              {
                ...source,
                dark: {
                  ...source.dark,
                  status: "unresolved" as const,
                  selectedGroupId: null,
                  temperatureBasis: null,
                  temperatureDeltaCelsius: null,
                  blockingReason: "ambiguous_candidates" as const,
                  ambiguousGroupIds: ["dark-a", "dark-b"],
                },
              },
            ],
          },
        },
      },
    };
    const { root } = fixture(blocked);

    expect(root.textContent).toContain("Blocked");
    expect(root.textContent).toContain("Ambiguous · 2 candidates");
  });

  it("sends explicit matching controls back for native replanning", () => {
    const ready = {
      ...demoReviewModel,
      activeWorkspace: "calibration" as const,
      reviewSessionReady: true,
    };
    const { root, actions } = fixture(ready);
    const policy = getByRole<HTMLSelectElement>(root, "combobox", {
      name: "Selection policy",
    });

    fireEvent.change(policy, { target: { value: "require_bias" } });

    expect(actions.onUpdateCalibrationSettings).toHaveBeenCalledWith({
      flatPedestalPolicy: "require_bias",
      maximumExposureDeltaSeconds: 0.25,
      maximumTemperatureDeltaC: 2,
      maximumLightDarkTemperatureDeltaC: 2,
    });
    fireEvent.click(
      getByRole(root, "button", { name: "Increase exposure tolerance" }),
    );
    expect(actions.onUpdateCalibrationSettings).toHaveBeenLastCalledWith({
      flatPedestalPolicy: "require_bias",
      maximumExposureDeltaSeconds: 0.26,
      maximumTemperatureDeltaC: 2,
      maximumLightDarkTemperatureDeltaC: 2,
    });
    fireEvent.click(getByRole(root, "button", { name: "↻ Rebuild plan" }));
    expect(actions.onRefreshMasterPlan).toHaveBeenCalledOnce();
    fireEvent.click(getByRole(root, "button", { name: "Build masters" }));
    expect(actions.onExecuteMasterPlan).toHaveBeenCalledOnce();
  });

  it("shows cancellable native build progress without exposing another run", () => {
    const running = {
      ...demoReviewModel,
      activeWorkspace: "calibration" as const,
      reviewSessionReady: true,
      calibration: {
        ...demoReviewModel.calibration,
        execution: {
          state: "running" as const,
          outputDirectory: "/session/masters",
          progress: {
            productIndex: 0,
            productCount: 2,
            groupId: "dark-2s-g120-o30",
            kind: "dark" as const,
            sequence: 2,
            stage: "master.integrate",
            state: "running" as const,
            completedUnits: 4,
            totalUnits: 10,
            code: null,
          },
          result: null,
          message: "Master 1/2 · dark-2s-g120-o30 · master.integrate · 4/10",
        },
      },
    };
    const { root, actions } = fixture(running);

    expect(
      root.querySelector<HTMLProgressElement>(
        "[data-master-execution-progress]",
      )?.value,
    ).toBe(4);
    expect(
      getByRole(root, "button", { name: "Cancel build" }).hasAttribute(
        "disabled",
      ),
    ).toBe(false);
    expect(
      root.querySelector<HTMLButtonElement>(
        '[data-action="execute-master-plan"]',
      )?.hidden,
    ).toBe(true);
    fireEvent.click(getByRole(root, "button", { name: "Cancel build" }));
    expect(actions.onCancelMasterPlan).toHaveBeenCalledOnce();
  });

  it("unlocks and cancels the transactional Light run independently", () => {
    const ready = {
      ...demoReviewModel,
      activeWorkspace: "calibration" as const,
      reviewSessionReady: true,
      calibration: {
        ...demoReviewModel.calibration,
        lightExecution: {
          ...demoReviewModel.calibration.lightExecution,
          masterDirectory: "/session/masters",
          message: "Verified masters ready",
        },
      },
    };
    const mounted = fixture(ready);
    const mode = getByRole(mounted.root, "combobox", {
      name: "Light output mode",
    });
    expect((mode as HTMLSelectElement).value).toBe("calibrated_frames");
    fireEvent.click(
      getByRole(mounted.root, "button", { name: "Calibrate frames" }),
    );
    expect(mounted.actions.onExecuteLightPlan).toHaveBeenCalledOnce();

    fireEvent.change(mode, { target: { value: "integrated" } });
    expect(mounted.actions.onUpdateLightOutputMode).toHaveBeenCalledWith(
      "integrated",
    );

    mounted.controller.update({
      ...ready,
      calibration: {
        ...ready.calibration,
        lightExecution: {
          state: "running",
          masterDirectory: "/session/masters",
          outputDirectory: "/session/lights",
          progress: {
            productIndex: 0,
            productCount: 1,
            groupId: "light-uvir-2s-g120-o30",
            sequence: 3,
            stage: "pipeline.integrate",
            state: "running",
            completedUnits: 6,
            totalUnits: 10,
            code: null,
            sourceIndex: 0,
            sourceCount: 1,
          },
          result: null,
          message: "Light 1/1 · pipeline.integrate · 6/10",
        },
      },
    });
    expect(
      mounted.root.querySelector<HTMLProgressElement>(
        "[data-light-execution-progress]",
      )?.value,
    ).toBe(6);
    fireEvent.click(
      getByRole(mounted.root, "button", { name: "Cancel Lights" }),
    );
    expect(mounted.actions.onCancelLightPlan).toHaveBeenCalledOnce();
  });

  it("requests role changes and supports tab arrow navigation", () => {
    const { root, actions } = fixture();
    const lightTab = getByRole(root, "tab", { name: /Lights/ });

    fireEvent.click(getByRole(root, "tab", { name: /Darks/ }));
    expect(actions.onSelectRole).toHaveBeenCalledWith("dark");

    fireEvent.keyDown(lightTab, { key: "ArrowRight" });
    expect(actions.onSelectRole).toHaveBeenLastCalledWith("bias");
  });

  it("uses truthful backend sort keys and leaves unsortable metadata static", () => {
    const { root, actions } = fixture();

    fireEvent.click(
      getByRole(root, "button", { name: "Sort Stars ascending" }),
    );
    expect(actions.onSort).toHaveBeenCalledWith("detected_stars", "ascending");
    expect(
      getByRole(root, "columnheader", { name: "Exposure" }).querySelector(
        "button",
      ),
    ).toBeNull();
  });

  it("disables frame-only actions when the selected role is empty", () => {
    const emptyModel: ReviewViewModel = {
      ...demoReviewModel,
      activeRole: "dark",
      frames: [],
      selectedFrameId: null,
    };
    const { root } = fixture(emptyModel);

    expect(
      getByRole(root, "button", { name: "Accept" }).hasAttribute("disabled"),
    ).toBe(true);
    expect(
      getByRole(root, "button", { name: "Start Blink playback" }).hasAttribute(
        "disabled",
      ),
    ).toBe(true);
    expect(root.textContent).toContain(
      "Select a frame to inspect its display preview",
    );
    expect(
      root.querySelector<HTMLElement>("[data-preview-badges]")?.hidden,
    ).toBe(true);
  });

  it("displays only a preview bound to the selected frame identity", () => {
    const selectedFrameId = demoReviewModel.selectedFrameId;
    expect(selectedFrameId).not.toBeNull();
    if (!selectedFrameId) return;

    const mismatched = fixture({
      ...demoReviewModel,
      preview: { frameId: "f".repeat(64), url: "blob:mismatched" },
    });
    const mismatchedImage = mismatched.root.querySelector<HTMLImageElement>(
      "[data-preview-image]",
    );
    expect(mismatchedImage?.hidden).toBe(true);
    mismatched.controller.destroy();

    const matched = fixture({
      ...demoReviewModel,
      preview: { frameId: selectedFrameId, url: "blob:matched" },
    });
    const matchedImage = matched.root.querySelector<HTMLImageElement>(
      "[data-preview-image]",
    );
    expect(matchedImage?.hidden).toBe(false);
    expect(matchedImage?.getAttribute("src")).toBe("blob:matched");
    expect(matchedImage?.alt).toContain("light_0002.fits");
  });

  it("requests backend sorting without changing processing order locally", () => {
    const { root, actions } = fixture();
    const before = within(root)
      .getAllByRole("row")
      .map((row) => row.textContent);

    fireEvent.click(getByRole(root, "button", { name: "Sort FWHM ascending" }));

    expect(actions.onSort).toHaveBeenCalledWith("fwhm_major", "ascending");
    expect(
      within(root)
        .getAllByRole("row")
        .map((row) => row.textContent),
    ).toEqual(before);
  });

  it("commits decisions against the exact confirmed selected identity", () => {
    const reviewReady = { ...demoReviewModel, reviewSessionReady: true };
    const { root, actions, controller } = fixture(reviewReady);
    const target = demoReviewModel.frames[2];
    expect(target).toBeDefined();
    if (!target) return;

    const frameTable = getByRole(root, "table", {
      name: "Lights review metrics",
    });
    fireEvent.click(getByText(frameTable, target.label));
    expect(actions.onSelectFrame).toHaveBeenCalledWith(target.id);

    controller.update({ ...reviewReady, selectedFrameId: target.id });
    fireEvent.click(getByRole(root, "button", { name: "Accept" }));
    expect(actions.onSetDecision).toHaveBeenCalledWith(
      target.id,
      "accepted",
      null,
    );
  });

  it("requires a visible stable reason before manual rejection", () => {
    const { root, actions } = fixture({
      ...demoReviewModel,
      reviewSessionReady: true,
    });
    const selected = demoReviewModel.frames[1];
    expect(selected).toBeDefined();
    if (!selected) return;

    fireEvent.click(getByRole(root, "button", { name: "Reject" }));
    const dialog = getByRole(root, "dialog", {
      name: "Why reject this frame?",
    });
    expect(dialog.textContent).toContain(selected.label);
    fireEvent.click(getByRole(dialog, "button", { name: "Trailing" }));

    expect(actions.onSetDecision).toHaveBeenCalledWith(
      selected.id,
      "rejected",
      "trailing",
    );
    expect(dialog.closest("[data-reject-dialog]")?.hasAttribute("hidden")).toBe(
      true,
    );
  });

  it("enables native undo only while a transaction is available", () => {
    const undoable = {
      ...demoReviewModel,
      reviewSessionReady: true,
      canUndo: true,
    };
    const { root, actions, controller } = fixture(undoable);
    const undo = getByRole(root, "button", {
      name: "Undo last review decision",
    });

    expect(undo.hasAttribute("disabled")).toBe(false);
    fireEvent.click(undo);
    expect(actions.onUndo).toHaveBeenCalledOnce();

    controller.update({ ...undoable, decisionPending: true });
    expect(undo.hasAttribute("disabled")).toBe(true);
    expect(
      root.querySelector("[data-decision-controls]")?.getAttribute("aria-busy"),
    ).toBe("true");
    expect(
      getByRole(root, "button", { name: "Reject" }).hasAttribute("disabled"),
    ).toBe(true);
  });

  it("provides guarded keyboard shortcuts for rapid manual review", () => {
    const reviewReady = {
      ...demoReviewModel,
      reviewSessionReady: true,
      canUndo: true,
    };
    const { root, actions, controller } = fixture(reviewReady);
    const selected = demoReviewModel.frames[1];
    const accepted = demoReviewModel.frames[0];
    expect(selected).toBeDefined();
    expect(accepted).toBeDefined();
    if (!selected || !accepted) return;

    fireEvent.keyDown(root, { key: "a" });
    expect(actions.onSetDecision).toHaveBeenCalledWith(
      selected.id,
      "accepted",
      null,
    );

    fireEvent.keyDown(root, { key: "r" });
    expect(
      getByRole(root, "dialog", { name: "Why reject this frame?" }),
    ).toBeDefined();
    fireEvent.keyDown(root, { key: "a" });
    expect(actions.onSetDecision).toHaveBeenCalledTimes(1);
    fireEvent.keyDown(root, { key: "Escape" });

    fireEvent.keyDown(root, { key: "z", metaKey: true });
    expect(actions.onUndo).toHaveBeenCalledOnce();

    controller.update({
      ...reviewReady,
      selectedFrameId: accepted.id,
    });
    fireEvent.keyDown(root, { key: "c" });
    expect(actions.onClearDecision).toHaveBeenCalledWith(accepted.id);

    const input = document.createElement("input");
    root.append(input);
    fireEvent.keyDown(input, { key: "r" });
    expect(
      root.querySelector("[data-reject-dialog]")?.hasAttribute("hidden"),
    ).toBe(true);
  });

  it("publishes review shortcuts to assistive technology", () => {
    const { root } = fixture();

    expect(
      getByRole(root, "button", { name: "Accept" }).getAttribute(
        "aria-keyshortcuts",
      ),
    ).toBe("A");
    expect(
      getByRole(root, "button", {
        name: "No review decision to undo",
      }).getAttribute("aria-keyshortcuts"),
    ).toContain("Meta+Z");
  });

  it("supports arrow-key table review and named playback controls", () => {
    const { root, actions } = fixture();
    const selected = root.querySelector<HTMLElement>(
      'tr[aria-selected="true"]',
    );
    const expected = demoReviewModel.frames[2];
    expect(selected).not.toBeNull();
    expect(expected).toBeDefined();
    if (!selected || !expected) return;

    fireEvent.keyDown(selected, { key: "ArrowDown" });
    expect(actions.onSelectFrame).toHaveBeenCalledWith(expected.id);
    fireEvent.click(
      getByRole(root, "button", { name: "Start Blink playback" }),
    );
    expect(actions.onSetPlaying).toHaveBeenCalledWith(true);
    fireEvent.click(getByRole(root, "button", { name: "Next frame" }));
    expect(actions.onRequestStep).toHaveBeenCalledWith("forward");
  });

  it("switches between fitted and actual preview pixels explicitly", () => {
    const { root, actions, controller } = fixture();

    const fit = getByRole(root, "button", { name: "Fit preview to viewer" });
    const actual = getByRole(root, "button", {
      name: "Show preview pixels at one hundred percent",
    });
    expect(fit.getAttribute("aria-pressed")).toBe("true");
    expect(actual.getAttribute("aria-pressed")).toBe("false");

    fireEvent.click(actual);
    expect(actions.onSetViewerScale).toHaveBeenCalledWith("actual");

    controller.update({ ...demoReviewModel, viewerScale: "actual" });
    expect(fit.getAttribute("aria-pressed")).toBe("false");
    expect(actual.getAttribute("aria-pressed")).toBe("true");
    expect(
      root.querySelector<HTMLElement>("[data-preview]")?.dataset.scale,
    ).toBe("actual");
  });

  it("does not expose the unfinished clipping control as a working action", () => {
    const { root } = fixture();

    expect(
      getByRole(root, "button", {
        name: "Clipping overlay is not available in this build",
      }).hasAttribute("disabled"),
    ).toBe(true);
  });

  it("keeps manual decisions unavailable before native session import", () => {
    const { root } = fixture();

    for (const name of ["Accept", "Reject", "Clear"]) {
      expect(getByRole(root, "button", { name }).hasAttribute("disabled")).toBe(
        true,
      );
    }
    expect(
      root.querySelector<HTMLElement>("[data-decision-controls]")?.title,
    ).toContain("Import a FITS session");
  });

  it("presents exact FITS statistics only for a real selected source", () => {
    const selectedFrameId = demoReviewModel.selectedFrameId;
    expect(selectedFrameId).not.toBeNull();
    if (!selectedFrameId) return;
    const frames = demoReviewModel.frames.map((frame) =>
      frame.id === selectedFrameId
        ? { ...frame, sourcePath: "/session/LIGHTS/light_0002.fits" }
        : frame,
    );
    const { root, actions, controller } = fixture({
      ...demoReviewModel,
      frames,
    });
    const button = getByRole(root, "button", {
      name: "Inspect exact FITS statistics",
    });
    expect(button.hasAttribute("disabled")).toBe(false);

    fireEvent.click(button);
    expect(actions.onOpenStatistics).toHaveBeenCalledWith(selectedFrameId);

    controller.update({
      ...demoReviewModel,
      frames,
      statisticsPanel: {
        open: true,
        frameId: selectedFrameId,
        frameLabel: "light_0002.fits",
        state: "ready",
        message: null,
        statistics: {
          algorithmId: "fits-three-pass-moments-v1",
          axes: [4_144, 2_822],
          storedFormat: "signed 16-bit integer",
          headerConformant: true,
          headerDiagnostics: 0,
          totalSamples: 11_694_368,
          usableSamples: 11_694_368,
          undefinedSamples: 0,
          nonFiniteSamples: 0,
          minimum: 384,
          maximum: 65_535,
          mean: 1_924.25,
          populationStandardDeviation: 84.125,
          sampleStandardDeviation: 84.125_004,
        },
      },
    });

    const dialog = getByRole(root, "dialog", { name: "light_0002.fits" });
    expect(dialog.textContent).toContain("fits-three-pass-moments-v1");
    expect(dialog.textContent).toContain("4144 × 2822");
    expect(dialog.textContent).toContain("11,694,368 / 11,694,368");
    expect(dialog.textContent).toContain("1,924.25");

    fireEvent.click(
      getByRole(dialog, "button", { name: "Close FITS statistics" }),
    );
    expect(actions.onCloseStatistics).toHaveBeenCalledOnce();
  });

  it("requests strict CFA quality and presents its diagnostic state", () => {
    const selectedFrameId = demoReviewModel.selectedFrameId;
    expect(selectedFrameId).not.toBeNull();
    if (!selectedFrameId) return;
    const frames = demoReviewModel.frames.map((frame) =>
      frame.id === selectedFrameId
        ? {
            ...frame,
            sourcePath: "/session/LIGHTS/light_0002.fits",
            bayerPattern: "rggb" as const,
            qualityState: "idle" as const,
            qualityMessage: "Ready for phase-neutral CFA diagnostics",
            qualityProfileId: null,
            qualityOrigin: null,
            metrics: {
              signalToNoise: null,
              fwhmPixels: null,
              eccentricity: null,
              detectedStars: null,
              usableStars: null,
              background: null,
              noise: null,
            },
          }
        : frame,
    );
    const { root, actions, controller } = fixture({
      ...demoReviewModel,
      frames,
    });
    const measure = getByRole(root, "button", {
      name: "Measure diagnostic frame quality",
    });
    expect(measure.hasAttribute("disabled")).toBe(false);

    fireEvent.click(measure);
    expect(actions.onMeasureQuality).toHaveBeenCalledWith(selectedFrameId);

    controller.update({
      ...demoReviewModel,
      frames: frames.map((frame) =>
        frame.id === selectedFrameId
          ? {
              ...frame,
              qualityState: "ready" as const,
              qualityMessage:
                "812 measured stars · raw CFA · RGGB · cfa-cell-mean-v1 + local-max-moments-v1 · saturation unclassified · diagnostic only",
              qualityProfileId: "desktop-diagnostic-quality-v1",
              qualityOrigin: "measured" as const,
              metrics: {
                signalToNoise: 34.8,
                fwhmPixels: 3.42,
                eccentricity: 0.41,
                detectedStars: 817,
                usableStars: 812,
                background: 1_921.8,
                noise: 19.1,
              },
            }
          : frame,
      ),
    });

    expect(measure.textContent).toBe("Quality measured");
    expect(measure.hasAttribute("disabled")).toBe(true);
    expect(root.textContent).toContain("QUALITY · DIAGNOSTIC");
    expect(root.textContent).toContain("Stellar SNR");
    expect(root.textContent).toContain("34.8");
    expect(root.textContent).toContain("3.42");
    expect(root.textContent).toContain("817");
  });

  it("identifies quality evidence restored from the verified cache", () => {
    const selectedFrameId = demoReviewModel.selectedFrameId;
    expect(selectedFrameId).not.toBeNull();
    if (!selectedFrameId) return;
    const frames = demoReviewModel.frames.map((frame) =>
      frame.id === selectedFrameId
        ? {
            ...frame,
            qualityState: "ready" as const,
            qualityOrigin: "restored" as const,
            qualityMessage:
              "Verified cache · 812 measured stars · raw CFA · RGGB · diagnostic only",
          }
        : frame,
    );
    const { root } = fixture({
      ...demoReviewModel,
      frames,
    });

    const badge = root.querySelector<HTMLElement>("[data-quality-badge]");
    expect(badge?.textContent).toBe("QUALITY · RESTORED");
    expect(badge?.title).toContain("Verified cache");
  });

  it("offers bounded batch quality measurement for eligible light frames", () => {
    const frames = demoReviewModel.frames.map((frame, index) => ({
      ...frame,
      sourcePath: `/session/LIGHTS/light_${index}.fits`,
      bayerPattern: "rggb" as const,
      qualityState: index < 2 ? ("idle" as const) : frame.qualityState,
    }));
    const { root, actions, controller } = fixture({
      ...demoReviewModel,
      reviewSessionReady: true,
      frames,
    });
    const batch = getByRole(root, "button", {
      name: "Measure diagnostic quality for 3 eligible light frames",
    });
    expect(batch.textContent).toBe("Measure all · 3");
    expect(batch.hasAttribute("disabled")).toBe(false);

    fireEvent.click(batch);
    expect(actions.onMeasureAllQuality).toHaveBeenCalledOnce();

    controller.update({
      ...demoReviewModel,
      reviewSessionReady: true,
      qualityBatchRunning: true,
      qualityBatchProgress: { completed: 1, total: 3 },
      frames,
    });
    expect(batch.textContent).toBe("Analyzing 1 / 3");
    expect(batch.hasAttribute("disabled")).toBe(true);
    expect(batch.getAttribute("aria-live")).toBe("polite");
  });

  it("has no automatically detectable accessibility violations", async () => {
    const frames = fixture();
    const { root } = frames;
    const selectionPanel = root.querySelector<HTMLDetailsElement>(
      "[data-selection-panel]",
    );
    if (selectionPanel) selectionPanel.open = true;
    const framesReport = await axe.run(root, {
      rules: {
        "color-contrast": { enabled: false },
      },
    });
    expect(framesReport.violations).toEqual([]);
    frames.controller.destroy();
    frames.root.remove();

    const calibration = fixture({
      ...demoReviewModel,
      activeWorkspace: "calibration",
    });
    const calibrationReport = await axe.run(calibration.root, {
      rules: {
        "color-contrast": { enabled: false },
      },
    });
    expect(calibrationReport.violations).toEqual([]);
    calibration.controller.destroy();
    calibration.root.remove();

    const registration = fixture({
      ...demoReviewModel,
      activeWorkspace: "registration",
    });
    const registrationReport = await axe.run(registration.root, {
      rules: {
        "color-contrast": { enabled: false },
      },
    });
    expect(registrationReport.violations).toEqual([]);
    registration.controller.destroy();
    registration.root.remove();

    const normalization = fixture({
      ...demoReviewModel,
      activeWorkspace: "normalization",
    });
    const advanced = normalization.root.querySelector<HTMLDetailsElement>(
      ".normalization-advanced",
    );
    if (advanced) advanced.open = true;
    const normalizationReport = await axe.run(normalization.root, {
      rules: {
        "color-contrast": { enabled: false },
      },
    });
    expect(normalizationReport.violations).toEqual([]);
  });
});
