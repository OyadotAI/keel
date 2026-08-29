//! loop: a ReAct agent, like smolagents ════════════════════════════════════════════════════

pub fn files(name: &str) -> Vec<(&'static str, String)> {
    let f = |s: &str| s.replace("{{NAME}}", name);
    let _ = &f;
    vec![
        ("backend/src/app.ts", LOOP_APP.into()),
        ("backend/src/store.ts", LOOP_STORE.into()),
        ("backend/src/agent.ts", LOOP_AGENT.into()),
        ("backend/src/app.test.ts", LOOP_TEST.into()),
        ("backend/migrations/0002_runs.sql", LOOP_SQL.into()),
        ("frontend/app/page.tsx", f(LOOP_PAGE)),
    ]
}

const LOOP_SQL: &str = r##"create table if not exists runs (
  id uuid primary key,
  task text not null,
  state text not null default 'running',   -- running | done | failed | stopped
  answer text,
  steps jsonb not null default '[]',
  input_tokens int not null default 0,
  output_tokens int not null default 0,
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now()
);
"##;

const LOOP_STORE: &str = r##"import { db } from "./db";
import type { Step } from "./agent";

export type Run = { id: string; task: string; state: string; answer: string | null; steps: Step[]; input_tokens: number; output_tokens: number; created_at: string };

export interface Store {
  create(id: string, task: string): Promise<void>;
  update(id: string, patch: Partial<Run>): Promise<void>;
  get(id: string): Promise<Run | null>;
  list(limit: number): Promise<Run[]>;
}

export function pgStore(): Store {
  return {
    create: async (id, task) => { await db`insert into runs (id, task) values (${id}, ${task})`; },
    update: async (id, p) => {
      await db`update runs set state = coalesce(${p.state ?? null}, state), answer = coalesce(${p.answer ?? null}, answer),
        steps = coalesce(${p.steps ? db.json(p.steps as never) : null}, steps), input_tokens = coalesce(${p.input_tokens ?? null}, input_tokens),
        output_tokens = coalesce(${p.output_tokens ?? null}, output_tokens), updated_at = now() where id = ${id}`;
    },
    get: async (id) => (await db<Run[]>`select * from runs where id = ${id}`)[0] ?? null,
    list: async (limit) => db<Run[]>`select id, task, state, answer, '[]'::jsonb as steps, input_tokens, output_tokens, created_at from runs order by created_at desc limit ${limit}`,
  };
}

export function memoryStore(): Store {
  const runs = new Map<string, Run>();
  return {
    create: async (id, task) => { runs.set(id, { id, task, state: "running", answer: null, steps: [], input_tokens: 0, output_tokens: 0, created_at: new Date().toISOString() }); },
    update: async (id, p) => { const r = runs.get(id); if (r) Object.assign(r, p); },
    get: async (id) => runs.get(id) ?? null,
    list: async (limit) => [...runs.values()].reverse().slice(0, limit),
  };
}
"##;

const LOOP_AGENT: &str = r##"// The ReAct loop as smolagents runs it: each step is a model call that may request tool calls;
// tools run; observations go back; the loop ends when the model calls `final_answer`, or hits
// max_steps and is asked once more for its best answer. Every step is a record — model output,
// tool calls, observations, tokens, duration — so a run can be replayed and inspected. The
// model is a function, so tests drive the loop with a scripted one and never call a provider.

export type ToolCall = { id: string; name: string; input: Record<string, unknown> };
export type Step = { n: number; thought: string; calls: ToolCall[]; observations: { id: string; output: string; error?: boolean }[]; input_tokens: number; output_tokens: number; ms: number };
export type ModelReply = { text: string; calls: ToolCall[]; input_tokens: number; output_tokens: number; stop: "tool_use" | "end_turn" | "max_tokens" };
export type Message = { role: "user" | "assistant"; content: unknown };
export type Model = (system: string, messages: Message[], tools: ToolDef[]) => Promise<ModelReply>;
export type ToolDef = { name: string; description: string; input_schema: Record<string, unknown>; run: (input: Record<string, unknown>) => Promise<string> };

