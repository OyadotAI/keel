//! orchestrator: planner, dependency queue, workers and a judge, like CrewAI / AutoGen ════════

pub fn files(name: &str) -> Vec<(&'static str, String)> {
    let f = |s: &str| s.replace("{{NAME}}", name);
    vec![
        ("backend/src/app.ts", ORCH_APP.into()),
        ("backend/src/store.ts", ORCH_STORE.into()),
        ("backend/src/engine.ts", ORCH_ENGINE.into()),
        ("backend/src/worker.ts", ORCH_WORKER.into()),
        ("backend/src/app.test.ts", ORCH_TEST.into()),
        ("backend/migrations/0002_orchestrator.sql", ORCH_SQL.into()),
        ("frontend/app/page.tsx", f(ORCH_PAGE)),
        ("CLAUDE.md", f(ORCH_CLAUDE_MD)),
        ("AGENTS.md", f(ORCH_AGENTS_MD)),
        ("README.md", f(ORCH_README_MD)),
        (".claude/agents/graph-semantics.md", ORCH_REVIEWER.into()),
    ]
}

const ORCH_APP: &str = r##"import { Hono } from "hono";
import { z } from "zod";
import { pgStore, type Store } from "./store";
import { plan, tick, claude, TOOLS, type Model } from "./engine";

// POST /api/runs plans a goal into a task graph and stores it; the worker (bun run worker)
// drains the queue. With no worker running — the first five minutes — the API drains it itself
// in the background (INLINE_WORKER=false turns that off for the cluster, where the Deployment
// does it). GET /api/runs/:id is the live graph the page polls.

const inlineWorker = process.env.INLINE_WORKER !== "false";

export function createApp(store: Store, model: Model = claude(), opts: { inline?: boolean } = {}) {
  const inline = opts.inline ?? inlineWorker;
  const drain = async () => { try { while (await tick(store, model)); } catch (e) { console.error(JSON.stringify({ level: "error", msg: e instanceof Error ? e.message : String(e) })); } };
  const app = new Hono()
    .get("/api/health", (c) => c.json({ status: "ok" }))
    .get("/api/health/ready", (c) => c.json({ status: "ok", db: "ok" as const }))
    .get("/api/tools", (c) => c.json({ tools: TOOLS.map((t) => ({ name: t.name, description: t.description })) }))

    .post("/api/runs", async (c) => {
      const p = z.object({ goal: z.string().min(1).max(4000) }).safeParse(await c.req.json().catch(() => ({})));
      if (!p.success) return c.json({ error: { message: "goal required", code: "invalid" } }, 400);
      const id = crypto.randomUUID();
      await store.createRun({ id, goal: p.data.goal, state: "planning", input_tokens: 0, output_tokens: 0, created_at: new Date().toISOString() });
      try {
        const g = await plan(p.data.goal, model);
        await store.addTasks(g.tasks.map((t) => ({ id: crypto.randomUUID(), run_id: id, ...t, budget_tokens: Number(process.env.TASK_BUDGET_TOKENS ?? 20_000), state: "queued", attempts: 0, output: null, verdict: null, input_tokens: 0, output_tokens: 0 })));
        await store.updateRun(id, { state: "running", input_tokens: g.input_tokens, output_tokens: g.output_tokens });
      } catch (e) {
        await store.updateRun(id, { state: "failed" });
        return c.json({ error: { message: e instanceof Error ? e.message : String(e), code: "plan_failed" } }, 502);
      }
      if (inline) setTimeout(drain, 0);
      return c.json({ id, tasks: (await store.getTasks(id)).length }, 201);
    })
    .post("/api/tick", async (c) => c.json({ ran: await tick(store, model) }))
    .get("/api/runs", async (c) => c.json({ runs: await store.listRuns(50) }))
    .get("/api/runs/:id", async (c) => {
      const run = await store.getRun(c.req.param("id"));
      if (!run) return c.json({ error: { message: "no such run", code: "not_found" } }, 404);
      return c.json({ ...run, tasks: await store.getTasks(run.id) });
    });
  return app;
}
const app = createApp(pgStore());
export type AppType = typeof app;
export default app;
"##;

const ORCH_STORE: &str = r##"import { db } from "./db";

export type Run = { id: string; goal: string; state: "planning" | "running" | "done" | "failed"; input_tokens: number; output_tokens: number; created_at: string };
export type Task = { id: string; run_id: string; key: string; description: string; expected_output: string; depends_on: string[]; tools: string[]; budget_tokens: number; state: "queued" | "running" | "done" | "failed" | "dead"; attempts: number; output: string | null; verdict: { pass: boolean; reason: string } | null; input_tokens: number; output_tokens: number };

export interface Store {
  createRun(r: Run): Promise<void>;
  updateRun(id: string, p: Partial<Run>): Promise<void>;
  getRun(id: string): Promise<Run | null>;
  listRuns(limit: number): Promise<Run[]>;
  addTasks(ts: Task[]): Promise<void>;
  getTasks(runId: string): Promise<Task[]>;
  /// The queue: one queued task whose dependencies are all done, claimed for this worker.
  /// Dependency release is the query itself — nothing has to notify anything when a task ends.
  claimReady(): Promise<Task | null>;
  updateTask(id: string, p: Partial<Task>): Promise<void>;
}

export function pgStore(): Store {
  return {
    createRun: async (r) => { await db`insert into runs (id, goal, state) values (${r.id}, ${r.goal}, ${r.state})`; },
    updateRun: async (id, p) => { await db`update runs set state = coalesce(${p.state ?? null}, state), input_tokens = coalesce(${p.input_tokens ?? null}, input_tokens), output_tokens = coalesce(${p.output_tokens ?? null}, output_tokens), updated_at = now() where id = ${id}`; },
    getRun: async (id) => (await db<Run[]>`select * from runs where id = ${id}`)[0] ?? null,
    listRuns: async (limit) => db<Run[]>`select * from runs order by created_at desc limit ${limit}`,
    addTasks: async (ts) => db.begin(async (tx) => { for (const t of ts) await tx`insert into tasks (id, run_id, key, description, expected_output, depends_on, tools, budget_tokens) values (${t.id}, ${t.run_id}, ${t.key}, ${t.description}, ${t.expected_output}, ${t.depends_on}, ${t.tools}, ${t.budget_tokens})`; }),
    getTasks: async (runId) => db<Task[]>`select * from tasks where run_id = ${runId} order by key`,
    claimReady: async () => (await db<Task[]>`update tasks set state = 'running', attempts = attempts + 1, updated_at = now() where id = (
        select t.id from tasks t where t.state = 'queued'
          and not exists (select 1 from tasks d where d.run_id = t.run_id and d.key = any(t.depends_on) and d.state <> 'done')
        order by t.updated_at limit 1 for update skip locked) returning *`)[0] ?? null,
    updateTask: async (id, p) => { await db`update tasks set state = coalesce(${p.state ?? null}, state), description = coalesce(${p.description ?? null}, description), output = coalesce(${p.output ?? null}, output),
      verdict = coalesce(${p.verdict ? db.json(p.verdict) : null}, verdict), input_tokens = coalesce(${p.input_tokens ?? null}, input_tokens), output_tokens = coalesce(${p.output_tokens ?? null}, output_tokens), updated_at = now() where id = ${id}`; },
  };
}

