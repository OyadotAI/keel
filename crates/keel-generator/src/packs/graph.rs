//! graph: a stateful graph agent, like LangGraph ══════════════════════════════════════════

pub fn files(name: &str) -> Vec<(&'static str, String)> {
    let f = |s: &str| s.replace("{{NAME}}", name);
    vec![
        ("backend/src/app.ts", GRAPH_APP.into()),
        ("backend/src/store.ts", GRAPH_STORE.into()),
        ("backend/src/graph.ts", GRAPH_ENGINE.into()),
        ("backend/src/app.test.ts", GRAPH_TEST.into()),
        ("backend/migrations/0002_checkpoints.sql", GRAPH_SQL.into()),
        ("frontend/app/page.tsx", f(GRAPH_PAGE)),
        ("CLAUDE.md", f(GRAPH_CLAUDE_MD)),
        ("AGENTS.md", f(GRAPH_AGENTS_MD)),
        ("README.md", f(GRAPH_README_MD)),
        (
            ".claude/agents/graph-semantics.md",
            GRAPH_AGENT_REVIEW.into(),
        ),
    ]
}

const GRAPH_SQL: &str = r##"-- LangGraph's checkpointer, as tables: one row per super-step per thread, with the state
-- after that step and the node(s) that ran. Time travel is a checkpoint id; resume is a
-- thread id.
create table if not exists threads (
  id uuid primary key,
  graph text not null,
  status text not null default 'idle',  -- idle | running | interrupted | done | failed
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now()
);
create table if not exists checkpoints (
  id uuid primary key,
  thread_id uuid not null references threads(id),
  step int not null,
  parent_id uuid,
  node text not null,
  state jsonb not null,
  next text[] not null default '{}',
  interrupt jsonb,
  created_at timestamptz not null default now(),
  unique (thread_id, step)
);
"##;

const GRAPH_STORE: &str = r##"import { db } from "./db";
import type { Checkpoint } from "./graph";

export type Thread = { id: string; graph: string; status: string };

export interface Store {
  createThread(id: string, graph: string): Promise<void>;
  setStatus(id: string, status: string): Promise<void>;
  thread(id: string): Promise<Thread | null>;
  threads(limit: number): Promise<Thread[]>;
  put(cp: Checkpoint): Promise<void>;
  latest(threadId: string): Promise<Checkpoint | null>;
  history(threadId: string): Promise<Checkpoint[]>;
  checkpoint(id: string): Promise<Checkpoint | null>;
}

export function pgStore(): Store {
  return {
    createThread: async (id, graph) => { await db`insert into threads (id, graph) values (${id}, ${graph})`; },
    setStatus: async (id, status) => { await db`update threads set status = ${status}, updated_at = now() where id = ${id}`; },
    thread: async (id) => (await db<Thread[]>`select id, graph, status from threads where id = ${id}`)[0] ?? null,
    threads: async (limit) => db<Thread[]>`select id, graph, status from threads order by updated_at desc limit ${limit}`,
    put: async (cp) => { await db`insert into checkpoints (id, thread_id, step, parent_id, node, state, next, interrupt) values (${cp.id}, ${cp.thread_id}, ${cp.step}, ${cp.parent_id}, ${cp.node}, ${db.json(cp.state as never)}, ${cp.next}, ${cp.interrupt ? db.json(cp.interrupt as never) : null})`; },
    latest: async (t) => (await db<Checkpoint[]>`select * from checkpoints where thread_id = ${t} order by step desc limit 1`)[0] ?? null,
    history: async (t) => db<Checkpoint[]>`select * from checkpoints where thread_id = ${t} order by step`,
    checkpoint: async (id) => (await db<Checkpoint[]>`select * from checkpoints where id = ${id}`)[0] ?? null,
  };
}

export function memoryStore(): Store {
  const threads = new Map<string, Thread>(); const cps: Checkpoint[] = [];
  return {
    createThread: async (id, graph) => { threads.set(id, { id, graph, status: "idle" }); },
    setStatus: async (id, status) => { const t = threads.get(id); if (t) t.status = status; },
    thread: async (id) => threads.get(id) ?? null,
    threads: async (limit) => [...threads.values()].reverse().slice(0, limit),
    put: async (cp) => { cps.push(structuredClone(cp)); },
    latest: async (t) => cps.filter((c) => c.thread_id === t).sort((a, b) => b.step - a.step)[0] ?? null,
    history: async (t) => cps.filter((c) => c.thread_id === t).sort((a, b) => a.step - b.step),
    checkpoint: async (id) => cps.find((c) => c.id === id) ?? null,
  };
}
"##;

const GRAPH_ENGINE: &str = r##"// A StateGraph the way LangGraph defines one: nodes are functions from state to a partial
// update, edges are fixed or conditional, channels merge updates with a reducer (append for
// messages, replace otherwise). Execution is super-steps; after each one the state is
// checkpointed, so a thread resumes from its last checkpoint and can be forked from any
// earlier one. `interrupt()` inside a node pauses the graph for a human; `resume` continues
// it with their value. All of that is in this file, and the graph itself is in `workflow`.

export type Checkpoint = { id: string; thread_id: string; step: number; parent_id: string | null; node: string; state: Record<string, unknown>; next: string[]; interrupt: unknown | null };
export type Reducer = (a: unknown, b: unknown) => unknown;
export type Node<S> = (state: S, ctx: { interrupt: (value: unknown) => unknown }) => Promise<Partial<S>> | Partial<S>;

export const START = "__start__", END = "__end__";
export const append: Reducer = (a, b) => [...((a as unknown[]) ?? []), ...(Array.isArray(b) ? b : [b])];

class Interrupt { constructor(public value: unknown) {} }

