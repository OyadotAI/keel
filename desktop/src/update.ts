// Updates: the Tauri updater against the public releases feed. Checked at launch and every six
// hours, downloaded in the background, and installed only when the person says — a restart ends
// every agent the app is running, so it is never Keel's decision to make.

import { useSyncExternalStore } from "react";

export type Update =
  | { state: "idle" }
  | { state: "checking" }
  | { state: "current" }
  | { state: "downloading"; version: string }
  | { state: "ready"; version: string; notes?: string }
  | { state: "failed"; why: string };

let now: Update = { state: "idle" };
const listeners = new Set<() => void>();
const put = (u: Update) => {
  now = u;
  listeners.forEach((l) => l());
};
export const useUpdate = () =>
  useSyncExternalStore(
    (l) => (listeners.add(l), () => listeners.delete(l)),
    () => now,
  );

const SIX_HOURS = 6 * 60 * 60 * 1000;
let started = false;

/// Look for an update and fetch it. Quiet on the way: only a finished download is shown, and a
/// failed check is kept for Settings rather than raised — being offline is not news.
export async function check(): Promise<void> {
  if (now.state === "checking" || now.state === "downloading" || now.state === "ready") return;
  put({ state: "checking" });
  try {
    const { check } = await import("@tauri-apps/plugin-updater");
    const update = await check({ timeout: 30_000 });
    if (!update) return put({ state: "current" });
    put({ state: "downloading", version: update.version });
    await update.download();
    pending = update;
    put({ state: "ready", version: update.version, notes: update.body ?? undefined });
  } catch (e) {
    put({ state: "failed", why: e instanceof Error ? e.message : String(e) });
  }
}

let pending: { install(): Promise<void> } | undefined;

/// Install what was downloaded and start again on it.
export async function restart(): Promise<void> {
  if (!pending) return;
  try {
    // The daemons first: the installer replaces `keel` beside the app, and Windows will not
    // write a file a process is running. They come back on their own if the install fails.
    const { invoke } = await import("@tauri-apps/api/core");
    await invoke("stop_all");
    await pending.install();
    const { relaunch } = await import("@tauri-apps/plugin-process");
    await relaunch();
  } catch (e) {
    // Said, not swallowed: an app run from the disk image, or one macOS moved aside on first
    // launch, cannot replace itself, and a button that does nothing when clicked is worse.
    pending = undefined;
    put({ state: "failed", why: `The update could not install: ${e instanceof Error ? e.message : String(e)}. Move Keel to Applications and try again.` });
  }
}

/// Once per app: a dev build never checks, since there is nothing to update it to.
export function watchForUpdates() {
  if (started || import.meta.env.DEV) return;
  started = true;
  void check();
  setInterval(() => void check(), SIX_HOURS);
}
