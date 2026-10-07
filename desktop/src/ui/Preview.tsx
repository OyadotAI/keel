import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import { useStore } from "../store";
import { frameable, get } from "../api";
import { typeInto } from "../typers";
import { urlIn, whyStopped } from "../devlog";
import { arm, clearPins, sendPins, setNote, showingLane, unpin, useDesign, type Pin } from "../design";
import { useCovered } from "../cover";
import { flat } from "../pins";
import { Icon } from "./icons";

/// The project's dev server, and the page it serves. One server for the project: a lane that is
/// not the one running it is told whose it is, rather than shown another lane's tree as its own.
/// A server the agent started in the background is a job of this lane, and is shown the same way.
export function Preview({ lane }: { lane: string }) {
  const dev = useStore((s) => s.lanes[lane]?.dev);
  const wt = useStore((s) => s.lanes[lane]?.wt);
  const toggle = useStore((s) => s.dev);
  const fromJob = useStore((s) => {
    for (const j of s.lanes[lane]?.jobs ?? []) {
      if (j.finished) continue;
      const u = j.url ?? urlIn(j.log);
      if (u) return `${u}\n${j.command}`;
    }
    return undefined;
  });
  if (!dev) return <div className="panel-empty muted">Asking the daemon about the dev server…</div>;
  if (dev.running && dev.elsewhere)
    return (
      <div className="panel-empty muted">
        The dev server is running for {dev.owner ? `the lane on keel/${dev.owner}` : "the project's own tree"}, not this one{wt ? "" : " — this lane shares the project tree"}. One runs at a time.
      </div>
    );
  const [jobUrl, jobCommand] = fromJob?.split("\n") ?? [];
  if (!dev.running && jobUrl) return <Running lane={lane} url={jobUrl} label={`started by the agent: ${jobCommand}`} />;
  if (!dev.running) return <NotRunning lane={lane} dev={dev} start={() => toggle(lane, true)} />;
  return <Running lane={lane} url={dev.url ?? undefined} log={dev.log} stop={() => toggle(lane, false)} />;
}

function Running({ lane, url, label, log, stop }: { lane: string; url?: string; label?: string; log?: string[]; stop?: () => void }) {
  const shown = url ? frameable(url) : null;
  return (
    <div className="preview">
      <div className="panel-head small">
        <span className="change-path" title={label}>{url ?? "Starting…"}</span>
        {shown && <PickButton lane={lane} />}
        {url && (
          <button className="ghost" onClick={() => void openUrl(url)} title="Open in the browser" aria-label="Open in the browser">
            <Icon name="maximize" size={13} />
          </button>
        )}
        {stop && <button onClick={stop}>Stop</button>}
      </div>
      {shown ? (
        <>
          <Page lane={lane} url={shown} />
          <Pins lane={lane} />
        </>
      ) : url ? (
        <div className="panel-empty">
          <p className="muted">This server is not on this machine's own address, so it cannot be shown inside Keel.</p>
          <button onClick={() => openUrl(url)}>Open {url} in the browser</button>
        </div>
      ) : (
        <pre className="code">{log?.slice(-30).join("\n") || "Waiting for it to say where it is listening…"}</pre>
      )}
    </div>
  );
}

/// Nothing running: why, if it was tried, and the two ways to start it — the agent, which can
/// install what is missing and knows the project, or Keel's own guess at the command.
function NotRunning({ lane, dev, start }: { lane: string; dev: NonNullable<ReturnType<typeof useStore.getState>["lanes"][string]["dev"]>; start: () => void }) {
  const [said, setSaid] = useState<string>();
  const why = whyStopped(dev.log);
  return (
    <div className="panel-empty">
      <h3>{why ? `The dev server stopped. ${why}` : dev.detected ? "The dev server is not running" : "No dev server found in this project"}</h3>
      {dev.log.length > 0 && <pre className="code small preview-log">{dev.log.slice(-8).join("\n")}</pre>}
      <p className="muted">
        The agent can start it — installing dependencies first if they are missing — and Keel shows the page here once it says where it is listening.
      </p>
      <div className="actions" style={{ justifyContent: "center" }}>
        <button className="primary" onClick={() => void askToRun(lane).then((ok) => setSaid(ok ? "Asked the agent — follow its progress in this lane." : "Could not send the request. Check the lane’s connection and try again."))}>
          Ask the agent to run it
        </button>
        {dev.detected && (
          <button onClick={start} title={`Keel runs ${dev.detected} itself`}>
            Start with <code>{dev.detected}</code>
          </button>
        )}
      </div>
      {said && <p className="small faint">{said}</p>}
    </div>
  );
}

/// Hand starting the app to the agent: the project's own `/run` skill or command when it has
/// one, a plain request otherwise. Run in the background, it becomes a job Keel keeps alive.
async function askToRun(lane: string): Promise<boolean> {
  const l = useStore.getState().lanes[lane];
  const ep = l && useStore.getState().projects[l.project]?.endpoint;
  let skill = false;
  if (ep) {
    const state = await get<{ workspace?: { skills?: { name: string }[]; commands?: { name: string }[] } }>(ep, "/api/state").catch(() => undefined);
    const names = [...(state?.workspace?.skills ?? []), ...(state?.workspace?.commands ?? [])].map((n) => n.name);
    skill = names.includes("run");
  }
  if (!l) return false;
  const message = skill ? "/run" : "Start this project's full development environment so I can preview it. Use its root dev command, including any backend or other required services, rather than starting only the frontend. Install missing dependencies, run it in the background, verify the frontend can reach its API, and tell me the local URL.";
  return l.interface === "terminal" ? typeInto(lane, message) : useStore.getState().send(lane, message);
}