export function memoryStore(): Store {
  const runs = new Map<string, Run>(); const tasks: Task[] = [];
  return {
    createRun: async (r) => { runs.set(r.id, { ...r }); },
    updateRun: async (id, p) => { const r = runs.get(id); if (r) Object.assign(r, p); },
    getRun: async (id) => runs.get(id) ?? null,
    listRuns: async (limit) => [...runs.values()].reverse().slice(0, limit),
    addTasks: async (ts) => { tasks.push(...ts.map((t) => ({ ...t }))); },
    getTasks: async (runId) => tasks.filter((t) => t.run_id === runId).map((t) => ({ ...t })),
    claimReady: async () => {
      const t = tasks.find((t) => t.state === "queued" && t.depends_on.every((k) => tasks.find((d) => d.run_id === t.run_id && d.key === k)?.state === "done"));
      if (!t) return null; t.state = "running"; t.attempts++; return { ...t };
    },
    updateTask: async (id, p) => { const t = tasks.find((t) => t.id === id); if (t) Object.assign(t, p); },
  };
}
"##;

const ORCH_ENGINE: &str = r##"// CrewAI's shape: a Task has a description, an expected_output, context (the tasks it depends
import { lookup } from "node:dns/promises";
// on) and the tools it may use; a crew runs them in dependency order. Here the planner writes
// that list from a goal, the store is the queue, a worker runs whatever is released, and a judge
// with a cold context — only the task and its output, none of the worker's transcript — decides
// whether it passed. A failed node is re-planned (the planner rewrites it with the verdict) and
// requeued; its dependants wait. Every step is a row, so a run survives a worker restart.
import type { Store, Task } from "./store";

export type ToolCall = { id: string; name: string; input: Record<string, unknown> };
export type ModelReply = { text: string; calls: ToolCall[]; input_tokens: number; output_tokens: number; stop: "tool_use" | "end_turn" | "max_tokens" };
export type Message = { role: "user" | "assistant"; content: unknown };
export type ToolDef = { name: string; description: string; input_schema: Record<string, unknown>; run: (input: Record<string, unknown>) => Promise<string> };
export type Model = (system: string, messages: Message[], tools: ToolDef[]) => Promise<ModelReply>;

export const TOOLS: ToolDef[] = [
  { name: "calculator", description: "Evaluate an arithmetic expression (+ - * / ( ) and numbers).", input_schema: { type: "object", properties: { expression: { type: "string" } }, required: ["expression"] },
    run: async ({ expression }) => { const e = String(expression); if (!/^[\d\s+\-*/().]+$/.test(e)) throw new Error("only arithmetic"); return String(Function(`"use strict"; return (${e})`)()); } },
  { name: "web_fetch", description: "Fetch a public http(s) URL and return its text (tags stripped, truncated).", input_schema: { type: "object", properties: { url: { type: "string" } }, required: ["url"] },
    run: async ({ url }) => { const r = await fetchPublic(String(url), (u, init) => fetch(u, { ...init, signal: AbortSignal.timeout(10_000) })); return (await r.text()).replace(/<script[\s\S]*?<\/script>|<style[\s\S]*?<\/style>/gi, "").replace(/<[^>]+>/g, " ").replace(/\s+/g, " ").slice(0, 8000); } },
];

export type PlannedTask = { key: string; description: string; expected_output: string; depends_on: string[]; tools: string[] };
const PLANNER = `You are a planner. Split the goal into 2-6 tasks as JSON: {"tasks":[{"key":"t1","description":"...","expected_output":"what a correct result contains","depends_on":["keys"],"tools":["calculator"|"web_fetch"]}]}. Keys are unique; depends_on names earlier keys only; tools are only the ones the task needs. Reply with JSON only.`;

export async function plan(goal: string, model: Model): Promise<{ tasks: PlannedTask[]; input_tokens: number; output_tokens: number }> {
  const r = await model(PLANNER, [{ role: "user", content: goal }], []);
  const parsed = JSON.parse(extractJson(r.text)) as { tasks: PlannedTask[] };
  const keys = new Set(parsed.tasks.map((t) => t.key));
  for (const t of parsed.tasks) {
    t.depends_on = (t.depends_on ?? []).filter((k) => keys.has(k) && k !== t.key);
    t.tools = (t.tools ?? []).filter((n) => TOOLS.some((x) => x.name === n));
  }
  if (!parsed.tasks.length || hasCycle(parsed.tasks)) throw new Error("planner produced no usable graph");
  return { tasks: parsed.tasks, input_tokens: r.input_tokens, output_tokens: r.output_tokens };
}

/// Re-plan one failed node: same key and dependencies, a rewritten description that carries what
/// the judge saw. The rest of the graph is untouched, so completed work is never redone.
export async function replan(task: Task, model: Model): Promise<{ description: string; input_tokens: number; output_tokens: number }> {
  const r = await model("Rewrite this task's description so a worker gets it right this time. Keep the intent. Reply with the new description only, plain text.",
    [{ role: "user", content: `Task: ${task.description}\nExpected: ${task.expected_output}\nAttempt produced: ${task.output ?? ""}\nJudge said: ${task.verdict?.reason ?? ""}` }], []);
  return { description: r.text.trim() || task.description, input_tokens: r.input_tokens, output_tokens: r.output_tokens };
}

/// A worker turn: the ReAct loop over the task's scoped tools, until the model answers in text or
/// the token budget is spent. Dependency outputs are the context, exactly as CrewAI passes them.
export async function work(task: Task, context: { key: string; output: string }[], model: Model, tools = TOOLS): Promise<{ output: string | null; input_tokens: number; output_tokens: number; error?: string }> {
  const scoped = tools.filter((t) => task.tools.includes(t.name));
  const messages: Message[] = [{ role: "user", content: `${task.description}\n\nExpected output: ${task.expected_output}${context.length ? "\n\nContext from earlier tasks:\n" + context.map((c) => `[${c.key}]\n${c.output}`).join("\n\n") : ""}` }];
  let input_tokens = 0, output_tokens = 0;
  for (let n = 0; n < 12; n++) {
    if (input_tokens + output_tokens > task.budget_tokens) return { output: null, input_tokens, output_tokens, error: `budget of ${task.budget_tokens} tokens exceeded` };
    const r = await model("You complete one task using only the tools given. When done, reply with the result as plain text and no tool call.", messages, scoped);
    input_tokens += r.input_tokens; output_tokens += r.output_tokens;
    if (!r.calls.length) return { output: r.text, input_tokens, output_tokens };
    const blocks: unknown[] = r.text ? [{ type: "text", text: r.text }] : [];
    for (const c of r.calls) blocks.push({ type: "tool_use", id: c.id, name: c.name, input: c.input });
    messages.push({ role: "assistant", content: blocks });
    const results = [];
    for (const c of r.calls) {
      const tool = scoped.find((t) => t.name === c.name);
      try { if (!tool) throw new Error(`tool ${c.name} is not available to this task`); results.push({ type: "tool_result", tool_use_id: c.id, content: (await tool.run(c.input)).slice(0, 8000) }); }
      catch (e) { results.push({ type: "tool_result", tool_use_id: c.id, content: `Error: ${e instanceof Error ? e.message : String(e)}`, is_error: true }); }
    }
    messages.push({ role: "user", content: results });
  }
  return { output: null, input_tokens, output_tokens, error: "too many steps" };
}

