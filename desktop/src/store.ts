// Everything the window shows, in one store. Projects hold lanes; a lane holds its conversation.
//
// Views read with narrow selectors (`useStore(s => s.lanes[id]?.conv.turns[i].blocks[step])`), so a
// streamed token re-renders the one block it landed in. Updates arrive one batch per frame from
// the stream worker, never one per event.

import { readSetup, type Meter, type Readiness, type Warning } from "./setup";
import { create } from "zustand";
import { invoke } from "@tauri-apps/api/core";
import { type Endpoint, get, post, url } from "./api";
import { applyFact, applyFrames, restoreTranscript, type Conversation, type Fact, type Frame } from "./reduce";
import { open as openStream, type Stream } from "./streams";
import { endBanner, needsYou, notify } from "./notify";

import { typeInto, interruptTerminal, terminalConnected, exitTerminal } from "./typers";
import { refreshReview } from "./prereview";
import { closeTab, onBrowser, type Op as BrowserOp } from "./browser";
import { inArea, prepare, type DiffResponse, type GitStatus, type Prepared } from "./git";

export interface Pending {
  id: string;
  lane: string;
  tool: string;
  command: string;
  rules: string[];
  input: Record<string, unknown>;
  session_id: string;
  /// Why Keel is asking, when it is not the usual reason.
  reason?: string;
  /// Asked by a subagent the lane's turn started; its own steps are not in the turn.
  subagent?: boolean;
  provider?: Agent;
  kind?: "permission" | "question" | "elicitation";
  choices?: { label: string; value: unknown }[];
}

export interface Job {
  id: string;
  lane: string;
  command: string;
  dir: string;
  started: number;
  finished?: number | null;
  exit?: number | null;
  log: string[];
  /// The first page on this machine the job announced, kept apart from the log's last lines.
  url?: string | null;
  reported: boolean;
}

export interface Session {
  id: string;
  title?: string | null;
  branch?: string | null;
  last_active?: string | null;
  messages: number;
  live: boolean;
  busy: boolean;
}

export interface Dev {
  running: boolean;
  url?: string | null;
  command?: string | null;
  elsewhere: boolean;
  owner: string;
  detected?: string | null;
  log: string[];
}

/// What a finished background job says when it is handed back to the conversation. The prefix
/// is how the turn is drawn as a report from the machine rather than as something you typed.
export const JOB_PREFIX = "Background job ";

export interface Project {
  path: string;
  name: string;
  /// Absent until the daemon is up. Daemons start when a project is first used, not at launch.
  endpoint?: Endpoint;
  starting?: boolean;
  error?: string;
  lanes: string[];
  collapsed?: boolean;
  /// Claude Code's sessions in this project, for History. Titles and counts, never contents.
  sessions?: Session[];
  history?: boolean;
}

export type ChatInterface = "chat" | "terminal";
export type Agent = "claude" | "codex" | "antigravity";
export const AGENT_NAME: Record<Agent, string> = { claude: "Claude", codex: "Codex", antigravity: "Antigravity" };
export const agentName = (agent?: Agent) => AGENT_NAME[agent ?? "claude"];

export interface Attachment { path: string; name: string; mime?: string; preview?: string }
export interface QueuedMessage { id: string; text: string; attachments: Attachment[]; system?: string }

function makeReceipt(lane: Lane, prompt: string, attachments: Attachment[], wt?: string, system?: string): NonNullable<Lane["receipt"]> {
  return { id: crypto.randomUUID(), prompt, session: lane.known ? lane.session : undefined, mode: lane.mode ?? "plan", model: lane.model, wt, system, attachments: attachments.map(({ path, name, mime }) => ({ path, name, mime })) };
}

function isComposerSubmission(lane: Lane, prompt: string, attachments: Attachment[]): boolean {
  return prompt === lane.draft || (!lane.draft?.trim() && attachments.length > 0 && attachments === lane.attachments);
}

export interface Lane {
  /// Invalidate manual verification when this checkout receives a filesystem or git change.
  reviewRevision?: number;
  context?: import("./context").ContextSnapshot;
  id: string;
  project: string;
  interface?: ChatInterface;
  title: string;
  /// The conversation, chosen by Keel when the lane is made so the transcript to follow is known
  /// before anything is typed.
  session?: string;
  /// Which CLI the lane's terminal runs.
  agent: Agent;
  /// The transcript exists: the next launch resumes the conversation instead of starting it.
  known: boolean;
  /// The conversation is running in a `claude` Keel did not start — a terminal, another app. The
  /// lane follows it and starts nothing until the person takes it over: two processes driving one
  /// conversation is two agents writing one transcript.
  elsewhere?: boolean;
  /// On a branch and checkout of its own (`keel/<wt>`), made on the first send.
  isolated: boolean;
  wt?: string;
  git?: GitStatus;
  /// The file whose diff is open, and the diff, prepared once.
  viewing?: string;
  viewingStaged?: boolean;
  diff?: Prepared;
  diffError?: string;
  /// A discard refused because it would lose work, with what it would lose.
  confirmDiscard?: string;
  /// The checkout was rewound to before a turn; `undo` puts it back.
  rewound?: { number: number; undo: string };
  /// Commands the daemon is running for this lane, outliving the turn that started them.
  jobs: Job[];
  dev?: Dev;
  /// Prompts waiting for the turn in flight to end — a finished job's report, mainly. Drained by
  /// every way a turn ends, not just the happy one.
  queued: QueuedMessage[];
  attachments?: Attachment[];
  conv: Conversation;
  running: boolean;
  loaded: boolean;
  error?: string;
  pending: Pending[];
  draft?: string;
  mode?: "plan" | "acceptEdits";
  model?: string;
  models?: { value?: string; id?: string; displayName?: string; display_name?: string; name?: string }[];
  commands?: string[];
  managed?: boolean;
  /// A terminal turn the follower saw open live and has not seen close. Claude Code's pid file is
  /// the other witness, and it can be wrong: 2.1.306 left a session `idle` through a whole turn
  /// typed after an interruption, and the lane, the status bar and Turns all read idle while the
  /// agent ran commands. Only a live open counts, and the session ending clears it.
  transcriptOpen?: boolean;
  cursor?: number;
  submitting?: boolean;
  disconnected?: boolean;
  receipt?: { id: string; prompt: string; session?: string; mode: string; model?: string; wt?: string; system?: string; attachments?: Attachment[] };
}

