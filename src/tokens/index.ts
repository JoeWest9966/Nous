/** A theme is a token set, not a component branch: switching one swaps variables on `:root` and
 *  nothing else changes. Adding a theme means adding a block to `tokens.css` and a name here. */
export type ThemeName = "default" | "light";

export const THEMES: readonly ThemeName[] = ["default", "light"];

const STORAGE_KEY = "n-fe-next-theme";

export const isTheme = (value: string | null): value is ThemeName =>
  value !== null && (THEMES as readonly string[]).includes(value);

export function readTheme(): ThemeName {
  let saved: string | null = null;
  try {
    saved = localStorage.getItem(STORAGE_KEY);
  } catch {
    // A browser with storage disabled still renders; it just forgets the choice.
  }
  return isTheme(saved) ? saved : "default";
}

export function applyTheme(name: ThemeName): void {
  document.documentElement.dataset.theme = name;
  try {
    localStorage.setItem(STORAGE_KEY, name);
  } catch {
    // Same as above: the theme still applies for this page.
  }
}