/// Cold context: the judge sees the task, the acceptance criteria and the output. Not the
/// worker's reasoning, not the tool calls — so it cannot be talked into a pass.
export async function judge(task: Pick<Task, "description" | "expected_output">, output: string, model: Model): Promise<{ pass: boolean; reason: string; input_tokens: number; output_tokens: number }> {
  const r = await model(`You are a strict reviewer. Given a task, its acceptance criteria and an output, reply with JSON only: {"pass": true|false, "reason": "one sentence"}.`,
    [{ role: "user", content: `Task: ${task.description}\nAcceptance criteria: ${task.expected_output}\nOutput:\n${output}` }], []);
  try { const v = JSON.parse(extractJson(r.text)) as { pass: boolean; reason: string }; return { pass: !!v.pass, reason: String(v.reason ?? ""), input_tokens: r.input_tokens, output_tokens: r.output_tokens }; }
  catch { return { pass: false, reason: "judge gave no verdict", input_tokens: r.input_tokens, output_tokens: r.output_tokens }; }
}

export const MAX_ATTEMPTS = Number(process.env.MAX_TASK_ATTEMPTS ?? 3);

/// One queue step: claim a released task, work it, judge it, and settle it. Returns false when
/// nothing was ready. The worker loops this; tests call it directly.
export async function tick(store: Store, model: Model, tools = TOOLS): Promise<boolean> {
  const task = await store.claimReady(); if (!task) return false;
  const siblings = await store.getTasks(task.run_id);
  const context = siblings.filter((s) => task.depends_on.includes(s.key)).map((s) => ({ key: s.key, output: s.output ?? "" }));
  let tok = { input_tokens: 0, output_tokens: 0 };
  const add = (r: { input_tokens: number; output_tokens: number }) => { tok = { input_tokens: tok.input_tokens + r.input_tokens, output_tokens: tok.output_tokens + r.output_tokens }; };
  const w = await work(task, context, model, tools); add(w);
  let verdict = w.output === null ? { pass: false, reason: w.error ?? "no output" } : await judge(task, w.output, model).then((v) => { add(v); return v; });
  const failed = { ...task, output: w.output, verdict };
  if (verdict.pass) await store.updateTask(task.id, { state: "done", output: w.output, verdict, input_tokens: task.input_tokens + tok.input_tokens, output_tokens: task.output_tokens + tok.output_tokens });
  else if (task.attempts < MAX_ATTEMPTS) {
    const p = await replan(failed, model); add(p);
    await store.updateTask(task.id, { state: "queued", description: p.description, output: w.output, verdict, input_tokens: task.input_tokens + tok.input_tokens, output_tokens: task.output_tokens + tok.output_tokens });
  } else await store.updateTask(task.id, { state: "dead", output: w.output, verdict, input_tokens: task.input_tokens + tok.input_tokens, output_tokens: task.output_tokens + tok.output_tokens });

  // Settle the run from its tasks; its tokens are the planner's plus every tick's.
  const all = await store.getTasks(task.run_id); const run = await store.getRun(task.run_id);
  const state = all.some((t) => t.state === "dead") ? "failed" : all.every((t) => t.state === "done") ? "done" : "running";
  await store.updateRun(task.run_id, { state, input_tokens: (run?.input_tokens ?? 0) + tok.input_tokens, output_tokens: (run?.output_tokens ?? 0) + tok.output_tokens });
  return true;
}

function extractJson(s: string): string { const i = s.indexOf("{"), j = s.lastIndexOf("}"); return i >= 0 && j > i ? s.slice(i, j + 1) : s; }
function hasCycle(ts: PlannedTask[]): boolean {
  const seen = new Map<string, number>(); const by = new Map(ts.map((t) => [t.key, t]));
  const visit = (k: string): boolean => { const s = seen.get(k); if (s === 1) return true; if (s === 2) return false; seen.set(k, 1); for (const d of by.get(k)?.depends_on ?? []) if (visit(d)) return true; seen.set(k, 2); return false; };
  return ts.some((t) => visit(t.key));
}

