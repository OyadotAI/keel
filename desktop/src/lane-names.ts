// Durable lane/session naming, serialized saves, and protection from stale list responses.
import { post, type Endpoint } from "./api";
import type { Agent, Lane, Project, Session } from "./store";

export interface Naming {
  revision: number;
  source?: string | null;
  pending?: string;
  edit?: string;
  pendingLegacy?: boolean;
  legacy?: string;
}
interface Dependencies {
  read(): { lanes: Record<string, Lane>; projects: Record<string, Project> };
  lane(id: string, patch: Partial<Lane>): void;
  project(path: string, patch: Partial<Project>): void;
  endpoint(path: string): Promise<Endpoint>;
  persist(): boolean | void;
  label(provider: Agent): string;
}
export const SESSION_NAME_LIMIT = 80;
const PLACEHOLDERS = new Set(["New lane", "New branch", "Untitled session"]);
const identity = (provider: Agent | undefined, session: string) => `${provider ?? "claude"}:${session}`;

/// Older layouts had only a label; infer migration intent once, leaving generated placeholders alone.
export function restoredNaming(title: string, saved?: Naming): Naming {
  return saved ?? { revision: 0, ...(!PLACEHOLDERS.has(title) && !title.startsWith("keel/") ? { legacy: title } : {}) };
}

/// Own asynchronous naming independently from terminal activity and turn submission.
export function createLaneNames(d: Dependencies) {
  const receipts = new Map<string, Session>();
  const saves = new Map<string, Promise<boolean>>();
  const registrations = new Map<string, Promise<void>>();
  const key = (l: Lane) => identity(l.agent, l.session ?? l.id);
  const error = (l: Lane) => renameError(d.label(l.agent ?? "claude"));

  const guarded = (s: Session) => guardedSession(d, receipts, s);
  const adopt = (lid: string, session: Session) => adoptTitle(d, lid, session, rename);
  function publish(path: string, raw: Session) {
    const session = guarded(raw);
    for (const l of Object.values(d.read().lanes)) if (matches(l, session)) adopt(l.id, session);
    const p = d.read().projects[path];
    if (p) d.project(path, { sessions: [...(p.sessions ?? []).filter((s) => identity(s.provider, s.id) !== identity(session.provider, session.id)), session] });
    d.persist();
  }
  async function write(lid: string, requested: Lane, name: string, legacy: boolean, edit: string): Promise<boolean> {
    try {
      const ep = await d.endpoint(requested.project);
      const session = await post<Session>(ep, "/api/session/rename", { id: requested.session, provider: requested.agent ?? "claude", wt: requested.wt, name, legacy });
      const current = d.read().lanes[lid];
      if (current && matches(current, session) && current.naming?.edit === edit) {
        d.lane(lid, { naming: { revision: session.title_revision ?? 0, source: session.title_source }, nameSaving: false, nameError: undefined });
      }
      publish(requested.project, session);
      return true;
    } catch {
      const current = d.read().lanes[lid];
      if (current?.naming?.edit === edit) { d.lane(lid, { nameSaving: false, nameError: error(current) }); d.persist(); }
      return false;
    }
  }
  async function rename(lid: string, raw: string, legacy = false): Promise<boolean> {
    const l = d.read().lanes[lid]; const name = raw.trim();
    if (!l || !name) return false;
    if (l.nameLocked) return false;
    if (!validName(name)) {
      d.lane(lid, { nameError: "A session name must be one line and at most 80 characters." }); return false;
    }
    const edit = crypto.randomUUID();
    d.lane(lid, { title: name, naming: { revision: l.naming?.revision ?? 0, source: l.naming?.source, pending: name, edit, pendingLegacy: legacy }, nameSaving: true, nameError: undefined });
    try { if (d.persist() === false) throw new Error("Name intent was not saved"); } catch { d.lane(lid, { nameSaving: false, nameError: error(l) }); return false; }
    // Codex and Antigravity choose the real id on the first prompt. Keep the intent with its lane.
    if (!l.known && l.agent !== "claude") { d.lane(lid, { nameSaving: false }); return true; }
    const id = key(l);
    const job = (saves.get(id) ?? Promise.resolve(true)).then(() => write(lid, l, name, legacy, edit));
    trackSave(saves, id, job);
    return job;
  }
  async function register(lid: string, force = false): Promise<void> {
    const l = d.read().lanes[lid];
    if (!l?.session) return;
    if (!l.known && (!force || l.agent !== "claude")) return void (l.agent === "claude" && l.naming?.pending !== undefined && await rename(lid, l.naming.pending, l.naming.pendingLegacy));
    const id = key(l);
    if (registrations.has(id)) return registrations.get(id);
    const job = (async () => {
      try {
        const ep = await d.endpoint(l.project);
        const session = await post<Session>(ep, "/api/session/register", { id: l.session, provider: l.agent ?? "claude", wt: l.wt });
        const retry = recoveredPending(d, lid, session, l.naming?.edit);
        publish(l.project, session);
        if (retry) await rename(lid, retry.name, retry.legacy);
      } catch {
        const current = d.read().lanes[lid];
        if (current) d.lane(lid, { nameError: `Couldn’t refresh the ${d.label(l.agent ?? "claude")} session name. Reconnect to retry.` });
      }
    })().finally(() => registrations.delete(id));
    registrations.set(id, job);
    return job;
  }
  function reconcile(path: string, list: Session[]): Session[] {
    const resolved = list.map(guarded);
    for (const l of Object.values(d.read().lanes).filter((lane) => lane.project === path)) {
      const s = resolved.find((s) => matches(l, s));
      if (s) adopt(l.id, s);
    }
    d.persist();
    return resolved;
  }
  async function flush(lid: string): Promise<boolean> {
    let l = d.read().lanes[lid];
    if (!l) return true;
    if (l.naming?.legacy !== undefined) { await register(lid, true); l = d.read().lanes[lid]; }
    if (!l) return true;
    if (unboundName(l)) {
      d.lane(lid, { nameError: "Start this conversation before closing so its name can be saved in History." }); return false;
    }
    const id = key(l);
    let job = saves.get(id);
    if (l.naming?.pending !== undefined && !l.nameSaving) job = rename(lid, l.naming.pending, l.naming.pendingLegacy);
    while (job) {
      if (!await job && d.read().lanes[lid]?.naming?.pending !== undefined) return false;
      const latest = saves.get(id);
      if (latest === job) break;
      job = latest;
    }
    l = d.read().lanes[lid];
    if (l?.naming?.source === "pending") {
      await register(lid, true); l = d.read().lanes[lid];
      if (l?.naming?.source === "pending") {
        d.lane(lid, { nameError: l.nameError ?? "Start this conversation before closing so its name can be saved in History." }); return false;
      }
    }
    return l?.naming?.pending === undefined;
  }
  return { rename, register, reconcile, flush };
}

