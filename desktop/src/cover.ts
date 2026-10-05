// What is drawn over the page right now. Its own module, with nothing imported, because every
// menu and dialog imports it.

import { useSyncExternalStore } from "react";

/// A native webview draws above the page, so anything shown over the preview's rectangle — a
/// menu, a dialog, the palette — would be hidden behind it. Each of those counts itself in here.
let covers = 0;
const coverListeners = new Set<() => void>();
export function cover(): () => void {
  covers++;
  coverListeners.forEach((l) => l());
  return () => {
    covers--;
    coverListeners.forEach((l) => l());
  };
}
export const useCovered = () =>
  useSyncExternalStore(
    (l) => (coverListeners.add(l), () => coverListeners.delete(l)),
    () => covers > 0,
  );
