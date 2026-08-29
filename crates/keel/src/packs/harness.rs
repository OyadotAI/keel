//! harness: an agent harness, like OpenHands / SWE-agent / Keel ═══════════════════════════════

pub fn files(name: &str) -> Vec<(&'static str, String)> {
    let f = |s: &str| s.replace("{{NAME}}", name);
    let _ = &f;
    vec![
        ("backend/src/app.ts", HARNESS_APP.into()),
        ("backend/src/store.ts", HARNESS_STORE.into()),
        ("backend/src/tools.ts", HARNESS_TOOLS.into()),
        ("backend/src/harness.ts", HARNESS_HARNESS.into()),
        ("backend/src/worker.ts", HARNESS_WORKER.into()),
        ("backend/src/app.test.ts", HARNESS_TEST.into()),
        ("backend/migrations/0002_harness.sql", HARNESS_SQL.into()),
        ("frontend/app/page.tsx", f(HARNESS_PAGE)),
    ]
}

const HARNESS_APP: &str = r##"import { Hono } from "hono";
import { z } from "zod";
import { pgStore, type Store } from "./store";
import { HOLD } from "./harness";

// The harness's surface, the shape Keel and OpenHands expose: POST a task against a repository,
// a worker runs the agent in a checkout, and everything it did is readable here — every call,
// the gate's verdict after every turn, the diff. Calls the policy held show up under
// /api/approvals until a person answers; the answer travels back to the worker through the
// store, and a denial carries its reason to the agent.

export function createApp(store: Store) {
  const app = new Hono()
    .get("/api/health", (c) => c.json({ status: "ok" }))
    .get("/api/health/ready", (c) => c.json({ status: "ok", db: "ok" as const }))
    .get("/api/policy", (c) => c.json({ hold: [...HOLD] }))

    .post("/api/tasks", async (c) => {
      const p = z.object({ repo: z.string().min(1).max(500), task: z.string().min(1).max(8000) }).safeParse(await c.req.json().catch(() => ({})));
      if (!p.success) return c.json({ error: { message: "repo and task required", code: "invalid" } }, 400);
      const id = crypto.randomUUID();
      await store.create({ id, ...p.data });
      return c.json({ id, state: "queued" }, 201);
    })
    .get("/api/tasks", async (c) => c.json({ tasks: await store.list(50) }))
    .get("/api/tasks/:id", async (c) => { const t = await store.get(c.req.param("id")); return t ? c.json(t) : c.json({ error: { message: "no such task", code: "not_found" } }, 404); })

    .get("/api/approvals", async (c) => c.json({ pending: await store.held() }))
    .post("/api/approvals/:id", async (c) => {
      const p = z.object({ allow: z.boolean(), reason: z.string().max(2000).default("") }).safeParse(await c.req.json().catch(() => ({})));
      if (!p.success) return c.json({ error: { message: "allow required", code: "invalid" } }, 400);
      const call = await store.getCall(c.req.param("id"));
      if (!call || call.decision !== "held") return c.json({ error: { message: "nothing to decide", code: "not_found" } }, 404);
      await store.updateCall(call.id, { decision: p.data.allow ? "approved" : "denied", reason: p.data.reason || (p.data.allow ? "approved" : "denied by reviewer") });
      return c.json({ ok: true });
    });

  return app;
}

const app = createApp(pgStore());
export type AppType = typeof app;
export default app;
"##;

const HARNESS_STORE: &str = r##"import { db } from "./db";

export type Task = { id: string; repo: string; task: string; state: string; turns: number; answer: string | null; diff: string; created_at: string };
export type Call = { id: string; task_id: string; turn: number; tool: string; input: Record<string, unknown>; output: string | null; error: boolean; decision: "allowed" | "held" | "approved" | "denied"; reason: string | null; ms: number };
export type Verdict = { task_id: string; turn: number; ok: boolean; output: string; ms: number };
export type TaskView = Task & { calls: Call[]; verdicts: Verdict[] };