export class StateGraph<S extends Record<string, unknown>> {
  private nodes = new Map<string, Node<S>>();
  private edges = new Map<string, string | ((s: S) => string)>();
  constructor(private channels: Partial<Record<keyof S, Reducer>> = {}) {}
  addNode(name: string, fn: Node<S>) { this.nodes.set(name, fn); return this; }
  addEdge(from: string, to: string) { this.edges.set(from, to); return this; }
  addConditionalEdges(from: string, route: (s: S) => string) { this.edges.set(from, route); return this; }
  nodeNames() { return [...this.nodes.keys()]; }

  private merge(state: S, update: Partial<S>): S {
    const out = { ...state };
    for (const [k, v] of Object.entries(update)) { const r = this.channels[k as keyof S]; (out as Record<string, unknown>)[k] = r ? r(out[k], v) : v; }
    return out;
  }
  private nextOf(node: string, state: S): string {
    const e = this.edges.get(node); if (!e) return END;
    return typeof e === "function" ? e(state) : e;
  }

  /// Run from a checkpoint (or from START with `input`) until END or an interrupt. Each
  /// super-step yields a checkpoint; the caller stores it. `resume` answers a pending interrupt.
  async *run(threadId: string, from: Checkpoint | null, input: Partial<S> | null, resume?: unknown, maxSteps = 50): AsyncGenerator<Checkpoint> {
    let state = (from?.state ?? {}) as S;
    if (input) state = this.merge(state, input);
    let next = from?.next?.length ? from.next[0] : this.nextOf(START, state);
    let step = from ? from.step + 1 : 0; let parent = from?.id ?? null;
    let resumeValue = from?.interrupt !== null && from?.interrupt !== undefined ? resume : undefined;
    if (!from) { const cp = { id: crypto.randomUUID(), thread_id: threadId, step: step++, parent_id: null, node: START, state, next: [next], interrupt: null }; parent = cp.id; yield cp; }
    while (next !== END && step < maxSteps) {
      const fn = this.nodes.get(next); if (!fn) throw new Error(`no node ${next}`);
      let interrupted: unknown = null;
      const ctx = { interrupt: (v: unknown) => { if (resumeValue !== undefined) { const r = resumeValue; resumeValue = undefined; return r; } throw new Interrupt(v); } };
      let update: Partial<S> = {};
      try { update = await fn(state, ctx); } catch (e) { if (e instanceof Interrupt) interrupted = e.value; else throw e; }
      if (interrupted !== null) {
        const cp = { id: crypto.randomUUID(), thread_id: threadId, step, parent_id: parent, node: next, state, next: [next], interrupt: interrupted };
        yield cp; return;
      }
      state = this.merge(state, update);
      const after = this.nextOf(next, state);
      const cp = { id: crypto.randomUUID(), thread_id: threadId, step: step++, parent_id: parent, node: next, state, next: after === END ? [] : [after], interrupt: null };
      parent = cp.id; next = after; yield cp;
    }
    if (next !== END) throw new Error(`stopped after ${maxSteps} steps`);
  }
}

// ── the graph in this project: draft → review (human) → publish ────────────────────────
export type Doc = { topic: string; draft: string; messages: string[]; approved: boolean; revisions: number };
export type LLM = (prompt: string) => Promise<string>;

export function workflow(llm: LLM) {
  return new StateGraph<Doc>({ messages: append })
    .addNode("draft", async (s) => ({ draft: await llm(`Write a short paragraph about: ${s.topic}${s.revisions ? `\nPrevious draft was rejected; feedback: ${s.messages.at(-1)}` : ""}`), messages: [`drafted (rev ${s.revisions})`] }))
    .addNode("review", (s, { interrupt }) => {
      const decision = interrupt({ question: "Approve this draft?", draft: s.draft }) as { approved: boolean; feedback?: string };
      return { approved: decision.approved, revisions: s.revisions + (decision.approved ? 0 : 1), messages: [decision.approved ? "approved" : `rejected: ${decision.feedback ?? ""}`] };
    })
    .addNode("publish", (s) => ({ messages: [`published: ${s.draft.slice(0, 40)}…`] }))
    .addEdge(START, "draft").addEdge("draft", "review")
    .addConditionalEdges("review", (s) => (s.approved ? "publish" : s.revisions >= 3 ? END : "draft"))
    .addEdge("publish", END);
}

export function claudeLLM(fetchImpl: typeof fetch = fetch): LLM {
  return async (prompt) => {
    const res = await fetchImpl("https://api.anthropic.com/v1/messages", { method: "POST", headers: { "x-api-key": process.env.ANTHROPIC_API_KEY ?? "", "anthropic-version": "2023-06-01", "content-type": "application/json" }, body: JSON.stringify({ model: process.env.MODEL ?? "claude-sonnet-4-20250514", max_tokens: 600, messages: [{ role: "user", content: prompt }] }) });
    if (!res.ok) throw new Error(`anthropic ${res.status}`);
    const a = (await res.json()) as { content: { type: string; text?: string }[] };
    return a.content.filter((b) => b.type === "text").map((b) => b.text).join("");
  };
}
"##;

const GRAPH_APP: &str = r##"import { Hono } from "hono";
import { z } from "zod";
import { pgStore, type Store } from "./store";
import { workflow, claudeLLM, type Doc, type LLM, type Checkpoint } from "./graph";

// LangGraph's server, reduced to the four calls that matter: start a thread, read its state
// and history, resume an interrupt, fork from an earlier checkpoint. Every call is a
// checkpoint read and a loop of checkpoint writes; nothing lives in memory between requests.

