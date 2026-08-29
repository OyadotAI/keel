//! graph: a stateful graph agent, like LangGraph ══════════════════════════════════════════

pub fn files(name: &str) -> Vec<(&'static str, String)> {
    let f = |s: &str| s.replace("{{NAME}}", name);
    let _ = &f;
    vec![
        ("backend/src/app.ts", GRAPH_APP.into()),
        ("backend/src/store.ts", GRAPH_STORE.into()),
        ("backend/src/graph.ts", GRAPH_ENGINE.into()),
        ("backend/src/app.test.ts", GRAPH_TEST.into()),
        ("backend/migrations/0002_checkpoints.sql", GRAPH_SQL.into()),
        ("frontend/app/page.tsx", f(GRAPH_PAGE)),
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