export const FINAL = "final_answer";

/// The tools the agent has. Add one here; the model sees its schema on the next run.
export const TOOLS: ToolDef[] = [
  { name: "calculator", description: "Evaluate an arithmetic expression (+ - * / ( ) and numbers).", input_schema: { type: "object", properties: { expression: { type: "string" } }, required: ["expression"] },
    run: async ({ expression }) => { const e = String(expression); if (!/^[\d\s+\-*/().]+$/.test(e)) throw new Error("only arithmetic"); return String(Function(`"use strict"; return (${e})`)()); } },
  { name: "web_search", description: "Search the web; returns titles and snippets.", input_schema: { type: "object", properties: { query: { type: "string" } }, required: ["query"] },
    run: async ({ query }) => { const r = await fetch(`https://duckduckgo.com/html/?q=${encodeURIComponent(String(query))}`, { headers: { "user-agent": "Mozilla/5.0" } }); const html = await r.text(); const hits = [...html.matchAll(/class="result__a"[^>]*>([^<]+)</g)].slice(0, 5).map((m) => m[1]); return hits.join("\n") || "no results"; } },
  { name: FINAL, description: "Give the final answer to the task. Call this when done.", input_schema: { type: "object", properties: { answer: { type: "string" } }, required: ["answer"] }, run: async ({ answer }) => String(answer) },
];

export const SYSTEM = `You solve tasks step by step using tools. Think briefly, then call tools. When you know the answer, call final_answer with it. Never make up tool results.`;

export type RunResult = { answer: string | null; state: "done" | "failed" | "stopped"; steps: Step[]; input_tokens: number; output_tokens: number };

export async function runAgent(task: string, model: Model, opts: { tools?: ToolDef[]; maxSteps?: number; onStep?: (s: Step) => Promise<void> | void; signal?: AbortSignal } = {}): Promise<RunResult> {
  const tools = opts.tools ?? TOOLS; const maxSteps = opts.maxSteps ?? 10;
  const messages: Message[] = [{ role: "user", content: task }];
  const steps: Step[] = []; let input_tokens = 0, output_tokens = 0;

  for (let n = 1; n <= maxSteps; n++) {
    if (opts.signal?.aborted) return { answer: null, state: "stopped", steps, input_tokens, output_tokens };
    const t0 = Date.now();
    const reply = await model(SYSTEM, messages, tools);
    input_tokens += reply.input_tokens; output_tokens += reply.output_tokens;
    const step: Step = { n, thought: reply.text, calls: reply.calls, observations: [], input_tokens: reply.input_tokens, output_tokens: reply.output_tokens, ms: 0 };
    const blocks: unknown[] = reply.text ? [{ type: "text", text: reply.text }] : [];
    for (const c of reply.calls) blocks.push({ type: "tool_use", id: c.id, name: c.name, input: c.input });
    messages.push({ role: "assistant", content: blocks });

    const final = reply.calls.find((c) => c.name === FINAL);
    if (final) { step.ms = Date.now() - t0; steps.push(step); await opts.onStep?.(step); return { answer: String(final.input.answer ?? ""), state: "done", steps, input_tokens, output_tokens }; }
    if (!reply.calls.length) {
      // Text with no tool call: treat as the answer, the way smolagents does when the model stops calling.
      step.ms = Date.now() - t0; steps.push(step); await opts.onStep?.(step);
      return { answer: reply.text || null, state: reply.text ? "done" : "failed", steps, input_tokens, output_tokens };
    }
    for (const c of reply.calls) {
      const tool = tools.find((t) => t.name === c.name);
      try {
        if (!tool) throw new Error(`unknown tool ${c.name}`);
        const output = await Promise.race([tool.run(c.input), new Promise<string>((_, rej) => setTimeout(() => rej(new Error("tool timed out")), 30_000).unref())]);
        step.observations.push({ id: c.id, output: output.slice(0, 8000) });
      } catch (e) {
        // An error is an observation, not an exception: the model gets to recover.
        step.observations.push({ id: c.id, output: `Error: ${e instanceof Error ? e.message : String(e)}`, error: true });
      }
    }
    messages.push({ role: "user", content: step.observations.map((o) => ({ type: "tool_result", tool_use_id: o.id, content: o.output, is_error: o.error ?? false })) });
    step.ms = Date.now() - t0; steps.push(step); await opts.onStep?.(step);
  }
  // Out of steps: one last call with no tools, for the best answer so far.
  const last = await model(SYSTEM, [...messages, { role: "user", content: "You are out of steps. Give your best final answer now as plain text." }], []);
  input_tokens += last.input_tokens; output_tokens += last.output_tokens;
  return { answer: last.text || null, state: last.text ? "done" : "failed", steps, input_tokens, output_tokens };
}

