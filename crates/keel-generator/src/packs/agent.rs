//! agent: an AI agent backend with streaming tool use, like Open WebUI / LibreChat ════════════

pub fn files(name: &str) -> Vec<(&'static str, String)> {
    let f = |s: &str| s.replace("{{NAME}}", name);
    vec![
        ("backend/src/app.ts", AGENT_APP.into()),
        ("backend/src/store.ts", AGENT_STORE.into()),
        ("backend/src/agent.ts", AGENT_AGENT.into()),
        ("backend/src/app.test.ts", AGENT_TEST.into()),
        ("backend/migrations/0002_agent.sql", AGENT_SQL.into()),
        ("frontend/app/page.tsx", f(AGENT_PAGE)),
        ("CLAUDE.md", f(AGENT_CLAUDE_MD)),
        ("AGENTS.md", f(AGENT_AGENTS_MD)),
        ("README.md", f(AGENT_README_MD)),
        (".claude/agents/tool-safety.md", AGENT_REVIEWER.into()),
    ]
}

const AGENT_APP: &str = r##"import { Hono } from "hono";
import { streamSSE } from "hono/streaming";
import { z } from "zod";
import { pgStore, type Store } from "./store";
import { runTurn, toApiMessages, claude, TOOLS, DEFAULT_SYSTEM, type Model } from "./agent";

// Open WebUI's surface, reduced: POST /api/chat/completions streams one assistant turn as SSE
// (delta / tool_call / tool_result / done), conversations and their messages are listed
// separately, and the system prompt is a setting. The caller is the `x-user` header — put the
// session's user id there from the auth layer in front of this.
//
// The budget is checked before the model is called and charged after, per user per UTC day.
// It lives in the store so every replica sees the same number.

const DAILY_TOKEN_BUDGET = Number(process.env.DAILY_TOKEN_BUDGET ?? 200_000);
const today = () => new Date().toISOString().slice(0, 10);

export function createApp(store: Store, model: Model = claude()) {
  const user = (c: { req: { header(n: string): string | undefined } }) => c.req.header("x-user") ?? "anonymous";
  const app = new Hono()
    .get("/api/health", (c) => c.json({ status: "ok" }))
    .get("/api/health/ready", (c) => c.json({ status: "ok", db: "ok" as const }))
    .get("/api/tools", (c) => c.json({ tools: TOOLS.map((t) => ({ name: t.name, description: t.description, timeout_ms: t.timeout_ms })) }))

    .get("/api/settings/system_prompt", async (c) => c.json({ value: (await store.getSetting("system_prompt")) ?? DEFAULT_SYSTEM }))
    .put("/api/settings/system_prompt", async (c) => {
      const p = z.object({ value: z.string().min(1).max(20_000) }).safeParse(await c.req.json().catch(() => ({})));
      if (!p.success) return c.json({ error: { message: "value required", code: "invalid" } }, 400);
      await store.setSetting("system_prompt", p.data.value); return c.json({ ok: true });
    })

    .get("/api/budget", async (c) => { const used = await store.spent(user(c), today()); return c.json({ used, limit: DAILY_TOKEN_BUDGET, day: today() }); })

    .post("/api/chat/completions", async (c) => {
      const p = z.object({ conversation_id: z.string().uuid().optional(), message: z.string().min(1).max(50_000) }).safeParse(await c.req.json().catch(() => ({})));
      if (!p.success) return c.json({ error: { message: "message required", code: "invalid" } }, 400);
      const u = user(c);
      const used = await store.spent(u, today());
      if (used >= DAILY_TOKEN_BUDGET) return c.json({ error: { message: `daily budget of ${DAILY_TOKEN_BUDGET} tokens spent`, code: "budget" } }, 429);

      let conv = p.data.conversation_id ? await store.getConversation(p.data.conversation_id) : null;
      if (p.data.conversation_id && (!conv || conv.user_id !== u)) return c.json({ error: { message: "no such conversation", code: "not_found" } }, 404);
      if (!conv) { conv = { id: crypto.randomUUID(), user_id: u, title: p.data.message.slice(0, 80), created_at: new Date().toISOString() }; await store.createConversation(conv); }
      const id = conv.id;
      const history = toApiMessages(await store.getMessages(id));
      const userMsg = { role: "user" as const, content: [{ type: "text", text: p.data.message }] };
      await store.addMessage({ conversation_id: id, role: "user", content: userMsg.content, input_tokens: 0, output_tokens: 0 });
      const system = (await store.getSetting("system_prompt")) ?? DEFAULT_SYSTEM;

      return streamSSE(c, async (s) => {
        const ctl = new AbortController(); s.onAbort(() => ctl.abort());
        await s.writeSSE({ event: "conversation", data: JSON.stringify({ id }) });
        try {
          const turn = await runTurn([...history, userMsg], model, { store, user: u, system, signal: ctl.signal }, (e) => s.writeSSE({ event: e.type, data: JSON.stringify(e) }));
          await store.addMessage({ conversation_id: id, role: "assistant", content: turn.content, input_tokens: turn.input_tokens, output_tokens: turn.output_tokens });
          await store.spend(u, today(), turn.input_tokens + turn.output_tokens);
          await s.writeSSE({ event: "done", data: JSON.stringify({ input_tokens: turn.input_tokens, output_tokens: turn.output_tokens, rounds: turn.rounds }) });
        } catch (e) {
          await s.writeSSE({ event: "error", data: JSON.stringify({ message: e instanceof Error ? e.message : String(e) }) });
        }
      });
    })
    .get("/api/conversations", async (c) => c.json({ conversations: await store.listConversations(user(c), 50) }))
    .get("/api/conversations/:id", async (c) => {
      const conv = await store.getConversation(c.req.param("id"));
      if (!conv || conv.user_id !== user(c)) return c.json({ error: { message: "no such conversation", code: "not_found" } }, 404);
      return c.json({ ...conv, messages: await store.getMessages(conv.id) });
    });
  return app;
}

const app = createApp(pgStore());
export type AppType = typeof app;
export default app;
"##;

const AGENT_STORE: &str = r##"import { db } from "./db";

export type Conversation = { id: string; user_id: string; title: string; created_at: string };
export type Message = { id?: number; conversation_id: string; role: "user" | "assistant"; content: unknown; input_tokens: number; output_tokens: number; created_at?: string };
export type Note = { id: number; user_id: string; body: string; created_at: string };

export interface Store {
  createConversation(c: Conversation): Promise<void>;
  listConversations(user: string, limit: number): Promise<(Conversation & { messages: number })[]>;
  getConversation(id: string): Promise<Conversation | null>;
  addMessage(m: Message): Promise<void>;
  getMessages(conversationId: string): Promise<Message[]>;
  getSetting(key: string): Promise<string | null>;
  setSetting(key: string, value: string): Promise<void>;
  addNote(user: string, body: string): Promise<Note>;
  listNotes(user: string, query?: string): Promise<Note[]>;
  /// Tokens this user has spent today (UTC).
  spent(user: string, day: string): Promise<number>;
  spend(user: string, day: string, tokens: number): Promise<void>;
}