export function createApp(store: Store, llm: LLM = claudeLLM()) {
  const graph = workflow(llm);
  const drive = async (threadId: string, from: Checkpoint | null, input: Partial<Doc> | null, resume?: unknown) => {
    await store.setStatus(threadId, "running");
    let last = from;
    try {
      for await (const cp of graph.run(threadId, from, input, resume)) { await store.put(cp); last = cp; }
      await store.setStatus(threadId, last?.interrupt != null ? "interrupted" : "done");
    } catch (e) { await store.setStatus(threadId, "failed"); throw e; }
    return last;
  };

  const app = new Hono()
    .get("/api/health", (c) => c.json({ status: "ok" }))
    .get("/api/health/ready", (c) => c.json({ status: "ok", db: "ok" as const }))
    .get("/api/graph", (c) => c.json({ nodes: graph.nodeNames() }))

    .post("/api/threads", async (c) => {
      const p = z.object({ topic: z.string().min(1).max(500) }).safeParse(await c.req.json().catch(() => ({})));
      if (!p.success) return c.json({ error: { message: "topic required", code: "invalid" } }, 400);
      const id = crypto.randomUUID();
      await store.createThread(id, "workflow");
      const last = await drive(id, null, { topic: p.data.topic, draft: "", messages: [], approved: false, revisions: 0 });
      return c.json({ id, status: (await store.thread(id))!.status, checkpoint: last }, 201);
    })
    .get("/api/threads", async (c) => c.json({ threads: await store.threads(50) }))
    .get("/api/threads/:id", async (c) => {
      const t = await store.thread(c.req.param("id")); if (!t) return c.json({ error: { message: "no such thread", code: "not_found" } }, 404);
      return c.json({ ...t, checkpoint: await store.latest(t.id) });
    })
    .get("/api/threads/:id/history", async (c) => c.json({ checkpoints: await store.history(c.req.param("id")) }))

    // Answer the interrupt: LangGraph's Command(resume=...).
    .post("/api/threads/:id/resume", async (c) => {
      const t = await store.thread(c.req.param("id")); if (!t) return c.json({ error: { message: "no such thread", code: "not_found" } }, 404);
      const last = await store.latest(t.id);
      if (!last || last.interrupt == null) return c.json({ error: { message: "thread is not waiting", code: "invalid" } }, 409);
      const body = await c.req.json().catch(() => null);
      const end = await drive(t.id, last, null, body);
      return c.json({ id: t.id, status: (await store.thread(t.id))!.status, checkpoint: end });
    })

    // Time travel: a new thread that continues from any checkpoint, with optional state edits.
    .post("/api/threads/:id/fork", async (c) => {
      const p = z.object({ checkpoint_id: z.string().uuid(), update: z.record(z.unknown()).optional() }).safeParse(await c.req.json().catch(() => ({})));
      if (!p.success) return c.json({ error: { message: "checkpoint_id required", code: "invalid" } }, 400);
      const cp = await store.checkpoint(p.data.checkpoint_id);
      if (!cp || cp.thread_id !== c.req.param("id")) return c.json({ error: { message: "no such checkpoint", code: "not_found" } }, 404);
      const id = crypto.randomUUID();
      await store.createThread(id, "workflow");
      const seed: Checkpoint = { ...cp, id: crypto.randomUUID(), thread_id: id, parent_id: cp.id, state: { ...cp.state, ...(p.data.update ?? {}) }, interrupt: null };
      await store.put(seed);
      const last = await drive(id, seed, null);
      return c.json({ id, status: (await store.thread(id))!.status, checkpoint: last }, 201);
    });

  return app;
}

const app = createApp(pgStore());
export type AppType = typeof app;
export default app;
"##;

const GRAPH_TEST: &str = r##"import { describe, expect, test } from "bun:test";
import { createApp } from "./app";
import { memoryStore } from "./store";
import { workflow, StateGraph, START, END, append } from "./graph";

const llm = async (prompt: string) => `Draft about ${/about: (.*?)(\n|$)/.exec(prompt)?.[1]} ${prompt.includes("rejected") ? "(revised)" : ""}`.trim();

describe("graph", () => {
  test("channels reduce and edges route", async () => {
    const g = new StateGraph<{ n: number; log: string[] }>({ log: append })
      .addNode("inc", (s) => ({ n: s.n + 1, log: [`n=${s.n + 1}`] }))
      .addEdge(START, "inc").addConditionalEdges("inc", (s) => (s.n < 3 ? "inc" : END));
    const cps = []; for await (const cp of g.run("t", null, { n: 0, log: [] })) cps.push(cp);
    expect(cps.at(-1)!.state).toEqual({ n: 3, log: ["n=1", "n=2", "n=3"] });
    expect(cps.map((c) => c.node)).toEqual([START, "inc", "inc", "inc"]);
    expect(cps[1].parent_id).toBe(cps[0].id);
  });

  test("a thread pauses at the human step and resumes from its checkpoint", async () => {
    const app = createApp(memoryStore(), llm);
    const start = await app.request("/api/threads", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ topic: "otters" }) });
    expect(start.status).toBe(201);
    const { id, status, checkpoint } = await start.json();
    expect(status).toBe("interrupted");
    expect(checkpoint.interrupt.question).toBe("Approve this draft?");
    expect(checkpoint.state.draft).toBe("Draft about otters");

    // Reject once: it loops back to draft, with the feedback in the prompt, and asks again.
    const rej = await app.request(`/api/threads/${id}/resume`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ approved: false, feedback: "shorter" }) });
    const r = await rej.json();
    expect(r.status).toBe("interrupted");
    expect(r.checkpoint.state.draft).toContain("(revised)");
    expect(r.checkpoint.state.revisions).toBe(1);

    // Approve: publish, END.
    const ok = await (await app.request(`/api/threads/${id}/resume`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ approved: true }) })).json();
    expect(ok.status).toBe("done");
    expect(ok.checkpoint.state.messages.at(-1)).toContain("published");
    expect((await app.request(`/api/threads/${id}/resume`, { method: "POST", body: "{}" })).status).toBe(409);

    // History is every checkpoint in order.
    const h = await (await app.request(`/api/threads/${id}/history`)).json();
    expect(h.checkpoints.map((c: { node: string }) => c.node)).toEqual([START, "draft", "review", "review", "draft", "review", "review", "publish"]);
  });

  test("forking from an earlier checkpoint replays with edited state", async () => {
    const app = createApp(memoryStore(), llm);
    const { id } = await (await app.request("/api/threads", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ topic: "otters" }) })).json();
    const h = await (await app.request(`/api/threads/${id}/history`)).json();
    const first = h.checkpoints[0];
    const fork = await app.request(`/api/threads/${id}/fork`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ checkpoint_id: first.id, update: { topic: "beavers" } }) });
    expect(fork.status).toBe(201);
    const f = await fork.json();
    expect(f.id).not.toBe(id);
    expect(f.checkpoint.state.draft).toBe("Draft about beavers");
    expect(workflow(llm).nodeNames()).toEqual(["draft", "review", "publish"]);
  });
});
"##;

