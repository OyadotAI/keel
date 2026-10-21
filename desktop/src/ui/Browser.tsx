// The lane's Browser tab: the page the agent is using, with the person's own back, forward, reload
// and address. The page is a native webview laid inside `.browser-page`, inset by its ring; it is
// moved off-screen whenever this tab is not showing it or something is drawn on top.
//
// Nothing above the page changes height while the agent works: 40 px of bar and 26 px of status,
// always. A banner that came and went resized the native page twice per action, which closes
// menus on pages that close them on resize.
import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import { useStore } from "../store";
import { useCovered } from "../cover";
import { toWindow } from "../zoom";
import { act, address, setError, useTab, type Op, type Tab } from "../browser";
import { Empty } from "./kit";
import { Icon } from "./icons";

/// The ring's width, in CSS pixels: the page is placed this far inside its frame.
const RING = 2;

function Place({ lane, working }: { lane: string; working: boolean }) {
  const host = useRef<HTMLDivElement>(null);
  const covered = useCovered();
  useEffect(() => {
    const el = host.current;
    const away = () => void invoke("browser_place", { lane, bounds: null }).catch(() => undefined);
    if (!el || covered) {
      away();
      return;
    }
    // Followed every frame, not on resize alone: the panel slides in, tabs and rows settle, fonts
    // load — the frame moves without changing size, a ResizeObserver says nothing, and the page was
    // left where the frame had been mid-animation, over the status row. One rectangle read a frame
    // is cheap; the native call is made only when it changed.
    let frame = 0;
    let last = "";
    const follow = () => {
      const r = el.getBoundingClientRect();
      const inner = new DOMRect(r.left + RING, r.top + RING, Math.max(1, r.width - 2 * RING), Math.max(1, r.height - 2 * RING));
      const at = toWindow(inner);
      const key = `${Math.round(at.x)},${Math.round(at.y)},${Math.round(at.width)},${Math.round(at.height)}`;
      if (key !== last) {
        last = key;
        void invoke("browser_place", { lane, bounds: at }).catch(() => undefined);
      }
      frame = requestAnimationFrame(follow);
    };
    follow();
    return () => {
      cancelAnimationFrame(frame);
      away();
    };
  }, [lane, covered]);
  return (
    <div ref={host} className={`browser-page${working ? " working" : ""}`}>
      {covered && <div className="panel-empty muted">The browser is hidden while a menu or dialog is open.</div>}
    </div>
  );
}

/// The address as a person reads it: the lock or globe says the scheme, so it is not repeated.
const shown = (url: string) => url.replace(/^https?:\/\//, "").replace(/\/$/, "");

function Bar({ url, loading, run }: { url?: string; loading?: boolean; run: (op: Op) => void }) {
  const [typed, setTyped] = useState<string>();
  const go = () => {
    if (!typed?.trim()) return;
    run({ op: "open", url: address(typed) });
    setTyped(undefined);
  };
  return (
    <div className="browser-bar">
      <div className="browser-nav">
        <button className="ghost icon-button" disabled={!url} onClick={() => run({ op: "back" })} title="Back" aria-label="Back"><Icon name="arrow-left" size={14} /></button>
        <button className="ghost icon-button browser-forward" disabled={!url} onClick={() => run({ op: "forward" })} title="Forward" aria-label="Forward"><Icon name="arrow-right" size={14} /></button>
        <button className={`ghost icon-button${loading ? " spinning" : ""}`} disabled={!url} onClick={() => run({ op: "reload" })} title={loading ? "Loading…" : "Reload"} aria-label="Reload"><Icon name="rotate-cw" size={14} /></button>
      </div>
      <label className="browser-address">
        {url && typed === undefined && <Icon name={url.startsWith("https://") ? "lock" : "globe"} size={12} />}
        <input
          aria-label="Address"
          placeholder="Search or type an address"
          value={typed ?? (url ? shown(url) : "")}
          title={url}
          spellCheck={false}
          onFocus={(e) => {
            // The whole address while editing, selected, so typing replaces it.
            const input = e.currentTarget;
            setTyped(url ?? "");
            requestAnimationFrame(() => input.select());
          }}
          onChange={(e) => setTyped(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") go();
            if (e.key === "Escape") {
              setTyped(undefined);
              e.currentTarget.blur();
            }
          }}
          onBlur={() => setTyped(undefined)}
        />
      </label>
      {url && <button className="ghost icon-button" onClick={() => void openUrl(url)} title="Open in your browser" aria-label="Open in your browser"><Icon name="external-link" size={14} /></button>}
    </div>
  );
}

const hostOf = (url?: string) => {
  try {
    return url ? new URL(url).host : "";
  } catch {
    return "";
  }
};

/// What is happening in the tab, in a row that is always there.
function Status({ tab, working }: { tab?: Tab; working: boolean }) {
  const status = tab?.error ? "error" : working ? "working" : tab?.loading ? "loading" : "idle";
  const text =
    status === "error" ? tab?.error
    : status === "working" ? <><strong>Agent is browsing</strong>{tab?.activity && <span className={tab.acting ? undefined : "faint"}> · {tab.activity}</span>}</>
    : status === "loading" ? `Loading ${hostOf(tab?.url)}…`
    : tab?.url ? "Agent's browser · logins here stay with this project"
    : "No page open";
  return (
    <div className={`browser-status ${status}`} role="status" aria-live="polite">
      {status === "working" ? <span className="busy-dot" aria-hidden="true" /> : <span className={`browser-status-dot ${status}`} aria-hidden="true" />}
      <span className="browser-status-text">{text}</span>
    </div>
  );
}

export function Browser({ lane }: { lane: string }) {
  const project = useStore((s) => s.lanes[lane]?.project);
  const running = useStore((s) => !!s.lanes[lane]?.running);
  const tab = useTab(lane);
  // Held across a run, and only while the lane's turn runs, so it cannot outlive the agent.
  const working = !!tab?.browsing && running;
  const run = (op: Op) => {
    if (!project) return;
    setError(lane, undefined);
    act(lane, project, op).catch((e) => setError(lane, e instanceof Error ? e.message : String(e)));
  };
  return (
    <div className="preview browser">
      <Bar url={tab?.url} loading={tab?.loading} run={run} />
      <Status tab={tab} working={working} />
      {tab?.url ? (
        <Place lane={lane} working={working} />
      ) : tab?.room ? (
        <Empty title="This page was closed to make room for another lane">The agent reopens it on its next browser step, or type an address above.</Empty>
      ) : (
        <Empty title="No page open yet">When the agent needs a website, it opens it here. This browser is Keel's, not yours: sign in once and the login stays with this project.</Empty>
      )}
    </div>
  );
}