export interface Store {
  create(t: Pick<Task, "id" | "repo" | "task">): Promise<void>;
  /// One queued task, claimed for this worker. SKIP LOCKED so replicas never take the same one.
  claim(): Promise<Task | null>;
  update(id: string, patch: Partial<Pick<Task, "state" | "turns" | "answer" | "diff">>): Promise<void>;
  get(id: string): Promise<TaskView | null>;
  list(limit: number): Promise<Task[]>;
  addCall(c: Call): Promise<void>;
  updateCall(id: string, patch: Partial<Pick<Call, "output" | "error" | "decision" | "reason" | "ms">>): Promise<void>;
  getCall(id: string): Promise<Call | null>;
  /// Calls waiting on a person, oldest first.
  held(): Promise<(Call & { task: string })[]>;
  addVerdict(v: Verdict): Promise<void>;
}

export function pgStore(): Store {
  return {
    create: async (t) => { await db`insert into tasks (id, repo, task) values (${t.id}, ${t.repo}, ${t.task})`; },
    claim: async () => (await db<Task[]>`update tasks set state = 'running', updated_at = now()
      where id = (select id from tasks where state = 'queued' order by created_at limit 1 for update skip locked) returning *`)[0] ?? null,
    update: async (id, p) => {
      await db`update tasks set state = coalesce(${p.state ?? null}, state), turns = coalesce(${p.turns ?? null}, turns),
        answer = coalesce(${p.answer ?? null}, answer), diff = coalesce(${p.diff ?? null}, diff), updated_at = now() where id = ${id}`;
    },
    get: async (id) => {
      const t = (await db<Task[]>`select * from tasks where id = ${id}`)[0];
      if (!t) return null;
      const calls = await db<Call[]>`select * from calls where task_id = ${id} order by created_at`;
      const verdicts = await db<Verdict[]>`select * from verdicts where task_id = ${id} order by turn`;
      return { ...t, calls, verdicts };
    },
    list: async (limit) => db<Task[]>`select id, repo, task, state, turns, answer, '' as diff, created_at from tasks order by created_at desc limit ${limit}`,
    addCall: async (c) => { await db`insert into calls (id, task_id, turn, tool, input, output, error, decision, reason, ms) values (${c.id}, ${c.task_id}, ${c.turn}, ${c.tool}, ${db.json(c.input as never)}, ${c.output}, ${c.error}, ${c.decision}, ${c.reason}, ${c.ms})`; },
    updateCall: async (id, p) => {
      await db`update calls set output = coalesce(${p.output ?? null}, output), error = coalesce(${p.error ?? null}, error), decision = coalesce(${p.decision ?? null}, decision),
        reason = coalesce(${p.reason ?? null}, reason), ms = coalesce(${p.ms ?? null}, ms) where id = ${id}`;
    },
    getCall: async (id) => (await db<Call[]>`select * from calls where id = ${id}`)[0] ?? null,
    held: async () => db`select c.*, t.task from calls c join tasks t on t.id = c.task_id where c.decision = 'held' order by c.created_at`,
    addVerdict: async (v) => { await db`insert into verdicts (task_id, turn, ok, output, ms) values (${v.task_id}, ${v.turn}, ${v.ok}, ${v.output}, ${v.ms}) on conflict (task_id, turn) do update set ok = excluded.ok, output = excluded.output, ms = excluded.ms`; },
  };
}

export function memoryStore(): Store {
  const tasks = new Map<string, Task>(); const calls = new Map<string, Call>(); const verdicts: Verdict[] = [];
  return {
    create: async (t) => { tasks.set(t.id, { ...t, state: "queued", turns: 0, answer: null, diff: "", created_at: new Date().toISOString() }); },
    claim: async () => { const t = [...tasks.values()].find((t) => t.state === "queued"); if (t) t.state = "running"; return t ? { ...t } : null; },
    update: async (id, p) => { const t = tasks.get(id); if (t) Object.assign(t, p); },
    get: async (id) => { const t = tasks.get(id); return t ? { ...t, calls: [...calls.values()].filter((c) => c.task_id === id), verdicts: verdicts.filter((v) => v.task_id === id) } : null; },
    list: async (limit) => [...tasks.values()].reverse().slice(0, limit),
    addCall: async (c) => { calls.set(c.id, { ...c }); },
    updateCall: async (id, p) => { const c = calls.get(id); if (c) Object.assign(c, p); },
    getCall: async (id) => { const c = calls.get(id); return c ? { ...c } : null; },
    held: async () => [...calls.values()].filter((c) => c.decision === "held").map((c) => ({ ...c, task: tasks.get(c.task_id)?.task ?? "" })),
    addVerdict: async (v) => { const i = verdicts.findIndex((x) => x.task_id === v.task_id && x.turn === v.turn); if (i >= 0) verdicts[i] = v; else verdicts.push(v); },
  };
}
"##;