export function pgStore(): Store {
  return {
    createConversation: async (c) => { await db`insert into conversations (id, user_id, title) values (${c.id}, ${c.user_id}, ${c.title})`; },
    listConversations: async (user, limit) => db`select c.*, (select count(*)::int from messages m where m.conversation_id = c.id) as messages from conversations c where user_id = ${user} order by created_at desc limit ${limit}`,
    getConversation: async (id) => (await db<Conversation[]>`select * from conversations where id = ${id}`)[0] ?? null,
    addMessage: async (m) => { await db`insert into messages (conversation_id, role, content, input_tokens, output_tokens) values (${m.conversation_id}, ${m.role}, ${db.json(m.content as never)}, ${m.input_tokens}, ${m.output_tokens})`; },
    getMessages: async (id) => db<Message[]>`select * from messages where conversation_id = ${id} order by id`,
    getSetting: async (key) => (await db<{ value: string }[]>`select value from settings where key = ${key}`)[0]?.value ?? null,
    setSetting: async (key, value) => { await db`insert into settings (key, value) values (${key}, ${value}) on conflict (key) do update set value = excluded.value, updated_at = now()`; },
    addNote: async (user, body) => (await db<Note[]>`insert into user_notes (user_id, body) values (${user}, ${body}) returning *`)[0],
    listNotes: async (user, q) => db<Note[]>`select * from user_notes where user_id = ${user} and (${q ?? null}::text is null or body ilike ${"%" + (q ?? "") + "%"}) order by id desc limit 20`,
    spent: async (user, day) => (await db<{ tokens: number }[]>`select tokens from budgets where user_id = ${user} and day = ${day}`)[0]?.tokens ?? 0,
    spend: async (user, day, tokens) => { await db`insert into budgets (user_id, day, tokens) values (${user}, ${day}, ${tokens}) on conflict (user_id, day) do update set tokens = budgets.tokens + excluded.tokens`; },
  };
}

export function memoryStore(): Store {
  const convs = new Map<string, Conversation>(); const msgs: Message[] = []; const settings = new Map<string, string>();
  const notes: Note[] = []; const budgets = new Map<string, number>();
  return {
    createConversation: async (c) => { convs.set(c.id, c); },
    listConversations: async (user, limit) => [...convs.values()].filter((c) => c.user_id === user).reverse().slice(0, limit).map((c) => ({ ...c, messages: msgs.filter((m) => m.conversation_id === c.id).length })),
    getConversation: async (id) => convs.get(id) ?? null,
    addMessage: async (m) => { msgs.push({ ...m, id: msgs.length + 1, created_at: new Date().toISOString() }); },
    getMessages: async (id) => msgs.filter((m) => m.conversation_id === id),
    getSetting: async (key) => settings.get(key) ?? null,
    setSetting: async (key, value) => { settings.set(key, value); },
    addNote: async (user, body) => { const n = { id: notes.length + 1, user_id: user, body, created_at: new Date().toISOString() }; notes.push(n); return n; },
    listNotes: async (user, q) => notes.filter((n) => n.user_id === user && (!q || n.body.toLowerCase().includes(q.toLowerCase()))).reverse().slice(0, 20),
    spent: async (user, day) => budgets.get(`${user}:${day}`) ?? 0,
    spend: async (user, day, tokens) => { budgets.set(`${user}:${day}`, (budgets.get(`${user}:${day}`) ?? 0) + tokens); },
  };
}
"##;

const AGENT_AGENT: &str = r##"// One assistant turn, streamed: the model speaks, may call tools, the results go back, and it
import { lookup } from "node:dns/promises";
// speaks again — until it stops calling tools. The model is a function so tests script it and
// never touch a provider; `claude()` below is the real one, streaming the Messages API.
import type { Store } from "./store";

export type ToolCall = { id: string; name: string; input: Record<string, unknown> };
export type ModelReply = { text: string; calls: ToolCall[]; input_tokens: number; output_tokens: number; stop: "tool_use" | "end_turn" | "max_tokens" };
export type Message = { role: "user" | "assistant"; content: unknown };
export type ToolSchema = { name: string; description: string; input_schema: Record<string, unknown> };
/// Streaming model: text arrives through onDelta as it is generated; the reply is the whole turn.
export type Model = (req: { system: string; messages: Message[]; tools: ToolSchema[]; signal?: AbortSignal }, onDelta: (text: string) => void) => Promise<ModelReply>;

/// The registry entry: schema the model sees, a timeout, and the function. Tools get the store
/// and the calling user, so a tool can never read another user's notes.
export type Tool = ToolSchema & { timeout_ms: number; run: (input: Record<string, unknown>, ctx: { store: Store; user: string; fetch: Fetch }) => Promise<string> };
type Fetch = (url: string | URL, init?: RequestInit) => Promise<Response>;

export const TOOLS: Tool[] = [
  { name: "web_fetch", description: "Fetch a public http(s) URL and return its text content (tags stripped, truncated).", timeout_ms: 10_000,
    input_schema: { type: "object", properties: { url: { type: "string" } }, required: ["url"] },
    run: async ({ url }, { fetch }) => {
      const r = await fetchPublic(String(url), fetch, { headers: { "user-agent": "agent-pack/1.0" } });
      if (!r.ok) throw new Error(`fetch ${r.status}`);
      const html = await r.text();
      return html.replace(/<script[\s\S]*?<\/script>|<style[\s\S]*?<\/style>/gi, "").replace(/<[^>]+>/g, " ").replace(/\s+/g, " ").trim().slice(0, 8000);
    } },
  { name: "notes", description: "Save or search the user's notes. action=add with body, or action=search with an optional query.", timeout_ms: 2_000,
    input_schema: { type: "object", properties: { action: { type: "string", enum: ["add", "search"] }, body: { type: "string" }, query: { type: "string" } }, required: ["action"] },
    run: async ({ action, body, query }, { store, user }) => {
      if (action === "add") { const n = await store.addNote(user, String(body ?? "")); return `saved note #${n.id}`; }
      const ns = await store.listNotes(user, query ? String(query) : undefined);
      return ns.length ? ns.map((n) => `#${n.id} ${n.body}`).join("\n") : "no notes";
    } },
];

export const DEFAULT_SYSTEM = "You are a helpful assistant. Use web_fetch to read pages the user links, and notes to remember things they ask you to keep. Be concise.";

export type TurnEvent = { type: "delta"; text: string } | { type: "tool_call"; call: ToolCall } | { type: "tool_result"; id: string; output: string; error: boolean };
export type Turn = { content: unknown[]; text: string; input_tokens: number; output_tokens: number; rounds: number };