/// Copy the native resume command for the History entry's provider, without changing its identity.
export function resumeCommand(session: Session): string {
  const command = { claude: "claude --resume", codex: "codex resume", antigravity: "agy --conversation" };
  return `${command[session.provider ?? "claude"]} ${session.id}`;
}

function validName(name: string): boolean {
  return [...name].length <= SESSION_NAME_LIMIT && [...name].every((c) => c.codePointAt(0)! >= 32 && c.codePointAt(0) !== 127);
}
function renameError(provider: string): string {
  return `Couldn’t rename the ${provider} session. Rename again to retry.`;
}

function adoptTitle(d: Dependencies, lid: string, session: Session, rename: (id: string, name: string, legacy?: boolean) => Promise<boolean>) {
  const l = d.read().lanes[lid];
  if (!l || !matches(l, session) || l.naming?.pending !== undefined) return;
  const legacy = l.naming?.legacy;
  const source = session.title_source;
  if (legacy !== undefined) {
    d.lane(lid, { naming: { ...l.naming, revision: l.naming?.revision ?? 0, legacy: undefined } });
    if (source !== "explicit" && source !== "pending" && legacy !== session.title) {
      void rename(lid, legacy, true); return;
    }
  }
  const revision = session.title_revision ?? 0;
  if (revision < (l.naming?.revision ?? 0)) return;
  const title = session.title || (l.known ? "Untitled session" : l.title);
  d.lane(lid, { title, known: l.known || (!!source && source !== "pending" && revision > 0), nameError: source === "pending" ? l.nameError : undefined, naming: { revision, source } });
}

const matches = (l: Lane, s: Session) => l.session === s.id && (l.agent ?? "claude") === (s.provider ?? "claude");

/// Match session metadata by provider as well as id; unrelated providers may use equal ids.
export function sessionForLane(lane: Lane, sessions?: Session[]): Session | undefined {
  return sessions?.find((session) => matches(lane, session));
}

/** Reconnect retires acknowledged or superseded intent, while first-ID binding preserves an early name. */
function recoveredPending(d: Dependencies, lid: string, session: Session, edit?: string): { name: string; legacy?: boolean } | undefined {
  const l = d.read().lanes[lid]; const naming = l?.naming;
  if (!l || naming?.pending === undefined || naming.edit !== edit || l.nameSaving) return;
  const explicit = session.title_source === "explicit";
  const acknowledged = session.title === naming.pending && (explicit || session.title_source === "pending");
  const superseded = explicit && naming.revision > 0 && (session.title_revision ?? 0) > naming.revision;
  if (acknowledged || superseded) {
    d.lane(lid, { naming: { revision: session.title_revision ?? 0, source: session.title_source }, nameSaving: false, nameError: undefined });
    return;
  }
  return { name: naming.pending, legacy: naming.pendingLegacy };
}

function guardedSession(d: Dependencies, receipts: Map<string, Session>, s: Session): Session {
  const id = identity(s.provider, s.id);
  const lane = Object.values(d.read().lanes).find((l) => matches(l, s) && (l.naming?.revision ?? 0) > (s.title_revision ?? 0));
  const previous = receipts.get(id) ?? (lane ? { ...s, title: lane.title, title_source: lane.naming?.source, title_revision: lane.naming?.revision } : undefined);
  if (previous && (previous.title_revision ?? 0) > (s.title_revision ?? 0)) {
    return { ...s, title: previous.title, title_source: previous.title_source, title_revision: previous.title_revision };
  }
  receipts.set(id, s);
  return s;
}

function unboundName(lane: Lane): boolean {
  return !lane.known && lane.agent !== "claude" && (lane.naming?.pending !== undefined || lane.naming?.legacy !== undefined);
}

/** Settled jobs cannot block a later recovered name or retain a closed session indefinitely. */
function trackSave(saves: Map<string, Promise<boolean>>, id: string, job: Promise<boolean>): void {
  saves.set(id, job);
  const retire = () => { if (saves.get(id) === job) saves.delete(id); };
  void job.then(retire, retire);
}