function PickButton({ lane }: { lane: string }) {
  const { armed } = useDesign(lane);
  return (
    <button className={armed ? "primary" : ""} onClick={() => arm(lane, !armed)} title={armed ? "Stop picking (Esc in the page)" : "Pick elements to change: click to pin, ↑↓ for parent and child"}>
      <Icon name="eye" size={13} /> {armed ? "Picking" : "Pick"}
    </button>
  );
}

/// Where the native preview sits. The page holds a placeholder; the dev server is a child webview
/// laid exactly over it, moved with it, and hidden whenever something is drawn on top.
function Page({ lane, url }: { lane: string; url: string }) {
  const host = useRef<HTMLDivElement>(null);
  const covered = useCovered();
  const [error, setError] = useState<string>();
  useEffect(() => {
    showingLane(lane);
    return () => showingLane(undefined);
  }, [lane]);
  useEffect(() => {
    const el = host.current;
    if (!el || covered) {
      void invoke("preview_hide").catch(() => undefined);
      return;
    }
    const bounds = () => {
      const r = el.getBoundingClientRect();
      return { x: r.left, y: r.top, width: r.width, height: r.height };
    };
    let disposed = false;
    let frame = 0;
    const move = () => {
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(() => void invoke("preview_bounds", { bounds: bounds() }).catch(() => undefined));
    };
    invoke("preview_show", { url, bounds: bounds() }).then(
      () => { if (!disposed) { setError(undefined); move(); } },
      (e) => { if (!disposed) setError(String(e)); },
    );
    const ro = new ResizeObserver(move);
    ro.observe(el);
    ro.observe(document.body);
    if (el.parentElement) ro.observe(el.parentElement);
    window.addEventListener("resize", move);
    document.addEventListener("scroll", move, true);
    return () => {
      disposed = true;
      ro.disconnect();
      window.removeEventListener("resize", move);
      document.removeEventListener("scroll", move, true);
      cancelAnimationFrame(frame);
      void invoke("preview_hide").catch(() => undefined);
    };
  }, [url, covered]);
  return (
    <div ref={host} className="preview-host">
      {error ? <div className="panel-empty error">Could not show the preview: {error}</div> : covered ? <div className="panel-empty muted">The preview is hidden while a menu or dialog is open.</div> : null}
    </div>
  );
}

const VERDICT: Record<NonNullable<Pin["verdict"]>, { word: string; tone: string; why: string }> = {
  changed: { word: "Changed", tone: "ok", why: "The element looks different after the turn." },
  unchanged: { word: "Unchanged", tone: "bad", why: "Identical before and after: the edit probably went to a file that does not render this element." },
  gone: { word: "Gone", tone: "warn", why: "The element is no longer on the page — removed, renamed, or the page moved." },
  unchecked: { word: "Not checked", tone: "idle", why: "The preview was not showing this lane when the turn ended." },
};

/// The pins, each with its note and, once the turn has ended, whether the page changed.
function Pins({ lane }: { lane: string }) {
  const { pins, waiting } = useDesign(lane);
  const [note, setSaid] = useState<string>();
  const [sending, setSending] = useState(false);
  if (!pins.length) return null;
  const unsent = pins.filter((p) => !p.sent).length;
  return (
    <div className="pins">
      <div className="pins-head small">
        <span>
          {pins.length} pinned{waiting ? " · checking after the turn" : ""}
        </span>
        <span className="spacer" />
        <button className="link small" onClick={() => clearPins(lane)}>
          Clear
        </button>
        <button
          className="primary"
          disabled={!unsent || sending}
          onClick={async () => {
            setSending(true);
            try {
              const ok = await sendPins(lane);
              setSaid(ok ? undefined : "Could not send the changes. Check the lane’s connection and try again.");
            } catch {
              setSaid("Could not send the changes. Check the lane’s connection and try again.");
            } finally {
              setSending(false);
            }
          }}
          title="Type these into the agent as one prompt"
        >
          Send {unsent || ""} to agent
        </button>
      </div>
      {note && <div className="small error" style={{ padding: "0 12px 6px" }}>{note}</div>}
      <ol className="pin-list">
        {pins.map((p, i) => (
          <li key={p.id}>
            <span className="pin-n">{i + 1}</span>
            <div className="pin-body">
              <div className="pin-what">
                <code>&lt;{p.tag}&gt;</code> <span className="faint">{p.text || p.selector}</span>
                {p.verdict && (
                  <span className={`pin-verdict ${VERDICT[p.verdict].tone}`} title={VERDICT[p.verdict].why}>
                    {VERDICT[p.verdict].word}
                  </span>
                )}
              </div>
              <div className="pin-source small faint">
                {/* In full, exactly as it goes into the prompt: what is sent is what was shown. */}
                {p.sources.length ? p.sources.map((s) => (s.file ? `${flat(s.file)}${s.line ? `:${s.line}` : ""}` : `${s.kind} ${flat(s.name, 80)}`)).join(" · ") : "no source hint"}
              </div>
              {p.sent ? (
                p.note && <div className="small">{p.note}</div>
              ) : (
                <input className="field pin-note" value={p.note} onChange={(e) => setNote(lane, p.id, e.target.value)} placeholder="What should change? e.g. make it 320px wide" aria-label={`Note for pin ${i + 1}`} />
              )}
            </div>
            <button className="ghost" onClick={() => unpin(lane, p.id)} aria-label={`Remove pin ${i + 1}`}>
              <Icon name="x" size={12} />
            </button>
          </li>
        ))}
      </ol>
    </div>
  );
}