export async function runTurn(history: Message[], model: Model, ctx: { store: Store; user: string; system: string; tools?: Tool[]; fetch?: Fetch; signal?: AbortSignal; maxRounds?: number }, emit: (e: TurnEvent) => void | Promise<void>): Promise<Turn> {
  const tools = ctx.tools ?? TOOLS; const fetchImpl = ctx.fetch ?? fetch;
  const messages = [...history]; const content: unknown[] = []; let text = "", input_tokens = 0, output_tokens = 0, rounds = 0;
  for (; rounds < (ctx.maxRounds ?? 8); ) {
    rounds++;
    const reply = await model({ system: ctx.system, messages, tools: tools.map(({ name, description, input_schema }) => ({ name, description, input_schema })), signal: ctx.signal }, (t) => { text += t; void emit({ type: "delta", text: t }); });
    input_tokens += reply.input_tokens; output_tokens += reply.output_tokens;
    const blocks: unknown[] = reply.text ? [{ type: "text", text: reply.text }] : [];
    for (const c of reply.calls) blocks.push({ type: "tool_use", id: c.id, name: c.name, input: c.input });
    content.push(...blocks); messages.push({ role: "assistant", content: blocks });
    if (!reply.calls.length) break;
    const results: unknown[] = [];
    for (const call of reply.calls) {
      await emit({ type: "tool_call", call });
      const tool = tools.find((t) => t.name === call.name);
      let output: string, error = false;
      try {
        if (!tool) throw new Error(`unknown tool ${call.name}`);
        // Each tool has its own deadline; a hung fetch must not hold the whole turn.
        let timer: ReturnType<typeof setTimeout> | undefined;
        output = await Promise.race([tool.run(call.input, { store: ctx.store, user: ctx.user, fetch: fetchImpl }), new Promise<string>((_, rej) => { timer = setTimeout(() => rej(new Error(`${tool.name} timed out after ${tool.timeout_ms}ms`)), tool.timeout_ms); })]).finally(() => clearTimeout(timer));
      } catch (e) { output = `Error: ${e instanceof Error ? e.message : String(e)}`; error = true; }
      await emit({ type: "tool_result", id: call.id, output, error });
      results.push({ type: "tool_result", tool_use_id: call.id, content: output, is_error: error });
    }
    // Tool results live in the transcript as a user message, exactly as the API wants them back.
    content.push({ type: "__tool_results", blocks: results }); messages.push({ role: "user", content: results });
  }
  return { content, text, input_tokens, output_tokens, rounds };
}

/// Stored assistant content -> API messages. Tool results were saved inside the assistant row
/// (marked `__tool_results`) so a conversation is one row per visible turn; they unfold here.
export function toApiMessages(stored: { role: "user" | "assistant"; content: unknown }[]): Message[] {
  const out: Message[] = [];
  for (const m of stored) {
    if (m.role === "user") { out.push({ role: "user", content: m.content }); continue; }
    let cur: unknown[] = [];
    for (const b of m.content as { type: string; blocks?: unknown }[]) {
      if (b.type === "__tool_results") { out.push({ role: "assistant", content: cur }); out.push({ role: "user", content: b.blocks }); cur = []; } else cur.push(b);
    }
    if (cur.length) out.push({ role: "assistant", content: cur });
  }
  return out;
}

/// Claude via the streaming Messages API. ANTHROPIC_API_KEY from the environment, never the repo.
export function claude(fetchImpl: Fetch = fetch): Model {
  return async ({ system, messages, tools, signal }, onDelta) => {
    const res = await fetchImpl("https://api.anthropic.com/v1/messages", {
      method: "POST", signal,
      headers: { "x-api-key": process.env.ANTHROPIC_API_KEY ?? "", "anthropic-version": "2023-06-01", "content-type": "application/json" },
      body: JSON.stringify({ model: process.env.MODEL ?? "claude-opus-5", max_tokens: 8192, stream: true, system, messages, tools }),
    });
    if (!res.ok || !res.body) throw new Error(`anthropic ${res.status}: ${(await res.text()).slice(0, 300)}`);
    const reply: ModelReply = { text: "", calls: [], input_tokens: 0, output_tokens: 0, stop: "end_turn" };
    const open = new Map<number, { call: ToolCall; json: string }>();
    for await (const ev of sse(res.body)) {
      const d = ev as { type: string; index?: number; message?: { usage: { input_tokens: number } }; content_block?: { type: string; id?: string; name?: string }; delta?: { type?: string; text?: string; partial_json?: string; stop_reason?: string }; usage?: { output_tokens: number } };
      if (d.type === "message_start") reply.input_tokens = d.message!.usage.input_tokens;
      else if (d.type === "content_block_start" && d.content_block?.type === "tool_use") open.set(d.index!, { call: { id: d.content_block.id!, name: d.content_block.name!, input: {} }, json: "" });
      else if (d.type === "content_block_delta") {
        if (d.delta?.type === "text_delta") { reply.text += d.delta.text ?? ""; onDelta(d.delta.text ?? ""); }
        else if (d.delta?.type === "input_json_delta") open.get(d.index!)!.json += d.delta.partial_json ?? "";
      } else if (d.type === "content_block_stop" && open.has(d.index!)) { const o = open.get(d.index!)!; o.call.input = o.json ? JSON.parse(o.json) : {}; reply.calls.push(o.call); open.delete(d.index!); }
      else if (d.type === "message_delta") { reply.output_tokens = d.usage?.output_tokens ?? 0; reply.stop = d.delta?.stop_reason === "tool_use" ? "tool_use" : d.delta?.stop_reason === "max_tokens" ? "max_tokens" : "end_turn"; }
    }
    return reply;
  };
}