const HARNESS_TOOLS: &str = r##"import { readFile, writeFile, readdir, stat } from "node:fs/promises";
import { resolve, sep, relative } from "node:path";
import { execFile } from "node:child_process";

// The agent's whole world: an in-process tool server over one checkout, and no shell. Every
// path is resolved and must stay inside the checkout — the agent cannot read ~/.ssh by asking
// for ../../. The gate is the repository's own check command, run by us, never by the agent
// with arguments of its choosing.

export type ToolDef = { name: string; description: string; input_schema: Record<string, unknown>; run: (input: Record<string, unknown>) => Promise<string> };
export type Gate = (cwd: string) => Promise<{ ok: boolean; output: string }>;

export function jail(root: string, p: unknown): string {
  const abs = resolve(root, String(p ?? ""));
  if (abs !== root && !abs.startsWith(root + sep)) throw new Error(`path escapes checkout: ${p}`);
  return abs;
}

export function shellGate(cmd = process.env.CHECK_COMMAND ?? "make check", timeoutMs = 10 * 60_000): Gate {
  return (cwd) => new Promise((res) => {
    execFile("sh", ["-c", cmd], { cwd, timeout: timeoutMs, maxBuffer: 4 << 20 }, (err, stdout, stderr) => {
      res({ ok: !err, output: (stdout + stderr).slice(-8000) });
    });
  });
}

export const gitDiff = (cwd: string): Promise<string> => new Promise((res) => {
  execFile("git", ["diff"], { cwd, maxBuffer: 4 << 20 }, (err, stdout) => res(err ? "" : stdout));
});

export function makeTools(root: string, gate: Gate): ToolDef[] {
  return [
    { name: "list_files", description: "List files under a directory of the checkout (relative path, '.' for root).", input_schema: { type: "object", properties: { path: { type: "string" } } },
      run: async ({ path }) => {
        const dir = jail(root, path ?? ".");
        const names = (await readdir(dir)).filter((n) => n !== "node_modules" && n !== ".git");
        const lines = await Promise.all(names.map(async (n) => ((await stat(resolve(dir, n))).isDirectory() ? n + "/" : n)));
        return lines.sort().join("\n") || "(empty)";
      } },
    { name: "read_file", description: "Read a file from the checkout.", input_schema: { type: "object", properties: { path: { type: "string" } }, required: ["path"] },
      run: async ({ path }) => (await readFile(jail(root, path), "utf8")).slice(0, 100_000) },
    { name: "edit_file", description: "Replace one exact occurrence of `old` with `new` in a file. Empty `old` creates or overwrites the file with `new`.", input_schema: { type: "object", properties: { path: { type: "string" }, old: { type: "string" }, new: { type: "string" } }, required: ["path", "old", "new"] },
      run: async ({ path, old, new: neu }) => {
        const file = jail(root, path); const rel = relative(root, file);
        if (!old) { await writeFile(file, String(neu)); return `wrote ${rel}`; }
        const text = await readFile(file, "utf8");
        const i = text.indexOf(String(old));
        if (i < 0) throw new Error("`old` not found in file");
        if (text.indexOf(String(old), i + 1) >= 0) throw new Error("`old` matches more than once; include more context");
        await writeFile(file, text.slice(0, i) + String(neu) + text.slice(i + String(old).length));
        return `edited ${rel}`;
      } },
    { name: "run_gate", description: "Run the repository's own check command and return its verdict.", input_schema: { type: "object", properties: {} },
      run: async () => { const v = await gate(root); return `${v.ok ? "PASS" : "FAIL"}\n${v.output}`; } },
    { name: "finish", description: "Stop working and report what was done.", input_schema: { type: "object", properties: { summary: { type: "string" } }, required: ["summary"] },
      run: async ({ summary }) => String(summary) },
  ];
}
"##;

