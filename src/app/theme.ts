/** The theme the user picked: follow the system, or force light or dark. */
export type ThemePreference = "system" | "light" | "dark";

export const THEME_STORAGE_KEY = "jobniac.theme";

export const THEME_PREFERENCES: readonly ThemePreference[] = ["system", "light", "dark"];

function isThemePreference(value: unknown): value is ThemePreference {
  return value === "system" || value === "light" || value === "dark";
}

/** The stored preference. Storage can be missing or throw (blocked data), then it is "system". */
export function readThemePreference(): ThemePreference {
  try {
    const value = window.localStorage.getItem(THEME_STORAGE_KEY);
    return isThemePreference(value) ? value : "system";
  } catch {
    return "system";
  }
}

/** Remembers the preference. A failing store only means the choice lasts until the window closes. */
export function writeThemePreference(preference: ThemePreference): void {
  try {
    window.localStorage.setItem(THEME_STORAGE_KEY, preference);
  } catch {
    // Nothing to do: the theme is still applied to the page.
  }
}

/** Sets the `data-theme` attribute the stylesheet reads. "system" removes it. */
export function applyThemePreference(preference: ThemePreference): void {
  const root = document.documentElement;
  if (preference === "system") {
    root.removeAttribute("data-theme");
  } else {
    root.setAttribute("data-theme", preference);
  }
}

/** Applies the stored preference. Called in `main.tsx` before the first render, so no flash. */
export function initTheme(): ThemePreference {
  const preference = readThemePreference();
  applyThemePreference(preference);
  return preference;
}