async function* sse(body: ReadableStream<Uint8Array>): AsyncGenerator<unknown> {
  const reader = body.getReader(); const dec = new TextDecoder(); let buf = "";
  for (;;) {
    const { value, done } = await reader.read(); if (done) break;
    buf += dec.decode(value, { stream: true });
    let i; while ((i = buf.indexOf("\n\n")) >= 0) {
      const block = buf.slice(0, i); buf = buf.slice(i + 2);
      const data = block.split("\n").filter((l) => l.startsWith("data:")).map((l) => l.slice(5).trim()).join("");
      if (data) yield JSON.parse(data);
    }
  }
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

const AGENT_TEST: &str = r##"import { describe, expect, test } from "bun:test";
// Tools fetch through a fake here, so the public-address check (which does DNS) is bypassed.
process.env.ALLOW_PRIVATE_URLS = "1";
import { createApp } from "./app";
import { memoryStore } from "./store";
import { runTurn, toApiMessages, TOOLS, type Model, type ModelReply } from "./agent";

// A scripted model: each call returns the next reply and streams its text as one delta.
const script = (replies: Partial<ModelReply>[]): Model => { let i = 0; return async (_req, onDelta) => { const r = { text: "", calls: [], input_tokens: 10, output_tokens: 5, stop: "end_turn" as const, ...replies[Math.min(i++, replies.length - 1)] }; if (r.text) onDelta(r.text); return r; }; };
const post = (app: ReturnType<typeof createApp>, body: unknown, user = "u1") => app.request("/api/chat/completions", { method: "POST", headers: { "content-type": "application/json", "x-user": user }, body: JSON.stringify(body) });
const events = (text: string) => text.split("\n\n").filter(Boolean).map((b) => ({ event: /event: (\w+)/.exec(b)?.[1], data: JSON.parse(/data: (.*)/.exec(b)?.[1] ?? "null") }));

describe("agent", () => {
  test("a turn streams deltas, stores both messages with token counts, and charges the budget", async () => {
    const store = memoryStore();
    const app = createApp(store, script([{ text: "hello there" }]));
    const res = await post(app, { message: "hi" });
    expect(res.status).toBe(200);
    const ev = events(await res.text());
    expect(ev.find((e) => e.event === "delta")?.data.text).toBe("hello there");
    const id = ev[0].data.id;
    const conv = await (await app.request(`/api/conversations/${id}`, { headers: { "x-user": "u1" } })).json();
    expect(conv.messages.length).toBe(2);
    expect(conv.messages[1].output_tokens).toBe(5);
    expect((await (await app.request("/api/budget", { headers: { "x-user": "u1" } })).json()).used).toBe(15);
    // Another user cannot read it.
    expect((await app.request(`/api/conversations/${id}`, { headers: { "x-user": "u2" } })).status).toBe(404);
  });

  test("the notes tool writes to the store and the result goes back to the model", async () => {
    const store = memoryStore(); const seen: unknown[] = [];
    const model: Model = async (req, onDelta) => { seen.push([...req.messages]); if (req.messages.length === 1) return { text: "", calls: [{ id: "t1", name: "notes", input: { action: "add", body: "buy milk" } }], input_tokens: 1, output_tokens: 1, stop: "tool_use" }; onDelta("saved"); return { text: "saved", calls: [], input_tokens: 1, output_tokens: 1, stop: "end_turn" }; };
    const turn = await runTurn([{ role: "user", content: "remember: buy milk" }], model, { store, user: "u1", system: "s" }, () => {});
    expect((await store.listNotes("u1"))[0].body).toBe("buy milk");
    const last = (seen[1] as { role: string; content: { type: string; content: string }[] }[]).at(-1)!;
    expect(last.role).toBe("user"); expect(last.content[0].content).toBe("saved note #1");
    expect(turn.rounds).toBe(2); expect(turn.input_tokens).toBe(2);
    // The stored shape round-trips to API messages: assistant(tool_use) / user(tool_result) / assistant(text).
    const api = toApiMessages([{ role: "assistant", content: turn.content }]);
    expect(api.map((m) => m.role)).toEqual(["assistant", "user", "assistant"]);
  });

  test("a tool that exceeds its timeout becomes an error result, not a hung turn", async () => {
    const slow = { ...TOOLS.find((t) => t.name === "web_fetch")!, timeout_ms: 20 };
    const never = () => new Promise<Response>(() => {});
    const model = script([{ calls: [{ id: "t1", name: "web_fetch", input: { url: "https://example.com" } }], stop: "tool_use" }, { text: "gave up" }]);
    const results: string[] = [];
    await runTurn([{ role: "user", content: "read it" }], model, { store: memoryStore(), user: "u", system: "s", tools: [slow], fetch: never }, (e) => { if (e.type === "tool_result") results.push(e.output); });
    expect(results[0]).toContain("timed out");
    // web_fetch also refuses non-http schemes, so the model cannot reach file:// or localhost sockets by scheme.
    await expect(slow.run({ url: "file:///etc/passwd" }, { store: memoryStore(), user: "u", fetch: never })).rejects.toThrow("http(s) only");
  });

  test("the daily budget refuses the call before the model is reached", async () => {
    const store = memoryStore(); let calls = 0;
    const app = createApp(store, async () => { calls++; return { text: "x", calls: [], input_tokens: 1, output_tokens: 1, stop: "end_turn" }; });
    await store.spend("u1", new Date().toISOString().slice(0, 10), 999_999_999);
    const res = await post(app, { message: "hi" });
    expect(res.status).toBe(429); expect((await res.json()).error.code).toBe("budget"); expect(calls).toBe(0);
    expect((await post(app, { message: "hi" }, "u2")).status).toBe(200);
  });

  test("the system prompt is editable and the model receives the edited one", async () => {
    let system = "";
    const app = createApp(memoryStore(), async (req) => { system = req.system; return { text: "ok", calls: [], input_tokens: 1, output_tokens: 1, stop: "end_turn" }; });
    expect((await app.request("/api/settings/system_prompt", { method: "PUT", headers: { "content-type": "application/json" }, body: JSON.stringify({ value: "Answer in French." }) })).status).toBe(200);
    await (await post(app, { message: "hi" })).text();
    expect(system).toBe("Answer in French.");
    expect((await app.request("/api/settings/system_prompt", { method: "PUT", headers: { "content-type": "application/json" }, body: "{}" })).status).toBe(400);
  });
});
"##;

const AGENT_SQL: &str = r##"create table if not exists conversations (
  id uuid primary key,
  user_id text not null,
  title text not null,
  created_at timestamptz not null default now()
);
create index if not exists conversations_user on conversations (user_id, created_at desc);
create table if not exists messages (
  id bigserial primary key,
  conversation_id uuid not null references conversations(id),
  role text not null,                      -- user | assistant
  content jsonb not null,                  -- Anthropic content blocks
  input_tokens int not null default 0,
  output_tokens int not null default 0,
  created_at timestamptz not null default now()
);
create index if not exists messages_conv on messages (conversation_id, id);
create table if not exists settings (
  key text primary key,
  value text not null,
  updated_at timestamptz not null default now()
);
create table if not exists user_notes (
  id bigserial primary key,
  user_id text not null,
  body text not null,
  created_at timestamptz not null default now()
);
-- Tokens spent per user per UTC day. One row per (user, day); the budget check reads it before
-- every model call. Replicas share it through the database; move to Redis INCR if the read
-- becomes the hot path.
create table if not exists budgets (
  user_id text not null,
  day date not null,
  tokens int not null default 0,
  primary key (user_id, day)
);
"##;

const AGENT_PAGE: &str = r##""use client";
import { useEffect, useState } from "react";

type Line = { kind: "user" | "assistant" | "tool"; text: string };

