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
        ("CLAUDE.md", f(HARNESS_CLAUDE_MD)),
        ("AGENTS.md", f(HARNESS_AGENTS_MD)),
        ("README.md", f(HARNESS_README_MD)),
        (
            ".claude/agents/agent-policy.md",
            HARNESS_AGENT_POLICY.into(),
        ),
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

const HARNESS_CLAUDE_MD: &str = r##"# {{NAME}} — working agreement

{{NAME}} is a coding-agent harness in the shape of OpenHands, SWE-agent and Keel itself: POST a
task against a repository, a worker clones it and runs a model over a five-tool world with no
shell, a policy holds the calls you name until a person answers, and the repository's own check
command runs after every turn. "Done" here means a task row with `state = done`, every call it
made in `calls`, a verdict per turn in `verdicts`, and the diff stored on the task — a run you
can review without having watched it.

## Architecture

| File | Owns |
|---|---|
| `backend/src/app.ts` | The HTTP surface: tasks, approvals, policy. Builds from a `Store`; exports `AppType`. |
| `backend/src/store.ts` | `Store` interface, `pgStore` (Postgres, `SKIP LOCKED` claim) and `memoryStore` (tests). |
| `backend/src/tools.ts` | The agent's world: `jail`, the five tools, `shellGate` (the check command), `gitDiff`. |
| `backend/src/harness.ts` | `runTask`: the turn loop, the hold/approve/deny policy, the per-turn verdict; `claude()` model adapter. |
| `backend/src/worker.ts` | `tick`: claim a task, `git clone` it, run it. The `bun run worker` process. |
| `backend/src/app.test.ts` | Five tests over a scripted model and a temp directory; no provider, no `make`, no `git`. |
| `backend/migrations/0002_harness.sql` | `tasks`, `calls`, `verdicts`. |
| `frontend/app/page.tsx` | The review page: calls, verdicts and diff per task; approve/deny for held calls. Polls every 2s. |

### Request path

1. `POST /api/tasks {repo, task}` — validated by zod, inserted as `queued`, `201 {id}`.
2. `worker.ts tick()` claims one queued task (`update … where id = (select … for update skip locked)`), so replicas never take the same task.
3. `gitClone(repo, CHECKOUTS/<id>)` — `--` before the repo, and only `https://`, `ssh://`, `git@host:` or an absolute path is accepted.
4. `runTask` sends the system prompt, the task and the tool definitions to the model; each reply's calls are recorded in `calls` before anything runs.
5. A call whose tool is in `HOLD` is written as `decision = held`, the task flips to `waiting`, and the worker polls `store.getCall` until a person answers via `POST /api/approvals/:id` — or `waitMs` elapses, which is a denial.
6. Approved or allowed calls run through `tools.ts`; every path is resolved by `jail` and must stay inside the checkout. Output is capped at 20 000 characters.
7. After every turn `verdict()` runs `CHECK_COMMAND` in the checkout and stores `{ok, output}` keyed `(task_id, turn)`, then refreshes `tasks.diff` from `git diff`.
8. `finish` (or a reply with no calls, or `maxTurns`, or a denial) sets the final state and answer.

### Data model

| Table | Column | Why |
|---|---|---|
| `tasks` | `state` | `queued → running → waiting → running → done/failed`. `waiting` is what the page shows while a person is being asked. |
| | `turns`, `diff`, `answer` | Updated after every turn, so a task you open mid-run already shows what it has done. |
| `calls` | `id` | The model's own `tool_use` id, so a result is matched to its call without a lookup. |
| | `decision`, `reason` | `allowed \| held \| approved \| denied`. The reason is what the agent is told. |
| | `input jsonb`, `output`, `error`, `ms` | The full trace; the review page renders this, nothing else. |
| `verdicts` | `(task_id, turn)` primary key | One verdict per turn; `addVerdict` upserts. |
| `tasks_ready`, `calls_held` | indexes | The claim query and `/api/approvals` are the two hot paths. |