interface State {
  chatInterface: ChatInterface;
  setChatInterface(value: ChatInterface): void;
  switchInterface(lane: string, value: ChatInterface): Promise<void>;
  order: string[];
  projects: Record<string, Project>;
  lanes: Record<string, Lane>;
  active?: string;
  /// Settings over the lane, for the active lane's project. Not saved: a relaunch opens on work.
  settings: boolean;
  /// The new-project page over the window.
  creating: boolean;
  /// A confirmation open for the active lane: merge, discard or close.
  asking: "merge" | "discard" | "close" | null;
  /// The lane whose sidebar row is an input right now — set by F2, a double-click, the menu or the palette.
  renaming: string | null;
  shortcuts: boolean;
  /// The extensions page — skills, subagents, MCP servers, plugins — over the window.
  extensions: boolean;
  extensionsSection?: "agents";
  /// Whether each project is trusted, for the marker that says so while it holds.
  trusted: Record<string, boolean>;
  /// What is missing from each project's agent setup, for the warning that says so.
  setup: Record<string, Warning[]>;
  /// The license in force. Machine-wide, so one value whichever project's daemon read it last.
  license?: Meter;
  /// Open readiness findings per project, for the tab's count.
  readiness: Record<string, Readiness>;
  /// Setup warnings "Install all" handed to a lane's agent, per project, so the popover says they
  /// are with the agent rather than missing. Not saved: the next scan is the truth either way.
  handed: Record<string, { lane: string; ids: string[] }>;
  /// A side-panel tab asked for from outside the window's own state (the sidebar's setup list).
  /// The window shows it and clears the request.
  panelTab?: "turns" | "git" | "review" | "jobs" | "preview" | "readiness" | "context" | "browser";
  /// The project's daemon, started if it is not running. For views that talk to it directly.
  ensure(project: string): Promise<Endpoint>;
  openProject(path: string): Promise<void>;
  closeProject(path: string): Promise<void>;
  newLane(project: string, isolated?: boolean, agent?: Agent, surface?: ChatInterface): string;
  /// Lanes whose terminal is running this session. Kept mounted when not on screen: closing a
  /// terminal ends the agent in it, and switching lanes must never stop a turn.
  opened: string[];
  /// Make sure a lane's checkout exists before its terminal starts in it.
  prepare(lane: string): Promise<{ ep: Endpoint; wt?: string } | undefined>;
  /// The lane's terminal is connected: type whatever was waiting for it.
  connected(lane: string): void;
  closeLane(lane: string): void;
  refreshGit(lane: string): Promise<void>;
  openDiff(lane: string, path?: string, staged?: boolean): Promise<void>;
  merge(lane: string): Promise<void>;
  refreshJobs(lane: string): Promise<void>;
  stopJob(lane: string, id: string): Promise<void>;
  refreshDev(lane: string): Promise<void>;
  dev(lane: string, start: boolean): Promise<void>;
  resume(project: string, session: Session): void;
  discard(lane: string, force: boolean): Promise<void>;
  select(lane: string): void;
  /// Read a lane's conversation back if it has one and it is not on screen yet.
  load(lane: string): void;
  send(lane: string, prompt: string, attachments?: Attachment[], system?: string): Promise<boolean>;
  stop(lane: string): Promise<void>;
  editQueued(lane: string, id: string): void;
  removeQueued(lane: string, id: string): void;
  draft(lane: string, text: string): void;
  /// `scope`: "once" lets this call through; "session" and "project" also remember `p.rules`.
  answer(lane: string, p: Pending, decision: "allow" | "deny", answer?: string, scope?: "once" | "session" | "project"): Promise<void>;
}

const SAVED = "keel.layout.v1";
const empty: Conversation = { turns: [] };
const watchers = new Map<string, Stream>();

interface Saved {
  order: string[];
  projects: { path: string; lanes: string[]; collapsed?: boolean }[];
  lanes: { id: string; project: string; title: string; session?: string; isolated?: boolean; wt?: string; agent?: Agent; known?: boolean; queued?: QueuedMessage[]; attachments?: Attachment[]; interface?: ChatInterface; draft?: string; model?: string; mode?: "plan" | "acceptEdits"; managed?: boolean; receipt?: Lane["receipt"] }[];
  active?: string;
}

function load(): Pick<State, "order" | "projects" | "lanes" | "active"> {
  try {
    const s = JSON.parse(localStorage.getItem(SAVED) ?? "null") as Saved | null;
    if (!s) return { order: [], projects: {}, lanes: {} };
    const projects: Record<string, Project> = {};
    for (const p of s.projects) projects[p.path] = { ...p, name: basename(p.path) };
    const lanes: Record<string, Lane> = {};
    for (const l of s.lanes) {
      // A lane saved before lanes were the CLI has no session of its own yet; it gets one.
      lanes[l.id] = { ...l, interface: l.interface ?? "terminal", session: l.session ?? crypto.randomUUID(), agent: l.agent ?? "claude", known: !!l.known, isolated: !!l.isolated, jobs: [], queued: l.queued ?? [], conv: empty, running: false, loaded: !l.session, pending: [] };
    }
    return { order: s.order.filter((p) => projects[p]), projects, lanes, active: s.active };
  } catch {
    return { order: [], projects: {}, lanes: {} };
  }
}

function save(s: State) {
  const out: Saved = {
    order: s.order,
    projects: s.order.map((p) => ({ path: p, lanes: s.projects[p].lanes, collapsed: s.projects[p].collapsed })),
    lanes: Object.values(s.lanes).map(({ id, project, title, session, isolated, wt, agent, known, interface: surface, attachments, queued, draft, model, mode, managed, receipt }) => ({ id, project, title, session, isolated, wt, agent, known, interface: surface, queued: queued.map((q) => ({ ...q, attachments: q.attachments.map(({ path, name, mime }) => ({ path, name, mime })) })), attachments: attachments?.map(({ path, name, mime }) => ({ path, name, mime })), draft, model, mode, managed, receipt })),
    active: s.active,
  };
  try {
    localStorage.setItem(SAVED, JSON.stringify(out));
  } catch {
    // Storage full or blocked: the layout is a convenience, the work is in git.
  }
}

export function basename(path: string): string {
  return path.split(/[\\/]/).filter(Boolean).pop() ?? path;
}

const id = () => crypto.randomUUID();