// A chat that streams: text arrives as deltas, tool calls show as they run. The system prompt is
// a setting on the backend; edit it here and the next turn uses it.
export default function Home() {
  const [conversation, setConversation] = useState<string | null>(null);
  const [lines, setLines] = useState<Line[]>([]);
  const [input, setInput] = useState("Save a note that the release is Friday, then tell me what notes I have.");
  const [busy, setBusy] = useState(false);
  const [system, setSystem] = useState("");
  const [budget, setBudget] = useState<{ used: number; limit: number } | null>(null);

  const refresh = () => { fetch("/api/budget").then((r) => r.json()).then(setBudget).catch(() => {}); };
  useEffect(() => { fetch("/api/settings/system_prompt").then((r) => r.json()).then((s) => setSystem(s.value)).catch(() => {}); refresh(); }, []);

  async function send() {
    setBusy(true);
    setLines((l) => [...l, { kind: "user", text: input }, { kind: "assistant", text: "" }]);
    const res = await fetch("/api/chat/completions", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ message: input, conversation_id: conversation ?? undefined }) });
    setInput("");
    if (!res.ok) { const e = await res.json(); setLines((l) => [...l, { kind: "tool", text: "Error: " + e.error.message }]); setBusy(false); return; }
    const reader = res.body!.getReader(); const dec = new TextDecoder(); let buf = "";
    for (;;) {
      const { value, done } = await reader.read(); if (done) break;
      buf += dec.decode(value, { stream: true });
      let i; while ((i = buf.indexOf("\n\n")) >= 0) {
        const block = buf.slice(0, i); buf = buf.slice(i + 2);
        const ev = /event: (\w+)/.exec(block)?.[1]; const data = /data: (.*)/.exec(block)?.[1];
        if (!ev || !data) continue;
        const d = JSON.parse(data);
        if (ev === "conversation") setConversation(d.id);
        if (ev === "delta") setLines((l) => { const last = l[l.length - 1]; return last.kind === "assistant" ? [...l.slice(0, -1), { ...last, text: last.text + d.text }] : [...l, { kind: "assistant", text: d.text }]; });
        if (ev === "tool_call") setLines((l) => [...l, { kind: "tool", text: `${d.call.name}(${JSON.stringify(d.call.input)})` }, { kind: "assistant", text: "" }]);
        if (ev === "tool_result") setLines((l) => [...l.slice(0, -1), { kind: "tool", text: "→ " + d.output.slice(0, 300) }, { kind: "assistant", text: "" }]);
        if (ev === "error") setLines((l) => [...l, { kind: "tool", text: "Error: " + d.message }]);
      }
    }
    setBusy(false); refresh();
  }

  return (
    <main>
      <h1>{{NAME}} — agent</h1>
      <p>Streams Claude with a <code>web_fetch</code> and a <code>notes</code> tool. Set <code>ANTHROPIC_API_KEY</code> in backend/.env.{budget && <> Budget today: {budget.used} / {budget.limit} tokens.</>}</p>
      <pre>{`curl -N localhost:8000/api/chat/completions -H 'content-type: application/json' -H 'x-user: me' -d '{"message":"fetch https://example.com and summarise it"}'`}</pre>
      <div style={{ border: "1px solid #ccc", padding: 8, minHeight: 200 }}>
        {lines.filter((l) => l.text).map((l, i) => <p key={i} style={{ whiteSpace: "pre-wrap", fontFamily: l.kind === "tool" ? "monospace" : undefined, opacity: l.kind === "tool" ? 0.7 : 1 }}>{l.kind === "user" ? "You: " : ""}{l.text}</p>)}
      </div>
      <textarea value={input} onChange={(e) => setInput(e.target.value)} rows={2} style={{ width: "100%" }} />
      <p><button onClick={send} disabled={busy || !input}>{busy ? "…" : "Send"}</button> <button onClick={() => { setConversation(null); setLines([]); }}>New conversation</button></p>
      <details>
        <summary>System prompt</summary>
        <textarea value={system} onChange={(e) => setSystem(e.target.value)} rows={4} style={{ width: "100%" }} />
        <p><button onClick={() => fetch("/api/settings/system_prompt", { method: "PUT", headers: { "content-type": "application/json" }, body: JSON.stringify({ value: system }) })}>Save</button></p>
      </details>
    </main>
  );
}
"##;

const AGENT_CLAUDE_MD: &str = r##"# {{NAME}} — agent

This service is a chat backend with streaming tool use, modelled on Open WebUI's
`/api/chat/completions` reduced to what a team can own: one assistant turn per request, streamed
as Server-Sent Events, with the model allowed to call two tools (`web_fetch`, `notes`) and speak
again until it stops. Conversations and messages are rows; the system prompt is a setting; every
user has a daily token budget kept in the database. "Done" here means: a turn streams end to end
through `POST /api/chat/completions`, both messages are stored with their token counts, the
budget is charged, and `bun test` proves it against a scripted model without touching a provider.

## Architecture

| File | Owns |
|---|---|
| `backend/src/app.ts` | The routes, one chained expression; `x-user` extraction; the budget check before the model and the charge after; the SSE framing of a turn |
| `backend/src/agent.ts` | `runTurn` (the tool loop), `TOOLS` (registry: schema, timeout, `run`), `claude()` (streaming Messages API client), `toApiMessages` (stored rows to API messages), `publicUrl`/`fetchPublic` (the SSRF guard) |
| `backend/src/store.ts` | The `Store` interface, `pgStore` (Postgres) and `memoryStore` (tests) — conversations, messages, settings, notes, budgets |
| `backend/src/app.test.ts` | Five tests, all against `memoryStore` and a scripted `Model` |
| `backend/src/server.ts` | Listens on `PORT`, drains on SIGTERM (from the stack) |
| `backend/src/db.ts` | The one `postgres` pool from `DATABASE_URL` (from the stack) |
| `backend/migrations/0002_agent.sql` | `conversations`, `messages`, `settings`, `user_notes`, `budgets` |
| `frontend/app/page.tsx` | A streaming chat page that parses the SSE events and edits the system prompt |

### The request path

1. `POST /api/chat/completions` with `{message, conversation_id?}`; the caller is the `x-user` header (`anonymous` when absent — put a real id there from the auth layer in front).
2. The body is validated with zod (`message` 1–50,000 chars, `conversation_id` a UUID or absent); anything else is `400 invalid`.
3. `store.spent(user, today)` is read; at or over `DAILY_TOKEN_BUDGET` the call is refused with `429 budget` before any model call.
4. The conversation is loaded and ownership checked (`404` if it belongs to another user), or created with the first 80 chars of the message as its title.
5. The stored history is unfolded by `toApiMessages`, the user message is appended and stored, and the system prompt is read from `settings` (falling back to `DEFAULT_SYSTEM`).
6. The response switches to SSE. First event: `conversation {id}`. Then `runTurn` streams `delta` events as text arrives, `tool_call` before each tool runs, `tool_result` after, up to `maxRounds` (8) model calls.
7. The assistant turn — text blocks, `tool_use` blocks and a `__tool_results` marker per round — is stored as one `messages` row with the turn's total input and output tokens; `store.spend` adds them to today's budget row.
8. Final event: `done {input_tokens, output_tokens, rounds}`. A thrown error becomes an `error {message}` event; the HTTP status is already 200 by then.

### Data model

| Table | Column that matters | Why |
|---|---|---|
| `conversations` | `user_id` | Every read checks `conv.user_id === user`; the index `(user_id, created_at desc)` serves the list |
| `messages` | `content jsonb` | Anthropic content blocks, verbatim, so history goes back to the API unchanged |
| `messages` | `input_tokens`, `output_tokens` | Per turn, from the stream's own `usage`; what the budget charges |
| `settings` | `key` primary key | `system_prompt` today; `on conflict do update` makes `PUT` idempotent |
| `user_notes` | `user_id` | The `notes` tool receives the calling user and can only read that user's rows |
| `budgets` | `(user_id, day)` primary key | One row per user per UTC day; `spend` is an upsert that adds, so replicas share one number |

## Invariants