/// Claude via the Messages API. Set ANTHROPIC_API_KEY; MODEL defaults to Sonnet.
export function claude(fetchImpl: typeof fetch = fetch): Model {
  return async (system, messages, tools) => {
    const res = await fetchImpl("https://api.anthropic.com/v1/messages", {
      method: "POST", headers: { "x-api-key": process.env.ANTHROPIC_API_KEY ?? "", "anthropic-version": "2023-06-01", "content-type": "application/json" },
      body: JSON.stringify({ model: process.env.MODEL ?? "claude-sonnet-4-20250514", max_tokens: 2048, system, messages, tools: tools.map((t) => ({ name: t.name, description: t.description, input_schema: t.input_schema })) }),
    });
    if (!res.ok) throw new Error(`anthropic ${res.status}: ${(await res.text()).slice(0, 300)}`);
    const a = (await res.json()) as { content: { type: string; text?: string; id?: string; name?: string; input?: Record<string, unknown> }[]; stop_reason: string; usage: { input_tokens: number; output_tokens: number } };
    return {
      text: a.content.filter((b) => b.type === "text").map((b) => b.text).join(""),
      calls: a.content.filter((b) => b.type === "tool_use").map((b) => ({ id: b.id!, name: b.name!, input: b.input ?? {} })),
      input_tokens: a.usage.input_tokens, output_tokens: a.usage.output_tokens,
      stop: a.stop_reason === "tool_use" ? "tool_use" : a.stop_reason === "max_tokens" ? "max_tokens" : "end_turn",
    };
  };
}
"##;

const LOOP_APP: &str = r##"import { Hono } from "hono";
import { streamSSE } from "hono/streaming";
import { z } from "zod";
import { pgStore, type Store } from "./store";
import { runAgent, claude, TOOLS, type Model } from "./agent";

// POST /api/runs starts a run and streams its steps as SSE; every step is stored as it happens,
// so a run can be read back after the connection is gone. Concurrency is bounded — an agent
// loop is an expensive request — and a run can be stopped.

const running = new Map<string, AbortController>();
const MAX_CONCURRENT = Number(process.env.MAX_CONCURRENT_RUNS ?? 4);

