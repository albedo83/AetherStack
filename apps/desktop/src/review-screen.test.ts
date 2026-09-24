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
    onImportSession: vi.fn(),
    onSelectRole: vi.fn(),
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

  it("has no automatically detectable accessibility violations", async () => {
    const { root } = fixture();
    const report = await axe.run(root, {
      rules: {
        "color-contrast": { enabled: false },
      },
    });
    expect(report.violations).toEqual([]);
  });
});