const HARNESS_HARNESS: &str = r##"import { makeTools, shellGate, gitDiff, type Gate, type ToolDef } from "./tools";
import type { Store, Call } from "./store";

// One coding-agent run, the way OpenHands and SWE-agent shape it and the way Keel supervises
// it: the model sees a task and a small tool set, and every call goes through a policy before
// it runs. A call the policy names is held — recorded, shown to a person, and the agent waits.
// Approved, it runs; denied, the reason goes back and the run stops rather than working around
// it. After every turn the repository's own check command runs and its verdict is stored next
// to the diff, so the review page shows what changed, what was called, and whether it passes.
// The model is a function, so tests script it and never call a provider.

export type ToolCall = { id: string; name: string; input: Record<string, unknown> };
export type ModelReply = { text: string; calls: ToolCall[]; input_tokens: number; output_tokens: number };
export type Message = { role: "user" | "assistant"; content: unknown };
export type Model = (system: string, messages: Message[], tools: ToolDef[]) => Promise<ModelReply>;

export const SYSTEM = `You are a coding agent working in a checkout of a repository. You have no shell: read and edit files with the tools, then run_gate to check your work. When the gate passes, or you cannot make progress, call finish with a short summary.`;

export const HOLD = new Set((process.env.HOLD_TOOLS ?? "edit_file").split(",").map((s) => s.trim()).filter(Boolean));

export type Opts = { gate?: Gate; diff?: (cwd: string) => Promise<string>; hold?: Set<string>; maxTurns?: number; pollMs?: number; waitMs?: number };
export type Result = { state: "done" | "failed"; answer: string; turns: number };

export async function runTask(store: Store, taskId: string, task: string, checkout: string, model: Model, opts: Opts = {}): Promise<Result> {
  const gate = opts.gate ?? shellGate(); const diff = opts.diff ?? gitDiff; const hold = opts.hold ?? HOLD;
  const maxTurns = opts.maxTurns ?? 20;
  const tools = makeTools(checkout, gate);
  const messages: Message[] = [{ role: "user", content: task }];
  let turn = 0;

  const finish = async (state: Result["state"], answer: string) => {
    await store.update(taskId, { state, answer, turns: turn, diff: await diff(checkout) });
    return { state, answer, turns: turn };
  };

  while (turn < maxTurns) {
    turn++;
    const reply = await model(SYSTEM, messages, tools);
    const blocks: unknown[] = reply.text ? [{ type: "text", text: reply.text }] : [];
    for (const c of reply.calls) blocks.push({ type: "tool_use", id: c.id, name: c.name, input: c.input });
    messages.push({ role: "assistant", content: blocks });
    if (!reply.calls.length) return finish("done", reply.text || "(no summary)");

    const results: unknown[] = [];
    for (const c of reply.calls) {
      const done = reply.calls.find((x) => x.name === "finish");
      const call: Call = { id: c.id, task_id: taskId, turn, tool: c.name, input: c.input, output: null, error: false, decision: hold.has(c.name) ? "held" : "allowed", reason: null, ms: 0 };
      await store.addCall(call);
      const t0 = Date.now();
      if (call.decision === "held") {
        await store.update(taskId, { state: "waiting" });
        const d = await decision(store, c.id, opts.pollMs ?? 500, opts.waitMs ?? 30 * 60_000);
        await store.update(taskId, { state: "running" });
        if (d.decision !== "approved") {
          await store.updateCall(c.id, { decision: "denied", reason: d.reason, output: `denied: ${d.reason}`, error: true, ms: Date.now() - t0 });
          return finish("failed", `${c.name} denied: ${d.reason}`);
        }
      }
      const tool = tools.find((t) => t.name === c.name);
      try {
        if (!tool) throw new Error(`unknown tool ${c.name}`);
        const output = (await tool.run(c.input)).slice(0, 20_000);
        await store.updateCall(c.id, { output, ms: Date.now() - t0 });
        results.push({ type: "tool_result", tool_use_id: c.id, content: output });
      } catch (e) {
        const msg = e instanceof Error ? e.message : String(e);
        await store.updateCall(c.id, { output: `Error: ${msg}`, error: true, ms: Date.now() - t0 });
        results.push({ type: "tool_result", tool_use_id: c.id, content: `Error: ${msg}`, is_error: true });
      }
      if (done && c.name === "finish") { await verdict(); return finish("done", String(c.input.summary ?? "")); }
    }
    messages.push({ role: "user", content: results });
    await verdict();
  }
  return finish("failed", `stopped after ${maxTurns} turns`);

  // The gate runs after every turn regardless of what the agent did, and its output is stored.
  async function verdict() {
    const t0 = Date.now();
    const v = await gate(checkout);
    await store.addVerdict({ task_id: taskId, turn, ok: v.ok, output: v.output, ms: Date.now() - t0 });
    await store.update(taskId, { turns: turn, diff: await diff(checkout) });
  }
}