export function createApp(store: Store, model: Model = claude()) {
  const app = new Hono()
    .get("/api/health", (c) => c.json({ status: "ok" }))
    .get("/api/health/ready", (c) => c.json({ status: "ok", db: "ok" as const }))
    .get("/api/tools", (c) => c.json({ tools: TOOLS.map((t) => ({ name: t.name, description: t.description, input_schema: t.input_schema })) }))

    .post("/api/runs", async (c) => {
      const p = z.object({ task: z.string().min(1).max(4000), max_steps: z.number().int().min(1).max(30).default(10) }).safeParse(await c.req.json().catch(() => ({})));
      if (!p.success) return c.json({ error: { message: "task required", code: "invalid" } }, 400);
      if (running.size >= MAX_CONCURRENT) { c.header("Retry-After", "10"); return c.json({ error: { message: "too many runs in flight", code: "busy" } }, 429); }
      const id = crypto.randomUUID();
      await store.create(id, p.data.task);
      const ctl = new AbortController(); running.set(id, ctl);
      return streamSSE(c, async (s) => {
        await s.writeSSE({ event: "run", data: JSON.stringify({ id }) });
        s.onAbort(() => ctl.abort());
        const steps: unknown[] = [];
        try {
          const r = await runAgent(p.data.task, model, { maxSteps: p.data.max_steps, signal: ctl.signal, onStep: async (step) => {
            steps.push(step);
            await store.update(id, { steps: steps as never });
            await s.writeSSE({ event: "step", data: JSON.stringify(step) });
          } });
          await store.update(id, { state: r.state, answer: r.answer, steps: r.steps, input_tokens: r.input_tokens, output_tokens: r.output_tokens });
          await s.writeSSE({ event: "done", data: JSON.stringify({ state: r.state, answer: r.answer, input_tokens: r.input_tokens, output_tokens: r.output_tokens }) });
        } catch (e) {
          await store.update(id, { state: "failed", answer: e instanceof Error ? e.message : String(e) });
          await s.writeSSE({ event: "error", data: JSON.stringify({ message: e instanceof Error ? e.message : String(e) }) });
        } finally { running.delete(id); }
      });
    })
    .get("/api/runs", async (c) => c.json({ runs: await store.list(50) }))
    .get("/api/runs/:id", async (c) => { const r = await store.get(c.req.param("id")); return r ? c.json(r) : c.json({ error: { message: "no such run", code: "not_found" } }, 404); })
    .post("/api/runs/:id/stop", (c) => { const ctl = running.get(c.req.param("id")); if (!ctl) return c.json({ error: { message: "not running", code: "not_found" } }, 404); ctl.abort(); return c.json({ ok: true }); });

  return app;
}

const app = createApp(pgStore());
export type AppType = typeof app;
export default app;
"##;

const LOOP_TEST: &str = r##"import { describe, expect, test } from "bun:test";
import { createApp } from "./app";
import { memoryStore } from "./store";
import { runAgent, TOOLS, type Model, type ModelReply } from "./agent";

// A scripted model drives the loop: tool call → observation → final_answer. No provider.
const script = (replies: Partial<ModelReply>[]): Model => { let i = 0; return async () => ({ text: "", calls: [], input_tokens: 10, output_tokens: 5, stop: "tool_use", ...replies[Math.min(i++, replies.length - 1)] }); };