const GRAPH_PAGE: &str = r##""use client";
import { useEffect, useState } from "react";

type Thread = { id: string; status: string; checkpoint: { node: string; state: { topic: string; draft: string; messages: string[]; revisions: number }; interrupt: { question: string } | null } | null };

// Start a thread, approve or reject at the human step, watch it publish. Every state shown
// here was read back from a checkpoint — refresh the page mid-way and nothing is lost.
export default function Home() {
  const [topic, setTopic] = useState("why otters hold hands");
  const [thread, setThread] = useState<Thread | null>(null);
  const [feedback, setFeedback] = useState("");
  const [threads, setThreads] = useState<{ id: string; status: string }[]>([]);

  const refresh = async () => setThreads((await (await fetch("/api/threads")).json()).threads);
  useEffect(() => { void refresh(); }, []);

  const start = async () => { const r = await fetch("/api/threads", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ topic }) }); setThread(await r.json()); void refresh(); };
  const resume = async (approved: boolean) => { if (!thread) return; const r = await fetch(`/api/threads/${thread.id}/resume`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ approved, feedback }) }); setThread(await r.json()); setFeedback(""); void refresh(); };
  const open = async (id: string) => setThread(await (await fetch(`/api/threads/${id}`)).json());

  const s = thread?.checkpoint?.state;
  return (
    <main>
      <h1>{{NAME}} — graph agent</h1>
      <p>draft → review (you) → publish, checkpointed every step. Set <code>ANTHROPIC_API_KEY</code> in backend/.env.</p>
      <p><input value={topic} onChange={(e) => setTopic(e.target.value)} size={50} /> <button onClick={start}>Start</button></p>
      {thread && s && (
        <section>
          <p>Thread <code>{thread.id.slice(0, 8)}</code> · <strong>{thread.status}</strong> · at <code>{thread.checkpoint?.node}</code> · revisions {s.revisions}</p>
          <blockquote>{s.draft}</blockquote>
          {thread.checkpoint?.interrupt && (
            <p>{thread.checkpoint.interrupt.question} <input placeholder="feedback if rejecting" value={feedback} onChange={(e) => setFeedback(e.target.value)} /> <button onClick={() => resume(true)}>Approve</button> <button onClick={() => resume(false)}>Reject</button></p>
          )}
          <ol>{s.messages.map((m, i) => <li key={i}>{m}</li>)}</ol>
        </section>
      )}
      <h2>Threads</h2>
      <ul>{threads.map((t) => <li key={t.id}><a href="#" onClick={(e) => { e.preventDefault(); void open(t.id); }}>{t.id.slice(0, 8)}</a> · {t.status}</li>)}</ul>
    </main>
  );
}
"##;

const GRAPH_CLAUDE_MD: &str = r##"# {{NAME}} — graph agent

A stateful graph runtime modelled on LangGraph: a `StateGraph` whose nodes are functions from
state to a partial update, whose edges are fixed or conditional, and whose channels merge
updates with a reducer (`append` for `messages`, replace otherwise). Execution is super-steps;
after each one the state is checkpointed, so a thread resumes from its last checkpoint, pauses
at `interrupt()` for a human, continues with their value, and can be forked from any earlier
checkpoint with edited state. The graph in this project is `draft → review (human) → publish`,
with rejection looping back to `draft` up to three times. "Done" here means every state the API
returns was read from a checkpoint, a restart between two requests changes nothing, and the gate
is green with no model and no database.

## Architecture

| File | Owns |
|---|---|
| `backend/src/graph.ts` | The engine: `StateGraph`, `Checkpoint`, `append`, `START`/`END`, `interrupt`. Then the project's graph `workflow(llm)`, the `Doc` state type, and `claudeLLM()`. |
| `backend/src/store.ts` | `Store`: threads and checkpoints; `pgStore` and `memoryStore`. |
| `backend/src/app.ts` | `drive()` — run the generator, put each checkpoint, set the thread status — and the five routes. `AppType` is exported here. |
| `backend/src/app.test.ts` | Engine semantics on a tiny counting graph; the workflow end to end with a fake `llm`; forking. |
| `backend/migrations/0002_checkpoints.sql` | `threads`, `checkpoints` with `unique (thread_id, step)`. |
| `backend/src/db.ts`, `server.ts`, `migrate.ts` | Pool, listener with SIGTERM drain, migration runner — unchanged from the stack. |
| `frontend/app/page.tsx` | Start a thread, approve or reject at the interrupt, open any thread; everything shown is a checkpoint read. |