## Invariants

1. **The agent never gets a shell.** The tool set is `list_files`, `read_file`, `edit_file`, `run_gate`, `finish`, and `run_gate` runs a command Keel chose (`CHECK_COMMAND`), never one the agent passed. Adding a `bash` tool is a product decision, not a feature. Guarded by `tools are jailed to the checkout`, which exercises the five by name; a sixth tool needs a test of its own before it ships.
2. **Every path stays inside the checkout.** `jail()` resolves and compares against `root + sep`; `../`, absolute paths and symlinked escapes all throw. Guarded by `tools are jailed to the checkout`.
3. **A held call blocks the run.** The worker does not continue to the next call or the next model turn while `decision = held`. Guarded by `a held call waits for approval and proceeds when approved` (task state is `waiting`, `/api/approvals` lists it, approving lets that same call run).
4. **A denial ends the run with its reason.** The model is not asked again, the file is untouched, and the answer contains the reason. Guarded by `a denial carries its reason and stops the run instead of substituting` (`modelCalls` stays 1).
5. **No answer is a denial, never an allow.** `decision()` times out to `{denied, "nobody answered"}`. Same test, second half.
6. **A decision is made once.** `POST /api/approvals/:id` on a call that is no longer `held` is 404. Same test as 3.
7. **The gate runs after every turn, whatever the agent did.** `verdict()` is called from the loop, not from a tool; the agent calling `run_gate` is extra, not instead. Guarded by `a run traces every call, gates every turn, and stores the diff` (`verdicts` is `[false, true, true]` for three turns).
8. **Every call is recorded before it runs.** `store.addCall` precedes `tool.run`, so a crash mid-tool leaves the call in the trace. Same test (`calls` names in order).
9. **A run always ends.** `maxTurns` (default 20) fails the task with `stopped after N turns`. Guarded by `the run ends at max turns and the API validates tasks`.
10. **One checkout per task.** `worker.ts` clones into `CHECKOUTS/<task id>`; two tasks on one repository never share a working tree. Not unit-tested (it needs `git`); `tick` takes the clone function so tests inject one.
11. **The repo argument cannot be a git option.** `gitClone` passes `--` and rejects anything not URL-shaped or absolute. A `--upload-pack=` repo string runs a command otherwise.

## Extending it

**Add a tool.** Append to `makeTools` in `tools.ts` with `name`, `description`, `input_schema` and `run`. Every path argument goes through `jail`. Add it to the list asserted in `app.test.ts`, and decide whether it belongs in `HOLD_TOOLS`. No migration.

**Hold a different set of tools.** `HOLD_TOOLS=edit_file,run_gate` in the environment; `GET /api/policy` shows what is held. Tests pass `hold:` explicitly and never read the env.

**Change the gate.** `CHECK_COMMAND` in the environment (default `make check`), or pass `opts.gate` to `runTask`. The gate is a `(cwd) => {ok, output}` function; tests use one that reads a file.

**Add a model provider.** Write a `Model` — `(system, messages, tools) => ModelReply` — beside `claude()` in `harness.ts` and pick it in `worker.ts`. `messages` are Anthropic-shaped content blocks (`text`, `tool_use`, `tool_result`); a provider with a different wire shape converts in its adapter, as `builder`'s `claude()` does.

**Add a field to the trace.** Migration `0003_*.sql` adds the column; extend `Call` in `store.ts`, both stores, and `updateCall`'s `coalesce` list. `memoryStore` is the test double — keep it in step or the tests lie.

**Clean up checkouts.** `tick` never deletes `CHECKOUTS/<id>`. Add `rm(dest, {recursive: true})` in a `finally` once the diff is stored, or a sweep on `tasks.updated_at`. Test it through `tick` with an injected clone.

**Add a follow-up message.** There is no route for one; a task is one prompt. Add `POST /api/tasks/:id/messages`, a `messages` table, and have `runTask` read it between turns. That changes invariant 9's meaning — decide what "always ends" means first.