/// Claude via the Messages API. Set ANTHROPIC_API_KEY; MODEL defaults to Opus.
export function claude(fetchImpl: typeof fetch = fetch): Model {
  return async (system, messages, tools) => {
    const res = await fetchImpl("https://api.anthropic.com/v1/messages", {
      method: "POST", headers: { "x-api-key": process.env.ANTHROPIC_API_KEY ?? "", "anthropic-version": "2023-06-01", "content-type": "application/json" },
      body: JSON.stringify({ model: process.env.MODEL ?? "claude-opus-5", max_tokens: 4096, system, messages, tools: tools.map((t) => ({ name: t.name, description: t.description, input_schema: t.input_schema })) }),
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

// Only public addresses: an agent-chosen URL is a request from inside the network, so loopback,
// RFC1918, link-local (cloud metadata) and ULA are refused after DNS, and redirects are walked
// by hand so a public host cannot bounce to a private one. Set ALLOW_PRIVATE_URLS=1 in tests.
export async function publicUrl(raw: string): Promise<URL> {
  const u = new URL(raw);
  if (!/^https?:$/.test(u.protocol)) throw new Error("http(s) only");
  if (process.env.ALLOW_PRIVATE_URLS === "1") return u;
  const host = u.hostname.replace(/^\[|\]$/g, "");
  const addrs = /^[\d.]+$|:/.test(host) ? [{ address: host }] : await lookup(host, { all: true });
  for (const { address } of addrs) if (isPrivate(address)) throw new Error(`refusing private address for ${u.hostname}`);
  return u;
}
export function isPrivate(ip: string): boolean {
  if (ip.includes(":")) { const l = ip.toLowerCase(); return l === "::1" || l === "::" || /^f[cd]/.test(l) || /^fe[89ab]/.test(l) || l.startsWith("::ffff:") && isPrivate(l.slice(7)); }
  const [a, b] = ip.split(".").map(Number);
  return a === 127 || a === 10 || a === 0 || (a === 172 && b >= 16 && b <= 31) || (a === 192 && b === 168) || (a === 169 && b === 254) || (a === 100 && b >= 64 && b <= 127);
}
export async function fetchPublic(raw: string, fetchImpl: (u: URL, init?: RequestInit) => Promise<Response>, init: RequestInit = {}, hops = 5): Promise<Response> {
  let u = await publicUrl(raw);
  for (let i = 0; i <= hops; i++) {
    const r = await fetchImpl(u, { ...init, redirect: "manual" });
    const loc = r.headers.get("location");
    if (r.status < 300 || r.status >= 400 || !loc) return r;
    u = await publicUrl(new URL(loc, u).toString());
  }
  throw new Error("too many redirects");
}
"##;

const ORCH_WORKER: &str = r##"import { pgStore } from "./store";
import { tick, claude } from "./engine";
import { db } from "./db";

// The worker Deployment: claim, work, judge, settle, repeat. Idle polls back off to a second.
// Run more replicas for more parallelism — SKIP LOCKED keeps them off each other's tasks.
const store = pgStore(); const model = claude();
let stop = false;
process.on("SIGTERM", () => { stop = true; });
while (!stop) {
  try { if (!(await tick(store, model))) await new Promise((r) => setTimeout(r, 1000)); }
  catch (e) { console.error(JSON.stringify({ level: "error", msg: e instanceof Error ? e.message : String(e) })); await new Promise((r) => setTimeout(r, 1000)); }
}
await db.end();
"##;

const ORCH_TEST: &str = r##"import { describe, expect, test } from "bun:test";
process.env.ALLOW_PRIVATE_URLS = "1";
import { createApp } from "./app";
import { memoryStore, type Task } from "./store";
import { plan, tick, work, MAX_ATTEMPTS, type Model, type ModelReply } from "./engine";

// The model is a function: the planner, worker, judge and re-planner are told apart by their
// system prompts, so one fake can play all four. No provider is ever called.
const reply = (text: string, extra: Partial<ModelReply> = {}): ModelReply => ({ text, calls: [], input_tokens: 10, output_tokens: 5, stop: "end_turn", ...extra });
const GRAPH = { tasks: [{ key: "a", description: "compute 6*7", expected_output: "the number 42", depends_on: [], tools: ["calculator"] }, { key: "b", description: "state the answer", expected_output: "a sentence", depends_on: ["a"], tools: [] }] };
const roles = (opts: { pass?: (task: string, output: string) => boolean } = {}): Model => async (system, messages, tools) => {
  if (system.startsWith("You are a planner")) return reply(JSON.stringify(GRAPH));
  if (system.startsWith("You are a strict reviewer")) { const m = String(messages[0].content); const out = m.slice(m.indexOf("Output:\n") + 8); return reply(JSON.stringify({ pass: opts.pass ? opts.pass(m, out) : true, reason: "checked" })); }
  if (system.startsWith("Rewrite")) return reply("REPLANNED: " + String(messages[0].content).split("\n")[0]);
  // worker: use the calculator if it has one and has not yet, else answer with the context.
  const last = messages.at(-1)!;
  if (tools.some((t) => t.name === "calculator") && messages.length === 1) return reply("", { calls: [{ id: "c1", name: "calculator", input: { expression: "6*7" } }], stop: "tool_use" });
  if (Array.isArray(last.content)) return reply(`result ${(last.content[0] as { content: string }).content}`);
  return reply(`done (tools: ${tools.map((t) => t.name).join(",") || "none"}; ctx: ${String(last.content).includes("[a]") ? "a" : "-"})`);
};
const task = (p: Partial<Task>): Task => ({ id: crypto.randomUUID(), run_id: "r", key: "k", description: "d", expected_output: "e", depends_on: [], tools: [], budget_tokens: 20_000, state: "queued", attempts: 0, output: null, verdict: null, input_tokens: 0, output_tokens: 0, ...p });

describe("orchestrator", () => {
  test("the planner's graph is stored with dependencies and scoped tools; cycles are refused", async () => {
    const g = await plan("anything", roles());
    expect(g.tasks.map((t) => t.key)).toEqual(["a", "b"]); expect(g.tasks[1].depends_on).toEqual(["a"]);
    await expect(plan("x", async () => reply(JSON.stringify({ tasks: [{ key: "a", description: "", expected_output: "", depends_on: ["b"], tools: ["shell"] }, { key: "b", description: "", expected_output: "", depends_on: ["a"], tools: [] }] })))).rejects.toThrow("no usable graph");
  });

  test("the queue releases a task only when its dependencies are done", async () => {
    const store = memoryStore();
    await store.addTasks([task({ key: "b", depends_on: ["a"] }), task({ key: "a" })]);
    const first = await store.claimReady(); expect(first?.key).toBe("a");
    expect(await store.claimReady()).toBeNull();            // b is blocked, a is running
    await store.updateTask(first!.id, { state: "done", output: "42" });
    expect((await store.claimReady())?.key).toBe("b");
  });

  test("a worker sees only the task's tools and its dependencies' outputs, and the judge passes it", async () => {
    const store = memoryStore();
    await store.addTasks([task({ run_id: "r", key: "a", tools: ["calculator"] }), task({ run_id: "r", key: "b", depends_on: ["a"] })]);
    await store.createRun({ id: "r", goal: "g", state: "running", input_tokens: 0, output_tokens: 0, created_at: "" });
    expect(await tick(store, roles())).toBe(true); expect(await tick(store, roles())).toBe(true); expect(await tick(store, roles())).toBe(false);
    const ts = await store.getTasks("r");
    expect(ts[0].output).toBe("result 42"); expect(ts[0].verdict?.pass).toBe(true);
    expect(ts[1].output).toBe("done (tools: none; ctx: a)");
    const run = await store.getRun("r"); expect(run?.state).toBe("done"); expect(run!.input_tokens).toBeGreaterThan(0);
    // A call to a tool outside the scope is an error result, never an execution.
    const w = await work(task({ tools: [] }), [], async (_s, m) => m.length === 1 ? reply("", { calls: [{ id: "x", name: "calculator", input: { expression: "1" } }], stop: "tool_use" }) : reply("ok"));
    expect(w.output).toBe("ok");
  });

  test("a failed verdict re-plans the node and requeues it; after MAX_ATTEMPTS it is dead and the run fails", async () => {
    const store = memoryStore();
    await store.addTasks([task({ run_id: "r", key: "a" })]);
    await store.createRun({ id: "r", goal: "g", state: "running", input_tokens: 0, output_tokens: 0, created_at: "" });
    const model = roles({ pass: () => false });
    for (let i = 1; i <= MAX_ATTEMPTS; i++) {
      await tick(store, model);
      const [t] = await store.getTasks("r");
      expect(t.attempts).toBe(i);
      if (i < MAX_ATTEMPTS) { expect(t.state).toBe("queued"); expect(t.description.startsWith("REPLANNED:")).toBe(true); }
      else { expect(t.state).toBe("dead"); expect((await store.getRun("r"))?.state).toBe("failed"); }
    }
  });

  test("a task that exceeds its token budget fails with the reason", async () => {
    const greedy: Model = async () => reply("", { calls: [{ id: "c", name: "calculator", input: { expression: "1+1" } }], stop: "tool_use", input_tokens: 5000, output_tokens: 1000 });
    const w = await work(task({ tools: ["calculator"], budget_tokens: 10_000 }), [], greedy);
    expect(w.output).toBeNull(); expect(w.error).toContain("budget");
    expect(w.input_tokens).toBe(10_000);
  });

  test("POST /api/runs plans, drains inline, and the run viewer shows the finished graph", async () => {
    const app = createApp(memoryStore(), roles(), { inline: true });
    const res = await app.request("/api/runs", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ goal: "what is 6*7" }) });
    expect(res.status).toBe(201);
    const { id } = await res.json();
    for (let i = 0; i < 50 && (await (await app.request(`/api/runs/${id}`)).json()).state === "running"; i++) await new Promise((r) => setTimeout(r, 5));
    const run = await (await app.request(`/api/runs/${id}`)).json();
    expect(run.state).toBe("done"); expect(run.tasks.length).toBe(2); expect(run.tasks.every((t: Task) => t.state === "done")).toBe(true);
  });
});
"##;

const ORCH_SQL: &str = r##"create table if not exists runs (
  id uuid primary key,
  goal text not null,
  state text not null default 'planning',  -- planning | running | done | failed
  input_tokens int not null default 0,
  output_tokens int not null default 0,
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now()
);
create table if not exists tasks (
  id uuid primary key,
  run_id uuid not null references runs(id),
  key text not null,
  description text not null,
  expected_output text not null,
  depends_on text[] not null default '{}',
  tools text[] not null default '{}',
  budget_tokens int not null default 20000,
  state text not null default 'queued',    -- queued | running | done | failed | dead
  attempts int not null default 0,
  output text,
  verdict jsonb,                           -- { pass, reason } from the judge
  input_tokens int not null default 0,
  output_tokens int not null default 0,
  updated_at timestamptz not null default now(),
  unique (run_id, key)
);
create index if not exists tasks_ready on tasks (state, run_id);
"##;