describe("agent loop", () => {
  test("calls a tool, reads the observation, and answers", async () => {
    const model = script([
      { text: "I should compute it.", calls: [{ id: "t1", name: "calculator", input: { expression: "6*7" } }] },
      { text: "", calls: [{ id: "t2", name: "final_answer", input: { answer: "42" } }] },
    ]);
    const r = await runAgent("what is 6*7", model);
    expect(r.state).toBe("done"); expect(r.answer).toBe("42");
    expect(r.steps.length).toBe(2);
    expect(r.steps[0].observations[0].output).toBe("42");
    expect(r.input_tokens).toBe(20);
  });

  test("a tool error is an observation the model can recover from", async () => {
    const model = script([
      { calls: [{ id: "t1", name: "calculator", input: { expression: "rm -rf /" } }] },
      { calls: [{ id: "t2", name: "nope", input: {} }] },
      { calls: [{ id: "t3", name: "final_answer", input: { answer: "could not" } }] },
    ]);
    const r = await runAgent("x", model);
    expect(r.steps[0].observations[0].error).toBe(true);
    expect(r.steps[1].observations[0].output).toContain("unknown tool");
    expect(r.answer).toBe("could not");
  });

  test("max_steps ends the loop with a best answer", async () => {
    let calls = 0;
    const model: Model = async (_s, _m, tools) => { calls++; return tools.length ? { text: "", calls: [{ id: String(calls), name: "calculator", input: { expression: "1+1" } }], input_tokens: 1, output_tokens: 1, stop: "tool_use" } : { text: "probably 2", calls: [], input_tokens: 1, output_tokens: 1, stop: "end_turn" }; };
    const r = await runAgent("x", model, { maxSteps: 3 });
    expect(r.steps.length).toBe(3);
    expect(calls).toBe(4);
    expect(r.answer).toBe("probably 2");
  });

  test("the route streams steps and stores the run", async () => {
    const store = memoryStore();
    const app = createApp(store, script([{ calls: [{ id: "t", name: "final_answer", input: { answer: "hi" } }] }]));
    const res = await app.request("/api/runs", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ task: "say hi" }) });
    expect(res.status).toBe(200);
    const text = await res.text();
    expect(text).toContain("event: step"); expect(text).toContain("event: done");
    const id = JSON.parse(text.split("\n").find((l) => l.startsWith("data:"))!.slice(5)).id;
    const run = await (await app.request(`/api/runs/${id}`)).json();
    expect(run.state).toBe("done"); expect(run.answer).toBe("hi");
    expect(TOOLS.some((t) => t.name === "final_answer")).toBe(true);
  });
});
"##;

const LOOP_PAGE: &str = r##""use client";
import { useState } from "react";

type Step = { n: number; thought: string; calls: { name: string; input: unknown }[]; observations: { output: string; error?: boolean }[]; ms: number };

// Type a task, watch the steps arrive. Each step is a model call and its tool results.
export default function Home() {
  const [task, setTask] = useState("What is 17 * 23, and is it prime?");
  const [steps, setSteps] = useState<Step[]>([]);
  const [answer, setAnswer] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  async function run() {
    setBusy(true); setSteps([]); setAnswer(null);
    const res = await fetch("/api/runs", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ task }) });
    const reader = res.body!.getReader(); const dec = new TextDecoder(); let buf = "";
    for (;;) {
      const { value, done } = await reader.read(); if (done) break;
      buf += dec.decode(value, { stream: true });
      let i; while ((i = buf.indexOf("\n\n")) >= 0) {
        const block = buf.slice(0, i); buf = buf.slice(i + 2);
        const ev = /event: (\w+)/.exec(block)?.[1]; const data = /data: (.*)/.exec(block)?.[1];
        if (!ev || !data) continue;
        if (ev === "step") setSteps((s) => [...s, JSON.parse(data)]);
        if (ev === "done") setAnswer(JSON.parse(data).answer);
        if (ev === "error") setAnswer("Error: " + JSON.parse(data).message);
      }
    }
    setBusy(false);
  }

  return (
    <main>
      <h1>{{NAME}} — agent</h1>
      <p>A ReAct loop with a calculator and web search. Set <code>ANTHROPIC_API_KEY</code> in backend/.env.</p>
      <textarea value={task} onChange={(e) => setTask(e.target.value)} rows={3} style={{ width: "100%" }} />
      <p><button onClick={run} disabled={busy}>{busy ? "Running…" : "Run"}</button></p>
      {steps.map((s) => (
        <details key={s.n} open>
          <summary>Step {s.n} · {s.calls.map((c) => c.name).join(", ") || "thinking"} · {s.ms} ms</summary>
          {s.thought && <p><em>{s.thought}</em></p>}
          {s.calls.map((c, i) => <pre key={i}>{c.name}({JSON.stringify(c.input)}){"\n→ "}{s.observations[i]?.output}</pre>)}
        </details>
      ))}
      {answer && <p><strong>Answer:</strong> {answer}</p>}
    </main>
  );
}
"##;