## Operating it

| Variable | Required | Meaning |
|---|---|---|
| `DATABASE_URL` | yes | Postgres; both API and worker. |
| `ANTHROPIC_API_KEY` | worker | Messages API key. The API process never calls a model. |
| `MODEL` | no | Defaults to `claude-sonnet-4-20250514`. |
| `HOLD_TOOLS` | no | Comma list; default `edit_file`. |
| `CHECK_COMMAND` | no | The gate; default `make check`, 10 minute timeout, last 8 000 chars kept. |
| `CHECKOUTS` | no | Where clones go; default `/tmp/harness`. |
| `PORT` | no | API port, default 8000. |

**Processes.** `backend` serves HTTP and is stateless. `worker` (`bun run worker`) runs one task at a time per process; run more workers for more concurrency — `SKIP LOCKED` makes that safe. The `Dockerfile` builds `src/server.ts` only; a worker image needs `src/worker.ts` on the `bun build` line and its own Deployment (no Service, no HTTP probes).

**What is per-replica.** Nothing in memory survives a request; the queue, the held calls and the decision channel are all Postgres rows. A worker that dies mid-task leaves the task `running` forever — there is no lease or heartbeat yet (see Ceilings).

**Failure modes.**

| What | User sees |
|---|---|
| Clone fails (bad URL, no access) | Task `failed`, answer is git's stderr. |
| Model call fails (401, 529) | Task `failed`, answer is `anthropic <status>: …`. Not retried. |
| Gate exceeds 10 minutes | Verdict `ok: false` with whatever output there was; the run continues. |
| Nobody answers a held call for 30 minutes | Call `denied: nobody answered`, task `failed`. |
| Worker process dies mid-run | Task stuck `running`; nothing picks it up. |

**Logs and metrics.** The worker logs one JSON line at start; `calls.ms` and `verdicts.ms` are the timings that matter. Watch: count of `tasks.state = 'waiting'` (people are the bottleneck), age of the oldest `queued` task (workers are), and `verdicts` where `ok = false` on the final turn (the agent gave up green-less).

## Ceilings

