// The Recommended dialog: every skill and plugin Oya Factory recommends for this project and it
// lacks, each with why, its own Install and a live status, and Install all. An install that has
// to wait for a turn says so and finishes by itself; one that fails says why.
import { useState } from "react";
import { post, url, HttpError, type Endpoint } from "../api";
import { runOf, start, useRun } from "../runs";
import { useStore } from "../store";
import { SKILLS_CHANGED, useRecommendations, type CatalogPlugin, type CatalogSkill } from "../recommended-skills";
import { Dialog } from "./kit";

type Status = { phase: "idle" | "working" | "waiting" | "done" | "failed"; text?: string };
interface Item { key: string; name: string; kind: "Skill" | "Plugin"; why: string; description: string; install: () => Promise<boolean> }
interface PluginRun { last: string; waiting: boolean; code?: number }

/// How often a skill install that met a busy project tries again, and for how long.
const RETRY_MS = 2000;
const RETRY_FOR_MS = 30 * 60 * 1000;
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

/// A plugin installs over the daemon's stream; its run outlives this dialog (`runs.ts`), so closing
/// the dialog mid-install never starts a second one. Resolves when the run ends.
async function installPlugin(ep: Endpoint, p: CatalogPlugin): Promise<boolean> {
  const key = `recommended:plugin:${p.name}`;
  start<PluginRun>(key, url(ep, "/api/plugins/install", { name: p.name, marketplace: p.marketplace, commit: true }), ep.token, { last: "Installing…", waiting: false },
    (s, e) => e.event === "done" ? { ...s, code: Number(e.data), waiting: false } : e.event === "line" || e.event === "fatal" ? { ...s, last: String(e.data), waiting: String(e.data).startsWith("Waiting:"), code: e.event === "fatal" ? 1 : s.code } : s,
    (s, error) => (error ? { ...s, last: error, code: s.code ?? 1 } : s));
  while (runOf<PluginRun>(key)?.running) await sleep(500);
  return runOf<PluginRun>(key)?.state.code === 0;
}

/// A skill is one request; a busy project answers 409, so it waits and tries again, like a plugin.
async function installSkill(ep: Endpoint, s: CatalogSkill, onWait: () => void): Promise<boolean> {
  const until = Date.now() + RETRY_FOR_MS;
  for (;;) {
    try { await post(ep, "/api/skills/add", { id: s.id, commit: true }); return true; }
    catch (e) {
      if (!(e instanceof HttpError && e.status === 409) || Date.now() > until) throw e;
      onWait();
      await sleep(RETRY_MS);
    }
  }
}

/// The dialog. Items stay listed after they install, marked done, until it is closed.
export function RecommendedDialog({ close }: { close: () => void }) {
  const ep = useStore((s) => (s.active ? s.projects[s.lanes[s.active]?.project ?? ""]?.endpoint : undefined));
  const rec = useRecommendations();
  const [status, setStatus] = useState<Record<string, Status>>({});
  const [shown] = useState(() => new Map<string, Item>());
  const set = (key: string, st: Status) => setStatus((all) => ({ ...all, [key]: st }));
  if (ep) {
    for (const s of rec.skills) shown.set(`skill:${s.id}`, { key: `skill:${s.id}`, name: s.id, kind: "Skill", why: s.recommended ?? "", description: s.description, install: () => installSkill(ep, s, () => set(`skill:${s.id}`, { phase: "waiting", text: "Waits for the turn in this project to finish…" })) });
    for (const p of rec.plugins) shown.set(`plugin:${p.name}`, { key: `plugin:${p.name}`, name: p.name, kind: "Plugin", why: p.reason ?? "", description: p.description, install: () => installPlugin(ep, p) });
  }
  const items = [...shown.values()];
  async function install(item: Item) {
    if (status[item.key]?.phase === "working" || status[item.key]?.phase === "done") return;
    set(item.key, { phase: "working", text: "Installing…" });
    try {
      const ok = await item.install();
      set(item.key, ok ? { phase: "done", text: "Installed" } : { phase: "failed", text: runOf<PluginRun>(`recommended:plugin:${item.name}`)?.state.last ?? "Didn't install." });
    } catch (e) {
      set(item.key, { phase: "failed", text: e instanceof Error ? e.message : String(e) });
    }
    window.dispatchEvent(new Event(SKILLS_CHANGED));
  }
  const pending = items.filter((i) => status[i.key]?.phase !== "done");
  async function installAll() {
    for (const item of pending) await install(item);
  }
  return <Dialog title="Recommended for this project" onClose={close}>
    <div className="rec">
      {!rec.read && !items.length ? <p className="muted">Reading what this project has…</p>
        : !items.length ? <p className="muted">Nothing to add — this project has everything Oya Factory recommends.</p>
        : <ul className="rec-list">{items.map((item) => <RecRow key={item.key} item={item} status={status[item.key]} install={() => void install(item)} />)}</ul>}
      <div className="actions">
        {pending.length > 1 && <button className="primary" disabled={pending.some((i) => status[i.key]?.phase === "working" || status[i.key]?.phase === "waiting")} onClick={() => void installAll()}>Install all ({pending.length})</button>}
        <button onClick={close}>Done</button>
      </div>
    </div>
  </Dialog>;
}

/// One recommendation: what it is, why, and its own Install with the live status of the run.
function RecRow({ item, status, install }: { item: Item; status?: Status; install: () => void }) {
  const run = useRun<PluginRun>(`recommended:plugin:${item.name}`);
  const live = item.kind === "Plugin" && run?.running ? { phase: run.state.waiting ? "waiting" : "working", text: run.state.waiting ? "Waits for the turn in this project to finish…" : "Installing…" } as Status : status;
  const phase = live?.phase ?? "idle";
  return <li className={`rec-row ${phase}`}>
    <div className="rec-main">
      <p className="rec-name"><strong>{item.name}</strong><span className="rec-kind">{item.kind}</span></p>
      {item.why && <p className="rec-why">{item.why}</p>}
      <p className="rec-desc" title={item.description}>{item.description}</p>
      {live?.text && phase !== "idle" && <p className={`rec-status ${phase}`} role={phase === "failed" ? "alert" : "status"}>{phase === "done" ? "✓ " : ""}{live.text}</p>}
    </div>
    <button className={phase === "done" ? "" : "primary"} disabled={phase === "working" || phase === "waiting" || phase === "done"} onClick={install}>{phase === "done" ? "Installed" : phase === "failed" ? "Try again" : phase === "working" || phase === "waiting" ? "Installing…" : "Install"}</button>
  </li>;
}
