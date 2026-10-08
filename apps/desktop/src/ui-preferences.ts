export interface UiPreferences {
  readonly density: "comfortable" | "compact";
  readonly contrast: "standard" | "elevated";
  readonly motion: "system" | "reduced";
}

export interface UiPreferencesController {
  readonly destroy: () => void;
}

export const defaultUiPreferences: UiPreferences = {
  density: "comfortable",
  contrast: "standard",
  motion: "system",
};

const storageKey = "aetherstack.ui-preferences.v1";

/**
 * Settings are deliberately presentation-only. Scientific parameters remain
 * in their pipeline workspaces, where Rust can seal and verify them.
 */
export function uiPreferencesMarkup(): string {
  return `
    <section class="settings-workspace" aria-labelledby="settings-heading" data-settings-workspace hidden>
      <div class="workspace-heading overview-heading">
        <div><p class="eyebrow">Application settings</p><h2 id="settings-heading">A focused interface for long sessions</h2></div>
        <span class="overview-heading__seal">Local to this device</span>
      </div>
      <p class="workspace-intro">These controls change presentation only. Calibration, registration, normalization, and integration parameters stay inside their evidence-sealed pipeline stages.</p>
      <div class="settings-grid">
        ${preferenceGroup(
          "density",
          "Workspace density",
          "Choose generous spacing or fit more evidence on screen.",
          [
            ["comfortable", "Comfortable"],
            ["compact", "Compact"],
          ],
        )}
        ${preferenceGroup(
          "contrast",
          "Surface contrast",
          "Increase boundaries between controls and scientific evidence.",
          [
            ["standard", "Standard"],
            ["elevated", "Elevated"],
          ],
        )}
        ${preferenceGroup(
          "motion",
          "Interface motion",
          "Follow the operating system or remove nonessential animation.",
          [
            ["system", "Use system setting"],
            ["reduced", "Always reduce"],
          ],
        )}
        <section class="settings-card settings-card--policy" aria-labelledby="settings-policy-heading">
          <div class="settings-card__icon" aria-hidden="true">◇</div>
          <div><h3 id="settings-policy-heading">Scientific policy</h3><p>Processing defaults are not hidden here. Every scientific choice is visible beside its data, included in its native plan, and covered by a SHA-256 evidence seal.</p></div>
        </section>
      </div>
      <div class="settings-footer"><p data-settings-status role="status" aria-live="polite">Preferences loaded</p><button class="button button--quiet" type="button" data-ui-reset>Restore interface defaults</button></div>
    </section>`;
}

export function mountUiPreferences(
  workspace: HTMLElement,
  storage: Pick<Storage, "getItem" | "setItem" | "removeItem"> = localStorage,
  documentRoot: HTMLElement = document.documentElement,
): UiPreferencesController {
  let preferences = loadUiPreferences(storage);

  const render = (message: string): void => {
    applyUiPreferences(documentRoot, preferences);
    for (const button of workspace.querySelectorAll<HTMLButtonElement>(
      "[data-ui-preference]",
    )) {
      const key = button.dataset.uiPreference;
      const selected =
        (key === "density" && button.dataset.uiValue === preferences.density) ||
        (key === "contrast" &&
          button.dataset.uiValue === preferences.contrast) ||
        (key === "motion" && button.dataset.uiValue === preferences.motion);
      button.setAttribute("aria-pressed", String(selected));
    }
    required<HTMLElement>(workspace, "[data-settings-status]").textContent =
      message;
  };

  const onClick = (event: MouseEvent): void => {
    const target = event.target instanceof Element ? event.target : null;
    if (target?.closest<HTMLElement>("[data-ui-reset]")) {
      preferences = defaultUiPreferences;
      storage.removeItem(storageKey);
      render("Interface defaults restored");
      return;
    }
    const button = target?.closest<HTMLButtonElement>("[data-ui-preference]");
    if (!button) return;
    const next = updatePreference(
      preferences,
      button.dataset.uiPreference,
      button.dataset.uiValue,
    );
    if (next === preferences) return;
    preferences = next;
    storage.setItem(storageKey, JSON.stringify(preferences));
    render("Interface preference saved on this device");
  };

  workspace.addEventListener("click", onClick);
  render("Preferences loaded");
  return { destroy: () => workspace.removeEventListener("click", onClick) };
}

export function loadUiPreferences(
  storage: Pick<Storage, "getItem">,
): UiPreferences {
  try {
    const encoded = storage.getItem(storageKey);
    if (!encoded) return defaultUiPreferences;
    const value: unknown = JSON.parse(encoded);
    if (!isRecord(value)) return defaultUiPreferences;
    return {
      density: value.density === "compact" ? "compact" : "comfortable",
      contrast: value.contrast === "elevated" ? "elevated" : "standard",
      motion: value.motion === "reduced" ? "reduced" : "system",
    };
  } catch {
    return defaultUiPreferences;
  }
}

export function applyUiPreferences(
  root: HTMLElement,
  preferences: UiPreferences,
): void {
  root.dataset.uiDensity = preferences.density;
  root.dataset.uiContrast = preferences.contrast;
  root.dataset.uiMotion = preferences.motion;
}

function updatePreference(
  current: UiPreferences,
  key: string | undefined,
  value: string | undefined,
): UiPreferences {
  if (key === "density" && (value === "comfortable" || value === "compact"))
    return { ...current, density: value };
  if (key === "contrast" && (value === "standard" || value === "elevated"))
    return { ...current, contrast: value };
  if (key === "motion" && (value === "system" || value === "reduced"))
    return { ...current, motion: value };
  return current;
}

function preferenceGroup(
  key: keyof UiPreferences,
  title: string,
  description: string,
  options: readonly (readonly [string, string])[],
): string {
  const buttons = options
    .map(
      ([value, label]) =>
        `<button type="button" data-ui-preference="${key}" data-ui-value="${value}" aria-pressed="false">${label}</button>`,
    )
    .join("");
  return `<fieldset class="settings-card"><legend>${title}</legend><p>${description}</p><div class="settings-segmented">${buttons}</div></fieldset>`;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function required<T extends Element>(root: ParentNode, selector: string): T {
  const element = root.querySelector<T>(selector);
  if (!element)
    throw new Error(`Missing required settings element: ${selector}`);
  return element;
}