- **Decisions are polled**, 500 ms against Postgres per waiting task. Fine to tens of waiting tasks; `LISTEN/NOTIFY` on `calls` when it is not.
- **No lease on a running task.** A dead worker orphans it. Add `claimed_at` and a heartbeat, and requeue what is stale.
- **The gate runs on the worker host.** `CHECK_COMMAND` is `sh -c` in a clone of a repository someone submitted — its `Makefile` is arbitrary code on your worker. Run workers in a throwaway container per task (the `sandbox` pack's `dockerArgv` is the shape) before accepting repositories you do not own.
- **`git diff` misses new files.** Untracked files are not in the stored diff. `git add -N .` before diffing, or `git diff HEAD` after staging.
- **No context management.** `messages` grows for the whole run; a 20-turn run with large `read_file` outputs will hit the model's context limit and fail. Truncate old tool results or summarise.
- **One prompt per task**, no follow-ups, no streaming: the page polls every 2 s.
- **No authentication on the API.** Anyone who can reach `:8000` can queue a clone of any URL and approve calls. Put it behind the ingress auth or add a bearer token before exposing it.
- **Checkouts are never deleted.**

The stack rules — gate, typed seam, production checklist, deploy — are in `docs/PRODUCTION.md`. They apply.
"##;

const HARNESS_AGENTS_MD: &str = r##"# {{NAME}} — for agents

See `CLAUDE.md` for the rules. This is how to run it.

## Run

    make demo                          # postgres + redis, migrate, seed, API on :8000, page on :3000
    make check                         # the gate: typecheck both halves, bun test the backend
    cd backend && ANTHROPIC_API_KEY=… bun run worker    # the agent runner; nothing runs without it

The API and the worker are separate processes sharing Postgres. `make demo` starts the API and the
page; the worker is yours to start, because it needs a key.

## Routes

Queue a task:

    curl -s -X POST localhost:8000/api/tasks -H 'content-type: application/json' \
      -d '{"repo":"/abs/path/to/repo","task":"Add a README section describing how to run the tests."}'
    # 201 {"id":"3f2c…","state":"queued"}

List and read:

    curl -s localhost:8000/api/tasks              # {"tasks":[{id, repo, task, state, turns, answer, created_at}, …]} newest first, 50
    curl -s localhost:8000/api/tasks/3f2c…        # {…task, "diff": "...", "calls": [...], "verdicts": [...]}

A call the policy held:

    curl -s localhost:8000/api/approvals
    # {"pending":[{"id":"toolu_01…","task_id":"3f2c…","turn":2,"tool":"edit_file","input":{"path":"README.md","old":"…","new":"…"},"decision":"held","task":"Add a README section…"}]}
    curl -s -X POST localhost:8000/api/approvals/toolu_01… -H 'content-type: application/json' -d '{"allow":true}'
    # 200 {"ok":true}          — the worker sees the decision within 500 ms and runs the call
    curl -s -X POST localhost:8000/api/approvals/toolu_01… -H 'content-type: application/json' -d '{"allow":false,"reason":"not that file"}'
    # 404 nothing to decide   — already decided; a decision is made once

What is held:

    curl -s localhost:8000/api/policy             # {"hold":["edit_file"]}

Errors are `{"error":{"message","code"}}` with `400 invalid` or `404 not_found`.

## Tests

`backend/src/app.test.ts`, run by `bun test`. Nothing external:

- **`memoryStore()`** replaces Postgres. Same interface, so `createApp` and `runTask` do not know.
- **A scripted model** — `script([...replies])` returns each reply in turn and repeats the last. A reply is `{text, calls: [{id, name, input}]}`.
- **A temp directory** from `mkdtemp` is the checkout; the tools really read and write it.
- **Gate and diff are functions** passed in `opts`; the test gate reads `a.txt`, so no `make` or `git` is spawned.
- **Timing** is `pollMs: 5` and `waitMs: 15` where a wait matters, so the timeout test takes milliseconds.

To add a test: create a store, an app if you need routes, a `fresh()` directory, a scripted model, and call `runTask` with `{gate, diff, hold, pollMs}`. Assert on `store.get(id)` — `calls`, `verdicts`, `diff` — rather than on what the model was told; the trace is the contract. A test that needs `git` or a provider is wrong for this file; inject the function instead, as `tick(store, model, checkout)` does.
"##;

const HARNESS_README_MD: &str = r##"# {{NAME}}

A coding-agent harness you can review: every tool call, every gate result, the diff — and a
person in the loop for the calls you choose.

## What you get

- `POST /api/tasks` queues a task against a repository; a worker clones it and runs Claude over five tools (`list_files`, `read_file`, `edit_file`, `run_gate`, `finish`) — no shell, and no path outside the checkout.
- A policy (`HOLD_TOOLS`, default `edit_file`) that stops the run at a call and waits for `POST /api/approvals/:id`. Denied calls carry a reason back to the agent and end the run; an unanswered call is a denial after 30 minutes, never a silent allow.
- The repository's own check command (`CHECK_COMMAND`, default `make check`) run after every turn, verdict stored per turn.
- A trace: `calls` (tool, input, output, decision, ms), `verdicts` (per turn) and `tasks.diff`, all readable from `GET /api/tasks/:id` and rendered on the page.
- A typed client: the frontend imports `AppType` from the backend; a route change that breaks the page fails `make check`.
- Tests that need no provider, no Docker and no git.

## Five minutes

    make demo
    cd backend && ANTHROPIC_API_KEY=sk-ant-… bun run worker

Then, in another terminal, on a repository you own:

    curl -s -X POST localhost:8000/api/tasks -H 'content-type: application/json' \
      -d '{"repo":"/abs/path/to/repo","task":"Add a README section describing how to run the tests."}'
    # {"id":"3f2c…","state":"queued"}

    curl -s localhost:8000/api/approvals
    # a few seconds later: {"pending":[{"id":"toolu_…","tool":"edit_file","input":{"path":"README.md",…},…}]}

    curl -s -X POST localhost:8000/api/approvals/toolu_… -H 'content-type: application/json' -d '{"allow":true}'
    # {"ok":true}

    curl -s localhost:8000/api/tasks/3f2c… | jq '{state, turns, answer, verdicts: [.verdicts[].ok]}'
    # {"state":"done","turns":4,"answer":"Added a Testing section…","verdicts":[false,true,true,true]}

Open `http://localhost:3000` for the same thing with buttons.

## API

| Method | Path | Auth | What |
|---|---|---|---|
| GET | `/api/health` | none | liveness |
| GET | `/api/health/ready` | none | readiness |
| GET | `/api/policy` | none | `{hold: [tool names]}` |
| POST | `/api/tasks` | none | `{repo, task}` → `201 {id, state}` |
| GET | `/api/tasks` | none | latest 50, without diffs |
| GET | `/api/tasks/:id` | none | task with `calls`, `verdicts`, `diff` |
| GET | `/api/approvals` | none | held calls, oldest first, with the task text |
| POST | `/api/approvals/:id` | none | `{allow, reason?}`; 404 once decided |

There is no authentication. See Production.

## Compared with OpenHands / SWE-agent

**Same shape.** A task against a repository; an agent loop with a small, typed tool set; an edit tool that replaces one exact occurrence (SWE-agent's edit discipline); the trace as the unit of review; a worker separate from the API.

**Better here, specifically.**
- The gate is not optional and not the agent's: the repository's own check runs after every turn, from the harness, and the result sits beside the diff.
- Approvals are a real block: the run waits, and a denial reaches the model as a reason rather than an error it works around. Nothing answered is a denial.
- No shell at all, by construction, and every path jailed — the review is of five tool kinds, not of arbitrary commands.
- One TypeScript codebase you own end to end, with a typed seam to the page and tests that run in under a second without a model.
- kustomize manifests, migrations and secrets handling from the stack scaffold.

**Not here yet.**
- No sandbox for the agent's process: the worker runs on its host, and the gate runs the repository's own `Makefile` there. OpenHands runs the agent in a container per session.
- No browser, no shell tool, no Jupyter, no MCP; five tools.
- One model provider (Anthropic Messages API); no per-task model choice.
- One prompt per task: no follow-up messages, no conversation, no streaming (the page polls).
- No context management: long runs can exceed the model's context.
- No PR creation, no GitHub integration, no SWE-bench evaluation harness, no trajectory export.
- No multi-agent delegation, no microagents / repository-level prompt files.
- No authentication or multi-user scoping.

## Production

- **Environments.** `DATABASE_URL`, `ANTHROPIC_API_KEY` (worker only), `MODEL`, `HOLD_TOOLS`, `CHECK_COMMAND`, `CHECKOUTS`. Dev and prod use separate databases and separate `CHECKOUTS` volumes.
- **Processes.** `backend` (HTTP, stateless, HPA 2–5) and `worker` (one task at a time; scale by count of `queued` tasks). The worker is not in the image yet: add `src/worker.ts` to the Dockerfile's `bun build` line and a `k8s/base/worker.yaml` copied from `backend.yaml` minus the Service, HPA and HTTP probes.
- **Probes.** `/api/health` liveness, `/api/health/ready` readiness. The worker has none; alert on the age of the oldest `queued` task instead.
- **Migrations.** `backend/migrations/0002_harness.sql`; applied by the `migrate` init container before each rollout.
- **Secrets.** `backend/.env` → `.env.age` (committed) → `k8s/secrets.yaml` (rendered, gitignored). The Anthropic key lives there only.
- **Isolation.** The gate runs the submitted repository's check command on the worker host. Only accept repositories you trust, or run each task in a container (the `sandbox` pack's `dockerArgv` is the pattern) before opening this to others.
- **Auth.** None on the API. Front it with ingress auth, or add a bearer check, before it leaves the cluster network.
- **What pages you.** Oldest `queued` task older than N minutes (no worker); any task `running` with no `calls` update for 15 minutes (dead worker); `waiting` count climbing (nobody answering).

## Roadmap

- Lease and heartbeat on running tasks, so a dead worker's task is requeued.
- `LISTEN/NOTIFY` instead of polling for decisions.
- Per-task container for the run and the gate.
- `git diff` that includes new files.
- Follow-up messages on a task; streaming to the page.
- Context compaction for long runs.
- Bearer auth and per-user task scoping.
- Checkout cleanup.
"##;

const HARNESS_AGENT_POLICY: &str = r##"---
name: agent-policy
description: Run on any change to tools.ts, harness.ts, worker.ts or the approvals routes. Checks that the agent still has no shell, cannot leave its checkout, and cannot get past a hold — and that a person's answer means what the page says it means.
tools: Read, Grep, Glob, Bash
---

You review the boundary between the model and the machine. Report only what lets the agent do
something a person did not approve, each as `path:line — what — the input that triggers it — the fix`.

Check:
1. **Tool set.** `makeTools` in `backend/src/tools.ts` still returns exactly `list_files`, `read_file`, `edit_file`, `run_gate`, `finish`. A tool that takes a command string, or `run_gate` accepting arguments, is a shell.
2. **Jail on every path.** Every `path` argument passes through `jail(root, …)` before any `fs` call. `jail` compares `abs === root || abs.startsWith(root + sep)` — a check without `sep` lets `/work2` pass for root `/work`. Symlinks inside the checkout pointing out are not resolved; note it if a change starts following them.
3. **Hold is checked before run.** In `runTask`, `decision === "held"` is decided when the call is recorded, and `tool.run` is only reached after `decision(...)` returns `approved`. A refactor that runs the tool and records afterwards has removed the hold.
4. **Deny stops.** After a denial the function returns `finish("failed", …)`; there is no path that pushes a `tool_result` and continues to the next model turn.
5. **Timeout is a denial.** `decision()`'s fallthrough returns `denied`, never `approved` and never `allowed`.
6. **Decided once.** `POST /api/approvals/:id` refuses (404) unless `call.decision === "held"`. A second answer must not flip an approved call.
7. **The gate is Keel's command.** `shellGate` reads `CHECK_COMMAND` from the environment or the default; nothing from a tool input or a task body reaches `execFile("sh", …)`. The task text is untrusted.
8. **Clone arguments.** `gitClone` keeps `--` before `repo` and the `^(https?://|ssh://|git@…:|/)` check. `ext::` transports, `file://` with options, or a repo string starting with `-` must be refused.
9. **Output caps hold.** Tool output `.slice(0, 20_000)`, `read_file` 100 000, gate output last 8 000, `maxBuffer` 4 MB on `execFile`. A missing cap is a memory exhaustion from one `read_file` of a large binary.
10. **The trace is complete.** `store.addCall` before `tool.run`; `updateCall` in both the success and the catch path; `verdict()` after every turn including the `finish` turn.
11. **Timeouts.** `shellGate` 10 minutes, `gitClone` 5 minutes, `decision` `waitMs`. Any new `execFile` or `fetch` without one is a stuck worker.
12. **Worker failure is recorded.** `tick`'s catch marks the task `failed` with the message; no exception escapes the loop and kills the process silently.
13. **Policy from the environment only.** `HOLD` is built from `HOLD_TOOLS`; a request body cannot add to or remove from it.

Run `cd backend && bun test` and confirm the five tests pass. End with `agent-policy: N findings` and, if 0, which of the above you read.
"##;
