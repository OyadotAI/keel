// Design turns: elements pinned in the preview, each with a note, sent to the agent as one prompt,
// and checked once the turn ends. The check is what nobody else does — picking is table stakes,
// but a diff looks identical whether the edit reached the element or a file that never renders it.

import { useSyncExternalStore } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useStore } from "./store";
import { typeInto } from "./typers";

import { prompt, type Picked, type Pin } from "./pins";
export type { Pin, Verdict } from "./pins";

interface LaneDesign {
  pins: Pin[];
  armed: boolean;
  /// Sent and waiting for the turn to end.
  waiting: boolean;
}

const lanes = new Map<string, LaneDesign>();
const listeners = new Set<() => void>();
const EMPTY: LaneDesign = { pins: [], armed: false, waiting: false };
const get = (lane: string) => lanes.get(lane) ?? EMPTY;
function put(lane: string, f: (d: LaneDesign) => LaneDesign) {
  lanes.set(lane, f(get(lane)));
  listeners.forEach((l) => l());
}
export const useDesign = (lane: string) =>
  useSyncExternalStore(
    (l) => (listeners.add(l), () => listeners.delete(l)),
    () => get(lane),
  );

/// Which lane the preview is showing: picks land there.
let showing: string | undefined;
export function showingLane(lane: string | undefined) {
  showing = lane;
  if (lane) void send({ pins: get(lane).pins.map(({ id, selector }) => ({ id, selector })) });
}

const send = (msg: unknown) => invoke("preview_send", { msg }).catch(() => undefined);

export function arm(lane: string, on: boolean) {
  put(lane, (d) => ({ ...d, armed: on }));
  void send(on ? { arm: true } : { disarm: true });
}
export function setNote(lane: string, id: string, note: string) {
  put(lane, (d) => ({ ...d, pins: d.pins.map((p) => (p.id === id ? { ...p, note } : p)) }));
}
export function unpin(lane: string, id: string) {
  put(lane, (d) => ({ ...d, pins: d.pins.filter((p) => p.id !== id) }));
  void send({ pins: get(lane).pins.map(({ id, selector }) => ({ id, selector })) });
}
export function clearPins(lane: string) {
  put(lane, () => EMPTY);
  void send({ clear: true });
}

/// Send unsent pins through the active lane interface.
export async function sendPins(lane: string): Promise<boolean> {
  const unsent = get(lane).pins.filter((p) => !p.sent);
  if (!unsent.length) return false;
  const l = useStore.getState().lanes[lane];
  if (!l) return false;
  const delivered = l.interface === "terminal" ? typeInto(lane, prompt(unsent)) : await useStore.getState().send(lane, prompt(unsent));
  if (!delivered) return false;
  put(lane, (d) => ({ ...d, armed: false, waiting: true, pins: d.pins.map((p) => (p.sent ? p : { ...p, sent: true, verdict: undefined })) }));
  void send({ disarm: true });
  if (showing === lane) {
    baselining = lane;
    void send({ probe: unsent.map(({ id, selector }) => ({ id, selector })) });
  }
  return true;
}

/// Ask the page where each sent pin is now and what it looks like.
let probing: string | undefined;
let baselining: string | undefined;
function check(lane: string) {
  const sent = get(lane).pins.filter((p) => p.sent && !p.verdict);
  if (!sent.length) return put(lane, (d) => ({ ...d, waiting: false }));
  if (showing !== lane) {
    // The preview shows another lane, or is closed: no comparison is possible, and it says so.
    put(lane, (d) => ({ ...d, waiting: false, pins: d.pins.map((p) => (p.sent && !p.verdict ? { ...p, verdict: "unchecked" } : p)) }));
    return;
  }
  probing = lane;
  void send({ probe: sent.map(({ id, selector }) => ({ id, selector })) });
}

let started = false;
/// Once per app: picks and probe results from the preview, and every lane's turn ending.
export function watchDesign() {
  if (started) return;
  started = true;
  void listen<{ type: string; pin?: Picked; results?: { id: string; found: boolean; print?: string }[]; on?: boolean }>("preview", ({ payload: m }) => {
    const lane = showing;
    if (!lane) return;
    // A pick only while this lane is picking: the dev page can call preview_msg itself, and a
    // pin it invented must not appear unasked.
    if (m.type === "pick" && m.pin && get(lane).armed) {
      // Named fields, not a spread: the shell has already refused anything else, and Keel's own
      // fields (id, sent, verdict) are never the page's to set.
      const { selector, exact, tag, text, rect, sources, styles, print, url, frame } = m.pin;
      const pin: Pin = { selector, exact, tag, text, rect, sources, styles, print, url, frame, id: crypto.randomUUID(), note: "" };
      put(lane, (d) => ({ ...d, pins: [...d.pins.filter((p) => !(p.selector === pin.selector && !p.sent)), pin].slice(-12) }));
      void send({ pins: get(lane).pins.map(({ id, selector }) => ({ id, selector })) });
      // Only "stopped picking" is believed from the page (Esc there). Arming is Keel's alone: a
      // page that could say "armed" could then pin whatever it liked.
    } else if (m.type === "armed" && m.on === false) put(lane, (d) => ({ ...d, armed: false }));
    else if (m.type === "probe" && m.results && baselining === lane) {
      // The "before" of each pin sent just now, taken with the pointer off the page: at pick
      // time the element was under the cursor, its `:hover` style was in the fingerprint, and an
      // untouched element read as Changed.
      baselining = undefined;
      const by = new Map(m.results.map((r) => [r.id, r]));
      put(lane, (d) => ({ ...d, pins: d.pins.map((p) => (by.get(p.id)?.print ? { ...p, print: by.get(p.id)!.print! } : p)) }));
    } else if (m.type === "probe" && m.results && probing === lane) {
      probing = undefined;
      const by = new Map(m.results.map((r) => [r.id, r]));
      put(lane, (d) => ({
        ...d,
        waiting: false,
        pins: d.pins.map((p) => {
          const r = by.get(p.id);
          if (!p.sent || p.verdict || !r) return p;
          return { ...p, verdict: !r.found ? "gone" : r.print === p.print ? "unchanged" : "changed" };
        }),
      }));
    }
  });
  // A turn ending is Claude Code's own word (the lane goes idle); the page then needs a moment for
  // HMR to land before it is photographed.
  const was = new Map<string, boolean>();
  useStore.subscribe((s) => {
    for (const [id, l] of Object.entries(s.lanes)) {
      const before = was.get(id);
      was.set(id, l.running);
      if (before && !l.running && get(id).waiting) setTimeout(() => check(id), 2500);
    }
  });
}