const ORCH_PAGE: &str = r##""use client";
import { useEffect, useState } from "react";

type Task = { id: string; key: string; description: string; expected_output: string; depends_on: string[]; tools: string[]; state: string; attempts: number; output: string | null; verdict: { pass: boolean; reason: string } | null; input_tokens: number; output_tokens: number };
type Run = { id: string; goal: string; state: string; input_tokens: number; output_tokens: number; created_at: string; tasks?: Task[] };

const COLOR: Record<string, string> = { queued: "#ddd", running: "#ffd", done: "#dfd", failed: "#fdd", dead: "#f99", planning: "#ffd" };

// The run viewer: the task graph as a table in dependency order, coloured by state, polled while
// the run is live. Each row is a node — what it was asked, what it produced, what the judge said.
export default function Home() {
  const [runs, setRuns] = useState<Run[]>([]);
  const [run, setRun] = useState<Run | null>(null);
  const [goal, setGoal] = useState("Find the population of Iceland and of Malta from Wikipedia, then compute the ratio.");
  const [busy, setBusy] = useState(false);

  const load = () => fetch("/api/runs").then((r) => r.json()).then((d) => setRuns(d.runs)).catch(() => {});
  useEffect(() => { load(); }, []);
  useEffect(() => {
    if (!run || (run.state !== "running" && run.state !== "planning")) return;
    const t = setInterval(() => fetch(`/api/runs/${run.id}`).then((r) => r.json()).then(setRun).catch(() => {}), 1000);
    return () => clearInterval(t);
  }, [run]);

  async function start() {
    setBusy(true);
    const res = await fetch("/api/runs", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ goal }) });
    const d = await res.json();
    if (res.ok) setRun(await (await fetch(`/api/runs/${d.id}`)).json()); else alert(d.error.message);
    setBusy(false); load();
  }

  return (
    <main>
      <h1>{{NAME}} — orchestrator</h1>
      <p>Planner → task graph → workers with scoped tools → judge. Set <code>ANTHROPIC_API_KEY</code> in backend/.env; <code>bun run worker</code> runs tasks in the cluster, the API drains them itself locally.</p>
      <pre>{`curl localhost:8000/api/runs -H 'content-type: application/json' -d '{"goal":"..."}'\ncurl localhost:8000/api/runs/<id>`}</pre>
      <textarea value={goal} onChange={(e) => setGoal(e.target.value)} rows={2} style={{ width: "100%" }} />
      <p><button onClick={start} disabled={busy}>{busy ? "Planning…" : "Run"}</button></p>
      {run && (
        <section>
          <h2>{run.goal} <small style={{ background: COLOR[run.state] }}>{run.state}</small> <small>{run.input_tokens + run.output_tokens} tokens</small></h2>
          <table border={1} cellPadding={4} style={{ borderCollapse: "collapse", width: "100%" }}>
            <thead><tr><th>key</th><th>after</th><th>tools</th><th>state</th><th>task</th><th>output</th><th>judge</th></tr></thead>
            <tbody>{run.tasks?.map((t) => (
              <tr key={t.id} style={{ background: COLOR[t.state] }}>
                <td>{t.key}</td><td>{t.depends_on.join(", ")}</td><td>{t.tools.join(", ")}</td><td>{t.state}{t.attempts > 1 ? ` ×${t.attempts}` : ""}</td>
                <td>{t.description}</td><td style={{ whiteSpace: "pre-wrap", maxWidth: 400 }}>{t.output}</td><td>{t.verdict ? `${t.verdict.pass ? "pass" : "fail"}: ${t.verdict.reason}` : ""}</td>
              </tr>))}</tbody>
          </table>
        </section>
      )}
      <h2>Runs</h2>
      <ul>{runs.map((r) => <li key={r.id}><a href="#" onClick={(e) => { e.preventDefault(); fetch(`/api/runs/${r.id}`).then((x) => x.json()).then(setRun); }}>{r.goal}</a> — {r.state}</li>)}</ul>
    </main>
  );
}
"##;

const ORCH_CLAUDE_MD: &str = r##"# {{NAME}} — orchestrator

This service turns a goal into a task graph and runs it: a planner writes 2–6 tasks with
dependencies and scoped tools, the database is the queue, workers claim whatever is released,
and a judge with a cold context decides whether each task passed. It is modelled on CrewAI's
`Task` — `description`, `expected_output`, `context` (here `depends_on`) and `tools` — with
the crew replaced by rows, so a run survives a worker restart and several workers share one
queue. "Done" means: `POST /api/runs` stores a validated graph, `tick` claims, works, judges and
settles one task at a time until the run is `done` or `failed`, and `bun test` proves each step
against one scripted model that plays all four roles.

## Architecture

| File | Owns |
|---|---|
| `backend/src/app.ts` | The routes; planning on `POST /api/runs`; the inline drain (`INLINE_WORKER`) for the first five minutes |
| `backend/src/engine.ts` | `plan` (goal → graph, validated), `work` (the ReAct loop over scoped tools with a token budget), `judge` (cold-context verdict), `replan` (rewrite one failed task), `tick` (claim → work → judge → settle), `TOOLS`, `claude()` |
| `backend/src/store.ts` | `Store`: runs and tasks; `claimReady` is the queue query (`for update skip locked`); `pgStore` and `memoryStore` |
| `backend/src/worker.ts` | The worker process: `tick` in a loop, 1s idle backoff, stops on SIGTERM |
| `backend/src/app.test.ts` | Six tests, one fake model told apart by system prompt |
| `backend/src/server.ts`, `db.ts` | HTTP listener and the pool (from the stack) |
| `backend/migrations/0002_orchestrator.sql` | `runs`, `tasks` |
| `frontend/app/page.tsx` | The run viewer: the graph as a table coloured by state, polled while live |

### The request path

1. `POST /api/runs {goal}` (1–4,000 chars, zod). A `runs` row is inserted in state `planning`.
2. `plan(goal, model)` asks the planner for JSON; `extractJson` tolerates prose around it. Unknown `depends_on` keys and self-references are dropped, unknown tools are dropped, and an empty or cyclic graph throws → run `failed`, response `502 plan_failed`.
3. Each task is inserted `queued` with `budget_tokens = TASK_BUDGET_TOKENS` (20,000) inside one transaction; the run becomes `running` carrying the planner's tokens. Response `201 {id, tasks}`.
4. If `INLINE_WORKER` is not `false`, the API schedules `drain()` — `tick` until nothing is ready — on the next timer. In the cluster the worker process does this.
5. `tick`: `claimReady` returns one `queued` task whose every `depends_on` sibling is `done`, flips it to `running` and increments `attempts` in the same statement. `skip locked` keeps parallel workers apart.
6. `work`: the task's description, expected output and its dependencies' outputs form the first user message; the model may call only `task.tools`, up to 12 steps, until it answers in plain text or the budget is spent. A call to an unscoped tool is an error result, never an execution.
7. `judge` sees description, acceptance criteria and output — nothing else — and answers `{pass, reason}`; an unparseable verdict is a fail.
8. Settle: pass → `done`. Fail with `attempts < MAX_ATTEMPTS` → `replan` rewrites the description with the verdict and the task goes back to `queued`. Otherwise `dead`. The run is then `failed` if any task is dead, `done` if all are done, else `running`; every model call's tokens are added to both the task and the run.