export const useStore = create<State>()((set, getState) => {
  const lane = (lid: string, patch: Partial<Lane> | ((l: Lane) => Partial<Lane>)) =>
    set((s) => {
      const l = s.lanes[lid];
      if (!l) return s;
      return { lanes: { ...s.lanes, [lid]: { ...l, ...(typeof patch === "function" ? patch(l) : patch) } } };
    });
  /// Patch a project only while it is still open: a daemon finishing its start after the project
  /// was closed must not bring a half-entry back.
  const project = (path: string, patch: Partial<Project>) =>
    set((s) => (s.projects[path] ? { projects: { ...s.projects, [path]: { ...s.projects[path], ...patch } } } : s));

  const starting = new Map<string, Promise<Endpoint>>();

  /// The daemon for a project, started on first use. One start per project at a time: two
  /// callers asking at once — a lane replaying while the project opens — share it.
  function endpoint(path: string): Promise<Endpoint> {
    const p = getState().projects[path];
    if (!p) return Promise.reject(new Error("That project is closed."));
    if (p.endpoint) return Promise.resolve(p.endpoint);
    const inflight = starting.get(path);
    if (inflight) return inflight;
    project(path, { starting: true, error: undefined });
    const start = invoke<Endpoint>("open_project", { path })
      .then((ep) => {
        if (!getState().projects[path]) {
          // Closed while it started: stop the daemon that just came up.
          void invoke("close_project", { path });
          throw new Error("That project is closed.");
        }
        project(path, { endpoint: ep, starting: false });
        watch(path, ep);
        return ep;
      })
      .catch((e) => {
        project(path, { starting: false, error: String(e) });
        throw e instanceof Error ? e : new Error(String(e));
      })
      .finally(() => starting.delete(path));
    starting.set(path, start);
    return start;
  }

  /// The daemon stopped answering. Forget its address, so the next use asks the shell, which
  /// starts it again if it died and hands back the same one if it did not.
  function lost(path: string, ep: Endpoint) {
    const p = getState().projects[path];
    if (p?.endpoint?.port === ep.port) project(path, { endpoint: undefined });
  }

  const trust = (path: string, ep: Endpoint) =>
    void get<{ trusted: boolean }>(ep, "/api/permissions")
      .then((v) => set((s) => ({ trusted: { ...s.trusted, [path]: v.trusted } })))
      .catch(() => undefined);
  const setup = (path: string, ep: Endpoint) =>
    soon(`setup:${path}`, () =>
      readSetup(ep, path).then(
        ({ warnings, readiness, license }) =>
          set((s) => ({ setup: { ...s.setup, [path]: warnings }, readiness: readiness ? { ...s.readiness, [path]: readiness } : s.readiness, license: license ?? s.license })),
        () => undefined,
      ),
    );
  const lanesOf = (path: string) => getState().projects[path]?.lanes ?? [];
  /// Coarse on purpose: "this checkout changed, read it again". A `tree.changed` names its
  /// checkout — `wt` absent for the project's own tree — and only the lanes on it read.
  const checkoutChanged = (onlyTree: boolean) => (path: string, _ep: Endpoint, data: unknown) => {
    const wt = (data as { wt?: string | null })?.wt ?? undefined;
    for (const lid of lanesOf(path)) {
      const l = getState().lanes[lid];
      if (l && l.wt === wt) lane(lid, { reviewRevision: (l.reviewRevision ?? 0) + 1 });
      if (l && (!onlyTree || l.wt === wt)) gitSoon(lid);
    }
  };
  /// Whether a lane is working is Claude Code's own word for it, from its pid file.
  function sessionsChanged(path: string, list: Session[]) {
    project(path, { sessions: list });
    for (const lid of lanesOf(path)) {
      const l = getState().lanes[lid];
      // Claude's list says nothing about a Codex lane, and read as "not busy" it set a working
      // one idle every few seconds and drained its queue mid-turn. Its transcript says instead.
      if (l && (l.agent ?? "claude") !== "claude") continue;
      const s = l && list.find((x) => x.id === l.session);
      const busy = !!s && s.live && (s.busy || !!l.transcriptOpen);
      // A session that ended took its background agents with it.
      if (l && s && !s.live) forgetEnded(lid, l);
      if (!l || (l.managed && l.interface !== "terminal") || l.submitting || (l.running === busy && (!s || l.known))) continue;
      // Idle again: whatever it was asking is no longer being waited on — Esc in the terminal,
      // or the hook's own wait ran out — and what was queued goes next.
      lane(lid, { running: busy, known: l.known || !!s, ...(!busy && l.running ? { pending: [] } : {}) });
      save(getState());
      // Followed from the moment it works, not from the moment it is opened: the lane list says
      // what a terminal is doing, and it can only say what the transcript has been read for.
      if (busy) void follow(lid);
      if (!busy && l.running) {
        // No banner here: the turn's own `turn.ended` fact says it, once, with the checks' verdict.
        gitSoon(lid);
        drain(lid);
      }
    }
  }
  /// What each daemon event means here — one entry per event, so adding one is a line.
  const on: Record<string, (path: string, ep: Endpoint, data: unknown) => void> = {
    pending: (path, _ep, data) => {
      const d = (data as { data?: { lane?: string; withdrawn?: string } })?.data;
      // The hook that asked is gone — stopped, or timed out — so its card is a question nobody
      // is waiting on any more. The daemon has already dropped it; so does every lane here.
      if (d?.withdrawn) {
        for (const lid of lanesOf(path)) lane(lid, (l) => (l.pending.some((p) => p.id === d.withdrawn) ? { pending: l.pending.filter((p) => p.id !== d.withdrawn) } : {}));
        return;
      }
      void poll(path, d?.lane);
    },
    // A Codex or Antigravity terminal named its conversation: the lane resumes it from now on,
    // and can switch to the chat view. Never a running Claude lane's — Keel chose that id.
    "terminal.session": (path, _ep, data) => {
      const d = (data as { data?: { lane?: string; session?: string } })?.data;
      const l = d?.lane ? getState().lanes[d.lane] : undefined;
      if (!d?.session || !l || l.project !== path || l.agent === "claude" || l.interface !== "terminal" || l.known) return;
      lane(l.id, { session: d.session, known: true });
      save(getState());
      // Followed now, whichever lane is on screen: the sidebar says what it is doing.
      void follow(l.id);
    },
    "context.changed": (path, _ep, data) => {
      const d = (data as { data?: { lane: string; snapshot: import("./context").ContextSnapshot } })?.data;
      const current = d && getState().lanes[d.lane];
      if (d && current?.project === path && current.agent === d.snapshot.provider && current.known && current.session === d.snapshot.session) lane(d.lane, { context: d.snapshot });
    },
    "review.changed": (path, _ep, data) => {
      const wt = (data as { wt?: string | null })?.wt ?? undefined;
      for (const lid of lanesOf(path)) {
        const current = getState().lanes[lid];
        if (current && current.wt === wt) lane(lid, { reviewRevision: (current.reviewRevision ?? 0) + 1 });
      }
    },
    "permissions.changed": (path, ep) => trust(path, ep),
    "state.changed": (path, ep) => setup(path, ep),
    connected: (path, ep) => {
      // Whatever happened in the gap is gone: read everything this daemon owns, once.
      trust(path, ep);
      setup(path, ep);
      void poll(path);
      // Through `sessionsChanged`, not straight into the project: what ended in the gap — a turn,
      // a session and the background agents it took with it — is what a reconnect is for.
      void get<Session[]>(ep, "/api/sessions").then((sessions) => sessionsChanged(path, sessions), () => undefined);
      for (const lid of lanesOf(path)) {
        lane(lid, (l) => ({ reviewRevision: (l.reviewRevision ?? 0) + 1 }));
        gitSoon(lid);
        soon(`jobs:${lid}`, () => getState().refreshJobs(lid));
        void refreshReview(lid, ep, getState().lanes[lid]?.wt);
      }
    },
    // The lane's Browser tab: the agent asked the daemon, the daemon asks the window that has it.
    browser: (path, ep, data) => {
      const d = (data as { data?: { id: string; lane: string; op: BrowserOp } })?.data;
      // Only for the lane on screen: another lane's page opening does not move the panel.
      const reveal = (lid: string) => { if (getState().active === lid) set({ panelTab: "browser" }); };
      if (d) void onBrowser(path, ep, (lid) => lanesOf(path).includes(lid), d, reveal);
    },
    // The review before a pull request: the daemon's record, re-read for the lane it names. A
    // review filed under no lane belongs to whoever asks, so every lane reads it.
    "reviews.changed": (path, ep, data) => {
      const only = (data as { data?: { lane?: string } })?.data?.lane;
      for (const lid of lanesOf(path)) if (!only || only === lid) void refreshReview(lid, ep, getState().lanes[lid]?.wt);
    },
    "monitors.changed": (path, _ep, data) => {
      const only = (data as { data?: { lane?: string } })?.data?.lane;
      for (const lid of lanesOf(path)) if (!only || only === lid) soon(`jobs:${lid}`, () => getState().refreshJobs(lid));
    },
    lagged: (path, ep) => on.connected(path, ep, undefined),
    "dev.changed": (path) => {
      const lid = getState().active;
      if (lid && getState().lanes[lid]?.project === path) soon(`dev:${lid}`, () => getState().refreshDev(lid));
    },
    sessions: (path, _ep, data) => {
      const list = (data as { data?: Session[] })?.data;
      if (Array.isArray(list)) sessionsChanged(path, list);
    },
    "tree.changed": checkoutChanged(true),
    "git.changed": checkoutChanged(false),
    "worktrees.changed": checkoutChanged(false),
  };

  /// One event stream per project: approvals that need an answer, mainly.
  function watch(path: string, ep: Endpoint) {
    watchers.get(path)?.close();
    const stream = openStream(
      url(ep, "/api/events"),
      ep.token,
      (events) => {
        for (const e of events) on[e.event]?.(path, ep, e.data);
      },
      () => {
        if (watchers.get(path) !== stream) return;
        watchers.delete(path);
        // Reconnect through `endpoint`, which respawns a daemon that died rather than knocking
        // on its port forever. Stops when the project is closed.
        setTimeout(() => {
          if (!getState().projects[path] || watchers.has(path)) return;
          lost(path, ep);
          void endpoint(path).catch(() => undefined);
        }, 1000);
      },
    );
    watchers.set(path, stream);
  }

  const timers = new Map<string, ReturnType<typeof setTimeout>>();
  /// Once, shortly: several events about one thing are one read.
  function soon(key: string, run: () => unknown) {
    if (timers.has(key)) return;
    timers.set(
      key,
      setTimeout(() => {
        timers.delete(key);
        void run();
      }, 300),
    );
  }

  /// A turn ended — any way at all — or the lane's terminal came up. What was queued goes next.
  function drain(lid: string) {
    const l = getState().lanes[lid];
    if (!l || !l.queued.length) return;
    // The CLI queues typed input itself; anything else waits for the turn to end.
    if (l.running || l.submitting || l.disconnected || l.receipt) return;
    const [next, ...rest] = l.queued;
    lane(lid, { queued: rest });
    void getState().send(lid, next.text, next.attachments, next.system);
  }

  const gitTimers = new Map<string, ReturnType<typeof setTimeout>>();
  /// Read git again shortly: a save is several events, and a turn is many.
  function gitSoon(lid: string) {
    if (gitTimers.has(lid)) return;
    gitTimers.set(
      lid,
      setTimeout(() => {
        gitTimers.delete(lid);
        if (getState().active === lid) void getState().refreshGit(lid);
      }, 300),
    );
  }

  /// Questions waiting for a lane. The daemon hands each out once, so they are kept here.
  async function poll(path: string, only?: string) {
    const p = getState().projects[path];
    if (!p?.endpoint) return;
    for (const lid of p.lanes) {
      if (only && only !== lid) continue;
      const l = getState().lanes[lid];
      if (!l) continue;
      try {
        const got = await get<Pending[]>(p.endpoint, "/api/approve/poll", { lane: lid, session: l.session });
        if (got.length) {
          lane(lid, (l) => ({ pending: [...l.pending, ...got.filter((g) => !l.pending.some((x) => x.id === g.id))] }));
          for (const p of got) announceCard(lid, p);
        }
      } catch {
        // The next `pending` or reconnect asks again.
      }
    }
  }

  const follows = new Map<string, Stream>();
  const retries = new Map<string, number>();
  /// What a live batch of a terminal lane's transcript says about whether it is working: a turn
  /// that opened is running whatever Claude Code's pid file says, and one that closed is idle
  /// unless the pid file says busy — a file that never said busy sends no event to say idle.
  function transcriptSays(l: Lane, frames: Frame[]): { patch: Partial<Lane>; settled: boolean } {
    const ops = frames.map((f) => f.op);
    if (ops.includes("close")) {
      const s = getState().projects[l.project]?.sessions?.find((x) => x.id === l.session);
      if (s?.busy) return { patch: { transcriptOpen: false }, settled: false };
      return { patch: { transcriptOpen: false, running: false, pending: [] }, settled: l.running };
    }
    if (ops.includes("open")) return { patch: { transcriptOpen: true, running: true }, settled: false };
    return { patch: {}, settled: false };
  }

  /// A session that ended took its background agents with it, and has no turn open whatever its
  /// transcript was last seen doing.
  function forgetEnded(lid: string, l: Lane) {
    if (l.conv.background) lane(lid, (now) => ({ conv: { turns: now.conv.turns } }));
    if (l.transcriptOpen) lane(lid, { transcriptOpen: false });
  }

  /// A turn that ended while it was being watched: its lane, how long, and what the checks said.
  /// Nothing for a turn the person stopped — they were there.
  function announceEnd(lid: string, key: string | null) {
    const l = getState().lanes[lid];
    if (!l) return;
    const turns = l.conv.turns;
    const t = (key ? turns.find((x) => x.key === key) : undefined) ?? turns.at(-1);
    const said = t && endBanner(t);
    if (said) void notify(l.title, said);
  }

  function announceStop(lid: string, why: string) {
    const l = getState().lanes[lid];
    if (!l) return;
    void needsYou(l.title, `Stopped: ${why.split("\n")[0].slice(0, 140)}`);
  }

  /// What a batch of live records says to the person: cards, turns that ended, a turn that stopped.
  function announce(lid: string, live: { card?: Pending; fact?: Fact; fatal?: string }[]) {
    for (const x of live) {
      if (x.card) announceCard(lid, x.card);
      if (x.fact?.kind === "turn.ended") announceEnd(lid, x.fact.turn);
    }
    const fatal = [...live].reverse().find((x) => x.fatal)?.fatal;
    if (fatal) announceStop(lid, fatal);
  }

  /// A card the person has to answer, announced once whichever way it reached the window.
  const announced = new Set<string>();
  function announceCard(lid: string, p: Pending) {
    if (announced.has(p.id)) return;
    announced.add(p.id);
    const title = `${getState().lanes[lid]?.title ?? "A lane"} is waiting for you`;
    void needsYou(title, p.tool === "AskUserQuestion" ? "The agent asked a question." : `Allow ${p.tool}${p.command ? `: ${p.command.slice(0, 80)}` : ""}?`);
  }

  /// Whose transcripts Keel follows: Claude's anywhere, and a Codex or Antigravity terminal's —
  /// their chat lanes are Keel's own stream, read from the agent directly.
  function followable(l: Lane): boolean {
    return (l.agent ?? "claude") === "claude" || ((l.agent === "codex" || l.agent === "antigravity") && l.interface === "terminal");
  }

  /// Follow a lane's transcript for as long as the lane is open: every turn, as the CLI writes
  /// it, with Keel's facts about it — the files, the checks, the commit. A session nobody has
  /// typed into has no transcript yet; that is a wait, retried, never an error on screen.
  async function follow(lid: string) {
    const l = getState().lanes[lid];
    if (!l?.session || (l.managed && l.interface !== "terminal") || follows.has(lid) || !followable(l) || !l.known) return;
    let ep: Endpoint;
    try {
      ep = await endpoint(l.project);
    } catch (e) {
      lane(lid, { error: String(e) });
      return;
    }
    if (follows.has(lid) || !getState().lanes[lid]) return;
    let missing = false;
    let fresh = true;
    // Records before `caught-up` are the replay; only what arrives after it is news.
    let live = false;
    const stream = openStream(
      url(ep, "/api/session/tail", { id: l.session, from: 0, ops: 1, wt: l.wt, agent: l.agent ?? "claude" }),
      ep.token,
      (events) =>
        { const ended: (string | null)[] = []; let settled = false; lane(lid, (l) => {
          // A replay starts over: what an earlier stream folded is let go once this one speaks,
          // not before, so a retry that fails does not blank the turns on screen.
          let conv = fresh && events.some((e) => e.event === "turn") ? empty : l.conv;
          if (conv === empty) fresh = false;
          let patch: Partial<Lane> = {};
          for (const e of events) {
            if (e.event === "turn") {
              conv = applyFrames(conv, e.data as Frame[]);
              if (live) {
                const said = transcriptSays(l, e.data as Frame[]);
                patch = { ...patch, ...said.patch };
                settled ||= said.settled;
              }
            }
            else if (e.event === "fact") {
              conv = applyFact(conv, e.data as Fact);
              if (live && (e.data as Fact).kind === "turn.ended") ended.push((e.data as Fact).turn);
            }
            else if (e.event === "fatal") missing = true;
            // The process is gone, and its subagents with it. Claude's lanes also hear this from
            // the session list; Codex's and Antigravity's have only this to say it.
            else if (e.event === "ended" && conv.background) conv = { turns: conv.turns };
            else if (e.event === "caught-up") {
              live = true;
              patch = { ...patch, loaded: true, known: true, error: undefined };
              // A replay rebuilds the background agents from the transcript; a session that has
              // ended wrote no notification for the ones it took with it.
              const s = getState().projects[l.project]?.sessions?.find((x) => x.id === l.session);
              if (s && !s.live && conv.background) conv = { turns: conv.turns };
              // Back to a healthy stream: the next blip waits a second, not the ceiling.
              retries.delete(lid);
            }
          }
          return { ...patch, conv };
        }); for (const key of ended) announceEnd(lid, key); if (settled) { gitSoon(lid); drain(lid); } },
      (error) => {
        if (follows.get(lid) !== stream) return;
        follows.delete(lid);
        const now = getState().lanes[lid];
        if (!now) return;
        if (now.known) save(getState());
        // A network failure is the daemon gone: forget its address so the next use restarts it.
        // An answer that refused (a checkout removed outside Keel) is said, not retried blind.
        const network = !!error && /fetch|network|load failed/i.test(error);
        if (network) lost(l.project, ep);
        else if (error && !missing) lane(lid, { error: `Could not follow this conversation: ${error}` });
        const tries = (retries.get(lid) ?? 0) + 1;
        retries.set(lid, tries);
        // Not there yet is normal for a session that has not said anything. Still not there, for
        // one Keel has already seen, is not a wait — the pane sat empty saying nothing while this
        // retried every two seconds forever. It keeps looking, and says why it has nothing.
        if (missing && now.known && tries === 5)
          lane(lid, { error: `Keel cannot find this conversation's transcript under ${({ codex: "~/.codex/sessions", antigravity: "~/.gemini/antigravity-cli/conversations" } as Record<string, string>)[l.agent ?? ""] ?? "~/.claude/projects"}. It keeps looking; if the folder was moved or renamed, open the conversation from History.` });
        // Not written yet is a wait; anything else backs off, to a ceiling.
        const wait = missing ? 2000 : Math.min(30_000, 1000 * 2 ** Math.min(tries, 5));
        setTimeout(() => void follow(lid), wait);
      },
    );
    follows.set(lid, stream);
  }

  const chats = new Map<string, Stream>();
  const connecting = new Set<string>();
  async function listen(lid: string) {
    if (chats.has(lid) || connecting.has(lid)) return;
    const l = getState().lanes[lid];
    if (!l || l.interface === "terminal") return;
    connecting.add(lid);
    try {
      const ep = await endpoint(l.project);
      if (!getState().lanes[lid] || getState().lanes[lid].interface === "terminal") return;
      let caughtUp = false;
      const stream = openStream(url(ep, "/api/chat/events", { lane: lid, after: l.cursor ?? 0, session: l.known ? l.session : undefined, provider: l.agent, wt: l.wt }), ep.token, (events) => {
        let idle = false;
        // Only what arrives after `caught-up` is news; the backlog before it was already seen. Each
        // is kept with whether it arrived live, and sorted out once the batch is folded.
        const seen: { live: boolean; card?: Pending; fact?: Fact; fatal?: string }[] = [];
        lane(lid, (current) => {
          let conv = current.conv;
          const patch: Partial<Lane> = {};
          let cursor = current.cursor ?? 0;
          for (const incoming of events) {
            if (incoming.event === "caught-up") {
              caughtUp = true;
              patch.loaded = true;
              patch.disconnected = false;
              clearConnectionError(current, patch);
              if (current.managed || cursor) patch.running = !!(incoming.data as { running?: boolean }).running;
              if (!patch.running) idle = true;
              continue;
            }
            if (incoming.event !== "record") continue;
            const e = incoming.data as { seq: number; event: string; data: unknown };
            if (e.seq <= cursor) continue;
            if (cursor === 0) conv = empty;
            cursor = e.seq;
            patch.managed = true;
            if (e.event === "accepted") {
              patch.running = true;
              patch.error = undefined;
              const id = (e.data as { id: string }).id;
              if (current.receipt?.id === id) patch.receipt = undefined;
            } else if (e.event === "snapshot") {
              conv = restoreTranscript(conv, (e.data as { frames: Frame[] }).frames);
            } else if (e.event === "turn") {
              const frames = e.data as Frame[];
              Object.assign(patch, sessionPatch(frames));
              conv = applyFrames(conv, frames.filter((f) => f.op !== "session"));
            } else if (e.event === "request") {
              const p = e.data as Pending;
              patch.pending = [...(patch.pending ?? current.pending).filter((x) => x.id !== p.id), p];
              seen.push({ live: caughtUp, card: p });
            } else if (e.event === "resolved") {
              patch.pending = (patch.pending ?? current.pending).filter((p) => p.id !== (e.data as { id: string }).id);
            } else if (e.event === "capabilities") {
              const c = e.data as { models?: Lane["models"]; commands?: { name: string }[] };
              patch.models = c.models;
              patch.commands = c.commands?.map((c) => c.name);
            } else if (e.event === "fact") {
              conv = applyFact(conv, e.data as Fact);
              seen.push({ live: caughtUp, fact: e.data as Fact });
            } else if (e.event === "fatal") {
              patch.error = String(e.data);
              seen.push({ live: caughtUp, fatal: String(e.data) });
            }
            else if (e.event === "idle") {
              patch.running = false;
              patch.pending = [];
              idle = true;
            }
          }
          return { ...patch, conv, cursor };
        });
        announce(lid, seen.filter((x) => x.live));
        const now = getState().lanes[lid];
        if (now?.managed) { follows.get(lid)?.close(); follows.delete(lid); }
        if (idle || events.some((e) => e.event === "record" && ["accepted", "capabilities"].includes((e.data as { event: string }).event))) save(getState());
        if (idle && caughtUp) { gitSoon(lid); drain(lid); }
      }, (error) => {
        if (chats.get(lid) !== stream) return;
        chats.delete(lid);
        if (!getState().lanes[lid]) return;
        lane(lid, { disconnected: true, ...(error ? { error: `Connection interrupted. Reconnecting… ${error}` } : {}) });
        lost(l.project, ep);
        setTimeout(() => void listen(lid), 1500);
      });
      chats.set(lid, stream);
    } catch (error) {
      lane(lid, { disconnected: true, error: String(error) });
      setTimeout(() => void listen(lid), 2000);
    } finally { connecting.delete(lid); }
  }

  return {
    ...load(),
    chatInterface: (() => { try { return localStorage.getItem("keel.chat-interface") === "terminal" ? "terminal" : "chat"; } catch { return "chat"; } })(),
    setChatInterface(value) {
      set({ chatInterface: value });
      try { localStorage.setItem("keel.chat-interface", value); } catch { /* current session still works */ }
    },
    async switchInterface(lid, value) {
      const l = getState().lanes[lid];
      if (!l || l.interface === value) return;
      if (l.running || l.submitting || l.receipt) { lane(lid, { error: "Stop the active turn and resolve any unconfirmed submission before switching interfaces." }); return; }
      // Codex and Antigravity name their conversation on the first message; the daemon finds it
      // on disk (`discover.rs`). Before that there is nothing for the chat view to resume.
      if (l.interface === "terminal" && l.agent !== "claude" && !l.known) {
        lane(lid, { error: `This ${agentName(l.agent)} terminal has no conversation yet. Send it a message, then switch.` }); return;
      }
      if (l.interface === "terminal" && l.agent !== "claude" && terminalConnected(lid)) {
        lane(lid, { submitting: true, error: undefined });
        try { await exitTerminal(lid); }
        catch (error) { lane(lid, { error: String(error) }); return; }
        finally { lane(lid, { submitting: false }); }
        const current = getState().lanes[lid];
        if (!current || current.running || current.receipt || current.interface !== "terminal") return;
      }
      const ep = await endpoint(l.project);
      if (l.interface !== "terminal") {
        const current = getState().lanes[lid];
        if (!current || current.running || current.submitting || current.receipt) return;
        try { await post(ep, "/api/chat/control", { lane: lid, method: "close" }); }
        catch (error) { lane(lid, { error: String(error) }); return; }
      }
      follows.get(lid)?.close(); follows.delete(lid);
      chats.get(lid)?.close(); chats.delete(lid);
      lane(lid, { interface: value, conv: empty, cursor: 0, loaded: false, error: undefined });
      save(getState());
      getState().load(lid);
    },
    settings: false,
    creating: false,
    asking: null,
    renaming: null,
    shortcuts: false,
    extensions: false,
    trusted: {},
    setup: {},
    readiness: {},
    handed: {},
    opened: [],

    connected(lid) {
      const l = getState().lanes[lid];
      if (!l?.queued.length) return;
      const [next, ...rest] = l.queued;
      lane(lid, { queued: rest });
      void getState().send(lid, next.text, next.attachments, next.system);
      if (rest.length) setTimeout(() => getState().connected(lid), 500);
    },

    async prepare(lid) {
      const l = getState().lanes[lid];
      if (!l) return undefined;
      const ep = await endpoint(l.project);
      // A lane on its own branch gets its checkout before the agent starts in it.
      if (l.isolated && !l.wt) {
        const onDisk = await get<{ name: string }[]>(ep, "/api/worktree").catch(() => []);
        const taken = new Set([...onDisk.map((w) => w.name), ...Object.values(getState().lanes).map((x) => x.wt)]);
        let name = "lane-" + id().slice(0, 4);
        while (taken.has(name)) name = "lane-" + id().slice(0, 4);
        try {
          const wt = (await post<{ name: string }>(ep, "/api/worktree/create", { name })).name;
          lane(lid, { wt, title: l.title === "New branch" ? `keel/${wt}` : l.title });
          save(getState());
          return { ep, wt };
        } catch (e) {
          lane(lid, { error: `Could not make this lane's branch: ${e instanceof Error ? e.message : e}` });
          return undefined;
        }
      }
      return { ep, wt: l.wt };
    },
    ensure: (path) => endpoint(path),

    async openProject(path) {
      if (!getState().projects[path]) {
        set((s) => ({
          order: [...s.order, path],
          projects: { ...s.projects, [path]: { path, name: basename(path), lanes: [] } },
        }));
      }
      const lid = getState().projects[path].lanes[0] ?? getState().newLane(path);
      getState().select(lid);
      save(getState());
      await endpoint(path).catch(() => undefined);
    },

    async closeProject(path) {
      const lanes = getState().projects[path]?.lanes ?? [];
      set((s) => {
        const projects = { ...s.projects };
        delete projects[path];
        const gone = new Set(lanes);
        const drop = <T,>(m: Record<string, T>) => Object.fromEntries(Object.entries(m).filter(([k]) => k !== path));
        return {
          trusted: drop(s.trusted),
          setup: drop(s.setup),
          readiness: drop(s.readiness),
          handed: drop(s.handed),
          order: s.order.filter((p) => p !== path),
          projects,
          lanes: Object.fromEntries(Object.entries(s.lanes).filter(([k]) => !gone.has(k))),
          // Its terminals unmount with it; left here they stay alive, hidden, for good.
          opened: s.opened.filter((x) => !gone.has(x)),
          active: gone.has(s.active ?? "") ? undefined : s.active,
        };
      });
      save(getState());
      watchers.get(path)?.close();
      watchers.delete(path);
      for (const lid of lanes) {
        closeTab(lid);
        follows.get(lid)?.close();
        follows.delete(lid);
        chats.get(lid)?.close();
        chats.delete(lid);
        retries.delete(lid);
      }
      // A start still in flight is stopped by `endpoint` when it lands and finds no project.
      await invoke("close_project", { path }).catch(() => undefined);
    },

    newLane(project, isolated = false, agent = "claude", surface) {
      const lid = id();
      set((s) => ({
        lanes: {
          ...s.lanes,
          [lid]: { id: lid, project, interface: surface ?? getState().chatInterface, session: id(), agent, known: false, title: isolated ? "New branch" : "New lane", isolated, jobs: [], queued: [], conv: empty, running: false, loaded: true, pending: [] },
        },
        projects: { ...s.projects, [project]: { ...s.projects[project], lanes: [...s.projects[project].lanes, lid], collapsed: false } },
      }));
      save(getState());
      return lid;
    },

    select(lid) {
      set((s) => ({ active: lid, settings: false, creating: false, extensions: false, opened: s.opened.includes(lid) ? s.opened : [...s.opened, lid] }));
      save(getState());
      void follow(lid);
      void listen(lid);
      gitSoon(lid);
      soon(`jobs:${lid}`, () => getState().refreshJobs(lid));
      soon(`dev:${lid}`, () => getState().refreshDev(lid));
    },

    async refreshJobs(lid) {
      const l = getState().lanes[lid];
      const ep = l && getState().projects[l.project]?.endpoint;
      if (!ep) return;
      let jobs: Job[];
      try {
        jobs = await get<Job[]>(ep, "/api/monitors", { lane: lid });
      } catch {
        return;
      }
      if (!getState().lanes[lid]) return;
      lane(lid, { jobs });
      // Acknowledged before it is delivered, by the lane that owns it: the result reaching the
      // conversation twice is worse than not reaching it at all.
      // A job with no lane belongs to whoever asks — and is delivered by whoever asks first: the
      // ack is compare-and-set, so exactly one lane carries it into its conversation.
      for (const job of jobs.filter((j) => j.finished && !j.reported && (j.lane === lid || j.lane === ""))) {
        const ok = await post<boolean>(ep, "/api/monitors/ack", { id: job.id }).catch(() => false);
        if (!ok) continue;
        const secs = Math.max(0, (job.finished ?? 0) - job.started);
        const took = secs < 60 ? `${secs}s` : `${Math.floor(secs / 60)}m${secs % 60}s`;
        const tail = job.log.slice(-80).join("\n");
        const report = `${JOB_PREFIX}${job.id} finished — exit ${job.exit ?? -1}, after ${took}.\n\n$ ${job.command}\n\n${tail || "(no output)"}`;
        void getState().send(lid, report);
      }
    },

    async stopJob(lid, jobId) {
      const l = getState().lanes[lid];
      const ep = l && getState().projects[l.project]?.endpoint;
      if (ep) await post(ep, "/api/monitors/stop", { id: jobId }).catch(() => undefined);
    },

    async refreshDev(lid) {
      const l = getState().lanes[lid];
      const ep = l && getState().projects[l.project]?.endpoint;
      if (!ep) return;
      const dev = await get<Dev>(ep, "/api/dev", { wt: l.wt }).catch(() => undefined);
      if (dev) lane(lid, { dev });
    },

    async dev(lid, start) {
      const l = getState().lanes[lid];
      const ep = l && getState().projects[l.project]?.endpoint;
      if (!ep) return;
      try {
        const dev = start
          ? await post<Dev>(ep, "/api/dev/start", { command: l.dev?.detected ?? null }, { wt: l.wt })
          : await post<Dev>(ep, "/api/dev/stop", {}, { wt: l.wt });
        if (dev && typeof dev === "object") lane(lid, { dev });
      } catch (e) {
        lane(lid, { error: e instanceof Error ? e.message : String(e) });
      }
      void getState().refreshDev(lid);
    },

    resume(path, session) {
      // Already open in a lane: go there. Running somewhere else: follow it, start nothing.
      const open = Object.values(getState().lanes).find((l) => l.project === path && l.session === session.id);
      if (open) return getState().select(open.id);
      const lid = getState().newLane(path);
      lane(lid, { session: session.id, known: true, elsewhere: session.live, title: session.title || "Untitled session", loaded: false });
      save(getState());
      getState().select(lid);
    },

    closeLane(lid) {
      closeTab(lid);
      const closing = getState().lanes[lid];
      const closingEp = closing && getState().projects[closing.project]?.endpoint;
      if (closingEp) void post(closingEp, "/api/chat/control", { lane: lid, method: "close" }).catch(() => undefined);
      chats.get(lid)?.close();
      chats.delete(lid);
      follows.get(lid)?.close();
      follows.delete(lid);
      retries.delete(lid);
      set((s) => ({ opened: s.opened.filter((x) => x !== lid) }));
      set((s) => {
        const l = s.lanes[lid];
        if (!l) return s;
        const lanes = { ...s.lanes };
        delete lanes[lid];
        const p = s.projects[l.project];
        return {
          lanes,
          projects: p ? { ...s.projects, [l.project]: { ...p, lanes: p.lanes.filter((x) => x !== lid) } } : s.projects,
          active: s.active === lid ? p?.lanes.find((x) => x !== lid) : s.active,
        };
      });
      save(getState());
      const next = getState().active;
      if (next) getState().select(next);
    },

    async refreshGit(lid) {
      const l = getState().lanes[lid];
      const ep = l && getState().projects[l.project]?.endpoint;
      if (!ep) return;
      try {
        const git = await get<GitStatus>(ep, "/api/git/status", { wt: l.wt });
        lane(lid, { git });
        // An open diff follows the file: read again, or closed if the file is no longer changed.
        const { viewing, viewingStaged } = getState().lanes[lid] ?? {};
        if (viewing) void getState().openDiff(lid, git.changes.some((c) => c.path === viewing && (viewingStaged === undefined || inArea(c, viewingStaged))) ? viewing : undefined, viewingStaged);
      } catch {
        // The next event reads again.
      }
    },

    async openDiff(lid, path, staged) {
      if (!path) {
        lane(lid, { viewing: undefined, viewingStaged: undefined, diff: undefined, diffError: undefined });
        return;
      }
      const l = getState().lanes[lid];
      const ep = l && getState().projects[l.project]?.endpoint;
      if (!ep) return;
      lane(lid, (l) => ({ viewing: path, viewingStaged: staged, diff: l.viewing === path && l.viewingStaged === staged ? l.diff : undefined, diffError: undefined }));
      try {
        const d = await get<DiffResponse>(ep, "/api/git/diff", { path, wt: l.wt, staged });
        const current = getState().lanes[lid];
        if (current?.viewing === path && current.viewingStaged === staged) lane(lid, { diff: prepare(d) });
      } catch (e) {
        const current = getState().lanes[lid];
        if (current?.viewing === path && current.viewingStaged === staged) lane(lid, { diffError: `Could not read the diff: ${e instanceof Error ? e.message : e}` });
      }
    },

    async merge(lid) {
      const l = getState().lanes[lid];
      const ep = l && getState().projects[l.project]?.endpoint;
      if (!ep || !l.wt) return;
      try {
        // `--no-ff` into the branch the lane was cut from; a dirty project or a conflict refuses
        // and leaves the lane as it was.
        await post(ep, "/api/worktree/finish", { name: l.wt, message: l.title });
        getState().closeLane(lid);
      } catch (e) {
        const why = e instanceof Error ? e.message : String(e);
        // The checkout is already gone — merged and then a branch delete refused, or removed by
        // hand. Nothing is left to merge, and the lane must not be stuck in the sidebar for good.
        if (/^no lane /.test(why)) return getState().closeLane(lid);
        lane(lid, { error: why });
      }
    },

    async discard(lid, force) {
      const l = getState().lanes[lid];
      const ep = l && getState().projects[l.project]?.endpoint;
      if (!l) return;
      if (!l.wt) {
        getState().closeLane(lid);
        return;
      }
      if (!ep) return;
      try {
        await post(ep, "/api/worktree/discard", { name: l.wt, force });
        getState().closeLane(lid);
      } catch (e) {
        const why = e instanceof Error ? e.message : String(e);
        if (/^no lane /.test(why)) return getState().closeLane(lid);
        // The daemon refuses a discard that would lose work and says what; that becomes the
        // question. A forced one that still fails is an error, not the same question again.
        if (force) lane(lid, { confirmDiscard: undefined, error: why });
        else lane(lid, { confirmDiscard: why });
      }
    },

    load(lid) {
      // Whatever put the lane on screen: its daemon, its conversation, its checkout's state.
      const l = getState().lanes[lid];
      if (l) void endpoint(l.project).then(() => { gitSoon(lid); void getState().refreshDev(lid); }, () => undefined);
      set((s) => (s.opened.includes(lid) ? s : { opened: [...s.opened, lid] }));
      void follow(lid);
      void listen(lid);
    },

    draft(lid, text) { lane(lid, { draft: text }); save(getState()); },

    editQueued(lid, messageId) {
      const l = getState().lanes[lid];
      const message = l?.queued.find((q) => q.id === messageId);
      if (!l || !message) return;
      if (l.draft?.trim() || l.attachments?.length) {
        lane(lid, { error: "Your composer has an unsent draft. Send or clear it before editing a queued message." });
        return;
      }
      lane(lid, { draft: message.text, attachments: message.attachments, queued: l.queued.filter((q) => q.id !== messageId), error: undefined });
      save(getState());
    },
    removeQueued(lid, messageId) {
      lane(lid, (l) => ({ queued: l.queued.filter((q) => q.id !== messageId) }));
      save(getState());
    },

    async stop(lid) {
      const l = getState().lanes[lid];
      if (l?.interface === "terminal") { interruptTerminal(lid); return; }
      const ep = l && getState().projects[l.project]?.endpoint;
      if (!ep) return;
      lane(lid, { queued: [] });
      save(getState());
      try { await post(ep, "/api/chat/stop", {}, { lane: lid }); }
      catch (error) { lane(lid, { error: `Could not stop the turn: ${error}` }); }
    },

    async send(lid, prompt, attachments = [], system) {
      const l = getState().lanes[lid];
      if (!l || !prompt.trim()) return false;
      if (l.interface === "terminal") {
        if (!typeInto(lid, prompt)) lane(lid, { queued: [...l.queued, { id: id(), text: prompt, attachments, system }] });
        return true;
      }
      if (l.elsewhere) { lane(lid, { error: "This session is running outside Keel. Stop it there before resuming here." }); return false; }
      if (l.receipt && l.receipt.prompt !== prompt) { lane(lid, { error: "The previous submission has not been confirmed. Retry it before sending another message." }); return false; }
      const ownsDraft = isComposerSubmission(l, prompt, attachments);
      if (l.running || l.submitting) {
        lane(lid, { queued: [...l.queued, { id: id(), text: prompt, attachments, system }], ...(ownsDraft ? { draft: "", attachments: [] } : {}) });
        save(getState());
        return true;
      }
      lane(lid, { submitting: true, running: true, error: undefined });
      let submission: string | undefined;
      try {
        const ready = await getState().prepare(lid);
        if (!getState().lanes[lid]) return false;
        if (!ready) throw new Error("The lane could not be prepared.");
        const receipt = l.receipt ?? makeReceipt(l, prompt, attachments, ready.wt, system);
        submission = receipt.id;
        lane(lid, { receipt });
        save(getState());
        const delivery = await post<{ accepted: boolean; running: boolean }>(ready.ep, "/api/chat/send", { ...receipt, lane: lid, provider: l.agent, auto_commit: false });
        lane(lid, (now) => ({ managed: true, receipt: undefined, running: (now.cursor ?? 0) > (l.cursor ?? 0) ? now.running : delivery.running, draft: ownsDraft && now.draft === l.draft ? "" : now.draft, attachments: ownsDraft && now.attachments === l.attachments ? [] : now.attachments }));
        follows.get(lid)?.close();
        follows.delete(lid);
        void listen(lid);
        return true;
      } catch (error) {
        // A receipt delivered over SSE is authoritative even if the POST response was lost.
        if (submission && getState().lanes[lid]?.receipt?.id !== submission) {
          lane(lid, (now) => ({ draft: ownsDraft && now.draft === l.draft ? "" : now.draft, attachments: ownsDraft && now.attachments === l.attachments ? [] : now.attachments }));
          return true;
        }
        lane(lid, { running: false, error: `Message not confirmed: ${error}. Retry uses the same submission id.`, draft: getState().lanes[lid]?.draft || (ownsDraft ? prompt : "") });
        return false;
      } finally { lane(lid, { submitting: false }); save(getState()); drain(lid); }
    },

    async answer(lid, p, decision, text, scope = "once") {
      const l = getState().lanes[lid];
      const ep = l && getState().projects[l.project]?.endpoint;
      if (!ep) return;
      if (p.tool === "ExitPlanMode" && !p.provider && decision === "allow") {
        // The plan-write hook ends a read-only turn. Its approval sentinel and the
        // next edit-mode turn are one handoff; a generic allow means rejection to it.
        // Hold queue draining while the answer is in flight: idle can arrive first.
        if (l.submitting) return;
        lane(lid, { submitting: true });
        try {
          const result = await post<{ ok: boolean }>(ep, "/api/approve/answer", {
            id: p.id, decision: "deny", session: p.session_id || l.session,
            rules: [], scope: "once", answer: "keel:plan-approved",
          });
          if (!result.ok) throw new Error("This plan is no longer waiting for approval.");
          const plan = typeof p.input.plan === "string" ? `\n\n${p.input.plan}` : "";
          lane(lid, (now) => ({
            mode: "acceptEdits", error: undefined,
            pending: now.pending.filter((x) => x.id !== p.id),
            queued: [{ id: id(), text: `Implement the plan I approved in Keel. Proceed with the changes and verify them.${plan}`, attachments: [] }, ...now.queued],
          }));
        } catch (error) {
          lane(lid, { error: `Could not approve the plan: ${error}` });
        } finally {
          lane(lid, { submitting: false });
          save(getState());
          drain(lid);
        }
        return;
      }
      if (p.provider) {
        try { await post(ep, "/api/chat/control", { lane: lid, method: "answer", id: p.id, answer: { decision: p.provider === "codex" ? (decision === "allow" ? "accept" : "decline") : decision, reason: text } }); }
        catch (error) { lane(lid, { error: `Could not deliver the answer: ${error}` }); }
        return;
      }
      lane(lid, (l) => ({ pending: l.pending.filter((x) => x.id !== p.id) }));
      // A question's answer travels as a `deny` whose reason is the answer; a permission carries
      // the rules that would let it through. The same bodies the Swift app sent.
      const question = text !== undefined;
      try {
        await post(ep, "/api/approve/answer", {
          id: p.id,
          decision: question ? "deny" : decision,
          session: p.session_id || l.session,
          rules: question ? [] : p.rules,
          scope: question ? "session" : scope,
          answer: text ?? "",
        });
      } catch (e) {
        // Still waiting on the other side, so the card comes back rather than vanishing.
        lane(lid, (l) => ({
          pending: l.pending.some((x) => x.id === p.id) ? l.pending : [...l.pending, p],
          error: `That answer did not reach the agent: ${e}. It is still waiting — try again.`,
        }));
      }
    },
  };
});

function sessionPatch(frames: Frame[]): Partial<Lane> {
  const patch: Partial<Lane> = {};
  for (const frame of frames) if (frame.op === "session") {
    patch.session = String(frame.id);
    patch.known = true;
    if (Array.isArray(frame.commands)) patch.commands = frame.commands as string[];
  }
  return patch;
}

function clearConnectionError(current: Lane, patch: Partial<Lane>) {
  if (current.error?.startsWith("Connection interrupted.") && !patch.error) patch.error = undefined;
}
