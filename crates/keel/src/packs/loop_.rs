//! loop: a ReAct agent, like smolagents ════════════════════════════════════════════════════

pub fn files(name: &str) -> Vec<(&'static str, String)> {
    let f = |s: &str| s.replace("{{NAME}}", name);
    vec![
        ("backend/src/app.ts", LOOP_APP.into()),
        ("backend/src/store.ts", LOOP_STORE.into()),
        ("backend/src/agent.ts", LOOP_AGENT.into()),
        ("backend/src/app.test.ts", LOOP_TEST.into()),
        ("backend/migrations/0002_runs.sql", LOOP_SQL.into()),
        ("frontend/app/page.tsx", f(LOOP_PAGE)),
        ("CLAUDE.md", f(LOOP_CLAUDE_MD)),
        ("AGENTS.md", f(LOOP_AGENTS_MD)),
        ("README.md", f(LOOP_README_MD)),
        (".claude/agents/agent-loop.md", LOOP_AGENT_REVIEW.into()),
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

const LOOP_CLAUDE_MD: &str = r##"# {{NAME}} — agent loop

A ReAct agent service modelled on smolagents' `ToolCallingAgent`: each step is one model call
that may request tool calls; the tools run; their observations go back as the next message; the
loop ends when the model calls `final_answer`, answers in plain text, or runs out of steps and is
asked once more for its best answer. Every step is a record — thought, calls, observations,
tokens, milliseconds — streamed over SSE as it happens and stored so the run can be read back
later. The model is a function, so the tests script it and never call a provider. "Done" here
means a run's stored steps are enough to explain its answer, the gate is green with no network,
and nothing in `agent.ts` knows about HTTP or Postgres.

## Architecture

| File | Owns |
|---|---|
| `backend/src/agent.ts` | The loop (`runAgent`), the `Model`/`ToolDef`/`Step` types, `TOOLS` (`calculator`, `web_search`, `final_answer`), `SYSTEM`, and `claude()` — the one provider. |
| `backend/src/store.ts` | `Store`: `create`/`update`/`get`/`list` over `runs`; `pgStore` and `memoryStore`. |
| `backend/src/app.ts` | Routes; the per-process `running` map of `AbortController`s and the `MAX_CONCURRENT_RUNS` gate. `AppType` is exported here. |
| `backend/src/app.test.ts` | A scripted `Model` drives the loop; the route test reads the SSE text and the stored run. |
| `backend/migrations/0002_runs.sql` | `runs`. |
| `backend/src/db.ts`, `server.ts`, `migrate.ts` | Pool, listener with SIGTERM drain, migration runner — unchanged from the stack. |
| `frontend/app/page.tsx` | Client component: `POST /api/runs`, parses the SSE by hand, renders each step as it arrives. |

Request path for `POST /api/runs`:

1. zod: `task` 1–4000 chars, `max_steps` 1–30 (default 10). Invalid → `400`.
2. `running.size >= MAX_CONCURRENT_RUNS` → `429` with `Retry-After: 10`.
3. `store.create(id, task)` — the row exists before the first model call, state `running`.
4. The response is an SSE stream; first event `run` with `{id}`. Client disconnect aborts the run.
5. `runAgent` loops: `model(SYSTEM, messages, tools)` → if `final_answer` is among the calls, done;
   if no calls, the text is the answer; otherwise each tool runs (30 s cap, output cut at 8000
   chars, errors become observations) and the results go back as `tool_result` blocks.
6. After each step `onStep` runs: `store.update(id, {steps})` **then** `writeSSE("step")`.
7. On return: `store.update` with state, answer and token totals; `done` event. On throw:
   state `failed`, answer = the message, `error` event. Either way `running.delete(id)`.

Data model — `runs`:

| Column | Why |
|---|---|
| `id uuid` | Handed to the client in the first SSE event, so a dropped stream can `GET /api/runs/:id`. |
| `task` | The prompt as given. |
| `state` | `running` → `done` \| `failed` \| `stopped`. `stopped` means the abort signal was seen at a step boundary. |
| `answer` | The final answer, or the error message when `failed`. |
| `steps jsonb` | The full `Step[]`, rewritten after every step. `GET /api/runs` (the list) returns `[]` here on purpose. |
| `input_tokens`, `output_tokens` | Summed from the provider's `usage`; no estimates. |

## Invariants

1. **The model is a parameter.** `runAgent(task, model, …)` and `createApp(store, model)`;
   nothing in `agent.ts` reads the network except inside `claude()`. Guarded by every test in
   `describe("agent loop")` passing with the `script()` model and no `ANTHROPIC_API_KEY`.
2. **A tool error is an observation, never an exception.** Unknown tool, thrown error, timeout —
   all become `{output: "Error: …", error: true}` and the model gets another turn. Guarded by
   `a tool error is an observation the model can recover from`.
3. **`final_answer` ends the loop, and so does a reply with no tool calls.** The answer is the
   `answer` input or the text; empty text is `failed`. Guarded by
   `calls a tool, reads the observation, and answers`.
4. **`max_steps` bounds model calls at `max_steps + 1`**: the extra call has no tools and asks
   for a best answer. Guarded by `max_steps ends the loop with a best answer` (`calls === 4` for 3).
5. **Store before stream.** `onStep` writes the run and only then emits the SSE event, so a client
   never sees a step that is not on disk. Guarded by `the route streams steps and stores the run`
   (reads the run back by the id from the stream).
6. **Tokens are the provider's numbers**, summed per run, including the best-answer call. Guarded
   by the `input_tokens === 20` assertion in the first test.
7. **Tool output is bounded**: 30 s (`Promise.race` with an unref'd timer) and 8000 characters.
   A tool cannot stall a run or flood the context. No direct test; the cap is one line in
   `runAgent` and a reviewer checks it stays.
8. **The calculator never evaluates arbitrary code.** `Function()` runs only after the
   `^[\d\s+\-*/().]+$` allowlist. Guarded by the `rm -rf /` case in the error test.
9. **Stop is cooperative.** `POST /api/runs/:id/stop` aborts the controller; the loop checks the
   signal at the top of each step, so a tool or model call in flight completes first. State is
   then `stopped`, answer `null`.
10. **The run row exists before the first model call and is finalised in `finally`.** A crash
    mid-run leaves `failed` with the message, never `running` forever — except a process kill
    (see Ceilings).
11. **Concurrency is bounded per process** by `MAX_CONCURRENT_RUNS` and refused with `429` and
    `Retry-After`, not queued. No test yet; add one with two scripted runs whose model awaits a
    promise you resolve after asserting the third request is `429`.
12. **A tool is a `ToolDef` in `TOOLS`** with a JSON schema; the model sees exactly
    `{name, description, input_schema}` and `run` never leaves the process. Guarded by
    `GET /api/tools` shape and the `TOOLS.some(final_answer)` assertion.

## Extending it

**Add a tool**: append a `ToolDef` to `TOOLS` in `agent.ts` — `name`, one-sentence `description`
(the model reads it), `input_schema` as JSON Schema, `run(input) => Promise<string>`. Validate
`input` inside `run` and throw on bad input; the throw is the model's feedback. Test: a scripted
model that calls it, asserting the observation, in `describe("agent loop")`. No migration.

**Add a provider** (OpenAI, a local model): a second `Model` factory beside `claude()` that maps
the provider's response to `ModelReply` — `text`, `calls[{id,name,input}]`, `usage`, `stop`.
Choose it in `createApp(pgStore(), pick())` at the bottom of `app.ts` from an env var. Test with
`fetchImpl` injected: pass a fake `fetch` returning the provider's JSON and assert the mapping.

**Change the system prompt or add per-run instructions**: `SYSTEM` is a const; to make it
per-run, add `system?: string` to `runAgent`'s `opts` and to the `POST /api/runs` zod body, cap
its length, and store it in a new column via `backend/migrations/0003_*.sql`.

**Add planning steps** (smolagents' `planning_interval`): in `runAgent`, every N steps call the
model with no tools and a "summarise progress and plan" user message, push the reply as a step
with `calls: []`, and continue. Test with the `max_steps` pattern, asserting call count.

**Persist conversation history across runs**: add `parent_run_id uuid` to `runs`, load the
parent's steps in `POST /api/runs`, and rebuild `messages` from them before the loop. Test that a
second run's first model call receives the first run's tool results.

**Run tools in parallel**: the `for (const c of reply.calls)` loop is sequential; replace with
`Promise.all` over the calls, keeping observation order equal to call order (the `tool_result`
blocks must match `tool_use` ids). Test with two calls in one reply.

**Add a route**: one more link in the chained `app` in `app.ts`, never a separate
`app.get(...)`, or `AppType` drops it. Test through `app.request`.

## Operating it

| Env | Required | Meaning |
|---|---|---|
| `ANTHROPIC_API_KEY` | yes, for real runs | Sent as `x-api-key` by `claude()`. Absent → every run `failed` with `anthropic 401`. |
| `MODEL` | no (`claude-sonnet-4-20250514`) | The Messages API model id. |
| `MAX_CONCURRENT_RUNS` | no (4) | Per process. Above it, `429`. |
| `DATABASE_URL` | yes in the cluster | The pool in `db.ts`. |
| `PORT` | no (8000) | Listener. |

Scaling: a run holds one HTTP connection open for its whole life (tens of seconds to minutes)
and makes one provider call per step, so the limit is provider rate and concurrent streams, not
CPU. Replicas multiply `MAX_CONCURRENT_RUNS`. What is per-replica today:

- **`running`** — the map from run id to `AbortController`. `POST /api/runs/:id/stop` only works
  on the replica that owns the run; elsewhere it is `404`. With replicas, either route stops by
  session affinity, or move the stop signal to Redis (`SET stop:<id>`) polled at each step boundary.
- **The concurrency gate** — counts this process's runs only.

Redis is provisioned by the stack and unused by this service.

Failure modes:

| What fails | The client sees | The row says |
|---|---|---|
| Provider 4xx/5xx | `error` event with `anthropic <status>: …` | `failed`, answer = message |
| Tool throws or exceeds 30 s | nothing special: the next step's observation says `Error: …` | continues |
| Client disconnects | — | `stopped` at the next step boundary (`onAbort` → `ctl.abort()`) |
| `MAX_CONCURRENT_RUNS` reached | `429`, `Retry-After: 10` | no row |
| Process killed mid-run | stream ends | `running` forever — see Ceilings |
| Postgres down | `POST /api/runs` throws before the stream starts → `500` | — |

Watch: runs by `state`, steps per run, `input_tokens + output_tokens` per run (cost), p95 step
`ms`, `429` count, rows stuck in `running` older than a few minutes. Logs are the stack's
one-JSON-line format; the run id is the correlation key.

## Ceilings

- **Stop and the concurrency gate are per process.** Upgrade: Redis for both, checked at the
  step boundary where the abort signal is checked today.
- **A killed process leaves `running` rows.** Upgrade: a sweeper (`update runs set state =
  'failed' where state = 'running' and updated_at < now() - interval '10 minutes'`) in a
  cron or at startup.
- **The run lives inside the request.** Long tasks depend on the client holding the stream, and
  a load balancer idle timeout ends the run. Upgrade: a worker that owns the loop, with the
  route enqueueing and a separate `GET /api/runs/:id/events` that tails the stored steps.
- **`web_search` scrapes DuckDuckGo's HTML** with a regex. It will break when the markup changes
  and it is not rate-limited. Upgrade: a search API with a key, behind the same `ToolDef`.
- **No auth on any route.** Anyone who can reach the service can spend provider tokens.
  Upgrade: put it behind the stack's session and rate limit per principal.
- **Tool timeout (30 s) and output cap (8000) are constants.** Upgrade: per-`ToolDef` fields.
- **`steps` is rewritten whole after every step** (`update … set steps = …`). Fine to a few
  hundred steps; `max_steps` is capped at 30. Upgrade: a `steps` table if that cap moves.
- **`pgStore.list` returns `[]` for `steps`; `memoryStore.list` returns the full array.** The
  tests cannot see the difference. Upgrade: strip steps in `memoryStore.list` too, and add a test.
- **`steps` accumulate in memory in the route (`steps.push`)** as well as in the loop — two
  copies per run. Harmless at 30 steps.

The stack rules — gate, typed seam, production checklist, deploy — are in `docs/PRODUCTION.md`.
They apply.
"##;

const LOOP_AGENTS_MD: &str = r##"# {{NAME}} — for agents

`CLAUDE.md` has the rules. This is how to run and test the agent loop.

## Run

    echo ANTHROPIC_API_KEY=sk-ant-... >> backend/.env   # bun loads backend/.env
    make demo                    # postgres + redis, migrate, seed, backend :8000, frontend :3000
    make check                   # the gate: typecheck both halves, bun test the backend (no key needed)
    make backend                 # API only, with reload

## Routes, with bodies

List the tools the model sees:

    curl -s localhost:8000/api/tools
    # {"tools":[{"name":"calculator","description":"Evaluate an arithmetic expression ...","input_schema":{...}},{"name":"web_search",...},{"name":"final_answer",...}]}

Start a run and watch the steps (SSE; `-N` disables buffering):

    curl -N -X POST localhost:8000/api/runs -H 'content-type: application/json' \
      -d '{"task":"What is 17 * 23, and is it prime?","max_steps":6}'
    # event: run
    # data: {"id":"6f1c..."}
    #
    # event: step
    # data: {"n":1,"thought":"I'll compute it.","calls":[{"id":"toolu_..","name":"calculator","input":{"expression":"17*23"}}],"observations":[{"id":"toolu_..","output":"391"}],"input_tokens":412,"output_tokens":58,"ms":1830}
    #
    # event: step
    # data: {"n":2,"thought":"","calls":[{"id":"toolu_..","name":"final_answer","input":{"answer":"391, and it is not prime (17 × 23)."}}],"observations":[],...}
    #
    # event: done
    # data: {"state":"done","answer":"391, and it is not prime (17 × 23).","input_tokens":1020,"output_tokens":91}

Invalid body, and the concurrency gate:

    curl -si -X POST localhost:8000/api/runs -d '{}' | head -1                  # HTTP/1.1 400
    # with MAX_CONCURRENT_RUNS runs in flight:                                    HTTP/1.1 429, Retry-After: 10

Read a run back (full steps), list runs (steps omitted), stop one:

    curl -s localhost:8000/api/runs/6f1c...        # {"id":"6f1c...","task":"...","state":"done","answer":"...","steps":[...],"input_tokens":1020,"output_tokens":91,"created_at":"..."}
    curl -s localhost:8000/api/runs                 # {"runs":[{"id":"...","state":"done","steps":[],...}, ...]}  newest first, 50
    curl -s -X POST localhost:8000/api/runs/6f1c.../stop   # {"ok":true}, or 404 if it is not running on this replica

## Tests

`backend/src/app.test.ts`, run by `bun test` (part of `make check`). No provider, no database:

- `script([...replies])` returns a `Model` that replays a list of partial `ModelReply`s, one per
  call, repeating the last. A reply is `{text, calls, input_tokens, output_tokens, stop}`; the
  defaults are `10`/`5` tokens and `stop: "tool_use"`.
- Loop tests call `runAgent(task, model, opts)` directly and assert on `state`, `answer`,
  `steps[n].observations` and token totals.
- The route test builds `createApp(memoryStore(), model)` and reads the SSE as text with
  `app.request` (Hono, no port), then fetches `/api/runs/:id` to check what was stored.
- The `max_steps` test uses a hand-written `Model` that looks at `tools.length` to tell the
  normal call from the best-answer call.

To add a test: for loop behaviour, script the model and assert on steps; for a tool, call
`TOOLS.find(t => t.name === "…")!.run(input)` directly, or script a model that calls it. For a
provider, inject `fetchImpl` into `claude(fetchImpl)` and return canned JSON. Nothing in this
suite should need `ANTHROPIC_API_KEY`; if it does, the mapping and the loop are entangled.

## Migrations

`runs` is `backend/migrations/0002_runs.sql`. New columns: `0003_*.sql`, additive with a default;
`make migrate` locally, the `migrate` init container in the cluster. `pgStore.update` coalesces
each column, so a new column needs its own `coalesce` line and a field on `Run`.
"##;

const LOOP_README_MD: &str = r##"# {{NAME}}

A tool-calling agent as a service: `POST` a task, watch each step stream back, read the whole
run later. Modelled on smolagents' `ToolCallingAgent`, in TypeScript you own.

## What you get

- `POST /api/runs` — a ReAct loop (think → call tools → observe → repeat) streamed as
  server-sent events: `run`, `step`…, `done` or `error`.
- Three tools out of the box — `calculator`, `web_search`, `final_answer` — and a `ToolDef`
  shape for adding yours in one place.
- Every step persisted as it happens: thought, tool calls, observations, tokens, duration.
  A dropped connection loses nothing; `GET /api/runs/:id` has it all.
- Bounded by construction: `max_steps` (≤ 30) plus one best-answer call, 30 s per tool call,
  8000 characters per observation, `MAX_CONCURRENT_RUNS` per process with `429` + `Retry-After`.
- Tool errors are observations, so the model recovers instead of the run dying.
- Claude through the Messages API; the model is a function, so the tests script it and run with
  no key and no network.
- A page that runs a task and shows the steps unfold; Compose locally; kustomize to a cluster.

## Five minutes

    echo ANTHROPIC_API_KEY=sk-ant-... >> backend/.env
    make demo

Then:

    curl -N -X POST localhost:8000/api/runs -H 'content-type: application/json' \
      -d '{"task":"What is 17 * 23, and is it prime?"}'
    # event: run   → {"id":"…"}
    # event: step  → calculator({"expression":"17*23"}) → "391"
    # event: step  → final_answer(...)
    # event: done  → {"state":"done","answer":"391 …","input_tokens":…,"output_tokens":…}

    curl -s localhost:8000/api/runs | head -c 300     # the list, newest first
    curl -s localhost:8000/api/runs/<id>              # the run with every step
    curl -s localhost:8000/api/tools                   # what the model can call

`http://localhost:3000` does the same with a textarea and a step-by-step view.

## API

| Method | Path | Auth | What |
|---|---|---|---|
| POST | `/api/runs` | none | `{task, max_steps?}` → SSE stream of steps; `400` invalid, `429` busy |
| GET | `/api/runs` | none | Last 50 runs, steps omitted |
| GET | `/api/runs/:id` | none | One run with all steps, or `404` |
| POST | `/api/runs/:id/stop` | none | Abort at the next step boundary; `404` if not running here |
| GET | `/api/tools` | none | Tool names, descriptions and schemas |
| GET | `/api/health`, `/api/health/ready` | none | Probes |

## Compared with smolagents

Same, so their docs transfer:

- The `ToolCallingAgent` loop: model → tool calls → observations → repeat; `final_answer` as a
  tool; a text reply with no calls ends the run; `max_steps` then one final "best answer" call.
- A step record with thought, calls, observations, tokens and duration, and errors fed back as
  observations rather than raised.
- Tools as name + description + JSON schema + a function.

Better here:

- It is a service with a stable HTTP API and a typed client (`AppType`), not a library you wrap.
- Steps are streamed and stored as they happen; smolagents keeps memory in the process and you
  build persistence yourself.
- Concurrency and stop are part of the API.
- Tests need no model, no key and no network, and run in under a second.
- One provider integration of 15 lines that you can read; token counts come from the provider.
- Kubernetes manifests, probes, secrets rendering and CI that roll only what changed.

Not here yet:

- **`CodeAgent`** — actions as Python code in a sandbox. This is tool-calling only.
- **Planning steps** (`planning_interval`), **managed/multi-agent** hierarchies, memory replay
  and `reset=False` continuation across runs.
- **Sandboxed executors** (E2B, Docker, Modal) and the vision and audio tools.
- **Many model backends** — one (`claude()`); OpenAI, local and HF Inference are a `Model`
  factory each.
- **Hub sharing** of tools and agents, Gradio UI, OpenTelemetry instrumentation.
- **Auth and per-user limits** — the routes are open.
- **Multi-replica stop** — `stop` reaches the replica running the run.

## Production

- Envs: `ANTHROPIC_API_KEY`, `MODEL`, `MAX_CONCURRENT_RUNS`, `DATABASE_URL`, `PORT`. Rendered
  from `backend/.env` into a Secret by `make k8s-secrets ENV=prod`; committed only as `.env.age`.
- Scaling: each run is one open connection and one provider call per step; replicas multiply
  the concurrency cap. Set the ingress idle timeout above your longest run, or move the loop to
  a worker (Roadmap 1).
- Probes: `/api/health` (liveness), `/api/health/ready` (readiness — does not yet check Postgres).
- Migrations: `backend/migrations/*.sql`, run by the `migrate` init container before each rollout.
- Deploy: `git push main` → dev; `make release` → prod. `k8s/README.md` explains the manifests.
- What pages you: `failed` rate (the provider key, quota or an outage), `429` rate (raise the cap
  or add replicas), rows stuck in `running` (a killed pod), token spend per hour.

## Roadmap

1. Move the loop to a worker; the route enqueues and clients tail stored steps.
2. Redis-backed stop and concurrency, so replicas behave as one.
3. A sweeper for `running` rows older than N minutes.
4. Auth and per-principal rate limits on `POST /api/runs`.
5. A search API behind `web_search` instead of scraped HTML.
6. Per-tool timeouts and output caps on `ToolDef`.
7. Planning steps and run continuation (`parent_run_id`).
"##;

const LOOP_AGENT_REVIEW: &str = r##"---
name: agent-loop
description: Run on any change to backend/src/agent.ts, TOOLS, the run routes or the SSE stream. Reads the change as the person who pays the provider bill and gets paged when a run never ends, and reports only what lets a run run away, leak, or lie about what it did.
tools: Read, Grep, Glob, Bash
---

You review changes to an agent loop that spends money on every step and executes tool code on
request. Report each finding as `path:line — what — the input or model reply that triggers it —
the fix`.

Check:
1. Termination: every path out of the `for` loop in `runAgent` is `final_answer`, no calls,
   abort, or `maxSteps`; the best-answer call passes `[]` tools so it cannot start a new step.
2. Bounds: `max_steps` stays capped in the zod body (≤ 30); the tool `Promise.race` timeout and
   the `slice(0, 8000)` on observations are still there and still apply to every tool.
3. Tool safety: a new `ToolDef.run` validates its input before acting; nothing reaches `eval`,
   `Function`, a shell, or the filesystem from model-controlled strings; the calculator's
   allowlist regex still precedes `Function`.
4. Outbound calls: every `fetch` in a tool or provider has a timeout or is inside the race, sends
   no secret in the URL, and does not follow model-supplied URLs without an allowlist.
5. Errors as observations: a thrown tool error becomes `{error: true}` and the loop continues;
   an exception escaping `runAgent` is a bug unless it is the abort or provider failure.
6. Message shape: every `tool_use` id in the assistant block has a matching `tool_result` in the
   next user block, in the same order; a mismatch is a `400` from the provider on the next step.
7. Persistence order: `store.update` runs before `writeSSE` in `onStep`, and the final update
   runs before the `done` event; `running.delete` is in `finally`.
8. Accounting: `input_tokens`/`output_tokens` are summed from provider `usage` for every call,
   including the best-answer call; no estimates.
9. Stop: the abort signal is checked at the step boundary and `s.onAbort` still calls
   `ctl.abort()`, so a vanished client does not keep spending.
10. Concurrency: `running.size` is checked before `store.create`, and the entry is removed on
    every exit path.
11. The route stays one chained expression so `AppType` includes it; the SSE event names
    (`run`, `step`, `done`, `error`) match what `frontend/app/page.tsx` parses.
12. Secrets: `ANTHROPIC_API_KEY` appears only in `claude()`'s header; provider error bodies are
    truncated before they are stored in `answer` or logged.

End with one line: `agent-loop: N findings`, and if 0, which of the above you ran.
"##;
