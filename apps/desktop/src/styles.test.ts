import { afterEach, describe, expect, it } from "vitest";

import "./styles.css";

describe("global visibility contract", () => {
  afterEach(() => {
    document.body.replaceChildren();
  });

  it("keeps hidden controls out of layout even when a component assigns display", () => {
    const grid = document.createElement("div");
    grid.className = "registered-stack__control-grid";
    grid.hidden = true;
    document.body.append(grid);

    expect(getComputedStyle(grid).display).toBe("none");
  });
});
