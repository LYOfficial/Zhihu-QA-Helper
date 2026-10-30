export type Theme = "paper" | "zhihu";

const STORAGE_KEY = "zhihu-qa-theme";

export function getTheme(): Theme {
  if (typeof window === "undefined") return "zhihu";
  return window.localStorage.getItem(STORAGE_KEY) === "paper" ? "paper" : "zhihu";
}

export function saveTheme(theme: Theme) {
  if (typeof window !== "undefined") window.localStorage.setItem(STORAGE_KEY, theme);
}