### Data model

| Table | Column that matters | Why |
|---|---|---|
| `runs` | `state` | `planning → running → done \| failed`; settled from tasks on every tick |
| `runs` | `input_tokens`, `output_tokens` | The planner's plus every tick's (work, judge, replan) — the run's whole cost |
| `tasks` | `unique (run_id, key)` | `depends_on` is a list of sibling keys; uniqueness is what makes the dependency query mean one thing |
| `tasks` | `depends_on text[]` | Release is `not exists (… d.key = any(t.depends_on) and d.state <> 'done')` — nothing notifies anything |
| `tasks` | `tools text[]` | The scope; `work` filters the registry to these names |
| `tasks` | `attempts`, `budget_tokens` | Bound the retries and the spend per node |
| `tasks` | `output`, `verdict jsonb` | What the worker produced and what the judge said; what the next attempt's re-plan reads |
| `tasks` | `updated_at` | `claimReady` orders by it, so the oldest released task goes first |

## Invariants

1. **A graph is validated before it is stored.** Unknown dependencies and tools are stripped, self-loops removed, and a cycle or an empty list throws. Guarded by `app.test.ts` "the planner's graph is stored with dependencies and scoped tools; cycles are refused".
2. **A task is released only when every dependency is `done`.** Not `running`, not `failed` — `done`. The query in `claimReady` is the only release logic. Guarded by "the queue releases a task only when its dependencies are done".
3. **A worker sees only its task's tools.** `work` filters `TOOLS` by `task.tools`; a call outside the scope returns `Error: tool … is not available to this task` as a tool result and the loop continues. Guarded by "a worker sees only the task's tools and its dependencies' outputs, and the judge passes it".
4. **The judge is cold.** It receives `description`, `expected_output` and `output` — never the worker's messages or tool calls — so it cannot be argued into a pass. Guarded by the same test (the fake judge's input is exactly those three lines) and by `judge`'s signature taking only `Pick<Task, "description" | "expected_output">`.
5. **Failure re-plans one node, never the graph.** `replan` rewrites `description` only; key, dependencies, tools and every other task are untouched, so completed work is not redone. Guarded by "a failed verdict re-plans the node and requeues it; after MAX_ATTEMPTS it is dead and the run fails".
6. **Attempts are bounded.** `attempts` is incremented in the claim statement and compared to `MAX_ATTEMPTS`; the third failure is `dead` and the run is `failed`. Same test.
7. **Tokens are bounded per task.** `work` checks `input_tokens + output_tokens > budget_tokens` before each model call and returns `error: "budget of N tokens exceeded"` with the tokens it did spend. Guarded by "a task that exceeds its token budget fails with the reason".
8. **Every model call is counted.** Planner, worker, judge and re-planner tokens all land on the run; `run.input_tokens > 0` after a run. Guarded by "a worker sees only…" (`run!.input_tokens` greater than 0).
9. **The claim is atomic.** `update … where id = (select … for update skip locked) returning *` — one statement, so two workers cannot take one task. Guarded structurally (the SQL is in one place) and by `memoryStore` mirroring the semantics; not by a concurrency test.
10. **The model is a function of `(system, messages, tools)`.** The four roles are distinguished by their system prompt, which is why one fake can play them all. Every test depends on this; do not move role identity into a closure.
11. **Calculator input is an allowlist.** `/^[\d\s+\-*/().]+$/` before `Function()`; nothing else reaches the evaluator. No dedicated test — the regex is the guard; add one if you widen it.

## Extending it

**Add a tool.** Append to `TOOLS` in `engine.ts` (`name`, `description`, `input_schema`, `run`); add its name to the `PLANNER` prompt's `tools` list so the planner may assign it. Any outbound HTTP needs a timeout (`AbortSignal.timeout`) and, before production, the same public-address guard the `agent` pack has — this pack's `web_fetch` does not have one. Test: a task with `tools: [name]`, a worker fake that calls it, assert the tool result content.

**Add a task field** (say `max_steps`). Migration `0003_…sql` adding the column; `Task` in `store.ts`; `PlannedTask` and the `PLANNER` JSON shape in `engine.ts` if the planner sets it, else the insert in `app.ts`; `updateTask` if it changes. Test: the graph test asserts it round-trips.