/// Wait for a person. The worker is a different process from the API, so the answer arrives
/// through the store. No answer in time is a denial with a reason, never a silent allow.
async function decision(store: Store, callId: string, pollMs: number, waitMs: number): Promise<{ decision: Call["decision"]; reason: string }> {
  const until = Date.now() + waitMs;
  while (Date.now() < until) {
    const c = await store.getCall(callId);
    if (c && c.decision !== "held") return { decision: c.decision, reason: c.reason ?? "" };
    await new Promise((r) => setTimeout(r, pollMs));
  }
  return { decision: "denied", reason: "nobody answered" };
}

/// Claude via the Messages API. Set ANTHROPIC_API_KEY; MODEL defaults to Sonnet.
type Fetch = (url: string | URL, init?: RequestInit) => Promise<Response>;
export function claude(fetchImpl: Fetch = fetch): Model {
  return async (system, messages, tools) => {
    const res = await fetchImpl("https://api.anthropic.com/v1/messages", {
      method: "POST", headers: { "x-api-key": process.env.ANTHROPIC_API_KEY ?? "", "anthropic-version": "2023-06-01", "content-type": "application/json" },
      body: JSON.stringify({ model: process.env.MODEL ?? "claude-sonnet-4-20250514", max_tokens: 4096, system, messages, tools: tools.map((t) => ({ name: t.name, description: t.description, input_schema: t.input_schema })) }),
    });
    if (!res.ok) throw new Error(`anthropic ${res.status}: ${(await res.text()).slice(0, 300)}`);
    const a = (await res.json()) as { content: { type: string; text?: string; id?: string; name?: string; input?: Record<string, unknown> }[]; usage: { input_tokens: number; output_tokens: number } };
    return {
      text: a.content.filter((b) => b.type === "text").map((b) => b.text).join(""),
      calls: a.content.filter((b) => b.type === "tool_use").map((b) => ({ id: b.id!, name: b.name!, input: b.input ?? {} })),
      input_tokens: a.usage.input_tokens, output_tokens: a.usage.output_tokens,
    };
  };
}
"##;

const HARNESS_WORKER: &str = r##"import { execFile } from "node:child_process";
import { mkdir } from "node:fs/promises";
import { join } from "node:path";
import { pgStore, type Store } from "./store";
import { runTask, claude, type Model } from "./harness";

// The worker: claim one task, give it a checkout of its own, run the agent in it. A clone per
// task means two tasks on one repository never see each other's edits, and the diff is the
// whole record of what the run did. Runs as its own process (`bun run worker`) and Deployment.

const CHECKOUTS = process.env.CHECKOUTS ?? "/tmp/harness";

// `--` ends git's options, so a repo string starting with `-` is a path, not a flag (a
// `--upload-pack=` there runs a command). Only URL-shaped or local absolute repos are accepted.
export const gitClone = (repo: string, dest: string) => new Promise<void>((res, rej) => {
  if (!/^(https?:\/\/|ssh:\/\/|git@[\w.-]+:|\/)/.test(repo)) return rej(new Error("repo must be an https, ssh or absolute path"));
  execFile("git", ["clone", "-q", "--", repo, dest], { timeout: 5 * 60_000 }, (err, _o, stderr) => (err ? rej(new Error(stderr || err.message)) : res()));
});

export async function tick(store: Store, model: Model, checkout: (repo: string, dest: string) => Promise<void> = gitClone): Promise<boolean> {
  const t = await store.claim();
  if (!t) return false;
  const dest = join(CHECKOUTS, t.id);
  try {
    await mkdir(CHECKOUTS, { recursive: true });
    await checkout(t.repo, dest);
    await runTask(store, t.id, t.task, dest, model);
  } catch (e) {
    await store.update(t.id, { state: "failed", answer: e instanceof Error ? e.message : String(e) });
  }
  return true;
}

