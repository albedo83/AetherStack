import { fireEvent, getByRole, getByText, within } from "@testing-library/dom";
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
    onUpdateCalibrationSettings: vi.fn(),
    onUpdateLightOutputMode: vi.fn(),
    onRefreshMasterPlan: vi.fn(),
    onExecuteMasterPlan: vi.fn(),
    onCancelMasterPlan: vi.fn(),
    onExecuteLightPlan: vi.fn(),
    onCancelLightPlan: vi.fn(),
    onImportSession: vi.fn(),
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
  };
  const controller = mountReviewScreen(root, model, actions);
  return { root, actions, controller };
}

describe("frame review workspace", () => {
  it("requests a native session import from the primary workspace action", () => {
    const { root, actions } = fixture();

    fireEvent.click(getByRole(root, "button", { name: "＋ Import session" }));

    expect(actions.onImportSession).toHaveBeenCalledOnce();
  });

  it("exposes busy import state without allowing a duplicate scan", () => {
    const { root } = fixture({
      ...demoReviewModel,
      sessionStatus: { tone: "busy", label: "Scanning FITS sources" },
    });

    const button = getByRole(root, "button", { name: "Scanning FITS…" });
    expect(button.hasAttribute("disabled")).toBe(true);
    expect(root.textContent).toContain("Scanning FITS sources");
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

    fireEvent.click(getByText(root, target.label));
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

  it("does not expose unfinished controls as working actions", () => {
    const { root } = fixture();

    for (const name of [
      "Diagnostics",
      "Review plan",
      "Clipping overlay is not available in this build",
    ]) {
      expect(getByRole(root, "button", { name }).hasAttribute("disabled")).toBe(
        true,
      );
    }
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
            metrics: {
              fwhmPixels: null,
              eccentricity: null,
              detectedStars: null,
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
              metrics: {
                fwhmPixels: 3.42,
                eccentricity: 0.41,
                detectedStars: 817,
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
    expect(root.textContent).toContain("3.42");
    expect(root.textContent).toContain("817");
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
  });
});