1. **Nothing reaches the model over budget.** The check is at the top of the handler, before the conversation is even loaded, and returns `429` with `code: "budget"`. Guarded by `app.test.ts` "the daily budget refuses the call before the model is reached" (asserts the model function was called zero times).
2. **A conversation belongs to one user.** Both `GET /api/conversations/:id` and `POST` with a `conversation_id` compare `conv.user_id` to the caller and return `404`, not `403`, so existence is not leaked. Guarded by "a turn streams deltas…" (u2 gets 404 on u1's conversation).
3. **A tool gets the calling user, never a user id from the model.** `Tool.run` receives `{store, user}` from `runTurn`'s context; the `notes` tool has no `user` parameter in its schema. Guarded by "the notes tool writes to the store…" (`store.listNotes("u1")`).
4. **Every tool has its own deadline.** `runTurn` races `tool.run` against `timeout_ms`; a timeout becomes a `tool_result` with `is_error: true` and the turn continues. Guarded by "a tool that exceeds its timeout becomes an error result, not a hung turn".
5. **`web_fetch` only reaches public addresses.** `publicUrl` refuses non-http(s) schemes, resolves the host and rejects loopback, RFC 1918, link-local (cloud metadata), CGNAT and ULA; `fetchPublic` walks redirects by hand and re-checks each hop. `ALLOW_PRIVATE_URLS=1` is for tests only. Guarded by the `file:///etc/passwd` assertion in the timeout test; the address classes are `isPrivate`.
6. **The stored transcript round-trips to the API shape.** Tool results are stored inside the assistant row under `__tool_results`, and `toApiMessages` unfolds them into `assistant(tool_use) / user(tool_result) / assistant(text)`. Guarded by "the notes tool…" (`api.map(m => m.role)`).
7. **The model is a function.** `createApp(store, model)` and `runTurn(history, model, …)` take a `Model`; `claude()` is the default, never a hard-wired import. Tests script it. Every test in `app.test.ts` depends on this.
8. **Token counts come from the stream, not from an estimate.** `claude()` reads `message_start.usage.input_tokens` and `message_delta.usage.output_tokens`; `runTurn` sums them across rounds. Guarded by "a turn streams deltas…" (`output_tokens` is 5, budget used is 15).
9. **The system prompt is read per turn, not cached.** A `PUT` takes effect on the next request on every replica. Guarded by "the system prompt is editable and the model receives the edited one".
10. **A turn is bounded.** `maxRounds` (8) caps model calls per request; the model cannot loop forever on tool calls. Not separately tested — `runTurn`'s `for` loop is the guard; a test that scripts nine `tool_use` replies and asserts `rounds === 8` is the one to add if you touch it.

## Extending it

**Add a tool.** Append to `TOOLS` in `agent.ts`: `name`, `description`, `input_schema` (JSON Schema the model sees), `timeout_ms`, and `run(input, {store, user, fetch})`. If it needs storage, add the method to `Store` and both implementations, and a migration `backend/migrations/0003_<name>.sql`. Test: script a model that calls it once, run `runTurn` with `memoryStore`, assert the store and the `tool_result` content. Any outbound HTTP goes through `fetchPublic`, never `fetch` directly.

**Add a setting.** Read it with `store.getSetting("<key>")` where it is used and add a `GET`/`PUT /api/settings/<key>` pair to the chain in `app.ts` with a zod bound. No migration: `settings` is key/value. Test: `PUT` then assert the model saw it, as the system-prompt test does.

**Add a per-user budget.** Today `DAILY_TOKEN_BUDGET` is one number for everyone. Add a `limit` column to `budgets` (or a `user_limits` table) with a migration, read it beside `spent` in the handler, and return it from `/api/budget`. Test: two users, two limits, one refused.

**Add a route.** Add it to the chain in `app.ts` — never `app.get(...)` on a separate line, or `AppType` stops carrying it and `frontend/lib/api.ts` degrades to `any`. Validate the body with zod, return `{error: {message, code}}` on failure.

**Change the model.** `MODEL` in the environment. To change provider, write another `Model` in `agent.ts` that streams deltas through `onDelta` and returns `ModelReply`; the loop, the tools and the store do not change.

**Add conversation deletion or rename.** Add `deleteConversation`/`renameConversation` to `Store` and both implementations, then a `DELETE`/`PATCH /api/conversations/:id` with the same ownership check as `GET`. Test: another user's delete is `404` and the row survives.

**Truncate history.** `toApiMessages` returns everything; long conversations will hit the context window. Insert a window (last N turns, or a summary row) between `getMessages` and `runTurn` in the handler. Test: 50 stored turns, assert the model receives fewer.

## Operating it

| Env var | Required | Meaning |
|---|---|---|
| `ANTHROPIC_API_KEY` | yes | Sent as `x-api-key`; never in the repository (`backend/.env`, committed as `.env.age`) |
| `MODEL` | no | Model id, default `claude-opus-5` |
| `DAILY_TOKEN_BUDGET` | no | Tokens per user per UTC day, default 200,000 |
| `DATABASE_URL` | yes | The pool in `db.ts`; compose service locally, Secret in the cluster |
| `PORT` | no | Default 8000 |
| `ALLOW_PRIVATE_URLS` | never in production | `1` disables the SSRF guard; the test file sets it |

**Scaling.** The API is stateless: the budget, settings and history are all in Postgres, so any replica can serve any turn. Per-replica today: nothing that matters. Move the budget to Redis `INCR` if `spent` becomes the hot read (the SQL comment says so). SSE responses hold a connection open for the length of a turn — size `proxy_read_timeout` (nginx has 300s) and the pod's `terminationGracePeriodSeconds` for the longest turn you accept.

**Failure modes.** Provider down or 4xx/5xx: `claude()` throws, the client sees an `error` event after `conversation`, the user message is already stored, nothing is charged. Tool hung: the `tool_result` says `timed out`, the model is told, the turn continues. Budget spent: `429` before anything happens; the page shows `Error: daily budget…`. Database down: `/api/health/ready` still says ok today — it does not check the pool; that is a gap to close before relying on readiness. Client disconnect: `s.onAbort` aborts the provider request via `AbortSignal`.

**What to watch.** `rounds` and `output_tokens` per turn from the `done` event (log them); `429 budget` rate per user; `tool_result` errors by tool name; p95 turn duration against the proxy timeout.

## Ceilings

- **Check-then-charge is not atomic.** The budget is read before the turn and added after; N parallel requests from one user at the limit all pass the check. Upgrade: reserve an estimate in the same statement (`update … where tokens + $est <= $limit returning`) and settle the difference.
- **One system prompt for everyone.** `settings` is global. Upgrade: key it by user or add a `conversations.system` column.
- **No auth.** `x-user` is trusted as given. This service expects a gateway in front that sets it; without one, everyone is `anonymous`. The frontend page does not set it.
- **Whole history every turn.** No windowing or summarisation; long conversations grow input tokens linearly and eventually exceed the context window.
- **Two simultaneous sends to one conversation interleave.** No per-conversation lock; both read the same history and both append. Upgrade: `select … for update` on the conversation row for the turn, or refuse with `409`.
- **`listConversations` is `limit 50`, no cursor.** Upgrade: keyset on `(created_at, id)` as `docs/PRODUCTION.md` prescribes.
- **DNS is checked once.** `publicUrl` resolves, then `fetch` resolves again — a rebinding host can answer differently. Upgrade: connect to the checked address and set `Host` yourself, or pin with a custom agent.
- **Readiness does not probe the database.** Upgrade: `select 1` with a short timeout in `/api/health/ready`.

The stack rules — gate, typed seam, production checklist, deploy — are in `docs/PRODUCTION.md`. They apply.
"##;

const AGENT_AGENTS_MD: &str = r##"# {{NAME}} — for agents

`CLAUDE.md` has the rules and the invariants. This is how to run and test the service.

## Run

    make demo          # postgres + redis, migrations, seed, API on :8000, page on :3000
    make check         # the gate: typecheck both halves, bun test the backend
    make backend       # API alone, with reload
    make migrate       # apply backend/migrations to DATABASE_URL

`ANTHROPIC_API_KEY` goes in `backend/.env`. Without it every turn ends in an `error` event
reading `anthropic 401`.

## Every route, with curl

Set a user once; the header is the whole identity model.

    U='-H x-user:me'
    J='-H content-type:application/json'

    curl -s localhost:8000/api/health
    # {"status":"ok"}

    curl -s localhost:8000/api/tools
    # {"tools":[{"name":"web_fetch","description":"Fetch a public http(s) URL…","timeout_ms":10000},{"name":"notes",…,"timeout_ms":2000}]}

    curl -s localhost:8000/api/settings/system_prompt
    # {"value":"You are a helpful assistant. Use web_fetch …"}

    curl -s -X PUT localhost:8000/api/settings/system_prompt $J -d '{"value":"Answer in French. Be brief."}'
    # {"ok":true}          — {} or an empty value is 400 {"error":{"message":"value required","code":"invalid"}}

    curl -s $U localhost:8000/api/budget
    # {"used":0,"limit":200000,"day":"2026-08-29"}

    curl -N $U $J localhost:8000/api/chat/completions -d '{"message":"Save a note that the release is Friday, then list my notes."}'
    # event: conversation
    # data: {"id":"0191…"}
    # event: tool_call
    # data: {"type":"tool_call","call":{"id":"toolu_…","name":"notes","input":{"action":"add","body":"the release is Friday"}}}
    # event: tool_result
    # data: {"type":"tool_result","id":"toolu_…","output":"saved note #1","error":false}
    # event: delta
    # data: {"type":"delta","text":"Saved. Your notes: #1 the release is Friday"}
    # event: done
    # data: {"input_tokens":812,"output_tokens":64,"rounds":3}

    # Continue the same conversation: pass the id from the first event.
    curl -N $U $J localhost:8000/api/chat/completions -d '{"conversation_id":"0191…","message":"fetch https://example.com and summarise it"}'

    curl -s $U localhost:8000/api/conversations
    # {"conversations":[{"id":"0191…","user_id":"me","title":"Save a note that the release is Friday, then list…","created_at":"…","messages":4}]}

    curl -s $U localhost:8000/api/conversations/0191…
    # {"id":…,"messages":[{"role":"user","content":[{"type":"text","text":"…"}],…},{"role":"assistant","content":[{"type":"tool_use",…},{"type":"__tool_results","blocks":[…]},{"type":"text","text":"…"}],"input_tokens":812,"output_tokens":64}]}

    curl -s -H x-user:someone-else localhost:8000/api/conversations/0191…
    # 404 {"error":{"message":"no such conversation","code":"not_found"}}

Over budget: `429 {"error":{"message":"daily budget of 200000 tokens spent","code":"budget"}}`.
Set `DAILY_TOKEN_BUDGET=100` and send two messages to see it.

## How the tests work

`backend/src/app.test.ts` never opens a socket or a database:

- **`memoryStore()`** from `store.ts` is the same `Store` interface as Postgres, in Maps. The
  handler code under test is the production code; only the store differs.
- **The model is scripted.** `script([...replies])` returns a `Model` that hands back the next
  `ModelReply` on each call and streams its text as one `delta`. A reply with `calls` makes the
  loop run a tool; the next reply is what the model "says" after seeing the result.
- **Fetch is injected.** `runTurn` takes `ctx.fetch`; the timeout test passes one that never
  resolves. `ALLOW_PRIVATE_URLS=1` is set at the top of the file so the guard does not do DNS.
- **SSE is parsed by hand.** `events(text)` splits the response body on blank lines into
  `{event, data}`.

Run one test: `cd backend && bun test -t "budget"`.

## Adding a test

Copy the shape of the nearest existing one:

```ts
test("a tool error is reported and the turn still ends", async () => {
  const boom = { ...TOOLS[1], run: async () => { throw new Error("db gone"); } };
  const model = script([{ calls: [{ id: "t1", name: "notes", input: { action: "search" } }], stop: "tool_use" }, { text: "sorry" }]);
  const out: string[] = [];
  const turn = await runTurn([{ role: "user", content: "notes?" }], model, { store: memoryStore(), user: "u", system: "s", tools: [boom] }, (e) => { if (e.type === "tool_result") out.push(e.output); });
  expect(out[0]).toBe("Error: db gone"); expect(turn.text).toBe("sorry");
});
```

A test that needs a real provider does not belong in this file; it belongs in a script you run
by hand with a key.
"##;

const AGENT_README_MD: &str = r##"# {{NAME}}

A streaming chat backend with tool use, per-user budgets and a typed API — the part of
Open WebUI you would have built yourself, as code you own.

## What you get

- `POST /api/chat/completions`: one assistant turn streamed as SSE — `delta`, `tool_call`,
  `tool_result`, `done` — with the model calling tools and continuing until it stops.
- Two tools out of the box: `web_fetch` (public URLs only — loopback, private ranges and cloud
  metadata are refused, redirects re-checked) and `notes` (per-user, in Postgres). Each has its
  own timeout; a hung tool becomes an error result, never a hung request.
- Conversations and messages stored as Anthropic content blocks, listed and read per user; a
  user cannot read another user's conversation.
- A daily token budget per user, enforced before the model is called and charged from the
  stream's real `usage`, shared across replicas through the database.
- An editable system prompt, read per turn.
- Next.js page that renders the stream and edits the prompt; Hono backend; Postgres; kustomize
  overlays for dev and prod; a test suite that runs with no database and no API key.

## Five minutes

    cp backend/.env.example backend/.env    # add ANTHROPIC_API_KEY=…
    make demo

Then, in another shell:

    curl -N localhost:8000/api/chat/completions -H x-user:me -H content-type:application/json \
      -d '{"message":"Save a note that standup moved to 10:00."}'
    # event: conversation … event: tool_call {"name":"notes",…} … event: tool_result "saved note #1" … event: done

    curl -N localhost:8000/api/chat/completions -H x-user:me -H content-type:application/json \
      -d '{"message":"fetch https://example.com and tell me its title"}'
    # event: tool_call {"name":"web_fetch",…} … event: delta "The page is titled Example Domain…"

    curl -s localhost:8000/api/conversations -H x-user:me
    # {"conversations":[{"title":"Save a note that standup moved to 10:00.","messages":2,…},…]}

    curl -s localhost:8000/api/budget -H x-user:me
    # {"used":1103,"limit":200000,"day":"2026-08-29"}

Open <http://localhost:3000> for the same thing with a text box.

## API

| Method | Path | Auth | What |
|---|---|---|---|
| GET | `/api/health` | none | Liveness |
| GET | `/api/health/ready` | none | Readiness (does not yet probe the database) |
| GET | `/api/tools` | none | Tool names, descriptions, timeouts |
| GET | `/api/settings/system_prompt` | none | The current system prompt |
| PUT | `/api/settings/system_prompt` | none | `{value}` — replaces it for every user |
| GET | `/api/budget` | `x-user` | `{used, limit, day}` for today (UTC) |
| POST | `/api/chat/completions` | `x-user` | `{message, conversation_id?}` → SSE turn; `429 budget` when spent |
| GET | `/api/conversations` | `x-user` | Latest 50, with message counts |
| GET | `/api/conversations/:id` | `x-user` | The conversation with every message; `404` if not yours |

"Auth" means the `x-user` header is the identity. Nothing verifies it: put an auth proxy or
gateway in front that sets it from a session, and strip it from client requests.

## Compared with Open WebUI

**Same shape, so their mental model carries over**

- `POST /api/chat/completions` as the one chat endpoint, streaming.
- Conversations with a title, listed newest first, each with its messages.
- A system prompt as a setting; tools the model can call with a name, description and JSON
  Schema; tool calls and results shown inline in the chat.
- Message content stored as Anthropic content blocks, so the Messages API docs describe the rows.

**Better here**

- Typed end to end: `AppType` is exported from `backend/src/app.ts` and the frontend client is
  built from it; a route change that breaks the page fails `make check`.
- Tests without a database or a provider: `memoryStore` and a scripted `Model`, five tests in
  under a second, in CI on every push.
- Per-user token budgets in the database, refused before the model is reached.
- An SSRF guard on tool fetches — private ranges, link-local and metadata addresses refused after
  DNS, redirects re-checked hop by hop.
- Per-tool timeouts inside the turn.
- Kubernetes manifests, probes, secrets from an encrypted env file, and a one-command deploy.
- One small TypeScript codebase: `app.ts`, `agent.ts`, `store.ts` — you can read the whole
  service in an afternoon.

**Not here yet**

- User accounts, sign-in, roles, an admin panel — `x-user` is a header you must set.
- The OpenAI-compatible request/response shape; this endpoint takes `{message}` and emits named
  SSE events, so OpenAI SDKs do not point at it unchanged.
- Multiple providers (Ollama, OpenAI, …) and per-chat model selection.
- RAG: document upload, embeddings, citations.
- Image, audio and file input; image generation.
- Regenerate, edit-and-resend, branching, delete or rename a conversation.
- Pipelines / functions / filters, web search integrations, MCP servers.
- Per-user system prompts and memories.
- Internationalisation and the full chat UI.

## Production

**Environments.** `k8s/overlays/dev` and `k8s/overlays/prod`; dev and prod never share a
database. `git push` to `main` deploys dev; `make release` tags and deploys prod.

**Secrets.** `ANTHROPIC_API_KEY` and `DATABASE_URL` live in `backend/.env`, committed only as
`backend/.env.age`; `make k8s-secrets ENV=prod` renders the Secret.

**Scaling.** Stateless replicas behind the HPA; every piece of state — history, budgets, the
prompt — is in Postgres. Turns hold an SSE connection open: set the ingress/proxy read timeout
and the pod's grace period to the longest turn you accept (nginx here: 300s).

**Probes.** `/api/health` for liveness, `/api/health/ready` for readiness. Readiness does not
yet check the pool; add a `select 1` before you let it gate rollouts.

**Migrations.** `backend/migrations/0002_agent.sql` creates the five tables; the migrate init
container applies new files before each rollout.

**What pages you.** `error` events per minute (provider down), `429 budget` spikes (a runaway
client), `tool_result` errors by tool, turn duration approaching the proxy timeout.

## Roadmap

- Atomic budget reservation (today: read before, add after; a burst can overshoot).
- History windowing or summarisation (today: the whole conversation every turn).
- Per-user and per-conversation system prompts.
- A lock per conversation so parallel sends do not interleave.
- Keyset pagination on conversations.
- Readiness that probes the database.
"##;

const AGENT_REVIEWER: &str = r##"---
name: tool-safety
description: Run on any change to backend/src/agent.ts, the TOOLS registry, the turn loop, the budget, or a route that reads conversations. Reviews what the model is allowed to reach and what the caller is allowed to read; reports only what an attacker or a runaway model could exploit.
tools: Read, Grep, Glob, Bash
---

You review the boundary between the model, the tools and the users. The model's output is
untrusted input; so is the `x-user` header; so is every URL the model chooses.

Report each finding as `path:line — what — how it is reached — the fix`. No style.

Check:
1. **Tool inputs.** Every `Tool.run` treats `input` as unvalidated JSON from the model: strings
   coerced with `String()`, enums checked, no `input.user` or `input.conversation_id` honoured.
   Failure: a tool that reads an id from `input` instead of `ctx.user`.
2. **Outbound fetch.** Every HTTP call a tool makes goes through `fetchPublic`, never bare
   `fetch`. `publicUrl` still rejects: non-http(s) schemes, `127/8`, `10/8`, `172.16/12`,
   `192.168/16`, `169.254/16`, `100.64/10`, `0/8`, `::1`, `fc00::/7`, `fe80::/10` and
   `::ffff:`-mapped v4. Redirects: `redirect: "manual"`, each hop re-checked, hop count bounded.
   Failure: `ALLOW_PRIVATE_URLS` read anywhere but tests, or a new tool with `fetch(url)`.
3. **Timeouts.** Every entry in `TOOLS` has a `timeout_ms` and `runTurn` still races it; a tool
   that times out yields `is_error: true`, not a throw out of the loop.
4. **Rounds.** `maxRounds` is still enforced and still the default of 8; no path lets the model
   call tools without decrementing it.
5. **Budget order.** In `POST /api/chat/completions`, `store.spent` is compared to
   `DAILY_TOKEN_BUDGET` before `store.getConversation`, before the model. `store.spend` is
   called with the turn's real `input_tokens + output_tokens`. Failure: a refactor that charges
   before the turn and forgets to refund, or charges an estimate.
6. **Ownership.** Every route that takes a conversation id compares `conv.user_id` to `user(c)`
   and returns 404 (not 403, not the row). Grep for `getConversation(` and check each caller.
7. **Notes scope.** `store.listNotes` and `store.addNote` are only ever called with `ctx.user`;
   the SQL still has `user_id = ${user}`.
8. **Stored content.** `addMessage` stores what the model returned, unmodified; `toApiMessages`
   never emits a `__tool_results` block to the API. Failure: a marker block reaching Anthropic
   as a content block.
9. **Secrets.** `ANTHROPIC_API_KEY` appears only in `claude()`'s header; never in a log line,
   an error message (`anthropic ${status}: ${body}` is body text, not the key) or an SSE event.
10. **Body bounds.** zod still caps `message` at 50,000 and `value` at 20,000; a new route with
    a body has a bound.
11. **Stream errors.** A throw inside `streamSSE` becomes an `error` event, not an unhandled
    rejection that kills the process; `s.onAbort` still aborts the provider request.
12. **Tool descriptions.** A new tool's `description` and schema do not invite the model to
    pass credentials, file paths or internal hostnames.

End with one line: `tool-safety: N findings`, and if 0, which of the twelve you checked.
"##;
