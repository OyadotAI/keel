//! builder: an agent builder platform, like Dify / Flowise / Langflow ═══════════════════════════

pub fn files(name: &str) -> Vec<(&'static str, String)> {
    let f = |s: &str| s.replace("{{NAME}}", name);
    let _ = &f;
    vec![
        ("backend/src/app.ts", BUILDER_APP.into()),
        ("backend/src/store.ts", BUILDER_STORE.into()),
        ("backend/src/runtime.ts", BUILDER_RUNTIME.into()),
        ("backend/src/worker.ts", BUILDER_WORKER.into()),
        ("backend/src/app.test.ts", BUILDER_TEST.into()),
        ("backend/migrations/0002_builder.sql", BUILDER_SQL.into()),
        ("frontend/app/page.tsx", f(BUILDER_PAGE)),
        ("CLAUDE.md", f(BUILDER_CLAUDE_MD)),
        ("AGENTS.md", f(BUILDER_AGENTS_MD)),
        ("README.md", f(BUILDER_README_MD)),
        (
            ".claude/agents/agent-runtime.md",
            BUILDER_AGENT_RUNTIME.into(),
        ),
    ]
}

const BUILDER_SQL: &str = r##"create table if not exists agents (
  id uuid primary key,
  name text not null,
  draft jsonb not null,                    -- the editable definition
  published_version int,                   -- null until first publish
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now()
);
create table if not exists agent_versions (
  agent_id uuid not null references agents(id),
  version int not null,
  definition jsonb not null,               -- immutable copy of the draft at publish time
  created_at timestamptz not null default now(),
  primary key (agent_id, version)
);
create table if not exists triggers (
  id uuid primary key,
  agent_id uuid not null references agents(id),
  kind text not null,                      -- schedule | webhook
  every_seconds int,
  token text unique,
  input text,
  next_at timestamptz,
  created_at timestamptz not null default now()
);
create table if not exists runs (
  id uuid primary key,
  agent_id uuid not null references agents(id),
  version int,                             -- null = ran the draft
  definition jsonb not null,               -- pinned at enqueue
  input text not null,
  state text not null default 'queued',    -- queued | running | done | failed
  output text,
  messages jsonb not null default '[]',
  input_tokens int not null default 0,
  output_tokens int not null default 0,
  source text not null default 'api',      -- api | test | schedule | webhook
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now()
);
create index if not exists runs_queue on runs (state, created_at);
create index if not exists triggers_due on triggers (kind, next_at);
"##;

const BUILDER_RUNTIME: &str = r##"// The runtime, the way Dify runs a published app: the definition is a frozen JSON blob (prompt,
import { lookup } from "node:dns/promises";
// tools, skills, budget), the model is a function, and every message and tool call is appended
// to the run as it happens. Skills are folded into the system prompt (Dify's "instructions"),
// tools are looked up by name in a registry. The budget is what makes a run always end: max
// steps and max output tokens, whichever comes first.

export type Definition = { system_prompt: string; tools: string[]; skills: { name: string; instructions: string }[]; max_steps: number; max_tokens: number };
export type ToolCall = { id: string; name: string; input: Record<string, unknown> };
export type Message =
  | { role: "user" | "assistant"; text: string }
  | { role: "tool_call"; id: string; name: string; input: Record<string, unknown> }
  | { role: "tool_result"; id: string; output: string; error?: boolean };
export type ModelReply = { text: string; calls: ToolCall[]; input_tokens: number; output_tokens: number };
export type ToolDef = { name: string; description: string; input_schema: Record<string, unknown> };
export type Model = (system: string, messages: Message[], tools: ToolDef[]) => Promise<ModelReply>;
export type Fetch = (url: string | URL, init?: RequestInit) => Promise<Response>;
export type Tool = ToolDef & { run: (input: Record<string, unknown>) => Promise<string> };

export const DEFAULT_DEFINITION: Definition = { system_prompt: "You are a helpful assistant.", tools: ["now", "calculator"], skills: [], max_steps: 8, max_tokens: 4000 };

/// The registry. A definition names tools from here; unknown names are refused at draft time.
export function registry(fetchImpl: Fetch = fetch): Record<string, Tool> {
  return {
    now: { name: "now", description: "The current date and time (ISO 8601).", input_schema: { type: "object", properties: {} }, run: async () => new Date().toISOString() },
    calculator: { name: "calculator", description: "Evaluate an arithmetic expression.", input_schema: { type: "object", properties: { expression: { type: "string" } }, required: ["expression"] },
      run: async ({ expression }) => { const e = String(expression); if (!/^[\d\s+\-*/().]+$/.test(e)) throw new Error("only arithmetic"); return String(Function(`"use strict"; return (${e})`)()); } },
    http_get: { name: "http_get", description: "GET a URL and return the first 8000 characters of the body.", input_schema: { type: "object", properties: { url: { type: "string" } }, required: ["url"] },
      run: async ({ url }) => { const r = await fetchPublic(String(url), fetchImpl); return (await r.text()).slice(0, 8000); } },
  };
}

export function systemPrompt(d: Definition): string {
  return d.skills.length ? `${d.system_prompt}\n\n# Skills\n${d.skills.map((s) => `## ${s.name}\n${s.instructions}`).join("\n\n")}` : d.system_prompt;
}

export type RunResult = { state: "done" | "failed"; output: string | null; messages: Message[]; input_tokens: number; output_tokens: number };

export async function execute(d: Definition, input: string, model: Model, opts: { tools?: Record<string, Tool>; onMessage?: (m: Message) => Promise<void> | void } = {}): Promise<RunResult> {
  const reg = opts.tools ?? registry();
  const tools = d.tools.map((n) => reg[n]).filter((t): t is Tool => !!t);
  const defs = tools.map(({ run: _r, ...t }) => t);
  const system = systemPrompt(d);
  const messages: Message[] = [];
  let input_tokens = 0, output_tokens = 0;
  const push = async (m: Message) => { messages.push(m); await opts.onMessage?.(m); };
  await push({ role: "user", text: input });

  for (let step = 1; step <= d.max_steps; step++) {
    const reply = await model(system, messages, defs);
    input_tokens += reply.input_tokens; output_tokens += reply.output_tokens;
    if (reply.text) await push({ role: "assistant", text: reply.text });
    if (!reply.calls.length) return { state: "done", output: reply.text || null, messages, input_tokens, output_tokens };
    for (const c of reply.calls) {
      await push({ role: "tool_call", id: c.id, name: c.name, input: c.input });
      const tool = tools.find((t) => t.name === c.name);
      try {
        if (!tool) throw new Error(`tool ${c.name} is not enabled for this agent`);
        const out = await Promise.race([tool.run(c.input), new Promise<string>((_, rej) => setTimeout(() => rej(new Error("tool timed out")), 30_000).unref())]);
        await push({ role: "tool_result", id: c.id, output: out.slice(0, 8000) });
      } catch (e) {
        // An error is an observation the model can recover from, not the end of the run.
        await push({ role: "tool_result", id: c.id, output: `Error: ${e instanceof Error ? e.message : String(e)}`, error: true });
      }
    }
    if (output_tokens >= d.max_tokens) return { state: "failed", output: `budget: ${output_tokens} output tokens ≥ ${d.max_tokens}`, messages, input_tokens, output_tokens };
  }
  return { state: "failed", output: `budget: ${d.max_steps} steps used without a final answer`, messages, input_tokens, output_tokens };
}