if (import.meta.main) {
  const store = pgStore(); const model = claude();
  console.log(JSON.stringify({ level: "info", msg: "worker started", checkouts: CHECKOUTS }));
  let stopping = false;
  process.on("SIGTERM", () => { stopping = true; });
  while (!stopping) {
    if (!(await tick(store, model))) await new Promise((r) => setTimeout(r, 1000));
  }
}
"##;

const HARNESS_TEST: &str = r##"import { describe, expect, test } from "bun:test";
import { mkdtemp, readFile, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createApp } from "./app";
import { memoryStore } from "./store";
import { runTask, type Model, type ModelReply } from "./harness";
import { makeTools, jail } from "./tools";

// A scripted model drives the agent over a real temp directory. The gate and the diff are
// functions, so nothing spawns `make` or `git`. No provider, no Docker.
const script = (replies: Partial<ModelReply>[]): Model => { let i = 0; return async () => ({ text: "", calls: [], input_tokens: 1, output_tokens: 1, ...replies[Math.min(i++, replies.length - 1)] }); };
const fresh = async () => { const dir = await mkdtemp(join(tmpdir(), "harness-")); await writeFile(join(dir, "a.txt"), "hello world\n"); return dir; };
const gate = async (cwd: string) => ({ ok: (await readFile(join(cwd, "a.txt"), "utf8")).includes("bye"), output: "checked" });
const diff = async (cwd: string) => "diff: " + (await readFile(join(cwd, "a.txt"), "utf8"));
const noHold = { gate, diff, hold: new Set<string>() };

