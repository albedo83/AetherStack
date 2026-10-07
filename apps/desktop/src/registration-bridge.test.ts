import { Channel, invoke } from "@tauri-apps/api/core";
import { open, save } from "@tauri-apps/plugin-dialog";
import { afterEach, describe, expect, it, vi } from "vitest";

import {
  cancelDrizzle,
  cancelRegistrationPlan,
  cancelRegisteredStack,
  cancelRegisteredStackSourceVerification,
  diagnoseFitsRegistration,
  executeDrizzle,
  executeRegistrationPlan,
  executeRegisteredStack,
  inspectRegisteredStackReport,
  previewRegisteredWeights,
  previewRegistrationPlan,
  selectRegistrationOutputDirectory,
  selectDrizzleOutputDirectory,
  selectRegisteredStackReport,
  selectRegisteredStackSourceDirectory,
  selectRegisteredStackOutput,
  verifyRegisteredStackSources,
  type DrizzleProgress,
  type RegistrationExecutionProgress,
  type RegisteredStackProgress,
} from "./registration-bridge.ts";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
  Channel: vi.fn(function MockChannel(this: { onmessage?: unknown }) {
    this.onmessage = undefined;
  }),
}));
vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: vi.fn(),
  save: vi.fn(),
}));

afterEach(() => {
  vi.clearAllMocks();
});

