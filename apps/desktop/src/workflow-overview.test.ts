import { getByRole } from "@testing-library/dom";
import { describe, expect, it } from "vitest";

import { demoReviewModel } from "./demo-data.ts";
import {
  renderWorkflowOverview,
  workflowOverviewMarkup,
} from "./workflow-overview.ts";

function overviewFixture() {
  const root = document.createElement("div");
  root.innerHTML = workflowOverviewMarkup();
  const run = root.querySelector<HTMLElement>("[data-run-workspace]")!;
  const results = root.querySelector<HTMLElement>("[data-results-workspace]")!;
  run.hidden = false;
  results.hidden = false;
  return { root, run, results };
}

describe("workflow overview", () => {
  it("derives readiness from native model evidence", () => {
    const { run, results } = overviewFixture();
    renderWorkflowOverview(run, results, demoReviewModel);

    expect(run.querySelectorAll(".workflow-stage")).toHaveLength(6);
    expect(run.querySelectorAll('[data-state="complete"]')).toHaveLength(0);
    expect(run.querySelectorAll('[data-state="ready"]')).toHaveLength(2);
    expect(getByRole(run, "progressbar").getAttribute("value")).toBe("0");
    expect(run.textContent).toContain("0 of 5 required stages published");
    expect(run.textContent).toContain("Optional");
  });

  it("lists only completed native artifacts and escapes runtime paths", () => {
    const { run, results } = overviewFixture();
    renderWorkflowOverview(run, results, {
      ...demoReviewModel,
      calibration: {
        ...demoReviewModel.calibration,
        execution: {
          state: "completed",
          outputDirectory: "/products",
          progress: null,
          message: "One master published",
          result: {
            manifestSha256: "a".repeat(64),
            planSha256: "b".repeat(64),
            memoryLimitBytes: 1_073_741_824,
            peakReservedBytes: 128,
            products: [
              {
                groupId: "dark-60s",
                kind: "dark",
                outputPath: '/products/<master>&"dark.fits',
                totalSamples: 4,
                usableSamples: 4,
                maskedSamples: 0,
                nonFiniteSamples: 0,
                minimum: 1,
                maximum: 4,
                mean: 2.5,
                populationStandardDeviation: 1.25,
                samplesWritten: 4,
                substitutedSamples: 0,
                bytesWritten: 32,
                normalization: null,
              },
            ],
          },
        },
      },
    });

    expect(run.querySelectorAll('[data-state="complete"]')).toHaveLength(1);
    expect(getByRole(run, "progressbar").getAttribute("value")).toBe("20");
    expect(results.querySelectorAll(".result-card")).toHaveLength(1);
    expect(results.textContent).toContain('/products/<master>&"dark.fits');
    expect(results.querySelector("master")).toBeNull();
    expect(
      getByRole(results, "button", { name: "Open Master dark workspace" }),
    ).toBeTruthy();
    expect(
      getByRole(results, "button", {
        name: "Reveal Master dark in the file manager",
      }),
    ).toBeTruthy();
  });

  it("does not make optional local normalization block a complete run", () => {
    const { run, results } = overviewFixture();
    renderWorkflowOverview(run, results, {
      ...demoReviewModel,
      reviewSessionReady: true,
      calibration: {
        ...demoReviewModel.calibration,
        execution: {
          ...demoReviewModel.calibration.execution,
          state: "completed",
        },
        lightExecution: {
          ...demoReviewModel.calibration.lightExecution,
          state: "completed",
        },
      },
      registration: {
        ...demoReviewModel.registration,
        execution: {
          ...demoReviewModel.registration.execution,
          state: "completed",
        },
        stack: {
          ...demoReviewModel.registration.stack,
          state: "completed",
        },
      },
    });

    expect(getByRole(run, "progressbar").getAttribute("value")).toBe("100");
    expect(run.textContent).toContain("5 of 5 required stages published");
    expect(run.textContent).not.toContain("optional stage published");
  });
});
