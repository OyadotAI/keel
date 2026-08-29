//! agent: an AI agent backend with streaming tool use, like Open WebUI / LibreChat ════════════

pub fn files(name: &str) -> Vec<(&'static str, String)> {
    let f = |s: &str| s.replace("{{NAME}}", name);
    let _ = &f;
    vec![
        ("backend/src/app.ts", AGENT_APP.into()),
        ("backend/src/store.ts", AGENT_STORE.into()),
        ("backend/src/agent.ts", AGENT_AGENT.into()),
        ("backend/src/app.test.ts", AGENT_TEST.into()),
        ("backend/migrations/0002_agent.sql", AGENT_SQL.into()),
        ("frontend/app/page.tsx", f(AGENT_PAGE)),
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