describe("harness", () => {
  test("tools are jailed to the checkout", async () => {
    const dir = await fresh(); const tools = makeTools(dir, gate);
    expect(() => jail(dir, "../../etc/passwd")).toThrow("escapes");
    await expect(tools.find((t) => t.name === "read_file")!.run({ path: "/etc/passwd" })).rejects.toThrow("escapes");
    expect(await tools.find((t) => t.name === "list_files")!.run({})).toBe("a.txt");
    await tools.find((t) => t.name === "edit_file")!.run({ path: "a.txt", old: "world", new: "there" });
    expect(await readFile(join(dir, "a.txt"), "utf8")).toBe("hello there\n");
  });

  test("a run traces every call, gates every turn, and stores the diff", async () => {
    const store = memoryStore(); const dir = await fresh();
    await store.create({ id: "t1", repo: dir, task: "say bye" });
    const model = script([
      { calls: [{ id: "c1", name: "read_file", input: { path: "a.txt" } }] },
      { calls: [{ id: "c2", name: "edit_file", input: { path: "a.txt", old: "hello", new: "bye" } }] },
      { calls: [{ id: "c3", name: "finish", input: { summary: "replaced hello" } }] },
    ]);
    const r = await runTask(store, "t1", "say bye", dir, model, noHold);
    expect(r).toEqual({ state: "done", answer: "replaced hello", turns: 3 });
    const t = (await store.get("t1"))!;
    expect(t.calls.map((c) => c.tool)).toEqual(["read_file", "edit_file", "finish"]);
    expect(t.calls[0].output).toBe("hello world\n");
    expect(t.verdicts.map((v) => v.ok)).toEqual([false, true, true]);
    expect(t.diff).toBe("diff: bye world\n");
  });

  test("a held call waits for approval and proceeds when approved", async () => {
    const store = memoryStore(); const app = createApp(store); const dir = await fresh();
    await store.create({ id: "t2", repo: dir, task: "x" });
    const model = script([{ calls: [{ id: "h1", name: "edit_file", input: { path: "a.txt", old: "hello", new: "bye" } }] }, { text: "done" }]);
    const run = runTask(store, "t2", "x", dir, model, { gate, diff, hold: new Set(["edit_file"]), pollMs: 5 });
    await new Promise((r) => setTimeout(r, 20));
    expect((await store.get("t2"))!.state).toBe("waiting");
    const pending = (await (await app.request("/api/approvals")).json()).pending;
    expect(pending.map((p: { id: string }) => p.id)).toEqual(["h1"]);
    expect((await app.request("/api/approvals/h1", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ allow: true }) })).status).toBe(200);
    expect((await run).state).toBe("done");
    expect(await readFile(join(dir, "a.txt"), "utf8")).toBe("bye world\n");
    expect((await store.getCall("h1"))!.decision).toBe("approved");
    // Decided once: a second answer finds nothing to decide.
    expect((await app.request("/api/approvals/h1", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ allow: false }) })).status).toBe(404);
  });

  test("a denial carries its reason and stops the run instead of substituting", async () => {
    const store = memoryStore(); const dir = await fresh(); let modelCalls = 0;
    await store.create({ id: "t3", repo: dir, task: "x" });
    const model: Model = async () => { modelCalls++; return { text: "", calls: [{ id: "d1", name: "edit_file", input: { path: "a.txt", old: "hello", new: "bye" } }], input_tokens: 1, output_tokens: 1 }; };
    const run = runTask(store, "t3", "x", dir, model, { gate, diff, hold: new Set(["edit_file"]), pollMs: 5 });
    await new Promise((r) => setTimeout(r, 20));
    await store.updateCall("d1", { decision: "denied", reason: "not that file" });
    const r = await run;
    expect(r.state).toBe("failed"); expect(r.answer).toContain("not that file");
    expect(modelCalls).toBe(1);
    expect(await readFile(join(dir, "a.txt"), "utf8")).toBe("hello world\n");
    // Nobody answering is a denial too, never a silent allow.
    await store.create({ id: "t4", repo: dir, task: "x" });
    const r2 = await runTask(store, "t4", "x", dir, model, { gate, diff, hold: new Set(["edit_file"]), pollMs: 5, waitMs: 15 });
    expect(r2.answer).toContain("nobody answered");
  });

  test("the run ends at max turns and the API validates tasks", async () => {
    const store = memoryStore(); const app = createApp(store); const dir = await fresh();
    await store.create({ id: "t5", repo: dir, task: "x" });
    const r = await runTask(store, "t5", "x", dir, script([{ calls: [{ id: "r", name: "read_file", input: { path: "a.txt" } }] }]), { ...noHold, maxTurns: 2 });
    expect(r).toEqual({ state: "failed", answer: "stopped after 2 turns", turns: 2 });
    expect((await store.get("t5"))!.verdicts.length).toBe(2);
    expect((await app.request("/api/tasks", { method: "POST", headers: { "content-type": "application/json" }, body: "{}" })).status).toBe(400);
    const res = await app.request("/api/tasks", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ repo: dir, task: "fix it" }) });
    expect(res.status).toBe(201);
    const { id } = await res.json();
    expect((await (await app.request(`/api/tasks/${id}`)).json()).state).toBe("queued");
    expect((await app.request("/api/tasks/nope")).status).toBe(404);
  });
});
"##;

const HARNESS_SQL: &str = r##"create table if not exists tasks (
  id uuid primary key,
  repo text not null,
  task text not null,
  state text not null default 'queued',   -- queued | running | waiting | done | failed
  turns int not null default 0,
  answer text,
  diff text not null default '',
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now()
);
create index if not exists tasks_ready on tasks (state, created_at);

-- Every tool call the agent made, with what came back. `decision` is the policy's answer:
-- allowed outright, held for a person, then approved or denied by them.
create table if not exists calls (
  id text primary key,
  task_id uuid not null references tasks(id),
  turn int not null,
  tool text not null,
  input jsonb not null default '{}',
  output text,
  error boolean not null default false,
  decision text not null default 'allowed', -- allowed | held | approved | denied
  reason text,
  ms int not null default 0,
  created_at timestamptz not null default now()
);
create index if not exists calls_task on calls (task_id, created_at);
create index if not exists calls_held on calls (decision) where decision = 'held';

-- The repository's own check command, run after every turn.
create table if not exists verdicts (
  task_id uuid not null references tasks(id),
  turn int not null,
  ok boolean not null,
  output text not null default '',
  ms int not null default 0,
  created_at timestamptz not null default now(),
  primary key (task_id, turn)
);
"##;

const HARNESS_PAGE: &str = r##""use client";
import { useEffect, useState } from "react";

