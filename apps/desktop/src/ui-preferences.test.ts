import { fireEvent, getByRole } from "@testing-library/dom";
import { describe, expect, it, vi } from "vitest";

import {
  applyUiPreferences,
  defaultUiPreferences,
  loadUiPreferences,
  mountUiPreferences,
  uiPreferencesMarkup,
} from "./ui-preferences.ts";

function memoryStorage(initial: string | null = null) {
  let value = initial;
  return {
    getItem: vi.fn(() => value),
    setItem: vi.fn((_key: string, next: string) => {
      value = next;
    }),
    removeItem: vi.fn(() => {
      value = null;
    }),
  };
}

describe("UI preferences", () => {
  it("falls back safely when persisted JSON is invalid", () => {
    expect(loadUiPreferences(memoryStorage("not JSON"))).toEqual(
      defaultUiPreferences,
    );
    expect(loadUiPreferences(memoryStorage('{"density":"unknown"}'))).toEqual(
      defaultUiPreferences,
    );
  });

  it("applies only validated presentation preferences", () => {
    const root = document.createElement("html");
    applyUiPreferences(root, {
      density: "compact",
      contrast: "elevated",
      motion: "reduced",
    });
    expect(root.dataset).toMatchObject({
      uiDensity: "compact",
      uiContrast: "elevated",
      uiMotion: "reduced",
    });
  });

  it("persists controls and restores deterministic defaults", () => {
    const host = document.createElement("div");
    host.innerHTML = uiPreferencesMarkup();
    const workspace = host.querySelector<HTMLElement>(
      "[data-settings-workspace]",
    )!;
    workspace.hidden = false;
    const storage = memoryStorage();
    const documentRoot = document.createElement("div");
    const controller = mountUiPreferences(workspace, storage, documentRoot);

    expect(
      getByRole(workspace, "button", { name: "Comfortable" }).getAttribute(
        "aria-pressed",
      ),
    ).toBe("true");
    fireEvent.click(getByRole(workspace, "button", { name: "Compact" }));
    fireEvent.click(getByRole(workspace, "button", { name: "Elevated" }));
    fireEvent.click(getByRole(workspace, "button", { name: "Always reduce" }));
    expect(documentRoot.dataset).toMatchObject({
      uiDensity: "compact",
      uiContrast: "elevated",
      uiMotion: "reduced",
    });
    expect(storage.setItem).toHaveBeenCalledTimes(3);

    fireEvent.click(
      getByRole(workspace, "button", { name: "Restore interface defaults" }),
    );
    expect(documentRoot.dataset).toMatchObject({
      uiDensity: "comfortable",
      uiContrast: "standard",
      uiMotion: "system",
    });
    expect(storage.removeItem).toHaveBeenCalledOnce();
    controller.destroy();
  });
});