describe("native registration bridge", () => {
  it("passes only the selected source and reference paths", async () => {
    const diagnostic = {
      schemaVersion: 3,
      confidence: { accepted: true },
      acceptedPlan: { autocrop: { x: 2, y: 5, width: 4137, height: 2815 } },
    };
    vi.mocked(invoke).mockResolvedValue(diagnostic);

    await expect(
      diagnoseFitsRegistration({
        sourcePath: "/session/light-02.fits",
        referencePath: "/session/light-01.fits",
      }),
    ).resolves.toBe(diagnostic);
    expect(invoke).toHaveBeenCalledWith("diagnose_fits_registration", {
      request: {
        sourcePath: "/session/light-02.fits",
        referencePath: "/session/light-01.fits",
      },
    });
  });

  it("submits only reviewed identities for native plan reconstruction", async () => {
    const plan = { schemaVersion: 1, planSha256: "a".repeat(64) };
    vi.mocked(invoke).mockResolvedValue(plan);

    await expect(
      previewRegistrationPlan({
        referenceFrameId: "1".repeat(64),
        sourceFrameIds: ["2".repeat(64), "3".repeat(64)],
      }),
    ).resolves.toBe(plan);
    expect(invoke).toHaveBeenCalledWith("preview_registration_plan", {
      request: {
        referenceFrameId: "1".repeat(64),
        sourceFrameIds: ["2".repeat(64), "3".repeat(64)],
      },
    });
  });

  it("binds atomic execution to the sealed digest and artifact identities", async () => {
    const result = { planSha256: "a".repeat(64), frames: [] };
    vi.mocked(invoke).mockResolvedValue(result);
    const planning = {
      referenceFrameId: "1".repeat(64),
      sourceFrameIds: ["2".repeat(64)],
    };
    const artifacts = [
      { frameId: "1".repeat(64), path: "/linear/reference.fits" },
      { frameId: "2".repeat(64), path: "/linear/source.fits" },
    ];
    const onProgress = vi.fn<(event: RegistrationExecutionProgress) => void>();

    await expect(
      executeRegistrationPlan(
        "/registered",
        planning,
        "a".repeat(64),
        artifacts,
        { bandHeight: 128, memoryLimitBytes: 1_073_741_824 },
        onProgress,
      ),
    ).resolves.toBe(result);

    expect(Channel).toHaveBeenCalledOnce();
    expect(invoke).toHaveBeenCalledWith("execute_registration_plan", {
      request: {
        planning,
        expectedPlanSha256: "a".repeat(64),
        artifacts,
        outputDirectory: "/registered",
        bandHeight: 128,
        memoryLimitBytes: 1_073_741_824,
      },
      onProgress: expect.objectContaining({ onmessage: onProgress }),
    });
  });

  it("uses native destination selection and cancellation", async () => {
    vi.mocked(open).mockResolvedValue("/registered");
    vi.mocked(invoke).mockResolvedValue(true);

    await expect(selectRegistrationOutputDirectory()).resolves.toBe(
      "/registered",
    );
    expect(open).toHaveBeenCalledWith({
      directory: true,
      multiple: false,
      title: "Select a directory for registered Light frames",
    });
    await expect(cancelRegistrationPlan()).resolves.toBe(true);
    expect(invoke).toHaveBeenLastCalledWith("cancel_registration_plan");
  });

  it("binds common-crop integration to the registered identities and plan", async () => {
    const result = {
      planSha256: "a".repeat(64),
      outputPath: "/results/integrated.fits",
    };
    vi.mocked(invoke).mockResolvedValue(result);
    const planning = {
      referenceFrameId: "1".repeat(64),
      sourceFrameIds: ["2".repeat(64)],
    };
    const artifacts = [
      { frameId: "1".repeat(64), path: "/registered/reference.fits" },
      { frameId: "2".repeat(64), path: "/registered/source.fits" },
    ];
    const onProgress = vi.fn<(event: RegisteredStackProgress) => void>();

    await expect(
      executeRegisteredStack(
        "/results/integrated.fits",
        planning,
        "a".repeat(64),
        artifacts,
        [],
        null,
        {
          bandHeight: 128,
          memoryLimitBytes: 1_073_741_824,
          integration: {
            estimator: "percentile_clipped",
            weightReferenceFrameId: null,
            lowFraction: 0.1,
            highFraction: 0.1,
            lowSigma: 4,
            highSigma: 3,
            esdOutlierFraction: 0.3,
            esdSignificance: 0.05,
            maximumIterations: 8,
            minimumRetainedSamples: 3,
            generateRejectionMaps: true,
            largeScaleLowEnabled: false,
            largeScaleHighEnabled: false,
            largeScaleLowLayers: 2,
            largeScaleHighLayers: 2,
            largeScaleLowGrowth: 2,
            largeScaleHighGrowth: 2,
          },
        },
        onProgress,
      ),
    ).resolves.toBe(result);
    expect(invoke).toHaveBeenCalledWith("execute_registered_stack", {
      request: {
        planning,
        expectedPlanSha256: "a".repeat(64),
        artifacts,
        qualityEvidence: [],
        qualityReferenceFrameId: null,
        outputPath: "/results/integrated.fits",
        bandHeight: 128,
        memoryLimitBytes: 1_073_741_824,
        integration: {
          estimator: "percentile_clipped",
          lowFraction: 0.1,
          highFraction: 0.1,
          lowSigma: 4,
          highSigma: 3,
          esdOutlierFraction: 0.3,
          esdSignificance: 0.05,
          maximumIterations: 8,
          minimumRetainedSamples: 3,
          generateRejectionMaps: true,
          largeScaleLowEnabled: false,
          largeScaleHighEnabled: false,
          largeScaleLowLayers: 2,
          largeScaleHighLayers: 2,
          largeScaleLowGrowth: 2,
          largeScaleHighGrowth: 2,
        },
      },
      onProgress: expect.objectContaining({ onmessage: onProgress }),
    });
  });

  it("forwards linear-fit residual controls without rewriting them", async () => {
    vi.mocked(invoke).mockResolvedValue({
      outputPath: "/results/linear-fit.fits",
    });
    const planning = {
      referenceFrameId: "1".repeat(64),
      sourceFrameIds: ["2".repeat(64)],
    };
    const integration = {
      estimator: "linear_fit_clipped" as const,
      weightReferenceFrameId: null,
      lowFraction: 0.1,
      highFraction: 0.1,
      lowSigma: 5,
      highSigma: 3.5,
      esdOutlierFraction: 0.3,
      esdSignificance: 0.05,
      maximumIterations: 8,
      minimumRetainedSamples: 3,
      generateRejectionMaps: true,
      largeScaleLowEnabled: false,
      largeScaleHighEnabled: false,
      largeScaleLowLayers: 2,
      largeScaleHighLayers: 2,
      largeScaleLowGrowth: 2,
      largeScaleHighGrowth: 2,
    };

    await executeRegisteredStack(
      "/results/linear-fit.fits",
      planning,
      "a".repeat(64),
      [
        { frameId: "1".repeat(64), path: "/registered/reference.fits" },
        { frameId: "2".repeat(64), path: "/registered/source.fits" },
      ],
      [],
      null,
      { bandHeight: 128, memoryLimitBytes: 1_073_741_824, integration },
      vi.fn(),
    );

    expect(invoke).toHaveBeenLastCalledWith(
      "execute_registered_stack",
      expect.objectContaining({
        request: expect.objectContaining({
          integration: {
            estimator: "linear_fit_clipped",
            lowFraction: 0.1,
            highFraction: 0.1,
            lowSigma: 5,
            highSigma: 3.5,
            esdOutlierFraction: 0.3,
            esdSignificance: 0.05,
            maximumIterations: 8,
            minimumRetainedSamples: 3,
            generateRejectionMaps: true,
            largeScaleLowEnabled: false,
            largeScaleHighEnabled: false,
            largeScaleLowLayers: 2,
            largeScaleHighLayers: 2,
            largeScaleLowGrowth: 2,
            largeScaleHighGrowth: 2,
          },
        }),
      }),
    );
  });

  it("forwards generalized ESD controls without rewriting them", async () => {
    vi.mocked(invoke).mockResolvedValue({
      outputPath: "/results/generalized-esd.fits",
    });
    const integration = {
      estimator: "generalized_esd" as const,
      weightReferenceFrameId: null,
      lowFraction: 0.1,
      highFraction: 0.1,
      lowSigma: 4,
      highSigma: 3,
      esdOutlierFraction: 0.25,
      esdSignificance: 0.01,
      maximumIterations: 8,
      minimumRetainedSamples: 15,
      generateRejectionMaps: true,
      largeScaleLowEnabled: false,
      largeScaleHighEnabled: false,
      largeScaleLowLayers: 2,
      largeScaleHighLayers: 2,
      largeScaleLowGrowth: 2,
      largeScaleHighGrowth: 2,
    };

    await executeRegisteredStack(
      "/results/generalized-esd.fits",
      {
        referenceFrameId: "1".repeat(64),
        sourceFrameIds: ["2".repeat(64)],
      },
      "a".repeat(64),
      [
        { frameId: "1".repeat(64), path: "/registered/reference.fits" },
        { frameId: "2".repeat(64), path: "/registered/source.fits" },
      ],
      [],
      null,
      { bandHeight: 128, memoryLimitBytes: 1_073_741_824, integration },
      vi.fn(),
    );

    expect(invoke).toHaveBeenLastCalledWith(
      "execute_registered_stack",
      expect.objectContaining({
        request: expect.objectContaining({
          integration: expect.objectContaining({
            estimator: "generalized_esd",
            esdOutlierFraction: 0.25,
            esdSignificance: 0.01,
            minimumRetainedSamples: 15,
            generateRejectionMaps: true,
            largeScaleLowEnabled: false,
            largeScaleHighEnabled: false,
            largeScaleLowLayers: 2,
            largeScaleHighLayers: 2,
            largeScaleLowGrowth: 2,
            largeScaleHighGrowth: 2,
          }),
        }),
      }),
    );
  });

  it("selects and cancels a registered stack natively", async () => {
    vi.mocked(save).mockResolvedValue("/results/integrated-common-crop.fits");
    vi.mocked(invoke).mockResolvedValue(true);

    await expect(selectRegisteredStackOutput()).resolves.toBe(
      "/results/integrated-common-crop.fits",
    );
    expect(save).toHaveBeenCalledWith({
      title: "Save the integrated registered common crop",
      defaultPath: "integrated-common-crop.fits",
      filters: [{ name: "FITS image", extensions: ["fits", "fit", "fts"] }],
    });
    await expect(cancelRegisteredStack()).resolves.toBe(true);
    expect(invoke).toHaveBeenLastCalledWith("cancel_registered_stack");
  });

  it("selects a prior integration report without accepting non-string results", async () => {
    vi.mocked(open).mockResolvedValue(
      "/results/integrated-integration-report.json",
    );
    await expect(selectRegisteredStackReport()).resolves.toBe(
      "/results/integrated-integration-report.json",
    );
    expect(open).toHaveBeenCalledWith({
      title: "Open an AetherStack integration report",
      multiple: false,
      directory: false,
      filters: [{ name: "AetherStack report", extensions: ["json"] }],
    });

    vi.mocked(open).mockResolvedValue([]);
    await expect(selectRegisteredStackReport()).resolves.toBeNull();
  });

  it("selects an explicit source directory for archived evidence", async () => {
    vi.mocked(open).mockResolvedValue("/archive/registered");
    await expect(selectRegisteredStackSourceDirectory()).resolves.toBe(
      "/archive/registered",
    );
    expect(open).toHaveBeenCalledWith({
      title: "Locate the registered sources named by this report",
      multiple: false,
      directory: true,
    });
  });

  it("requests canonical native weight evidence before execution", async () => {
    const result = {
      schemaVersion: 1,
      planSha256: "a".repeat(64),
      algorithmId: "balanced-psf-weight-v1",
      parametersSha256: "b".repeat(64),
      referenceFrameId: "1".repeat(64),
      weights: [{ frameId: "1".repeat(64), weight: 1 }],
    };
    vi.mocked(invoke).mockResolvedValue(result);
    const evidence = [
      {
        frameId: "1".repeat(64),
        signalToNoise: 20,
        fwhmPixels: 2.5,
        eccentricity: 0.4,
      },
    ];

    await expect(
      previewRegisteredWeights(
        "a".repeat(64),
        ["1".repeat(64)],
        "1".repeat(64),
        evidence,
      ),
    ).resolves.toBe(result);
    expect(invoke).toHaveBeenLastCalledWith("preview_registered_weights", {
      request: {
        expectedPlanSha256: "a".repeat(64),
        frameIds: ["1".repeat(64)],
        referenceFrameId: "1".repeat(64),
        qualityEvidence: evidence,
      },
    });
  });

  it("asks Rust to validate an integration report before inspection", async () => {
    const result = {
      schemaVersion: 1,
      reportSha256: "b".repeat(64),
      planSha256: "a".repeat(64),
      manifestSha256: "c".repeat(64),
      estimator: "weighted_mean",
      width: 4_144,
      height: 2_822,
      planes: 3,
      sourceCount: 12,
      productCount: 1,
      weighted: true,
      allProductsVerified: true,
      sources: [
        {
          frameId: "d".repeat(64),
          fileName: "registered-0001.fits",
          byteLength: 33_177_600,
          sha256: "e".repeat(64),
        },
      ],
      products: [
        {
          role: "science",
          fileName: "integrated.fits",
          path: "/results/integrated.fits",
          bytesWritten: 33_177_600,
          status: "verified",
        },
      ],
    };
    vi.mocked(invoke).mockResolvedValue(result);

    await expect(
      inspectRegisteredStackReport(
        "/results/integrated-integration-report.json",
      ),
    ).resolves.toBe(result);
    expect(invoke).toHaveBeenLastCalledWith("inspect_registered_stack_report", {
      request: {
        path: "/results/integrated-integration-report.json",
      },
    });
  });

  it("asks Rust to revalidate archived source fingerprints", async () => {
    const result = {
      reportSha256: "b".repeat(64),
      sourceDirectory: "/archive/registered",
      allSourcesVerified: true,
      sources: [],
    };
    vi.mocked(invoke).mockResolvedValue(result);
    const onProgress = vi.fn();

    await expect(
      verifyRegisteredStackSources(
        "/archive/report.json",
        "/archive/registered",
        onProgress,
      ),
    ).resolves.toBe(result);
    expect(invoke).toHaveBeenLastCalledWith("verify_registered_stack_sources", {
      request: {
        reportPath: "/archive/report.json",
        sourceDirectory: "/archive/registered",
      },
      onProgress: expect.objectContaining({ onmessage: onProgress }),
    });
    vi.mocked(invoke).mockResolvedValue(true);
    await expect(cancelRegisteredStackSourceVerification()).resolves.toBe(true);
    expect(invoke).toHaveBeenLastCalledWith(
      "cancel_registered_stack_source_verification",
    );
  });

  it("binds Drizzle execution to sealed geometry, sources, and parameters", async () => {
    const result = {
      registrationPlanSha256: "a".repeat(64),
      drizzlePlanSha256: "b".repeat(64),
      parametersSha256: "c".repeat(64),
      sciencePath: "/drizzle/drizzle-science.fits",
      weightPath: "/drizzle/drizzle-weight.fits",
      supportPath: "/drizzle/drizzle-support.fits",
    };
    vi.mocked(invoke).mockResolvedValue(result);
    const planning = {
      referenceFrameId: "1".repeat(64),
      sourceFrameIds: ["2".repeat(64)],
      geometryModel: "projective" as const,
    };
    const artifacts = [
      { frameId: "1".repeat(64), path: "/linear/reference.fits" },
      { frameId: "2".repeat(64), path: "/linear/source.fits" },
    ];
    const weights = [
      { frameId: "1".repeat(64), weight: 1 },
      { frameId: "2".repeat(64), weight: 0.75 },
    ];
    const onProgress = vi.fn<(event: DrizzleProgress) => void>();

    await expect(
      executeDrizzle(
        "/drizzle",
        planning,
        "a".repeat(64),
        artifacts,
        weights,
        {
          scale: 2,
          dropShrink: 0.8,
          maximumContributions: 64,
          maximumBandHeight: 128,
          memoryLimitBytes: 1_073_741_824,
        },
        onProgress,
      ),
    ).resolves.toBe(result);
    expect(Channel).toHaveBeenCalledOnce();
    expect(invoke).toHaveBeenLastCalledWith("execute_drizzle", {
      request: {
        planning,
        expectedPlanSha256: "a".repeat(64),
        artifacts,
        weights,
        outputDirectory: "/drizzle",
        scale: 2,
        dropShrink: 0.8,
        maximumContributions: 64,
        maximumBandHeight: 128,
        memoryLimitBytes: 1_073_741_824,
      },
      onProgress: expect.objectContaining({ onmessage: onProgress }),
    });
  });

  it("selects and cancels Drizzle natively", async () => {
    vi.mocked(open).mockResolvedValue("/drizzle");
    vi.mocked(invoke).mockResolvedValue(true);

    await expect(selectDrizzleOutputDirectory()).resolves.toBe("/drizzle");
    expect(open).toHaveBeenCalledWith({
      title: "Select a directory for Drizzle products",
      multiple: false,
      directory: true,
    });
    await expect(cancelDrizzle()).resolves.toBe(true);
    expect(invoke).toHaveBeenLastCalledWith("cancel_drizzle");
  });
});