type Task = { id: string; repo: string; task: string; state: string; turns: number; answer: string | null };
type Call = { id: string; turn: number; tool: string; input: Record<string, unknown>; output: string | null; error: boolean; decision: string; reason: string | null; ms: number };
type Verdict = { turn: number; ok: boolean; output: string; ms: number };
type View = Task & { diff: string; calls: Call[]; verdicts: Verdict[] };

const post = (url: string, body: unknown) => fetch(url, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body) });

// The review page: the diff beside the calls, the gate's verdict per turn, and the held calls
// with approve / deny. Polls, because a run is a worker process and this is its window.
export default function Home() {
  const [tasks, setTasks] = useState<Task[]>([]);
  const [sel, setSel] = useState<View | null>(null);
  const [repo, setRepo] = useState(""); const [task, setTask] = useState("Add a README section describing how to run the tests.");

  async function refresh() {
    setTasks((await (await fetch("/api/tasks")).json()).tasks);
    if (sel) setSel(await (await fetch(`/api/tasks/${sel.id}`)).json());
  }
  useEffect(() => { refresh(); const t = setInterval(refresh, 2000); return () => clearInterval(t); }, [sel?.id]); // eslint-disable-line react-hooks/exhaustive-deps

  async function submit() { await post("/api/tasks", { repo, task }); refresh(); }
  async function decide(id: string, allow: boolean) { await post(`/api/approvals/${id}`, { allow, reason: allow ? "" : prompt("Why not?") ?? "" }); refresh(); }

  const held = sel?.calls.filter((c) => c.decision === "held") ?? [];
  return (
    <main>
      <h1>{{NAME}} — agent harness</h1>
      <p>Queue a task: <code>{`curl -X POST http://localhost:8000/api/tasks -H 'content-type: application/json' -d '{"repo":"/path/to/repo","task":"fix the failing test"}'`}</code> — then <code>bun run worker</code> in backend/ with <code>ANTHROPIC_API_KEY</code> set.</p>
      <p><input value={repo} onChange={(e) => setRepo(e.target.value)} placeholder="repository path or URL" style={{ width: "40%" }} /> <input value={task} onChange={(e) => setTask(e.target.value)} style={{ width: "50%" }} /> <button onClick={submit} disabled={!repo}>Run</button></p>
      <table border={1} cellPadding={4}><thead><tr><th>state</th><th>task</th><th>turns</th><th>answer</th></tr></thead><tbody>
        {tasks.map((t) => <tr key={t.id} onClick={async () => setSel(await (await fetch(`/api/tasks/${t.id}`)).json())} style={{ cursor: "pointer" }}><td>{t.state}</td><td>{t.task}</td><td>{t.turns}</td><td>{t.answer}</td></tr>)}
      </tbody></table>
      {sel && (
        <section>
          <h2>{sel.task} <small>({sel.state})</small></h2>
          {held.length > 0 && <div style={{ border: "2px solid orange", padding: 8 }}>
            {held.map((c) => <p key={c.id}><strong>{c.tool}</strong> <code>{JSON.stringify(c.input).slice(0, 300)}</code> <button onClick={() => decide(c.id, true)}>Approve</button> <button onClick={() => decide(c.id, false)}>Deny</button></p>)}
          </div>}
          <div style={{ display: "flex", gap: 16 }}>
            <div style={{ flex: 1 }}>
              <h3>Calls</h3>
              {sel.calls.map((c) => <details key={c.id}><summary>turn {c.turn} · {c.tool} · {c.decision}{c.error ? " · error" : ""} · {c.ms} ms</summary><pre>{JSON.stringify(c.input, null, 1)}{"\n→ "}{c.output}</pre></details>)}
              <h3>Gate</h3>
              {sel.verdicts.map((v) => <details key={v.turn}><summary>turn {v.turn}: {v.ok ? "PASS" : "FAIL"} · {v.ms} ms</summary><pre>{v.output}</pre></details>)}
            </div>
            <div style={{ flex: 1 }}><h3>Diff</h3><pre style={{ whiteSpace: "pre-wrap" }}>{sel.diff || "(no changes yet)"}</pre></div>
          </div>
        </section>
      )}
    </main>
  );
}
"##;
