import { createRoot } from "react-dom/client";
import { openUrl } from "@tauri-apps/plugin-opener";
import { App } from "./ui/App";
import "./styles.css";

// A link in the agent's reply opens in the browser. Followed in place, it replaced the whole app
// with that page and left no way back but quitting.
document.addEventListener("click", (e) => {
  const a = (e.target as Element | null)?.closest?.("a[href]") as HTMLAnchorElement | null;
  if (!a) return;
  e.preventDefault();
  if (/^https?:\/\//.test(a.href)) void openUrl(a.href);
});

createRoot(document.getElementById("root")!).render(<App />);

// The webview's own menu offers Reload and Inspect Element, neither of which belongs in an app.
// Text fields keep it for cut, copy and paste; everything else gets Keel's menus or nothing.
window.addEventListener("contextmenu", (e) => {
  const t = e.target as HTMLElement | null;
  if (!t?.closest("input, textarea, [contenteditable=true]")) e.preventDefault();
});

// Look for a new version in the background; a dev build never does.
void import("./update").then((u) => u.watchForUpdates());

// Picks and checks from the design preview.
void import("./design").then((d) => d.watchDesign());