**Add a run owner.** A `user_id` column on `runs`, an `x-user` header in `app.ts`, and `listRuns`/`getRun` filtered by it (the `agent` pack's ownership check is the pattern). Test: another user's `GET /api/runs/:id` is 404.

**Change the judge's strictness.** Its system prompt in `judge`; keep JSON-only output and the "no verdict is a fail" fallback. Test: a fake judge that returns prose, assert `pass === false` and `reason === "judge gave no verdict"`.

**Run a task's tool calls in parallel.** In `work`, replace the sequential `for (const c of r.calls)` with `Promise.all`; keep result order equal to call order — the API matches `tool_use_id`s but the transcript reads better ordered.

**Cancel a run.** Add `state: "cancelled"` to `Run`, a `POST /api/runs/:id/cancel` that sets it and marks `queued` tasks `dead`; `tick` should skip claiming tasks whose run is cancelled (join `runs` in `claimReady`). Test: cancel, `tick` returns false.

**Reclaim stuck tasks.** See Ceilings: add `updated_at < now() - interval '10 minutes'` as a second `or` branch in `claimReady` for `running` tasks. Test: a task `running` with an old `updated_at` is claimed again and `attempts` goes up.

## Operating it

| Env var | Required | Meaning |
|---|---|---|
| `ANTHROPIC_API_KEY` | yes | For every role; never in the repository |
| `MODEL` | no | Model id for all four roles, default `claude-opus-5` |
| `INLINE_WORKER` | no | Anything but `false` makes the API drain the queue itself; set `false` where a worker runs |
| `TASK_BUDGET_TOKENS` | no | Per-task token cap, default 20,000 |
| `MAX_TASK_ATTEMPTS` | no | Attempts before `dead`, default 3 |
| `DATABASE_URL` | yes | The pool; the queue lives here |
| `PORT` | no | API port, default 8000 |

**Processes.** The API (`bun run dev` / the backend Deployment) and the worker (`bun run worker`). Parallelism is worker replicas: each `tick` runs one task start to finish, so N replicas run N tasks at once. There is no worker Deployment manifest in `k8s/base` yet — copy `backend.yaml`, change the command to `node dist/worker.js` (build it beside `server.ts` in the Dockerfile), drop the Service and probes, set `INLINE_WORKER=false` on the API.

**Per-replica today:** nothing. Every task, attempt and verdict is a row. Idle workers poll once a second.

**Failure modes.** Planner returns junk → `502 plan_failed`, run `failed`, no tasks. Worker dies mid-task → the task stays `running` and is never reclaimed (see Ceilings); its run stays `running`. Provider down during `work` → `tick` throws, the worker logs one JSON line and sleeps 1s; the task is stuck `running` the same way. Judge returns prose → fail, re-plan, try again. A task uses its budget → fail with the reason, re-plan.

**What to watch.** Tasks in `running` older than your longest expected task (the stuck signal); `dead` per hour; `attempts` distribution; run tokens per goal; `tick` exceptions in the worker log.

## Ceilings

- **No lease on a running task.** `claimReady` only takes `queued` rows, so a crash between claim and settle leaves a task `running` forever. Upgrade: reclaim `running` tasks whose `updated_at` is older than a lease, counting the attempt.
- **`web_fetch` checks the address once.** `fetchPublic` resolves DNS and refuses private ranges on every hop, but a host that changes its answer between the check and the connect (DNS rebinding) is not caught. Upgrade: pin the resolved address with undici's `connect.lookup`.
- **One tool at a time inside a task,** and one task per worker. Upgrade: `Promise.all` over `r.calls`; more replicas.
- **The planner's output is the only schema check.** Keys are strings, descriptions may be empty. Upgrade: a zod schema on the parsed graph.
- **Sequential `addTasks`.** One insert per task in a transaction; fine at six, slow at six hundred. Upgrade: a multi-row insert.
- **`listRuns` is `limit 50`, unfiltered, no owner.** Upgrade: `user_id` and keyset pagination.
- **Inline drain runs in the API process.** A busy API pod does model calls in the background; in the cluster set `INLINE_WORKER=false` and run the worker.
- **`Task.state` includes `failed` but nothing sets it.** Failures go straight to `queued` (re-plan) or `dead`. Either use it for "failed, awaiting re-plan" or remove it from the type.

The stack rules — gate, typed seam, production checklist, deploy — are in `docs/PRODUCTION.md`. They apply.
"##;

const ORCH_AGENTS_MD: &str = r##"# {{NAME}} — for agents

`CLAUDE.md` has the rules. This is how to run and test the service.

## Run

    make demo                     # infra, migrations, seed, API :8000, page :3000 — the API drains the queue itself
    make check                    # typecheck both halves, bun test the backend
    cd backend && bun run worker  # a worker process; set INLINE_WORKER=false on the API when one runs
    make migrate

`ANTHROPIC_API_KEY` in `backend/.env`. Without it `POST /api/runs` returns `502 plan_failed`
with `anthropic 401` in the message.

## Every route, with curl

    J='-H content-type:application/json'

    curl -s localhost:8000/api/health
    # {"status":"ok"}

    curl -s localhost:8000/api/tools
    # {"tools":[{"name":"calculator","description":"Evaluate an arithmetic expression…"},{"name":"web_fetch","description":"Fetch a public http(s) URL…"}]}

    curl -s $J localhost:8000/api/runs -d '{"goal":"Find the population of Iceland and of Malta from Wikipedia, then compute the ratio."}'
    # 201 {"id":"0191…","tasks":3}
    # 400 {"error":{"message":"goal required","code":"invalid"}}       — empty or missing goal
    # 502 {"error":{"message":"planner produced no usable graph","code":"plan_failed"}}

    curl -s localhost:8000/api/runs/0191…
    # {"id":"0191…","goal":"…","state":"running","input_tokens":1420,"output_tokens":388,"tasks":[
    #   {"key":"t1","description":"Fetch the population of Iceland…","expected_output":"A number","depends_on":[],"tools":["web_fetch"],"state":"done","attempts":1,"output":"Iceland: 387,758 (2024)","verdict":{"pass":true,"reason":"A population figure with a source year."},…},
    #   {"key":"t2",…,"state":"running","attempts":1,"output":null,"verdict":null,…},
    #   {"key":"t3","depends_on":["t1","t2"],"tools":["calculator"],"state":"queued",…}]}
    # Poll it: state goes running → done (every task done) or failed (a task is dead).

    curl -s localhost:8000/api/runs
    # {"runs":[{"id":"0191…","goal":"…","state":"done","input_tokens":…,"output_tokens":…,"created_at":"…"}]}

    curl -s -X POST localhost:8000/api/tick
    # {"ran":true}     — claimed and finished one released task; {"ran":false} when nothing is ready
    # Useful with INLINE_WORKER=false and no worker: step a run by hand.

A re-planned task shows `attempts: 2` and a rewritten `description`; a dead one shows
`state: "dead"` and the last verdict's reason.

## How the tests work

`backend/src/app.test.ts` uses no database and no provider:

- **One fake model plays four roles.** `roles()` returns a `Model` that inspects the system
  prompt: `"You are a planner"` → a fixed two-task graph; `"You are a strict reviewer"` → a
  verdict (`opts.pass` decides); `"Rewrite"` → `"REPLANNED: …"`; anything else is the worker,
  which calls the calculator once if it has it and then answers from the tool result.
- **`memoryStore()`** implements `claimReady` with the same release rule as the SQL, in a loop
  over an array.
- **`tick` is called directly**, once per task, so a test asserts the exact state after each
  step. The HTTP test uses `{inline: true}` and polls `GET /api/runs/:id` until it is not
  `running`.
- **`task({...})`** builds a full `Task` with defaults so a test names only what matters.

Run one: `cd backend && bun test -t "budget"`.

## Adding a test

```ts
test("a judge that answers in prose is a fail, and the task is re-planned", async () => {
  const store = memoryStore();
  await store.addTasks([task({ run_id: "r", key: "a" })]);
  await store.createRun({ id: "r", goal: "g", state: "running", input_tokens: 0, output_tokens: 0, created_at: "" });
  const model: Model = async (system) => system.startsWith("You are a strict reviewer") ? reply("Looks fine to me.") : system.startsWith("Rewrite") ? reply("REPLANNED") : reply("some output");
  await tick(store, model);
  const [t] = await store.getTasks("r");
  expect(t.verdict).toEqual({ pass: false, reason: "judge gave no verdict" }); expect(t.state).toBe("queued");
});
```

Keep the role dispatch on the system prompt; a test that needs a provider is a script you run
by hand.
"##;

const ORCH_README_MD: &str = r##"# {{NAME}}

A planner, a dependency queue in Postgres, workers with scoped tools and a cold-context judge —
CrewAI's task model as a service you can restart.

## What you get

- `POST /api/runs {goal}`: the planner writes 2–6 tasks, each with a description, acceptance
  criteria (`expected_output`), the tasks it depends on and the tools it may use. Cycles and
  unknown tools are refused before anything is stored.
- The database is the queue: a task is released when its dependencies are `done`, claimed with
  `for update skip locked`, so any number of workers share it and a restart loses nothing.
- Workers run a ReAct loop over only the task's tools, with a token budget per task and a step
  cap; dependency outputs are passed as context.
- A judge that sees the task, the criteria and the output — not the worker's transcript —
  decides pass or fail. A failed task is re-planned with the verdict and retried up to three
  times; then it is dead and the run is failed.
- Every model call's tokens on the task and on the run.
- A run viewer page; Hono API; kustomize overlays; six tests that run with no database and no
  API key.

## Five minutes

    cp backend/.env.example backend/.env    # add ANTHROPIC_API_KEY=…
    make demo

    curl -s localhost:8000/api/runs -H content-type:application/json \
      -d '{"goal":"What is 17% of 2,340, and is it more than 400? Show the arithmetic."}'
    # {"id":"0191…","tasks":2}

    curl -s localhost:8000/api/runs/0191…
    # {"state":"running","tasks":[{"key":"t1","tools":["calculator"],"state":"done","output":"397.8",…},{"key":"t2","depends_on":["t1"],"state":"running",…}]}

    sleep 10; curl -s localhost:8000/api/runs/0191…
    # {"state":"done","input_tokens":2210,"output_tokens":301,"tasks":[…,{"key":"t2","state":"done","verdict":{"pass":true,"reason":"…"}}]}

    curl -s localhost:8000/api/runs
    # {"runs":[{"goal":"What is 17% of…","state":"done",…}]}

Open <http://localhost:3000>: the graph as a table, coloured by state, live while it runs.

## API

| Method | Path | Auth | What |
|---|---|---|---|
| GET | `/api/health` | none | Liveness |
| GET | `/api/health/ready` | none | Readiness (does not yet probe the database) |
| GET | `/api/tools` | none | The tool registry the planner may assign from |
| POST | `/api/runs` | none | `{goal}` → plan and queue; `201 {id, tasks}`, `502 plan_failed` |
| GET | `/api/runs` | none | Latest 50 runs |
| GET | `/api/runs/:id` | none | The run with every task: state, attempts, output, verdict, tokens |
| POST | `/api/tick` | none | Run one queue step by hand; `{ran: bool}` |

There is no authentication and no owner on a run. Put a gateway in front, or add `user_id` —
`CLAUDE.md` has the recipe.

## Compared with CrewAI

**Same shape, so their docs describe this too**

- A task is `description`, `expected_output`, context from earlier tasks and a tool list; tasks
  run in dependency order; a later task receives the outputs of the tasks it depends on.
- Tools are a name, a description, a JSON Schema and a function.
- The worker loop is ReAct: think, call a tool, read the result, answer.

**Better here**

- Durable: every task, attempt, output and verdict is a row; kill the worker and start it again
  and the run continues from the next released task.
- Parallel by replica: `skip locked` lets N workers drain one queue without coordination.
- A judge with a cold context — it cannot be talked into a pass by the worker's reasoning —
  and re-planning of the one failed node with the verdict, not a restart of the crew.
- Bounded by construction: a token budget per task, a step cap, an attempt cap, and the totals
  on the run.
- An HTTP API and a viewer, typed end to end (`AppType` → the frontend client).
- Tests without a provider: one scripted model plays planner, worker, judge and re-planner.
- One TypeScript codebase — `app.ts`, `engine.ts`, `store.ts` — with Kubernetes manifests.

**Not here yet**

- Agents as first-class objects with a role, goal, backstory and their own LLM; every task here
  is run by the same anonymous worker prompt.
- The hierarchical process (a manager agent that delegates), delegation between agents, and
  asking a human.
- Memory — short-term, long-term, entity — and knowledge sources / RAG.
- Flows, conditional tasks, guardrails per task, structured (`pydantic`) task outputs.
- The tool library (search, scraping, file, code interpreter…); this ships `calculator` and
  `web_fetch`.
- Provider choice: Claude only, one model id for all roles.
- YAML configuration, callbacks, training, the CrewAI CLI and enterprise tooling.
- A public-address guard on `web_fetch` (the `agent` pack has one; port it before exposing this).

## Production

**Processes.** The API Deployment plus a worker Deployment running `bun src/worker.ts` (or the
built `dist/worker.js`); set `INLINE_WORKER=false` on the API once the worker exists. `k8s/base`
ships the API manifest; the worker one is yours to add — no Service, no probes, replicas =
desired parallelism.

**Environments and secrets.** `k8s/overlays/{dev,prod}`; `ANTHROPIC_API_KEY` and `DATABASE_URL`
from `backend/.env.age` via `make k8s-secrets ENV=…`. Dev and prod never share a database — a
shared queue would run prod goals on dev workers.

**Scaling.** Worker replicas. The API is stateless. One Postgres holds the queue; the claim is
one indexed statement (`tasks_ready`), fine to thousands of tasks.

**Probes.** `/api/health` and `/api/health/ready` on the API. The worker has none; watch the
stuck-task query instead: `select count(*) from tasks where state = 'running' and updated_at <
now() - interval '10 minutes'`.

**Migrations.** `0002_orchestrator.sql`; new ones through the migrate init container.

**What pages you.** Tasks stuck in `running` (a worker died mid-task — there is no lease yet),
`dead` tasks per hour, `502 plan_failed` rate, run tokens per goal above your expectation.

## Roadmap

- A lease on `running` tasks so a crashed worker's task is reclaimed.
- The public-address guard on `web_fetch`.
- Parallel tool calls within a task.
- Run ownership (`user_id`) and pagination.
- Cancel a run.
- A zod schema over the planner's JSON.
"##;

const ORCH_REVIEWER: &str = r##"---
name: graph-semantics
description: Run on any change to backend/src/engine.ts, store.ts (claimReady, updateTask), worker.ts, or the migration. Checks that the task graph still means what the queue thinks it means — release, scope, verdicts, attempts, settlement — and reports only what makes a run stall, double-run, or pass wrongly.
tools: Read, Grep, Glob, Bash
---

You review the queue and the graph. A wrong release rule runs a task before its inputs exist; a
wrong settle rule reports `done` on a run that is not.

Report each as `path:line — what — the run that breaks — the fix`.

Check:
1. **Release.** `claimReady` (both stores) still requires every key in `depends_on` to be a
   sibling in state `done` — not `running`, not `failed`, not missing. A missing key would
   satisfy `not exists`; confirm `plan` still strips unknown keys before insert.
2. **Atomic claim.** The SQL is one statement: `update … where id = (select … for update skip
   locked) returning *`; state set to `running` and `attempts + 1` in that statement. Two
   statements is a race.
3. **Cycles.** `hasCycle` runs on the filtered graph and `plan` throws on a cycle or an empty
   list. A self-dependency is removed before the check.
4. **Scope.** `work` filters tools by `task.tools`; an unscoped call yields an `is_error` tool
   result and the loop continues. The planner prompt lists exactly the names in `TOOLS`.
5. **Judge input.** `judge` receives `description`, `expected_output`, `output` and nothing
   from `messages`. Its signature still takes `Pick<Task, "description" | "expected_output">`.
   An unparseable reply is `{pass: false, reason: "judge gave no verdict"}`.
6. **Re-plan scope.** `replan` changes `description` only; `tick` writes back `description`,
   `output`, `verdict`, tokens and `state: "queued"` — never `depends_on`, `tools`, `key`.
7. **Attempts.** `task.attempts` compared to `MAX_ATTEMPTS` after the claim incremented it;
   the `MAX_ATTEMPTS`-th failure is `dead`, not `queued`.
8. **Budget.** `work` checks the budget before each model call and returns `output: null` with
   an `error`; `tick` treats `output === null` as a fail without calling the judge.
9. **Settlement.** Run state after a tick: `failed` if any task is `dead`, `done` if all
   `done`, else `running`. Tokens added to the run are this tick's only (`tok`), not the task's
   cumulative total — double counting shows as run tokens exceeding the sum of task tokens.
10. **Context.** The worker's context is the outputs of exactly `task.depends_on`, keyed; a
    task with no dependencies gets none.
11. **Step cap.** The `for (n < 12)` loop in `work` still ends with `error: "too many steps"`.
12. **Calculator.** The allowlist regex precedes `Function()`; no character outside
    `[\d\s+\-*/().]` reaches it.
13. **Worker loop.** `worker.ts` catches per-tick, logs one JSON line, sleeps, and exits the
    loop on SIGTERM; a throw must not kill the process without `db.end()`.
14. **Migration.** `unique (run_id, key)` still exists — the dependency query depends on it.

End with one line: `graph-semantics: N findings`, and if 0, which items you checked.
"##;
