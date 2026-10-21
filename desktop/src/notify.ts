// A banner when the window is not in front and something needs the person or has finished.
// An approval nobody sees is a turn stuck for four minutes and then refused; a turn that ended
// while you were in another app is the reason you switched away.
//
// One banner per event, from one trigger each: a turn's end comes from its `turn.ended` fact —
// after the checks and the commit, so it can say whether the checks passed — and only when that
// fact arrives live, never on a replay, a reconnect or a relaunch. Clicking a banner cannot open
// its lane: the desktop notification plugin keeps a title and a body and drops actions.
import { isPermissionGranted, requestPermission, sendNotification } from "@tauri-apps/plugin-notification";
import { getCurrentWindow, UserAttentionType } from "@tauri-apps/api/window";

let allowed: boolean | undefined;
const KEY = "keel.notifications";

async function permitted(): Promise<boolean> {
  if (allowed !== undefined) return allowed;
  try {
    allowed = (await isPermissionGranted()) || (await requestPermission()) === "granted";
  } catch {
    allowed = false;
  }
  return allowed;
}

/// Whether the person wants banners at all: Settings › Notifications, on unless turned off.
export function notificationsOn(): boolean {
  try {
    return localStorage.getItem(KEY) !== "off";
  } catch {
    return true;
  }
}

export function setNotificationsOn(on: boolean) {
  try {
    localStorage.setItem(KEY, on ? "on" : "off");
  } catch {
    /* this session still has it */
  }
}

/// Whether the system lets Keel show banners — a "Don't allow" clicked once is silent forever
/// otherwise, and reads as the feature being broken.
export async function systemAllows(): Promise<boolean> {
  try {
    return await isPermissionGranted();
  } catch {
    return false;
  }
}

/// Only when the window is not the one in front: a banner about what you are looking at is noise.
export async function notify(title: string, body: string) {
  if (!notificationsOn() || document.hasFocus() || !(await permitted())) return;
  try {
    sendNotification({ title, body });
  } catch {
    // A missed banner is not worth a failure on screen.
  }
}

/// Something is waiting on the person: a banner, and one dock bounce or taskbar flash.
export async function needsYou(title: string, body: string) {
  await notify(title, body);
  if (!notificationsOn() || document.hasFocus()) return;
  try {
    await getCurrentWindow().requestUserAttention(UserAttentionType.Informational);
  } catch {
    /* not every platform bounces */
  }
}

/// How many lanes are waiting on the person, on the dock icon; nothing when none are.
export function badge(count: number) {
  void getCurrentWindow()
    .setBadgeCount(count > 0 ? count : undefined)
    .catch(() => undefined);
}

const clock = (ms: number) => {
  const s = Math.round(ms / 1000);
  return s >= 60 ? `${Math.floor(s / 60)}m ${s % 60}s` : `${s}s`;
};

/// What a finished turn says: how long it took and what the checks said, if any ran.
export function turnEnded(ms: number | undefined, gate: { status: string; problems?: unknown[] } | undefined): string {
  const took = ms ? `Done in ${clock(ms)}.` : "Done.";
  if (gate?.status === "failed") {
    const n = gate.problems?.length ?? 0;
    return `${took} Checks failed${n ? `: ${n} problem${n === 1 ? "" : "s"}` : ""}.`;
  }
  if (gate?.status === "passed") return `${took} Checks passed.`;
  return took;
}

/// The banner for a turn that ended, or nothing. Not for a turn the person stopped — they were
/// there — nor for one that failed: its `fatal` already said "Stopped: …", and a second banner
/// reading "Done" would be the last word on a turn that was not.
export function endBanner(turn: { closed?: string; ms?: number; gate?: { status: string; problems?: unknown[] } }): string | undefined {
  if (turn.closed === "stopped" || turn.closed === "interrupted" || turn.closed === "failed") return undefined;
  return turnEnded(turn.ms, turn.gate);
}