Request path for `POST /api/threads`:

1. zod: `topic` 1–500 chars. `store.createThread(id, "workflow")`, status `idle`.
2. `drive(id, null, initialState)`: status `running`; `graph.run` yields a `__start__`
   checkpoint (step 0) with the merged input.
3. `draft` runs: `llm(prompt)`; its partial update is merged (`messages` appended); checkpoint
   step 1, `next: ["review"]`.
4. `review` calls `interrupt({question, draft})`. There is no resume value, so the engine throws
   `Interrupt`, catches it, and yields a checkpoint with `interrupt` set, `next: ["review"]`, the
   **same** step number as the node would have taken. The generator returns.
5. `drive` sets status `interrupted` and returns the last checkpoint. `201 {id, status, checkpoint}`.
6. `POST /api/threads/:id/resume` with `{approved, feedback}`: `latest()` must carry an
   interrupt or `409`. `drive(id, latest, null, body)`: `review` runs again from its start; its
   first `interrupt()` call returns the body; the update merges; the conditional edge routes to
   `publish`, `draft` or `END`.
7. `publish` appends a message; `next: []`; status `done`.

Data model:

| Table | Column | Why |
|---|---|---|
| `threads` | `id`, `graph` | `graph` is always `"workflow"` today — the column exists so a second graph does not need a migration. |
| | `status` | `idle` → `running` → `interrupted` \| `done` \| `failed`. Set only by `drive`. |
| `checkpoints` | `thread_id`, `step` | `unique (thread_id, step)`: two drives of one thread collide here, and the loser fails instead of writing a second history. |
| | `parent_id` | The previous checkpoint; across threads for a fork's seed. History is a chain, not just an order. |
| | `node` | Which node produced this state (`__start__` for the seed). |
| | `state jsonb` | The full state after the step, not a delta — reading one row is enough to resume. |
| | `next text[]` | Where execution continues; `[]` means END. One element today. |
| | `interrupt jsonb` | Non-null means "waiting for a human with this payload". |

## Invariants

1. **State changes only in `merge`.** Nodes return partial updates; a channel with a reducer
   reduces, anything else replaces; nodes never mutate `state`. Guarded by
   `channels reduce and edges route` (`log` appends, `n` replaces).
2. **Every super-step is a checkpoint, and the checkpoint is stored before the next node runs.**
   `graph.run` yields; `drive` awaits `store.put` before pulling the next one. Guarded by the
   history assertion in `a thread pauses at the human step and resumes from its checkpoint`.
3. **Resume re-runs the interrupted node from its start**, and the resume value answers the
   node's first `interrupt()` call only. Anything a node does before `interrupt()` happens twice
   — keep it idempotent or after the call. Guarded by the same test: `review` appears twice in
   history for each pause.
4. **An interrupted checkpoint is not a step forward.** It keeps `next: [node]` and takes the
   step number the node would have used; the resumed node's checkpoint is `step + 1`. Guarded by
   the history order `[START, draft, review, review, …]`.
5. **Resume is refused unless the latest checkpoint carries an interrupt** (`409`). Guarded by
   the `409` assertion at the end of the same test.
