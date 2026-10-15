import { useSyncExternalStore } from "react";

export type Appearance = "system" | "light" | "dark";
const KEY = "keel:appearance";
const scheme = window.matchMedia("(prefers-color-scheme: dark)");
const listeners = new Set<() => void>();
function read(): Appearance {
  try { const value = localStorage.getItem(KEY); return value === "light" || value === "dark" ? value : "system"; }
  catch { return "system"; }
}
let mode = read();
let snapshot = { mode, dark: mode === "dark" || (mode === "system" && scheme.matches) };
function apply() {
  const dark = mode === "dark" || (mode === "system" && scheme.matches);
  document.documentElement.dataset.theme = dark ? "dark" : "light";
  document.documentElement.style.colorScheme = dark ? "dark" : "light";
  if (snapshot.mode === mode && snapshot.dark === dark) return;
  snapshot = { mode, dark };
  for (const listener of listeners) listener();
}
export function setAppearance(value: Appearance) {
  mode = value;
  try { localStorage.setItem(KEY, value); } catch { /* Apply for this launch if storage is unavailable. */ }
  apply();
}
export function onAppearance(listener: () => void) { listeners.add(listener); return () => { listeners.delete(listener); }; }
export function isDark() { return snapshot.dark; }
export function useAppearance() { return useSyncExternalStore(onAppearance, () => snapshot); }
const storage = (event: StorageEvent) => { if (event.key === KEY || event.key === null) { mode = read(); apply(); } };
scheme.addEventListener("change", apply);
window.addEventListener("storage", storage);
apply();
if (import.meta.hot) import.meta.hot.dispose(() => {
  scheme.removeEventListener("change", apply);
  window.removeEventListener("storage", storage);
});