/// Claude via the Messages API. Set ANTHROPIC_API_KEY; MODEL defaults to Sonnet.
export function claude(fetchImpl: Fetch = fetch): Model {
  return async (system, messages, tools) => {
    // Collapse the flat trace into API turns: tool calls ride on the assistant turn, results on the next user turn.
    const turns: { role: "user" | "assistant"; content: unknown[] }[] = [];
    const add = (role: "user" | "assistant", block: unknown) => { const last = turns[turns.length - 1]; if (last?.role === role) last.content.push(block); else turns.push({ role, content: [block] }); };
    for (const m of messages) {
      if (m.role === "tool_call") add("assistant", { type: "tool_use", id: m.id, name: m.name, input: m.input });
      else if (m.role === "tool_result") add("user", { type: "tool_result", tool_use_id: m.id, content: m.output, is_error: m.error ?? false });
      else add(m.role, { type: "text", text: m.text });
    }
    const res = await fetchImpl("https://api.anthropic.com/v1/messages", {
      method: "POST", headers: { "x-api-key": process.env.ANTHROPIC_API_KEY ?? "", "anthropic-version": "2023-06-01", "content-type": "application/json" },
      body: JSON.stringify({ model: process.env.MODEL ?? "claude-sonnet-4-20250514", max_tokens: 2048, system, messages: turns, tools }),
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

const BUILDER_STORE: &str = r##"import { db } from "./db";
import type { Definition, Message } from "./runtime";

export type Agent = { id: string; name: string; draft: Definition; published_version: number | null; created_at: string; updated_at: string };
export type Version = { agent_id: string; version: number; definition: Definition; created_at: string };
export type Trigger = { id: string; agent_id: string; kind: "schedule" | "webhook"; every_seconds: number | null; token: string | null; input: string | null; next_at: string | null; created_at: string };
export type Run = { id: string; agent_id: string; version: number | null; definition: Definition; input: string; state: string; output: string | null; messages: Message[]; input_tokens: number; output_tokens: number; source: string; created_at: string; updated_at: string };

export interface Store {
  createAgent(id: string, name: string, draft: Definition): Promise<Agent>;
  getAgent(id: string): Promise<Agent | null>;
  listAgents(): Promise<Agent[]>;
  saveDraft(id: string, draft: Definition): Promise<void>;
  /// Copy the draft into a new immutable version and point the agent at it. Returns the version number.
  publish(id: string): Promise<number | null>;
  getVersion(id: string, version: number): Promise<Version | null>;
  listVersions(id: string): Promise<Version[]>;
  createTrigger(t: Trigger): Promise<void>;
  listTriggers(agentId: string): Promise<Trigger[]>;
  triggerByToken(token: string): Promise<Trigger | null>;
  /// Schedule triggers due at `now`, each advanced by its interval in the same statement so two
  /// workers cannot both fire it.
  dueTriggers(now: Date): Promise<Trigger[]>;
  enqueue(run: Omit<Run, "state" | "output" | "messages" | "input_tokens" | "output_tokens" | "created_at" | "updated_at">): Promise<void>;
  claim(): Promise<Run | null>;
  update(id: string, patch: Partial<Pick<Run, "state" | "output" | "messages" | "input_tokens" | "output_tokens">>): Promise<void>;
  getRun(id: string): Promise<Run | null>;
  listRuns(agentId: string | null, limit: number): Promise<Run[]>;
}

export function pgStore(): Store {
  return {
    createAgent: async (id, name, draft) => (await db<Agent[]>`insert into agents (id, name, draft) values (${id}, ${name}, ${db.json(draft as never)}) returning *`)[0],
    getAgent: async (id) => (await db<Agent[]>`select * from agents where id = ${id}`)[0] ?? null,
    listAgents: async () => db<Agent[]>`select * from agents order by created_at desc limit 100`,
    saveDraft: async (id, draft) => { await db`update agents set draft = ${db.json(draft as never)}, updated_at = now() where id = ${id}`; },
    publish: async (id) => db.begin(async (tx) => {
      const [a] = await tx<Agent[]>`select * from agents where id = ${id} for update`;
      if (!a) return null;
      const v = (a.published_version ?? 0) + 1;
      await tx`insert into agent_versions (agent_id, version, definition) values (${id}, ${v}, ${tx.json(a.draft as never)})`;
      await tx`update agents set published_version = ${v}, updated_at = now() where id = ${id}`;
      return v;
    }),
    getVersion: async (id, version) => (await db<Version[]>`select * from agent_versions where agent_id = ${id} and version = ${version}`)[0] ?? null,
    listVersions: async (id) => db<Version[]>`select * from agent_versions where agent_id = ${id} order by version desc`,
    createTrigger: async (t) => { await db`insert into triggers ${db(t as never, "id", "agent_id", "kind", "every_seconds", "token", "input", "next_at")}`; },
    listTriggers: async (agentId) => db<Trigger[]>`select * from triggers where agent_id = ${agentId} order by created_at`,
    triggerByToken: async (token) => (await db<Trigger[]>`select * from triggers where token = ${token}`)[0] ?? null,
    dueTriggers: async (now) => db<Trigger[]>`update triggers set next_at = ${now} + make_interval(secs => every_seconds)
      where kind = 'schedule' and next_at <= ${now} returning *`,
    enqueue: async (r) => { await db`insert into runs (id, agent_id, version, definition, input, source) values (${r.id}, ${r.agent_id}, ${r.version}, ${db.json(r.definition as never)}, ${r.input}, ${r.source})`; },
    claim: async () => (await db<Run[]>`update runs set state = 'running', updated_at = now()
      where id = (select id from runs where state = 'queued' order by created_at limit 1 for update skip locked) returning *`)[0] ?? null,
    update: async (id, p) => {
      await db`update runs set state = coalesce(${p.state ?? null}, state), output = coalesce(${p.output ?? null}, output),
        messages = coalesce(${p.messages ? db.json(p.messages as never) : null}, messages), input_tokens = coalesce(${p.input_tokens ?? null}, input_tokens),
        output_tokens = coalesce(${p.output_tokens ?? null}, output_tokens), updated_at = now() where id = ${id}`;
    },
    getRun: async (id) => (await db<Run[]>`select * from runs where id = ${id}`)[0] ?? null,
    listRuns: async (agentId, limit) => agentId
      ? db<Run[]>`select * from runs where agent_id = ${agentId} order by created_at desc limit ${limit}`
      : db<Run[]>`select * from runs order by created_at desc limit ${limit}`,
  };
}

export function memoryStore(): Store {
  const agents = new Map<string, Agent>(); const versions: Version[] = []; const triggers: Trigger[] = []; const runs = new Map<string, Run>();
  const now = () => new Date().toISOString();
  const clone = <T>(x: T): T => JSON.parse(JSON.stringify(x));
  return {
    createAgent: async (id, name, draft) => { const a = { id, name, draft, published_version: null, created_at: now(), updated_at: now() }; agents.set(id, a); return a; },
    getAgent: async (id) => agents.get(id) ?? null,
    listAgents: async () => [...agents.values()].reverse(),
    saveDraft: async (id, draft) => { const a = agents.get(id); if (a) { a.draft = draft; a.updated_at = now(); } },
    publish: async (id) => { const a = agents.get(id); if (!a) return null; const v = (a.published_version ?? 0) + 1; versions.push({ agent_id: id, version: v, definition: clone(a.draft), created_at: now() }); a.published_version = v; return v; },
    getVersion: async (id, version) => versions.find((v) => v.agent_id === id && v.version === version) ?? null,
    listVersions: async (id) => versions.filter((v) => v.agent_id === id).reverse(),
    createTrigger: async (t) => { triggers.push(t); },
    listTriggers: async (agentId) => triggers.filter((t) => t.agent_id === agentId),
    triggerByToken: async (token) => triggers.find((t) => t.token === token) ?? null,
    dueTriggers: async (at) => { const due = triggers.filter((t) => t.kind === "schedule" && t.next_at && new Date(t.next_at) <= at); for (const t of due) t.next_at = new Date(at.getTime() + (t.every_seconds ?? 0) * 1000).toISOString(); return due.map(clone); },
    enqueue: async (r) => { runs.set(r.id, { ...r, state: "queued", output: null, messages: [], input_tokens: 0, output_tokens: 0, created_at: now(), updated_at: now() }); },
    claim: async () => { const r = [...runs.values()].find((r) => r.state === "queued"); if (!r) return null; r.state = "running"; return { ...r }; },
    update: async (id, p) => { const r = runs.get(id); if (r) Object.assign(r, p, { updated_at: now() }); },
    getRun: async (id) => runs.get(id) ?? null,
    listRuns: async (agentId, limit) => [...runs.values()].filter((r) => !agentId || r.agent_id === agentId).reverse().slice(0, limit),
  };
}
"##;

const BUILDER_WORKER: &str = r##"import { pgStore, type Store, type Run } from "./store";
import { execute, claude, type Model } from "./runtime";

// The worker: claim one queued run, execute its pinned definition, record the outcome — plus a
// scheduler tick that turns due schedule triggers into queued runs. Its own process
// (`bun run worker`) and Deployment, so agent runs never sit on the request path.

export async function tick(store: Store, model: Model): Promise<Run | null> {
  const run = await store.claim();
  if (!run) return null;
  try {
    const r = await execute(run.definition, run.input, model, { onMessage: () => {} });
    await store.update(run.id, { state: r.state, output: r.output, messages: r.messages, input_tokens: r.input_tokens, output_tokens: r.output_tokens });
  } catch (e) {
    await store.update(run.id, { state: "failed", output: e instanceof Error ? e.message : String(e) });
  }
  return run;
}

/// Enqueue a run for every schedule trigger that is due. The store advances `next_at` in the
/// same statement it selects with, so a trigger fires once per interval however many workers run.
export async function schedule(store: Store, now = new Date()): Promise<number> {
  let n = 0;
  for (const t of await store.dueTriggers(now)) {
    const a = await store.getAgent(t.agent_id);
    const v = a?.published_version != null ? await store.getVersion(a.id, a.published_version) : null;
    if (!a || !v) continue; // unpublished agents do not run on a schedule
    await store.enqueue({ id: crypto.randomUUID(), agent_id: a.id, version: v.version, definition: v.definition, input: t.input ?? "", source: "schedule" });
    n++;
  }
  return n;
}

if (import.meta.main) {
  const store = pgStore(); const model = claude();
  console.log(JSON.stringify({ level: "info", msg: "worker started" }));
  let stopping = false;
  process.on("SIGTERM", () => { stopping = true; });
  while (!stopping) {
    await schedule(store);
    const did = await tick(store, model);
    if (!did) await new Promise((r) => setTimeout(r, 1000));
  }
}
"##;

const BUILDER_APP: &str = r##"import { Hono } from "hono";
import { z } from "zod";
import { pgStore, type Store } from "./store";
import { execute, registry, claude, DEFAULT_DEFINITION, type Model } from "./runtime";

// Dify's surface: an app has one editable draft and a list of published versions; runs pin the
// version they were enqueued with, so publishing later never changes what a queued run does.
// Runs are enqueued here and executed by worker.ts — except "test", which runs the draft
// inline so the builder page gets an answer without a worker. Triggers are schedules (the
// worker fires them) and webhooks (POST /api/hooks/:token enqueues with the body as input).

const Definition = z.object({
  system_prompt: z.string().min(1).max(20_000),
  tools: z.array(z.string()).max(20).default([]),
  skills: z.array(z.object({ name: z.string().min(1).max(100), instructions: z.string().min(1).max(10_000) })).max(20).default([]),
  max_steps: z.number().int().min(1).max(50).default(8),
  max_tokens: z.number().int().min(1).max(200_000).default(4000),
});

export function createApp(store: Store, model: Model = claude()) {
  const tools = registry();
  const notFound = (c: { json: (b: unknown, s: 404) => Response }) => c.json({ error: { message: "not found", code: "not_found" } }, 404);

  const app = new Hono()
    .get("/api/health", (c) => c.json({ status: "ok" }))
    .get("/api/health/ready", (c) => c.json({ status: "ok", db: "ok" as const }))
    .get("/api/tools", (c) => c.json({ tools: Object.values(tools).map(({ run: _r, ...t }) => t) }))

    .get("/api/agents", async (c) => c.json({ agents: await store.listAgents() }))
    .post("/api/agents", async (c) => {
      const p = z.object({ name: z.string().min(1).max(200), draft: Definition.optional() }).safeParse(await c.req.json().catch(() => ({})));
      if (!p.success) return c.json({ error: { message: "name required", code: "invalid" } }, 400);
      return c.json(await store.createAgent(crypto.randomUUID(), p.data.name, p.data.draft ?? DEFAULT_DEFINITION), 201);
    })
    .get("/api/agents/:id", async (c) => { const a = await store.getAgent(c.req.param("id")); return a ? c.json(a) : notFound(c); })
    .put("/api/agents/:id/draft", async (c) => {
      const a = await store.getAgent(c.req.param("id")); if (!a) return notFound(c);
      const p = Definition.safeParse(await c.req.json().catch(() => ({})));
      if (!p.success) return c.json({ error: { message: p.error.issues[0]?.message ?? "invalid", code: "invalid" } }, 400);
      const unknown = p.data.tools.filter((t) => !tools[t]);
      if (unknown.length) return c.json({ error: { message: `unknown tools: ${unknown.join(", ")}`, code: "invalid" } }, 400);
      await store.saveDraft(a.id, p.data);
      return c.json({ ok: true });
    })
    .post("/api/agents/:id/publish", async (c) => { const v = await store.publish(c.req.param("id")); return v == null ? notFound(c) : c.json({ version: v }, 201); })
    .get("/api/agents/:id/versions", async (c) => c.json({ versions: await store.listVersions(c.req.param("id")) }))

    .post("/api/agents/:id/runs", async (c) => {
      const a = await store.getAgent(c.req.param("id")); if (!a) return notFound(c);
      const p = z.object({ input: z.string().min(1).max(20_000), version: z.union([z.literal("draft"), z.number().int().min(1)]).optional() }).safeParse(await c.req.json().catch(() => ({})));
      if (!p.success) return c.json({ error: { message: "input required", code: "invalid" } }, 400);
      const id = crypto.randomUUID();
      if (p.data.version === "draft") {
        // Test the draft: run inline so the builder page sees the result now, and still store it.
        await store.enqueue({ id, agent_id: a.id, version: null, definition: a.draft, input: p.data.input, source: "test" });
        await store.update(id, { state: "running" });
        const r = await execute(a.draft, p.data.input, model, { tools });
        await store.update(id, { state: r.state, output: r.output, messages: r.messages, input_tokens: r.input_tokens, output_tokens: r.output_tokens });
        return c.json(await store.getRun(id), 201);
      }
      const want = p.data.version ?? a.published_version;
      const v = want == null ? null : await store.getVersion(a.id, want);
      if (!v) return c.json({ error: { message: "agent has no published version", code: "unpublished" } }, 409);
      await store.enqueue({ id, agent_id: a.id, version: v.version, definition: v.definition, input: p.data.input, source: "api" });
      return c.json({ id, state: "queued", version: v.version }, 202);
    })
    .get("/api/agents/:id/runs", async (c) => c.json({ runs: await store.listRuns(c.req.param("id"), 50) }))
    .get("/api/runs", async (c) => c.json({ runs: await store.listRuns(null, 50) }))
    .get("/api/runs/:id", async (c) => { const r = await store.getRun(c.req.param("id")); return r ? c.json(r) : notFound(c); })

    .get("/api/agents/:id/triggers", async (c) => c.json({ triggers: await store.listTriggers(c.req.param("id")) }))
    .post("/api/agents/:id/triggers", async (c) => {
      const a = await store.getAgent(c.req.param("id")); if (!a) return notFound(c);
      const p = z.discriminatedUnion("kind", [
        z.object({ kind: z.literal("schedule"), every_seconds: z.number().int().min(10), input: z.string().max(20_000).default("") }),
        z.object({ kind: z.literal("webhook") }),
      ]).safeParse(await c.req.json().catch(() => ({})));
      if (!p.success) return c.json({ error: { message: "kind must be schedule (with every_seconds) or webhook", code: "invalid" } }, 400);
      const t = p.data.kind === "schedule"
        ? { id: crypto.randomUUID(), agent_id: a.id, kind: "schedule" as const, every_seconds: p.data.every_seconds, token: null, input: p.data.input, next_at: new Date(Date.now() + p.data.every_seconds * 1000).toISOString(), created_at: new Date().toISOString() }
        : { id: crypto.randomUUID(), agent_id: a.id, kind: "webhook" as const, every_seconds: null, token: crypto.randomUUID().replace(/-/g, ""), input: null, next_at: null, created_at: new Date().toISOString() };
      await store.createTrigger(t);
      return c.json(t, 201);
    })
    .post("/api/hooks/:token", async (c) => {
      // The token is the credential: unguessable, one per trigger. The body (any JSON or text) is the run's input.
      const t = await store.triggerByToken(c.req.param("token")); if (!t || t.kind !== "webhook") return notFound(c);
      const a = await store.getAgent(t.agent_id);
      const v = a?.published_version != null ? await store.getVersion(a.id, a.published_version) : null;
      if (!a || !v) return c.json({ error: { message: "agent has no published version", code: "unpublished" } }, 409);
      const input = (await c.req.text()).slice(0, 20_000);
      if (!input) return c.json({ error: { message: "empty body", code: "invalid" } }, 400);
      const id = crypto.randomUUID();
      await store.enqueue({ id, agent_id: a.id, version: v.version, definition: v.definition, input, source: "webhook" });
      return c.json({ id, state: "queued" }, 202);
    });

  return app;
}

const app = createApp(pgStore());
export type AppType = typeof app;
export default app;
"##;

const BUILDER_TEST: &str = r##"import { describe, expect, test } from "bun:test";
// Tools fetch through a fake here, so the public-address check (which does DNS) is bypassed.
process.env.ALLOW_PRIVATE_URLS = "1";
import { createApp } from "./app";
import { memoryStore } from "./store";
import { tick, schedule } from "./worker";
import type { Model, ModelReply } from "./runtime";

// A scripted model: no provider, deterministic replies, so the tests drive the whole platform.
const script = (replies: Partial<ModelReply>[]): Model => { let i = 0; return async () => ({ text: "", calls: [], input_tokens: 10, output_tokens: 5, ...replies[Math.min(i++, replies.length - 1)] }); };
const json = (body: unknown, method = "POST") => ({ method, headers: { "content-type": "application/json" }, body: JSON.stringify(body) });

async function agent(app: ReturnType<typeof createApp>, draft?: unknown) {
  const a = await (await app.request("/api/agents", json({ name: "helper", draft }))).json() as { id: string };
  return a.id;
}

describe("builder", () => {
  test("publish pins a version; editing the draft afterwards does not change it", async () => {
    const store = memoryStore(); const app = createApp(store, script([]));
    const id = await agent(app);
    expect((await app.request(`/api/agents/${id}/runs`, json({ input: "hi" }))).status).toBe(409); // unpublished
    await app.request(`/api/agents/${id}/draft`, json({ system_prompt: "v1 prompt", tools: ["now"] }, "PUT"));
    expect((await (await app.request(`/api/agents/${id}/publish`, { method: "POST" })).json()).version).toBe(1);
    await app.request(`/api/agents/${id}/draft`, json({ system_prompt: "v2 draft", tools: [] }, "PUT"));
    const { versions } = await (await app.request(`/api/agents/${id}/versions`)).json();
    expect(versions[0].definition.system_prompt).toBe("v1 prompt");
    const r = await (await app.request(`/api/agents/${id}/runs`, json({ input: "go" }))).json();
    expect(r.state).toBe("queued");
    expect((await store.getRun(r.id))!.definition.system_prompt).toBe("v1 prompt");
    expect((await app.request(`/api/agents/${id}/draft`, json({ system_prompt: "x", tools: ["shell"] }, "PUT"))).status).toBe(400);
  });

  test("the worker runs the pinned version and records every message and tool call", async () => {
    const store = memoryStore();
    const model = script([
      { text: "Let me compute.", calls: [{ id: "c1", name: "calculator", input: { expression: "6*7" } }] },
      { text: "It is 42." },
    ]);
    const app = createApp(store, model);
    const id = await agent(app, { system_prompt: "math", tools: ["calculator"], skills: [{ name: "brevity", instructions: "be short" }] });
    await app.request(`/api/agents/${id}/publish`, { method: "POST" });
    const { id: runId } = await (await app.request(`/api/agents/${id}/runs`, json({ input: "6*7?" }))).json();
    expect(await tick(store, model)).not.toBeNull();
    const run = await (await app.request(`/api/runs/${runId}`)).json();
    expect(run.state).toBe("done"); expect(run.output).toBe("It is 42."); expect(run.version).toBe(1);
    expect(run.messages.map((m: { role: string }) => m.role)).toEqual(["user", "assistant", "tool_call", "tool_result", "assistant"]);
    expect(run.messages[3].output).toBe("42");
    expect(run.input_tokens).toBe(20);
    expect(await tick(store, model)).toBeNull();
  });

  test("the budget ends a run that never answers", async () => {
    const store = memoryStore();
    const model = script([{ calls: [{ id: "c", name: "now", input: {} }] }]);
    const app = createApp(store, model);
    const id = await agent(app, { system_prompt: "loop", tools: ["now"], max_steps: 3 });
    const run = await (await app.request(`/api/agents/${id}/runs`, json({ input: "forever", version: "draft" }))).json();
    expect(run.state).toBe("failed"); expect(run.output).toContain("3 steps");
    expect(run.messages.filter((m: { role: string }) => m.role === "tool_call").length).toBe(3);
    expect(run.source).toBe("test");
  });

  test("a webhook token enqueues a run with the body; an unknown token is 404", async () => {
    const store = memoryStore(); const app = createApp(store, script([]));
    const id = await agent(app);
    await app.request(`/api/agents/${id}/publish`, { method: "POST" });
    const t = await (await app.request(`/api/agents/${id}/triggers`, json({ kind: "webhook" }))).json();
    expect(t.token).toHaveLength(32);
    const res = await app.request(`/api/hooks/${t.token}`, { method: "POST", body: '{"order":17}' });
    expect(res.status).toBe(202);
    const { id: runId } = await res.json();
    const run = (await store.getRun(runId))!;
    expect(run.input).toBe('{"order":17}'); expect(run.source).toBe("webhook"); expect(run.version).toBe(1);
    expect((await app.request("/api/hooks/nope", { method: "POST", body: "x" })).status).toBe(404);
  });

  test("a schedule fires once when due, then not again until its interval passes", async () => {
    const store = memoryStore(); const app = createApp(store, script([]));
    const id = await agent(app);
    await app.request(`/api/agents/${id}/publish`, { method: "POST" });
    await app.request(`/api/agents/${id}/triggers`, json({ kind: "schedule", every_seconds: 60, input: "daily report" }));
    const t0 = Date.now();
    expect(await schedule(store, new Date(t0))).toBe(0);
    expect(await schedule(store, new Date(t0 + 61_000))).toBe(1);
    expect(await schedule(store, new Date(t0 + 62_000))).toBe(0);
    expect(await schedule(store, new Date(t0 + 122_000))).toBe(1);
    const { runs } = await (await app.request(`/api/agents/${id}/runs`)).json();
    expect(runs.length).toBe(2); expect(runs[0].source).toBe("schedule"); expect(runs[0].input).toBe("daily report");
  });
});
"##;

const BUILDER_PAGE: &str = r##""use client";
import { useEffect, useState } from "react";

type Agent = { id: string; name: string; draft: { system_prompt: string; tools: string[]; max_steps: number }; published_version: number | null };
type Run = { id: string; state: string; input: string; output: string | null; version: number | null; source: string; messages: { role: string; text?: string; name?: string; input?: unknown; output?: string }[]; created_at: string };

// Pick an agent, edit its draft, test it inline, publish it, and watch runs arrive from the
// worker, webhooks and schedules. The list polls; a run's messages are shown as they were stored.
export default function Home() {
  const [agents, setAgents] = useState<Agent[]>([]);
  const [tools, setTools] = useState<string[]>([]);
  const [sel, setSel] = useState<Agent | null>(null);
  const [prompt, setPrompt] = useState(""); const [picked, setPicked] = useState<string[]>([]);
  const [input, setInput] = useState("What time is it, and what is 12*12?");
  const [runs, setRuns] = useState<Run[]>([]); const [open, setOpen] = useState<Run | null>(null);
  const [busy, setBusy] = useState(false);

  const j = async (url: string, body?: unknown, method = body ? "POST" : "GET") => (await fetch(url, { method, headers: { "content-type": "application/json" }, body: body ? JSON.stringify(body) : undefined })).json();
  const load = async () => { const a = (await j("/api/agents")).agents as Agent[]; setAgents(a); if (!sel && a[0]) select(a[0]); };
  const select = (a: Agent) => { setSel(a); setPrompt(a.draft.system_prompt); setPicked(a.draft.tools); };

  useEffect(() => { load(); j("/api/tools").then((r) => setTools(r.tools.map((t: { name: string }) => t.name))); }, []);
  useEffect(() => { if (!sel) return; const f = () => j(`/api/agents/${sel.id}/runs`).then((r) => setRuns(r.runs)); f(); const t = setInterval(f, 2000); return () => clearInterval(t); }, [sel?.id]);

  const create = async () => { const name = window.prompt("Agent name?"); if (name) { select(await j("/api/agents", { name })); load(); } };
  const save = async () => { if (sel) await j(`/api/agents/${sel.id}/draft`, { ...sel.draft, system_prompt: prompt, tools: picked }, "PUT"); };
  const publish = async () => { if (!sel) return; await save(); await j(`/api/agents/${sel.id}/publish`, {}); load(); };
  const testDraft = async () => { if (!sel) return; setBusy(true); await save(); setOpen(await j(`/api/agents/${sel.id}/runs`, { input, version: "draft" })); setBusy(false); };
  const enqueue = async () => { if (sel) { const r = await j(`/api/agents/${sel.id}/runs`, { input }); if (r.error) alert(r.error.message); } };

  return (
    <main style={{ display: "grid", gridTemplateColumns: "180px 1fr 1fr", gap: 16 }}>
      <aside>
        <h1>{{NAME}} — agents</h1>
        <button onClick={create}>+ agent</button>
        <ul>{agents.map((a) => <li key={a.id}><a href="#" onClick={(e) => { e.preventDefault(); select(a); }}>{a.name}</a> {a.published_version ? `v${a.published_version}` : "draft"}</li>)}</ul>
      </aside>
      <section>
        {sel ? <>
          <h2>{sel.name} <small>{sel.published_version ? `published v${sel.published_version}` : "never published"}</small></h2>
          <textarea rows={8} style={{ width: "100%" }} value={prompt} onChange={(e) => setPrompt(e.target.value)} />
          <p>{tools.map((t) => <label key={t} style={{ marginRight: 12 }}><input type="checkbox" checked={picked.includes(t)} onChange={(e) => setPicked(e.target.checked ? [...picked, t] : picked.filter((x) => x !== t))} /> {t}</label>)}</p>
          <p><button onClick={save}>Save draft</button> <button onClick={publish}>Publish</button></p>
          <textarea rows={2} style={{ width: "100%" }} value={input} onChange={(e) => setInput(e.target.value)} />
          <p><button onClick={testDraft} disabled={busy}>{busy ? "Testing…" : "Test draft"}</button> <button onClick={enqueue}>Run published (worker)</button></p>
          <p>Trigger from anywhere: <code>{`curl -X POST http://localhost:8000/api/agents/${sel.id}/runs -H 'content-type: application/json' -d '{"input":"hello"}'`}</code> — then <code>bun run worker</code> in backend/.</p>
        </> : <p>Create an agent to start. Set <code>ANTHROPIC_API_KEY</code> in backend/.env.</p>}
      </section>
      <section>
        <h2>Runs</h2>
        <table><tbody>{runs.map((r) => <tr key={r.id} onClick={() => setOpen(r)} style={{ cursor: "pointer" }}><td>{r.state}</td><td>{r.version ? `v${r.version}` : "draft"}</td><td>{r.source}</td><td>{r.input.slice(0, 40)}</td></tr>)}</tbody></table>
        {open && <details open>
          <summary>{open.state} · {open.output?.slice(0, 80)}</summary>
          {open.messages.map((m, i) => <pre key={i}>{m.role}: {m.text ?? (m.name ? `${m.name}(${JSON.stringify(m.input)})` : m.output)}</pre>)}
        </details>}
      </section>
    </main>
  );
}
"##;

const BUILDER_CLAUDE_MD: &str = r##"# {{NAME}} — working agreement

{{NAME}} is an agent-builder platform in the shape of Dify and Flowise: an agent is a JSON
definition (system prompt, enabled tools, skills, a budget) with one editable draft and a list of
immutable published versions; runs pin the version they were enqueued with and record every
message and tool call; triggers turn schedules and webhooks into runs. "Done" means a `runs` row in
`done` or `failed` with its full `messages` trace and token counts, executed by the worker from a
definition that publishing later cannot change.

## Architecture

| File | Owns |
|---|---|
| `backend/src/app.ts` | The routes: agents, draft/publish/versions, runs (inline draft test or queued), triggers, webhooks. The `Definition` zod schema. Exports `AppType`. |
| `backend/src/store.ts` | `Store` interface; `pgStore` (`publish` in a transaction with `for update`, `dueTriggers` as one `update … returning`, `SKIP LOCKED` claim) and `memoryStore`. |
| `backend/src/runtime.ts` | `execute`: the step loop with the budget; the tool `registry` (`now`, `calculator`, `http_get`); `systemPrompt` (skills folded in); `claude()`; `publicUrl`/`fetchPublic` SSRF guard. |
| `backend/src/worker.ts` | `tick` (claim and execute one run) and `schedule` (due triggers → queued runs). The `bun run worker` process. |
| `backend/src/app.test.ts` | Five tests over `memoryStore` and a scripted model. |
| `backend/migrations/0002_builder.sql` | `agents`, `agent_versions`, `triggers`, `runs`. |
| `frontend/app/page.tsx` | Agent list, draft editor with tool checkboxes, test-inline, publish, runs with their traces. |

### Request path

1. `POST /api/agents {name, draft?}` creates an agent with `DEFAULT_DEFINITION` unless a draft is given; `published_version` is null.
2. `PUT /api/agents/:id/draft` validates against `Definition` and refuses tool names not in the registry (`400 unknown tools: …`).
3. `POST /api/agents/:id/publish` copies the draft into `agent_versions (agent_id, version)` and sets `published_version` — in one transaction, row locked.
4. `POST /api/agents/:id/runs {input}` enqueues with the **published** definition pinned into `runs.definition` (`202`); `{version: "draft"}` executes the draft inline on the request and returns the finished run (`201`); `{version: n}` pins that version. No published version is `409 unpublished`.
5. `worker.ts tick()` claims a queued run and calls `execute(run.definition, run.input, model)`.
6. `execute` loops up to `max_steps`: model reply → append assistant text → for each call, look the tool up among the definition's enabled tools, run it with a 30 s race, append `tool_result` (an error is a result with `error: true`, not the end of the run) → stop when a reply has no calls, or `output_tokens ≥ max_tokens`.
7. The worker writes `state`, `output`, `messages`, `input_tokens`, `output_tokens` once, at the end.
8. Triggers: `schedule()` runs every worker loop; `dueTriggers(now)` advances `next_at` in the same statement it selects with, so each interval fires once across replicas. `POST /api/hooks/:token` enqueues a run whose input is the raw request body.

### Data model

| Table | Column | Why |
|---|---|---|
| `agents` | `draft jsonb` | The only editable definition. |
| | `published_version` | Null until first publish; what `runs` and hooks use by default. |
| `agent_versions` | `(agent_id, version)` pk, `definition jsonb` | Immutable. Never updated, never deleted. |
| `runs` | `definition jsonb` | Pinned at enqueue; the worker reads this, never the agent. |
| | `version` | Null means the draft was run (`source = test`). |
| | `messages jsonb` | The full trace: user, assistant, tool_call, tool_result. |
| | `input_tokens`, `output_tokens` | Summed across steps; the cost. |
| | `source` | `api \| test \| schedule \| webhook`. |
| `triggers` | `kind`, `every_seconds`, `next_at` | Fixed-interval schedules. |
| | `token unique` | The webhook credential: 32 hex chars, one per trigger. |
| `runs_queue`, `triggers_due` | indexes | The claim and the scheduler scan. |

## Invariants

1. **Publishing pins; editing the draft afterwards changes nothing published.** `agent_versions` is insert-only and `runs.definition` is copied at enqueue. Guarded by `publish pins a version; editing the draft afterwards does not change it`.
2. **A run executes the definition it was enqueued with**, not the agent's current one. The worker never reads `agents`. Guarded by `the worker runs the pinned version and records every message and tool call` (`run.version` is 1 and the prompt is v1's).
3. **Unknown tools are refused at draft time.** `PUT …/draft` with `tools: ["shell"]` is `400`. Same test as 1. At run time a call to a tool not enabled is a `tool_result` error, not a crash.
4. **Every run ends.** `max_steps` and `max_tokens`, whichever first, fail the run with a `budget:` message. Guarded by `the budget ends a run that never answers`.
5. **The trace is complete and ordered.** `messages` roles come out `user, assistant, tool_call, tool_result, assistant` for one tool round. Guarded by test 2.
6. **A tool error is an observation.** `execute` pushes `{role: "tool_result", error: true}` and continues; the model gets to recover. Enforced by the `try/catch` in `execute`; a test that asserts a failing tool leads to `done` belongs in `app.test.ts` when you touch this.
7. **A webhook's token is its only credential, and it is unguessable.** 32 hex chars from `randomUUID`; unknown is `404`; the body is the input verbatim, capped at 20 000. Guarded by `a webhook token enqueues a run with the body; an unknown token is 404`.
8. **A schedule fires once per interval however many workers run.** `dueTriggers` is one `update … where next_at <= now returning`. Guarded by `a schedule fires once when due, then not again until its interval passes`.
9. **Unpublished agents do not run from triggers or the API.** `409 unpublished` on runs and hooks; `schedule()` skips them. Tests 1 and 5.
10. **Agent-chosen URLs are public only.** `http_get` goes through `fetchPublic`: DNS first, then loopback / RFC1918 / link-local (cloud metadata) / ULA / CGNAT refused, redirects walked by hand with the same check per hop. Tests set `ALLOW_PRIVATE_URLS=1` to fetch through a fake; that variable must never be set in a deployment.
11. **`calculator` is not `eval`.** The expression is allow-listed to `[\d\s+\-*/().]` before `Function` sees it. Extend the regex, never remove it.

## Extending it

**Add a tool.** One entry in `registry()` in `runtime.ts`: `name`, `description`, `input_schema`, `run`. If it takes a URL, fetch through `fetchPublic`. `GET /api/tools` and the page's checkboxes pick it up. Test: enable it in a draft, script a model that calls it, assert the `tool_result`. No migration.

**Add a definition field** (temperature, a model name). Add it to `Definition` in `app.ts` and to the `Definition` type in `runtime.ts`; read it in `execute` or `claude()`. Old versions in `agent_versions` lack it — read with a default, never migrate published JSON.

**Add a trigger kind** (cron, a queue). Extend the discriminated union in `POST …/triggers`, add columns by migration, and give `Store.dueTriggers` the new due rule — keeping it a single statement that advances and returns.

**Add a model provider.** Another `Model` beside `claude()`; the flat `Message[]` trace is provider-neutral and `claude()` shows how to collapse it into API turns.

**Persist the trace as it happens.** `execute` takes `onMessage`; the worker passes a no-op. Have it call `store.update(id, {messages})` (or append to a `run_messages` table by migration) so a run that dies mid-way keeps what it had.

**Chat / multi-turn.** A run is one input. For a conversation, add a `threads` table and seed `execute`'s `messages` from the thread's prior turns — and decide how the budget applies to a thread rather than a run.

**Auth and tenancy.** There is none. Add a principal at the boundary, an `owner` column on `agents`, and scope every `store` method by it; the `tenant` pack has the shape.

## Operating it

| Variable | Required | Meaning |
|---|---|---|
| `DATABASE_URL` | yes | Postgres; API and worker. |
| `ANTHROPIC_API_KEY` | yes | Both processes: the API runs draft tests inline. |
| `MODEL` | no | Defaults to `claude-sonnet-4-20250514`; `max_tokens` 2048 per step. |
| `ALLOW_PRIVATE_URLS` | never in prod | Disables the SSRF guard; tests only. |
| `PORT` | no | API port, default 8000. |

**Processes.** `backend` serves HTTP and executes draft tests inline (a request can last `max_steps` × model latency). `worker` runs queued runs one at a time and the scheduler tick every loop; scale it on queue depth. Not in the image yet: add `src/worker.ts` to the Dockerfile's build line and a `k8s/base/worker.yaml` with no Service or HTTP probes.

**What is per-replica.** Nothing. Queue, triggers and traces are Postgres; the `registry` is code.

**Failure modes.**

| What | User sees |
|---|---|
| Model 401/429/529 | Run `failed`, output `anthropic <status>: …`. Not retried. |
| Tool throws or exceeds 30 s | A `tool_result` with `error: true`; the run continues. |
| Budget hit | `failed`, output `budget: …`; the trace up to that point is kept. |
| Worker dies mid-run | Run stuck `running`, trace empty (it is written at the end). |
| Webhook to an unpublished agent | `409 unpublished`. |
| No worker | Runs stay `queued`; schedules do not fire (the worker is the scheduler). |

**Metrics and logs.** `runs.input_tokens + output_tokens` per agent per day is the bill. Queue age. `failed` grouped by the first word of `output` (`budget:` vs `anthropic`). Trigger drift: `next_at` in the past by more than one interval means no worker ran.

## Ceilings

- **Draft tests run on the request path.** Fine for a person clicking; not for a load test. Queue them with `source = test` and poll if that changes.
- **The trace is written once, at the end.** A crash loses it. `onMessage` is the hook.
- **Schedules are fixed intervals**, `every_seconds ≥ 10`, no cron, no timezone, no "at 09:00".
- **No lease on running runs**; a dead worker orphans them.
- **Three tools**, in code. A tool marketplace or per-agent HTTP tools means a `tools` table and a generic HTTP tool built on `fetchPublic`.
- **No memory across runs**, no RAG, no conversation threads.
- **No authentication on any route except the webhook token.** Anyone reaching `:8000` can publish and run agents on your key.
- **One provider**, no streaming.

The stack rules — gate, typed seam, production checklist, deploy — are in `docs/PRODUCTION.md`. They apply.
"##;

const BUILDER_AGENTS_MD: &str = r##"# {{NAME}} — for agents

See `CLAUDE.md` for the rules. This is how to run it.

## Run

    make demo                                        # postgres + redis, migrate, seed, API :8000, page :3000
    make check                                       # typecheck both halves, bun test the backend — no key needed
    cd backend && ANTHROPIC_API_KEY=… bun run worker  # executes queued runs and fires schedules

The API needs `ANTHROPIC_API_KEY` too, for draft tests (`version: "draft"`), which run inline.

## Routes

Create, edit, publish:

    curl -s -X POST localhost:8000/api/agents -H 'content-type: application/json' -d '{"name":"helper"}'
    # 201 {"id":"a1…","name":"helper","draft":{"system_prompt":"You are a helpful assistant.","tools":["now","calculator"],"skills":[],"max_steps":8,"max_tokens":4000},"published_version":null,…}

    curl -s -X PUT localhost:8000/api/agents/a1…/draft -H 'content-type: application/json' \
      -d '{"system_prompt":"Answer in one line.","tools":["now","calculator","http_get"],"skills":[{"name":"brevity","instructions":"Never exceed 20 words."}],"max_steps":6,"max_tokens":2000}'
    # 200 {"ok":true}          — 400 {"error":{"message":"unknown tools: shell"}} for a tool not in the registry

    curl -s -X POST localhost:8000/api/agents/a1…/publish        # 201 {"version":1}
    curl -s localhost:8000/api/agents/a1…/versions               # {"versions":[{"agent_id","version":1,"definition":{…},"created_at"}]}
    curl -s localhost:8000/api/tools                             # {"tools":[{"name":"now",…},{"name":"calculator",…},{"name":"http_get",…}]}

Run:

    curl -s -X POST localhost:8000/api/agents/a1…/runs -H 'content-type: application/json' -d '{"input":"What is 12*12?"}'
    # 202 {"id":"r1…","state":"queued","version":1}    — the worker picks it up; 409 unpublished if never published

    curl -s -X POST localhost:8000/api/agents/a1…/runs -H 'content-type: application/json' -d '{"input":"What is 12*12?","version":"draft"}'
    # 201 {…,"state":"done","output":"144.","version":null,"source":"test","messages":[{"role":"user","text":"What is 12*12?"},{"role":"assistant","text":"Let me compute."},{"role":"tool_call","id":"toolu_…","name":"calculator","input":{"expression":"12*12"}},{"role":"tool_result","id":"toolu_…","output":"144"},{"role":"assistant","text":"144."}],"input_tokens":…,"output_tokens":…}

    curl -s localhost:8000/api/runs/r1…                          # the run, with messages once the worker is done
    curl -s localhost:8000/api/agents/a1…/runs                   # {"runs":[…]} latest 50 for the agent
    curl -s localhost:8000/api/runs                              # all agents

Triggers:

    curl -s -X POST localhost:8000/api/agents/a1…/triggers -H 'content-type: application/json' -d '{"kind":"schedule","every_seconds":3600,"input":"Daily report"}'
    # 201 {"id":"t1…","kind":"schedule","every_seconds":3600,"input":"Daily report","next_at":"…",…}
    curl -s -X POST localhost:8000/api/agents/a1…/triggers -H 'content-type: application/json' -d '{"kind":"webhook"}'
    # 201 {"id":"t2…","kind":"webhook","token":"8f3a…(32 hex)",…}
    curl -s -X POST localhost:8000/api/hooks/8f3a… -d '{"order":17}'
    # 202 {"id":"r2…","state":"queued"}   — the body, as text, is the run's input; 404 for an unknown token
    curl -s localhost:8000/api/agents/a1…/triggers

Errors are `{"error":{"message","code"}}`: `400 invalid`, `404 not_found`, `409 unpublished`.

## Tests

`backend/src/app.test.ts`, `bun test`:

- **`memoryStore()`** replaces Postgres, including `publish` and `dueTriggers` semantics.
- **`script([...replies])`** is the model: each call returns the next reply, repeating the last. `createApp(store, model)` and `tick(store, model)` take it, so no provider is called.
- **`schedule(store, now)`** takes the clock; the interval test hands it `t0 + 61_000` and so on.
- **`ALLOW_PRIVATE_URLS=1`** is set at the top of the test file because tools fetch through fakes; it bypasses the DNS check and nothing else.

To add a test: `createApp(memoryStore(), script([...]))`, `agent(app, draft)`, publish, run, `tick`, then assert on `GET /api/runs/:id` — `state`, `output`, `messages` roles, tokens. Assert on the stored run, not on what the model was sent.
"##;

const BUILDER_README_MD: &str = r##"# {{NAME}}

Build agents from a prompt, tools and skills; publish immutable versions; run them from an API,
a webhook or a schedule; read every step they took.

## What you get

- Agents as JSON definitions: `system_prompt`, `tools` (from a registry), `skills` (name + instructions, folded into the prompt), `max_steps`, `max_tokens`.
- A draft you edit and versions you publish. Runs pin the version; publishing later never changes a queued run.
- `POST /api/agents/:id/runs` queues on the published version; `{version: "draft"}` tests inline and returns the trace now.
- Triggers: fixed-interval schedules fired by the worker, exactly once per interval across replicas; webhooks at `POST /api/hooks/:token` whose body is the input.
- Every run stored with its messages — user, assistant, tool_call, tool_result — and token counts.
- Three tools: `now`, `calculator` (allow-listed arithmetic), `http_get` (public addresses only; loopback, private ranges and cloud metadata refused after DNS, redirects re-checked).
- A budget that ends every run. Tests that need no model.

## Five minutes

    make demo
    cd backend && ANTHROPIC_API_KEY=sk-ant-… bun run worker

    A=$(curl -s -X POST localhost:8000/api/agents -H 'content-type: application/json' -d '{"name":"helper"}' | jq -r .id)
    curl -s -X POST localhost:8000/api/agents/$A/runs -H 'content-type: application/json' \
      -d '{"input":"What time is it, and what is 12*12?","version":"draft"}' | jq '{state, output, steps: [.messages[].role]}'
    # {"state":"done","output":"It is 14:03 UTC and 12*12 is 144.","steps":["user","tool_call","tool_result","tool_call","tool_result","assistant"]}

    curl -s -X POST localhost:8000/api/agents/$A/publish          # {"version":1}
    T=$(curl -s -X POST localhost:8000/api/agents/$A/triggers -H 'content-type: application/json' -d '{"kind":"webhook"}' | jq -r .token)
    curl -s -X POST localhost:8000/api/hooks/$T -d 'Summarise: order 17 shipped late.'
    # {"id":"r2…","state":"queued"}  — the worker runs it; GET /api/runs/r2… shows the trace

Open `http://localhost:3000` to edit, test and publish with buttons.

## API

| Method | Path | Auth | What |
|---|---|---|---|
| GET | `/api/health`, `/api/health/ready` | none | probes |
| GET | `/api/tools` | none | the registry |
| GET / POST | `/api/agents` | none | list / create `{name, draft?}` |
| GET | `/api/agents/:id` | none | agent with draft |
| PUT | `/api/agents/:id/draft` | none | replace the draft; unknown tools 400 |
| POST | `/api/agents/:id/publish` | none | `201 {version}` |
| GET | `/api/agents/:id/versions` | none | newest first |
| POST | `/api/agents/:id/runs` | none | `{input, version?}` → `202` queued, or `201` finished for `"draft"`; `409` unpublished |
| GET | `/api/agents/:id/runs`, `/api/runs`, `/api/runs/:id` | none | traces |
| GET / POST | `/api/agents/:id/triggers` | none | `{kind: "schedule", every_seconds, input}` or `{kind: "webhook"}` |
| POST | `/api/hooks/:token` | the token | body → run input, `202` |

## Compared with Dify / Flowise

**Same shape.** An app/agent with a system prompt, tools and instructions; draft vs published, with runs pinned to a version; a run log with every message and tool call; API and webhook triggers; token accounting per run.

**Better here, specifically.**
- Version pinning is a database fact (`runs.definition` copied at enqueue, `agent_versions` insert-only) with a test, not a UI convention.
- The budget is two numbers on the definition and a test that a looping agent stops.
- SSRF protection on agent-chosen URLs: DNS-then-check, private and metadata ranges refused, manual redirects. Most builders hand the model a bare `fetch`.
- Schedules fire exactly once per interval across any number of workers, by one SQL statement — no leader election, no Redis lock.
- Typed end to end, tests without a model or a database, and the stack's manifests, migrations and secrets handling. One codebase you can read in an afternoon.

**Not here yet.**
- No visual workflow / graph editor — Dify's node canvas and Flowise's flows. A definition here is prompt + tools + skills, not a DAG (the `graph` and `dag` packs are that).
- No knowledge base / RAG, no document upload, no embeddings.
- No chat sessions with history; a run is one input.
- No multi-provider model catalogue; one Anthropic model from the environment.
- No streaming responses; the page polls.
- No per-app API keys, users, workspaces or tenancy; no rate limits.
- Cron schedules, timezones, and a tool marketplace — none. Three tools, in code.
- No prompt variables / templating, no annotations, no evaluation datasets (see the `evals` pack).

## Production

- **Environments.** `DATABASE_URL`, `ANTHROPIC_API_KEY` (both processes), `MODEL`. Never set `ALLOW_PRIVATE_URLS` outside tests. Separate databases per environment.
- **Processes.** `backend` (HTTP + inline draft tests; HPA) and `worker` (runs + scheduler; scale on `queued` age). Add `src/worker.ts` to the Dockerfile's build line and a `k8s/base/worker.yaml` without Service or HTTP probes. Two or more workers are safe.
- **Probes.** `/api/health`, `/api/health/ready`. Worker: alert on oldest `queued` run and on any schedule with `next_at` more than one interval in the past.
- **Migrations.** `0002_builder.sql`; the migrate init container runs it before each rollout. `runs.messages` grows with use; partition or prune by `created_at` when it matters.
- **Secrets.** The Anthropic key through `.env.age` → `k8s/secrets.yaml`. Webhook tokens are in the database; treat `triggers` as sensitive.
- **Auth.** None on the management routes. Put them behind ingress auth before anything but `/api/hooks/*` is reachable from outside.
- **What pages you.** `queued` older than 60 s; `failed` rate by agent; token spend per hour above budget; a schedule that has not advanced.

## Roadmap

- Trace persisted per message, so a dead worker keeps what it had.
- Lease and requeue for running runs.
- Cron schedules with a timezone.
- A `tools` table and a generic HTTP tool, so tools are data.
- Threads (multi-turn) and streaming.
- API keys, owners, and per-agent scoping.
- Queued draft tests.
"##;

const BUILDER_AGENT_RUNTIME: &str = r##"---
name: agent-runtime
description: Run on any change to runtime.ts, worker.ts, store.ts publish/dueTriggers, or the runs and hooks routes. Checks that published versions stay immutable, every run ends, tools cannot reach the private network, and a schedule cannot fire twice.
tools: Read, Grep, Glob, Bash
---

You review the runtime as the person who will be billed for it and paged by it. Report only what
runs the wrong definition, runs forever, reaches something private, or fires twice — each as
`path:line — what — the input that triggers it — the fix`.

Check:
1. **Pinning.** `POST …/runs` copies `v.definition` (or `a.draft` for `"draft"`) into `enqueue`; `tick` executes `run.definition` and never calls `store.getAgent`. `schedule()` and `/api/hooks/:token` enqueue the published version's definition, not the draft.
2. **Versions are immutable.** No `update` or `delete` on `agent_versions` anywhere (`grep agent_versions backend/src`). `publish` uses `for update` on the agent row so two publishes cannot both become version N.
3. **Budget.** `execute` checks `output_tokens >= d.max_tokens` after every step and exits the `for` loop at `max_steps`; both return `failed` with a `budget:` prefix. A change that `continue`s past either is an unbounded bill.
4. **Tool timeout.** The `Promise.race` with 30 s stays, and the timer is `unref()`d so it does not hold the process.
5. **Tool errors do not end the run**, and unknown tool names produce an error result rather than throwing out of `execute`.
6. **SSRF guard.** Every tool that fetches uses `fetchPublic`; `publicUrl` resolves DNS before checking; `isPrivate` covers 127/8, 10/8, 172.16/12, 192.168/16, 169.254/16, 100.64/10, 0/8, `::1`, `fc00::/7`, `fe80::/10`, and `::ffff:` mapped v4; redirects use `redirect: "manual"` and re-check each hop. `ALLOW_PRIVATE_URLS` is read only in `publicUrl`, and no manifest or `.env.example` sets it.
7. **`calculator`.** The regex allow-list runs before `Function`; no letters, no backticks, no `[`.
8. **Draft validation.** `PUT …/draft` refuses tool names outside `registry()`; `Definition` bounds prompt size (20 000), skills (20), steps (50), tokens (200 000).
9. **Schedule once.** `pgStore.dueTriggers` is one statement: `update … set next_at = now + interval where next_at <= now returning *`. A select-then-update lets two workers fire the same trigger. `memoryStore` mirrors it.
10. **Webhook.** `triggerByToken` looks up by the whole token; the route checks `t.kind === "webhook"`; input capped at 20 000 and empty refused; unpublished is 409 not a draft run.
11. **Worker loop.** `tick`'s catch marks `failed`; `schedule()` errors do not kill the loop (wrap it if a change makes it throw).
12. **Inline draft runs** still write a `runs` row (`source: test`) before executing, so a crash mid-test leaves evidence.
13. **The trace.** `messages` roles alternate sensibly and `claude()` collapses `tool_call`/`tool_result` into `assistant`/`user` turns — a `tool_result` that lands on the assistant turn is a 400 from the API.

Run `cd backend && bun test`. End with `agent-runtime: N findings` and, if 0, which of the above you read.
"##;