6. **Fork never touches the source thread.** It creates a new thread, seeds it with a copy of the
   checkpoint (`parent_id` = source, `interrupt: null`, `update` merged by plain spread) and
   drives from there. Guarded by `forking from an earlier checkpoint replays with edited state`
   (`f.id !== id`, the fork's draft is about the edited topic).
7. **A fork's checkpoint must belong to the thread in the URL** or it is `404`. Guarded by the
   `cp.thread_id !== c.req.param("id")` check; add a test if you touch it.
8. **Conditional edges are pure functions of state**: `(s) => string`, no `ctx`, no `await`, no
   side effects. Routing is replayable from the checkpoint alone.
9. **Status is set only in `drive`**, and every exit sets it: `interrupted` when the last
   checkpoint has an interrupt, `done` otherwise, `failed` on throw (rethrown, so the route is a
   `500`). No status without a checkpoint behind it.
10. **The LLM is a parameter.** `workflow(llm)` and `createApp(store, llm)`; `graph.ts` reads the
    network only inside `claudeLLM()`. Guarded by the whole suite passing with the fake `llm`.
11. **`unique (thread_id, step)` is the concurrency guard.** Two concurrent resumes of one thread
    both compute step N; the second `insert` fails and that drive ends `failed`. `memoryStore`
    does not enforce it, so this is untested — see Ceilings.
12. **Routes stay one chained expression** so `AppType` carries them all.

## Extending it

**Add a node**: `.addNode("name", async (s, {interrupt}) => partial)` in `workflow()`, then the
edges in and out (`addEdge` or `addConditionalEdges`). If it needs new state, extend `Doc` and
the initial state in `POST /api/threads`; if a field should accumulate, give it a reducer in the
`StateGraph` constructor. Test in `app.test.ts` by driving the app with the fake `llm` and
asserting `history` node order and the final `state`. No migration: state is JSON.

**Add a human step**: call `interrupt(payload)` in the node; the payload is what the UI shows;
the resume body is what the call returns. Put side effects after the call (Invariant 3). Test:
status `interrupted`, `checkpoint.interrupt` equals the payload, resume moves on.

**Add a second graph**: a second `workflow2(llm)` factory; a `graphs` map in `createApp` keyed by
name; `POST /api/threads` takes `graph` and stores it in `threads.graph`; `drive` looks the graph
up by the thread's `graph`. Fork and resume then work unchanged. Test that each thread resumes
on its own graph.

**Add a reducer**: a `Reducer` beside `append` in `graph.ts` (e.g. a keyed merge), used in the
`StateGraph` constructor. Test on a one-node graph like `channels reduce and edges route`.

**Add an `update_state` route** (LangGraph's edit-in-place): a fork to the same thread is not
possible without a second history, so implement it as `POST /api/threads/:id/state` that writes
a new checkpoint `step + 1` with `node: "__update__"`, the merged state and the same `next`.
Test that resume continues from it.

**Expose streaming**: `drive` already iterates checkpoints; wrap `POST /api/threads` in
`streamSSE` and emit each checkpoint as it is put, then the final status.

**Add a route**: one more link on the chained `app` in `app.ts`. Test through `app.request`.

## Operating it

| Env | Required | Meaning |
|---|---|---|
| `ANTHROPIC_API_KEY` | yes, for real drafts | Used by `claudeLLM()`. Absent → the `draft` node throws `anthropic 401`, the thread is `failed`. |
| `MODEL` | no (`claude-sonnet-4-20250514`) | Messages API model id; `max_tokens` 600. |
| `DATABASE_URL` | yes in the cluster | The pool in `db.ts`. |
| `PORT` | no (8000) | Listener. |

Scaling: nothing lives in memory between requests — the `StateGraph` instance is stateless and
every request begins with a checkpoint read — so replicas are safe for reads and for requests on
different threads. Two requests on the **same** thread at once race; Postgres's unique index
makes the loser fail rather than fork the history silently. Redis is provisioned by the stack and
unused by this service.

Failure modes:

| What fails | The client sees | The thread says |
|---|---|---|
| LLM error in `draft` | `500` | `failed`; latest checkpoint is the one before `draft` |
| Node throws | `500` | `failed`; resume is `409` (no interrupt), fork from any checkpoint works |
| Resume on a thread not waiting | `409` | unchanged |
| Two resumes at once | one succeeds, one `500` | the loser's insert hit `unique (thread_id, step)` |
| 50 checkpoints in one thread | `500` | `failed` — see Ceilings |
| Process killed mid-drive | connection drops | `running` forever, checkpoints up to the kill are intact — see Ceilings |

Watch: threads by `status`, time from `interrupted` to resume (human latency), `revisions` at
`done`, `failed` rate with the exception message, checkpoints per thread. Logs are the stack's
one-JSON-line format; thread id is the correlation key.

## Ceilings

- **One node per super-step.** `next` is an array but only `next[0]` runs; there is no fan-out or
  parallel branch. Upgrade: `nextOf` returns `string[]`, run them with `Promise.all`, merge all
  updates through the reducers in a fixed order.
- **`maxSteps = 50` counts checkpoints over the thread's whole life**, not per request, and
  `drive` does not pass it. A thread rejected many times hits it. Upgrade: pass a per-drive
  budget from `drive`, or count from `from.step`.
- **A drive runs inside the HTTP request.** Many LLM calls in one drive hold the connection.
  Upgrade: a worker plus streaming (Extending it).
- **A killed process leaves `running`.** The checkpoints are intact, so a sweeper that sets
  `status` back from `running` to `interrupted`/`idle` from the latest checkpoint is enough.
- **The concurrency guard exists only in Postgres.** `memoryStore.put` accepts duplicates, so the
  tests do not exercise it. Upgrade: throw on a duplicate `(thread_id, step)` in `memoryStore`
  and add a test with two concurrent resumes.
- **One graph.** `threads.graph` is always `"workflow"`. Upgrade: Extending it.
- **No auth.** Every thread is visible and resumable by anyone who can reach the service.
- **Checkpoints store the full state every step.** Fine for a `Doc`; large states or long
  threads want deltas or pruning by `created_at`.
- **`/api/health/ready` does not touch the database.**

The stack rules — gate, typed seam, production checklist, deploy — are in `docs/PRODUCTION.md`.
They apply.
"##;

const GRAPH_AGENTS_MD: &str = r##"# {{NAME}} — for agents

`CLAUDE.md` has the rules. This is how to run and test the graph.

## Run

    echo ANTHROPIC_API_KEY=sk-ant-... >> backend/.env   # bun loads backend/.env
    make demo                    # postgres + redis, migrate, seed, backend :8000, frontend :3000
    make check                   # the gate: typecheck both halves, bun test the backend (no key needed)
    make backend                 # API only, with reload

## Routes, with bodies

The graph's nodes:

    curl -s localhost:8000/api/graph          # {"nodes":["draft","review","publish"]}

Start a thread — it drafts, then stops at the human step:

    curl -s -X POST localhost:8000/api/threads -H 'content-type: application/json' -d '{"topic":"why otters hold hands"}'
    # 201 {"id":"a1b2...","status":"interrupted","checkpoint":{"id":"...","thread_id":"a1b2...","step":2,"parent_id":"...","node":"review",
    #   "state":{"topic":"why otters hold hands","draft":"Otters hold hands...","messages":["drafted (rev 0)"],"approved":false,"revisions":0},
    #   "next":["review"],"interrupt":{"question":"Approve this draft?","draft":"Otters hold hands..."}}}

Reject with feedback — back to `draft`, then it asks again:

    curl -s -X POST localhost:8000/api/threads/a1b2.../resume -H 'content-type: application/json' -d '{"approved":false,"feedback":"shorter"}'
    # {"id":"a1b2...","status":"interrupted","checkpoint":{...,"state":{...,"revisions":1,"messages":["drafted (rev 0)","rejected: shorter","drafted (rev 1)"]},"interrupt":{...}}}

Approve — `publish`, then END:

    curl -s -X POST localhost:8000/api/threads/a1b2.../resume -H 'content-type: application/json' -d '{"approved":true}'
    # {"id":"a1b2...","status":"done","checkpoint":{...,"node":"publish","next":[],"interrupt":null,"state":{...,"messages":[...,"approved","published: Otters hold hands..."]}}}

    curl -si -X POST localhost:8000/api/threads/a1b2.../resume -d '{}' | head -1     # HTTP/1.1 409 — not waiting

Read the thread, its history, the list:

    curl -s localhost:8000/api/threads/a1b2...            # {"id":..,"graph":"workflow","status":"done","checkpoint":{latest}}
    curl -s localhost:8000/api/threads/a1b2.../history    # {"checkpoints":[step 0 __start__, draft, review (interrupt), review, draft, review, review, publish]}
    curl -s localhost:8000/api/threads                    # {"threads":[{"id","graph","status"}, ...]}  50, most recently updated first

Fork from the first checkpoint with a different topic — a new thread, the source untouched:

    curl -s -X POST localhost:8000/api/threads/a1b2.../fork -H 'content-type: application/json' \
      -d '{"checkpoint_id":"<id of step 0>","update":{"topic":"beavers"}}'
    # 201 {"id":"c3d4...","status":"interrupted","checkpoint":{...,"state":{"topic":"beavers","draft":"Beavers ..."}}}
    # 404 if the checkpoint is not on thread a1b2...; 400 without a uuid checkpoint_id

## Tests

`backend/src/app.test.ts`, run by `bun test` (part of `make check`). No model, no database:

- `llm` at the top is a fake: it echoes the topic out of the prompt and appends `(revised)` when
  the prompt mentions a rejection. That is enough to assert what each node produced.
- `channels reduce and edges route` builds a two-field `StateGraph` inline, iterates `g.run()`
  directly and asserts the final state, node order and `parent_id` chain — the engine with no
  store and no app.
- The workflow tests use `createApp(memoryStore(), llm)` and Hono's `app.request` (no port):
  start → reject → approve → `409`, then `history` node order; and fork.

To add a test: engine semantics go on a tiny inline graph like the first test; graph behaviour
goes through the app so `drive` and the store are covered. Assert on `history` node order — it
is the most sensitive signal that a checkpoint went missing or doubled. If a test needs
Postgres, the behaviour lives in SQL and should move to `graph.ts` or `app.ts`.

## Migrations

`backend/migrations/0002_checkpoints.sql` holds both tables. New state fields need no migration
(`state` is JSON). New columns: `0003_*.sql`, additive with a default; `make migrate` locally, the
`migrate` init container in the cluster; `pgStore.put` lists every column explicitly, so add it
there and on `Checkpoint`.
"##;

const GRAPH_README_MD: &str = r##"# {{NAME}}

A LangGraph-style stateful agent as a service: nodes, edges, reducers, a checkpoint after every
step, a human-in-the-loop pause, resume, and time travel — in TypeScript you own, with Postgres
as the checkpointer.

## What you get

- A `StateGraph` engine in one file: `addNode`, `addEdge`, `addConditionalEdges`, channel
  reducers (`append`), `START`/`END`, `interrupt()`.
- A working graph: `draft → review (human) → publish`, rejection loops back to `draft` with the
  feedback in the prompt, three rejections end the thread.
- Checkpoints as rows: every super-step's full state, the node that produced it, `next`, the
  parent checkpoint, and the interrupt payload when waiting.
- `POST /api/threads`, `…/resume`, `…/fork`, `…/history`: start, answer the human step,
  branch from any earlier checkpoint with edited state, read the whole chain.
- Refresh, redeploy or restart between two requests and nothing is lost — every response is a
  checkpoint read.
- Claude for the `draft` node; the LLM is a function, so the tests run with no key.
- A page to start, approve or reject, and open past threads; Compose locally; kustomize to a cluster.

## Five minutes

    echo ANTHROPIC_API_KEY=sk-ant-... >> backend/.env
    make demo

Then:

    curl -s -X POST localhost:8000/api/threads -H 'content-type: application/json' -d '{"topic":"why otters hold hands"}'
    # → {"id":"<id>","status":"interrupted","checkpoint":{"node":"review","interrupt":{"question":"Approve this draft?","draft":"..."},...}}

    curl -s -X POST localhost:8000/api/threads/<id>/resume -H 'content-type: application/json' -d '{"approved":false,"feedback":"shorter"}'
    # → status "interrupted" again, state.revisions 1, a new draft

    curl -s -X POST localhost:8000/api/threads/<id>/resume -H 'content-type: application/json' -d '{"approved":true}'
    # → status "done", last message "published: ..."

    curl -s localhost:8000/api/threads/<id>/history | grep -o '"node":"[a-z_]*"'
    # → __start__, draft, review, review, draft, review, review, publish

`http://localhost:3000` is the same flow with buttons, and a list of every thread.

## API

| Method | Path | Auth | What |
|---|---|---|---|
| POST | `/api/threads` | none | `{topic}` → runs to the first interrupt or END; `201 {id, status, checkpoint}` |
| GET | `/api/threads` | none | Last 50 threads, most recently updated first |
| GET | `/api/threads/:id` | none | Thread plus its latest checkpoint |
| GET | `/api/threads/:id/history` | none | Every checkpoint in step order |
| POST | `/api/threads/:id/resume` | none | Body is the interrupt's answer; `409` if not waiting |
| POST | `/api/threads/:id/fork` | none | `{checkpoint_id, update?}` → a new thread from that checkpoint; `404` if not on this thread |
| GET | `/api/graph` | none | Node names |
| GET | `/api/health`, `/api/health/ready` | none | Probes |

## Compared with LangGraph

Same, so their concepts and docs transfer:

- `StateGraph` with nodes as functions returning partial updates, `START`/`END`, fixed and
  conditional edges, channels with reducers (`append` is `add_messages` without the message
  types).
- Super-step execution with a checkpoint per step, `thread_id` as the resume key, a checkpoint id
  as the time-travel key, `parent` links between checkpoints.
- `interrupt()` semantics: the node re-runs from the top on resume and the first `interrupt()`
  call returns the resume value (`Command(resume=…)` is the `/resume` body).
- Fork-from-checkpoint with a state edit, as `update_state` + resume on a new thread.

Better here:

- One engine file you can read in ten minutes, and a typed HTTP API (`AppType`) instead of a
  platform SDK.
- Postgres is the checkpointer from the first commit — no in-memory saver that forgets on restart.
- The tests drive real threads through the API with a fake LLM, no database, in under a second.
- The unique index on `(thread_id, step)` refuses a double history rather than allowing it.
- Kubernetes manifests, probes, secrets rendering and CI that roll only what changed.

Not here yet:

- **Parallel branches, `Send`, map-reduce**: one node per super-step.
- **Subgraphs**, per-node **retry policies**, **streaming modes** (`values`, `updates`,
  `messages` tokens) — a drive returns when it pauses or ends.
- **Prebuilt agents** (`create_react_agent`, `ToolNode`) and message types; the `draft` node is a
  plain prompt.
- **`update_state` on an existing thread** — fork is the only edit, and it makes a new thread.
- **Multiple graphs** per server, assistants, cron, the Platform API surface, LangGraph Studio,
  LangSmith tracing.
- **Cross-thread memory** (the `Store` API), checkpoint pruning, thread deletion.
- **Auth** — every route is open.

## Production

- Envs: `ANTHROPIC_API_KEY`, `MODEL`, `DATABASE_URL`, `PORT`. Rendered from `backend/.env` into a
  Secret by `make k8s-secrets ENV=prod`; committed only as `backend/.env.age`.
- Scaling: stateless between requests; replicas are safe. A drive holds its HTTP connection for
  every LLM call it makes, so set the ingress timeout accordingly or move drives to a worker.
- Probes: `/api/health` (liveness), `/api/health/ready` (readiness — does not yet check Postgres).
- Migrations: `backend/migrations/*.sql`, run by the `migrate` init container before each rollout.
- Deploy: `git push main` → dev; `make release` → prod. `k8s/README.md` explains the manifests.
- What pages you: `failed` threads (the LLM key, quota, or a node throwing), threads stuck in
  `running` (a killed pod mid-drive), `500`s on `/resume` (a concurrency collision on the unique
  index), `interrupted` threads older than your review SLA.

## Roadmap

1. A per-drive step budget instead of the lifetime `maxSteps = 50`.
2. SSE on `POST /api/threads` and `/resume`, one event per checkpoint.
3. A sweeper for `running` threads whose drive died.
4. Parallel `next` with ordered reducer merges.
5. `update_state` on an existing thread.
6. Multiple graphs keyed by `threads.graph`.
7. Auth and per-user thread visibility.
"##;

const GRAPH_AGENT_REVIEW: &str = r##"---
name: graph-semantics
description: Run on any change to backend/src/graph.ts, the workflow, drive(), the checkpoint tables or the resume/fork routes. Checks that checkpoints still mean what LangGraph's mean — resume and time travel are only as correct as the chain they replay.
tools: Read, Grep, Glob, Bash
---

You check that a thread can be paused, resumed, restarted and forked without losing or
duplicating a step. Report each finding as `path:line — what — the sequence of requests that
shows it — the fix`.

Check:
1. `merge` is the only place state changes: no node assigns into `state`, no route edits
   `cp.state` except the fork seed's spread, and reducers are pure.
2. One checkpoint per super-step, yielded after the node's update is merged and `next` is
   computed; `drive` awaits `store.put` before iterating again.
3. The interrupt path: `Interrupt` is caught only in `run`; the interrupted checkpoint keeps
   `state` unchanged, `next: [node]`, the node's step number, and does not advance `parent`;
   the generator `return`s afterwards.
4. Resume: `resumeValue` is set only when `from.interrupt` is non-null, is consumed by the
   first `interrupt()` call, and is `undefined` afterwards so a second `interrupt()` in the same
   node pauses again rather than reusing the answer.
5. A node that calls `interrupt()` does its side effects after the call, or they are idempotent —
   the node runs twice per pause.
6. Conditional edge functions are `(s) => string` with no `await`, no `ctx`, no I/O; every string
   they return is a node name or `END`, or `run` throws `no node`.
7. `next` and `step` on the resumed checkpoint: `step = from.step + 1`, `parent_id = from.id`,
   and `next[0]` is the node that just ran when it interrupted.
8. Fork: the seed copies `state`, sets `interrupt: null`, `thread_id` to the new thread and
   `parent_id` to the source checkpoint; the route checks `cp.thread_id === :id`; the source
   thread's status and checkpoints are untouched.
9. `drive` sets `running` first and every exit sets `interrupted`, `done` or `failed`; a thrown
   error is rethrown so the route does not report success.
10. `unique (thread_id, step)` is still in the migration and `pgStore.put` still inserts rather
    than upserts — an upsert would let a concurrent resume overwrite history silently.
11. `maxSteps`: the loop cannot run forever on a cyclic graph; the `stopped after` error still
    fires and marks the thread `failed`.
12. The initial state in `POST /api/threads` covers every `Doc` field, so no node reads
    `undefined` and no reducer gets `undefined` as its left side.
13. The routes stay one chained expression so `AppType` includes them; the shapes the page reads
    (`checkpoint.state`, `checkpoint.interrupt.question`, `status`) still exist.

End with one line: `graph-semantics: N findings`, and if 0, which of the above you ran.
"##;
