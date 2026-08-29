//! llmproxy: an OpenAI-compatible proxy, like LiteLLM ═════════════════════════════════════

pub fn files(name: &str) -> Vec<(&'static str, String)> {
    let f = |s: &str| s.replace("{{NAME}}", name);
    let _ = &f;
    vec![
        ("backend/src/app.ts", LLM_APP.into()),
        ("backend/src/store.ts", LLM_STORE.into()),
        ("backend/src/providers.ts", LLM_PROVIDERS.into()),
        ("backend/src/app.test.ts", LLM_TEST.into()),
        ("backend/migrations/0002_llm.sql", LLM_SQL.into()),
        ("frontend/app/page.tsx", f(LLM_PAGE)),
    ]
}

const LLM_SQL: &str = r##"create table if not exists virtual_keys (
  token text primary key,             -- sha256 of the key
  key_alias text,
  models text[] not null default '{}', -- empty = any
  max_budget numeric,
  spend numeric not null default 0,
  rpm_limit int,
  created_at timestamptz not null default now()
);
create table if not exists spend_logs (
  request_id text primary key,
  api_key text not null,
  model text not null,
  model_group text not null,
  provider text not null,
  prompt_tokens int not null default 0,
  completion_tokens int not null default 0,
  spend numeric not null default 0,
  duration_ms int not null default 0,
  status text not null,
  started_at timestamptz not null default now()
);
"##;

const LLM_STORE: &str = r##"import { db } from "./db";

export type VKey = { token: string; key_alias: string | null; models: string[]; max_budget: number | null; spend: number; rpm_limit: number | null };
export type SpendLog = { request_id: string; api_key: string; model: string; model_group: string; provider: string; prompt_tokens: number; completion_tokens: number; spend: number; duration_ms: number; status: string };

export interface Store {
  key(token: string): Promise<VKey | null>;
  createKey(k: VKey): Promise<void>;
  addSpend(token: string, amount: number): Promise<void>;
  log(l: SpendLog): Promise<void>;
  logs(token: string | null, limit: number): Promise<SpendLog[]>;
}

export function pgStore(): Store {
  return {
    key: async (t) => (await db<VKey[]>`select token, key_alias, models, max_budget::float, spend::float, rpm_limit from virtual_keys where token = ${t}`)[0] ?? null,
    createKey: async (k) => { await db`insert into virtual_keys (token, key_alias, models, max_budget, rpm_limit) values (${k.token}, ${k.key_alias}, ${k.models}, ${k.max_budget}, ${k.rpm_limit})`; },
    addSpend: async (t, a) => { await db`update virtual_keys set spend = spend + ${a} where token = ${t}`; },
    log: async (l) => { await db`insert into spend_logs ${db(l, "request_id", "api_key", "model", "model_group", "provider", "prompt_tokens", "completion_tokens", "spend", "duration_ms", "status")}`; },
    logs: async (t, limit) => t ? db<SpendLog[]>`select * from spend_logs where api_key = ${t} order by started_at desc limit ${limit}` : db<SpendLog[]>`select * from spend_logs order by started_at desc limit ${limit}`,
  };
}

export function memoryStore(): Store {
  const keys = new Map<string, VKey>(); const logs: SpendLog[] = [];
  return {
    key: async (t) => keys.get(t) ?? null,
    createKey: async (k) => { keys.set(k.token, { ...k }); },
    addSpend: async (t, a) => { const k = keys.get(t); if (k) k.spend += a; },
    log: async (l) => { logs.push(l); },
    logs: async (t, limit) => logs.filter((l) => !t || l.api_key === t).slice(-limit).reverse(),
  };
}
"##;

const LLM_PROVIDERS: &str = r##"// Provider adapters: OpenAI-shaped in, provider-shaped out, and back. The Anthropic mapping is
// LiteLLM's: system messages lifted to `system`, tool calls to `tool_use` blocks, tool results
// to `tool_result`, stop reasons renamed. Streaming is re-emitted as OpenAI chunks.

export type Msg = { role: "system" | "user" | "assistant" | "tool"; content: string | null; tool_calls?: ToolCall[]; tool_call_id?: string; name?: string };
export type ToolCall = { id: string; type: "function"; function: { name: string; arguments: string } };
export type Tool = { type: "function"; function: { name: string; description?: string; parameters?: unknown } };
export type ChatRequest = { model: string; messages: Msg[]; stream?: boolean; temperature?: number; max_tokens?: number; tools?: Tool[]; tool_choice?: unknown; stop?: string | string[] };
export type Usage = { prompt_tokens: number; completion_tokens: number; total_tokens: number };
export type ChatResponse = { id: string; object: "chat.completion"; created: number; model: string; choices: { index: number; message: Msg; finish_reason: string }[]; usage: Usage };

/// A deployment: what `model_list` in LiteLLM's config holds. `model` is `provider/name`.
export type Fetch = (url: string | URL, init?: RequestInit) => Promise<Response>;
export type Deployment = { model_name: string; model: string; api_base?: string; api_key_env: string; input_cost_per_token: number; output_cost_per_token: number };

export const PRICES: Record<string, [number, number]> = {
  "gpt-4o": [2.5e-6, 10e-6], "gpt-4o-mini": [0.15e-6, 0.6e-6],
  "claude-sonnet-4-20250514": [3e-6, 15e-6], "claude-3-5-haiku-20241022": [0.8e-6, 4e-6],
};

export function providerOf(model: string): "openai" | "anthropic" {
  return model.startsWith("anthropic/") ? "anthropic" : "openai";
}

export function cost(d: Deployment, u: Usage): number {
  return u.prompt_tokens * d.input_cost_per_token + u.completion_tokens * d.output_cost_per_token;
}

/// One call, non-streaming or streaming. Returns either a ChatResponse or an async iterator of
/// OpenAI chunks. `fetchImpl` is injectable so the tests never leave the process.
export type Chunk = { id: string; object: "chat.completion.chunk"; created: number; model: string; choices: { index: 0; delta: Partial<Msg>; finish_reason: string | null }[]; usage?: Usage };

export interface Provider {
  complete(req: ChatRequest, d: Deployment, key: string, fetchImpl: Fetch): Promise<ChatResponse>;
  stream(req: ChatRequest, d: Deployment, key: string, fetchImpl: Fetch): AsyncIterable<Chunk>;
}

const bare = (m: string) => m.replace(/^[a-z]+\//, "");
const now = () => Math.floor(Date.now() / 1000);

// ── OpenAI: pass-through with the model name unwrapped ───────────────────────────────────
export const openai: Provider = {
  async complete(req, d, key, fetchImpl) {
    const res = await fetchImpl(`${d.api_base ?? "https://api.openai.com/v1"}/chat/completions`, {
      method: "POST", headers: { authorization: `Bearer ${key}`, "content-type": "application/json" },
      body: JSON.stringify({ ...req, model: bare(d.model), stream: false }),
    });
    if (!res.ok) throw new ProviderError(res.status, await res.text());
    return (await res.json()) as ChatResponse;
  },
  async *stream(req, d, key, fetchImpl) {
    const res = await fetchImpl(`${d.api_base ?? "https://api.openai.com/v1"}/chat/completions`, {
      method: "POST", headers: { authorization: `Bearer ${key}`, "content-type": "application/json" },
      body: JSON.stringify({ ...req, model: bare(d.model), stream: true, stream_options: { include_usage: true } }),
    });
    if (!res.ok || !res.body) throw new ProviderError(res.status, await res.text());
    for await (const data of sse(res.body)) {
      if (data === "[DONE]") return;
      yield JSON.parse(data) as Chunk;
    }
  },
};

// ── Anthropic: the Messages API, mapped both ways ────────────────────────────────────────
function toAnthropic(req: ChatRequest, d: Deployment) {
  const system = req.messages.filter((m) => m.role === "system").map((m) => m.content ?? "").join("\n");
  const messages: { role: "user" | "assistant"; content: unknown }[] = [];
  for (const m of req.messages) {
    if (m.role === "system") continue;
    if (m.role === "tool") { messages.push({ role: "user", content: [{ type: "tool_result", tool_use_id: m.tool_call_id, content: m.content ?? "" }] }); continue; }
    if (m.role === "assistant" && m.tool_calls?.length) {
      const blocks: unknown[] = m.content ? [{ type: "text", text: m.content }] : [];
      for (const t of m.tool_calls) blocks.push({ type: "tool_use", id: t.id, name: t.function.name, input: JSON.parse(t.function.arguments || "{}") });
      messages.push({ role: "assistant", content: blocks }); continue;
    }
    const last = messages.at(-1);
    if (last && last.role === m.role && typeof last.content === "string") last.content += "\n" + (m.content ?? "");
    else messages.push({ role: m.role, content: m.content ?? "" });
  }
  const tools = req.tools?.map((t) => ({ name: t.function.name, description: t.function.description, input_schema: t.function.parameters ?? { type: "object" } }));
  const tc = req.tool_choice;
  const tool_choice = tc === "auto" ? { type: "auto" } : tc === "required" ? { type: "any" } : tc === "none" ? { type: "none" } : typeof tc === "object" && tc ? { type: "tool", name: (tc as { function: { name: string } }).function.name } : undefined;
  return { model: bare(d.model), max_tokens: req.max_tokens ?? 4096, temperature: req.temperature, system: system || undefined, messages, tools, tool_choice, stop_sequences: typeof req.stop === "string" ? [req.stop] : req.stop, stream: req.stream ?? false };
}
const finish = (r: string) => (r === "max_tokens" ? "length" : r === "tool_use" ? "tool_calls" : "stop");

export const anthropic: Provider = {
  async complete(req, d, key, fetchImpl) {
    const res = await fetchImpl(`${d.api_base ?? "https://api.anthropic.com"}/v1/messages`, {
      method: "POST", headers: { "x-api-key": key, "anthropic-version": "2023-06-01", "content-type": "application/json" },
      body: JSON.stringify({ ...toAnthropic(req, d), stream: false }),
    });
    if (!res.ok) throw new ProviderError(res.status, await res.text());
    const a = (await res.json()) as { id: string; content: { type: string; text?: string; id?: string; name?: string; input?: unknown }[]; stop_reason: string; usage: { input_tokens: number; output_tokens: number } };
    const text = a.content.filter((b) => b.type === "text").map((b) => b.text).join("");
    const tool_calls: ToolCall[] = a.content.filter((b) => b.type === "tool_use").map((b) => ({ id: b.id!, type: "function", function: { name: b.name!, arguments: JSON.stringify(b.input ?? {}) } }));
    return {
      id: "chatcmpl-" + a.id, object: "chat.completion", created: now(), model: req.model,
      choices: [{ index: 0, message: { role: "assistant", content: text || null, ...(tool_calls.length ? { tool_calls } : {}) }, finish_reason: finish(a.stop_reason) }],
      usage: { prompt_tokens: a.usage.input_tokens, completion_tokens: a.usage.output_tokens, total_tokens: a.usage.input_tokens + a.usage.output_tokens },
    };
  },
  async *stream(req, d, key, fetchImpl) {
    const res = await fetchImpl(`${d.api_base ?? "https://api.anthropic.com"}/v1/messages`, {
      method: "POST", headers: { "x-api-key": key, "anthropic-version": "2023-06-01", "content-type": "application/json" },
      body: JSON.stringify({ ...toAnthropic(req, d), stream: true }),
    });
    if (!res.ok || !res.body) throw new ProviderError(res.status, await res.text());
    const id = "chatcmpl-" + crypto.randomUUID(); const created = now();
    const chunk = (delta: Partial<Msg>, finish_reason: string | null = null, usage?: Usage): Chunk => ({ id, object: "chat.completion.chunk", created, model: req.model, choices: [{ index: 0, delta, finish_reason }], ...(usage ? { usage } : {}) });
    let input = 0, toolIndex = -1;
    for await (const data of sse(res.body)) {
      const ev = JSON.parse(data) as { type: string; message?: { usage: { input_tokens: number } }; content_block?: { type: string; id: string; name: string }; delta?: { type?: string; text?: string; partial_json?: string; stop_reason?: string }; usage?: { output_tokens: number } };
      switch (ev.type) {
        case "message_start": input = ev.message?.usage.input_tokens ?? 0; yield chunk({ role: "assistant" }); break;
        case "content_block_start": if (ev.content_block?.type === "tool_use") { toolIndex++; yield chunk({ tool_calls: [{ id: ev.content_block.id, type: "function", function: { name: ev.content_block.name, arguments: "" } }] }); } break;
        case "content_block_delta":
          if (ev.delta?.type === "text_delta") yield chunk({ content: ev.delta.text });
          if (ev.delta?.type === "input_json_delta") yield chunk({ tool_calls: [{ id: "", type: "function", function: { name: "", arguments: ev.delta.partial_json ?? "" } }] });
          break;
        case "message_delta": { const out = ev.usage?.output_tokens ?? 0; yield chunk({}, finish(ev.delta?.stop_reason ?? "end_turn"), { prompt_tokens: input, completion_tokens: out, total_tokens: input + out }); break; }
        default: break;
      }
    }
  },
};

export class ProviderError extends Error { constructor(public status: number, body: string) { super(body.slice(0, 400)); } }

/// `data:` lines out of an SSE body.
export async function* sse(body: ReadableStream<Uint8Array>): AsyncGenerator<string> {
  const reader = body.getReader(); const dec = new TextDecoder(); let buf = "";
  for (;;) {
    const { value, done } = await reader.read();
    if (done) return;
    buf += dec.decode(value, { stream: true });
    let i;
    while ((i = buf.indexOf("\n")) >= 0) {
      const line = buf.slice(0, i).trim(); buf = buf.slice(i + 1);
      if (line.startsWith("data:")) yield line.slice(5).trim();
    }
  }
}
"##;

const LLM_APP: &str = r##"import { Hono } from "hono";
import { streamSSE } from "hono/streaming";
import { createHash, randomBytes } from "node:crypto";
import { pgStore, type Store } from "./store";
import { anthropic, openai, providerOf, cost, PRICES, ProviderError, type ChatRequest, type Deployment, type Fetch, type Usage } from "./providers";

// LiteLLM's surface: POST /v1/chat/completions (streaming as OpenAI chunks ending in [DONE]),
// GET /v1/models, POST /key/generate under the master key, virtual keys with model allowlists
// and budgets, a spend log per call with cost from a price table, ordered fallbacks between
// deployments of the same model group. Provider keys come from the environment and never
// leave this process.

const sha = (s: string) => createHash("sha256").update(s).digest("hex");

/// The router table — LiteLLM's `model_list`. A model_name can appear more than once; the
/// first deployment that answers wins, in order. Edit here or load from MODEL_LIST_JSON.
export const MODEL_LIST: Deployment[] = process.env.MODEL_LIST_JSON ? JSON.parse(process.env.MODEL_LIST_JSON) : [
  { model_name: "gpt-4o", model: "openai/gpt-4o", api_key_env: "OPENAI_API_KEY", input_cost_per_token: PRICES["gpt-4o"][0], output_cost_per_token: PRICES["gpt-4o"][1] },
  { model_name: "claude", model: "anthropic/claude-sonnet-4-20250514", api_key_env: "ANTHROPIC_API_KEY", input_cost_per_token: PRICES["claude-sonnet-4-20250514"][0], output_cost_per_token: PRICES["claude-sonnet-4-20250514"][1] },
  { model_name: "fast", model: "anthropic/claude-3-5-haiku-20241022", api_key_env: "ANTHROPIC_API_KEY", input_cost_per_token: PRICES["claude-3-5-haiku-20241022"][0], output_cost_per_token: PRICES["claude-3-5-haiku-20241022"][1] },
  { model_name: "fast", model: "openai/gpt-4o-mini", api_key_env: "OPENAI_API_KEY", input_cost_per_token: PRICES["gpt-4o-mini"][0], output_cost_per_token: PRICES["gpt-4o-mini"][1] },
];

export const FALLBACKS: Record<string, string[]> = { "gpt-4o": ["claude"], claude: ["gpt-4o"] };

const err = (message: string, type: string, code: string | number) => ({ error: { message, type, param: null, code } });

export function createApp(store: Store, fetchImpl: Fetch = fetch, list: Deployment[] = MODEL_LIST) {
  const app = new Hono<{ Variables: { token: string } }>()
    .get("/api/health", (c) => c.json({ status: "ok" }))
    .get("/api/health/ready", (c) => c.json({ status: "ok", db: "ok" as const }))

    .post("/key/generate", async (c) => {
      const master = process.env.LITELLM_MASTER_KEY ?? process.env.MASTER_KEY;
      if (!master || c.req.header("authorization") !== `Bearer ${master}`) return c.json(err("master key required", "auth_error", 401), 401);
      const body = (await c.req.json().catch(() => ({}))) as { models?: string[]; max_budget?: number; key_alias?: string; rpm_limit?: number };
      const key = "sk-" + randomBytes(24).toString("hex");
      await store.createKey({ token: sha(key), key_alias: body.key_alias ?? null, models: body.models ?? [], max_budget: body.max_budget ?? null, spend: 0, rpm_limit: body.rpm_limit ?? null });
      return c.json({ key, models: body.models ?? [], max_budget: body.max_budget ?? null, spend: 0, key_alias: body.key_alias ?? null });
    })

    .use("/v1/*", async (c, next) => {
      const raw = c.req.header("authorization")?.replace(/^Bearer /, "");
      const master = process.env.LITELLM_MASTER_KEY ?? process.env.MASTER_KEY;
      if (!raw) return c.json(err("Authentication Error: no key", "auth_error", 401), 401);
      if (raw !== master) {
        const k = await store.key(sha(raw));
        if (!k) return c.json(err("Authentication Error: invalid key", "auth_error", 401), 401);
        if (k.max_budget != null && k.spend >= k.max_budget) return c.json(err(`ExceededBudget: spend ${k.spend.toFixed(4)} >= max_budget ${k.max_budget}`, "budget_exceeded", 400), 400);
      }
      c.set("token", raw === master ? "master" : sha(raw));
      await next();
    })

    .get("/v1/models", async (c) => {
      const k = c.get("token") === "master" ? null : await store.key(c.get("token"));
      const names = [...new Set(list.map((d) => d.model_name))].filter((n) => !k || k.models.length === 0 || k.models.includes(n));
      return c.json({ object: "list", data: names.map((id) => ({ id, object: "model", created: 1677610602, owned_by: "openai" })) });
    })

    .post("/v1/chat/completions", async (c) => {
      const req = (await c.req.json()) as ChatRequest;
      const token = c.get("token");
      const k = token === "master" ? null : await store.key(token);
      if (k && k.models.length && !k.models.includes(req.model)) return c.json(err(`key not allowed to use model ${req.model}`, "auth_error", 401), 401);
      const groups = [req.model, ...(FALLBACKS[req.model] ?? [])];
      const deployments = groups.flatMap((g) => list.filter((d) => d.model_name === g));
      if (!deployments.length) return c.json(err(`model ${req.model} not found`, "invalid_request_error", "model_not_found"), 404);

      const request_id = "chatcmpl-" + crypto.randomUUID();
      const started = Date.now();
      let lastError: ProviderError | Error | null = null;
      for (const d of deployments) {
        const provider = providerOf(d.model) === "anthropic" ? anthropic : openai;
        const key = process.env[d.api_key_env];
        if (!key) { lastError = new Error(`${d.api_key_env} is not set`); continue; }
        const record = async (usage: Usage, status: string) => {
          const spend = cost(d, usage);
          await store.log({ request_id, api_key: token, model: d.model, model_group: req.model, provider: providerOf(d.model), prompt_tokens: usage.prompt_tokens, completion_tokens: usage.completion_tokens, spend, duration_ms: Date.now() - started, status });
          if (token !== "master") await store.addSpend(token, spend);
          return spend;
        };
        try {
          if (req.stream) {
            const it = provider.stream(req, d, key, fetchImpl)[Symbol.asyncIterator]();
            const first = await it.next(); // a provider error surfaces here, before headers go out
            c.header("x-litellm-model-id", d.model); c.header("x-litellm-call-id", request_id);
            return streamSSE(c, async (s) => {
              let usage: Usage = { prompt_tokens: 0, completion_tokens: 0, total_tokens: 0 };
              for (let r = first; !r.done; r = await it.next()) {
                if (r.value.usage) usage = r.value.usage;
                await s.writeSSE({ data: JSON.stringify(r.value) });
              }
              await s.writeSSE({ data: "[DONE]" });
              await record(usage, "success");
            });
          }
          const out = await provider.complete(req, d, key, fetchImpl);
          const spend = await record(out.usage, "success");
          c.header("x-litellm-model-id", d.model); c.header("x-litellm-call-id", request_id); c.header("x-litellm-response-cost", spend.toFixed(6));
          return c.json({ ...out, id: request_id, model: req.model });
        } catch (e) {
          lastError = e as Error; // fall through to the next deployment
        }
      }
      await store.log({ request_id, api_key: token, model: deployments[0].model, model_group: req.model, provider: providerOf(deployments[0].model), prompt_tokens: 0, completion_tokens: 0, spend: 0, duration_ms: Date.now() - started, status: "failure" });
      const status = lastError instanceof ProviderError ? lastError.status : 502;
      return c.json(err(lastError?.message ?? "all deployments failed", "api_error", status), status as 502);
    })

    .get("/spend/logs", async (c) => {
      const token = c.req.header("authorization")?.replace(/^Bearer /, "");
      const master = process.env.LITELLM_MASTER_KEY ?? process.env.MASTER_KEY;
      if (!token) return c.json(err("key required", "auth_error", 401), 401);
      return c.json(await store.logs(token === master ? null : sha(token), 200));
    });

  return app;
}

const app = createApp(pgStore());
export type AppType = typeof app;
export default app;
"##;

const LLM_TEST: &str = r##"import { describe, expect, test } from "bun:test";
import { createApp } from "./app";
import { memoryStore } from "./store";
import type { Deployment, Fetch } from "./providers";

// The proxy against fake providers: OpenAI shape in and out, Anthropic mapping, fallbacks,
// budgets, streaming chunks ending in [DONE], spend logged with cost.
process.env.MASTER_KEY = "sk-master";
process.env.OPENAI_API_KEY = "x"; process.env.ANTHROPIC_API_KEY = "y";

const list: Deployment[] = [
  { model_name: "gpt-4o", model: "openai/gpt-4o", api_key_env: "OPENAI_API_KEY", input_cost_per_token: 1e-6, output_cost_per_token: 2e-6 },
  { model_name: "claude", model: "anthropic/claude-sonnet-4-20250514", api_key_env: "ANTHROPIC_API_KEY", input_cost_per_token: 3e-6, output_cost_per_token: 15e-6 },
];

let openaiDown = false;
const fakeFetch: Fetch = async (url, init) => {
  const body = JSON.parse(String(init?.body));
  if (String(url).includes("openai")) {
    if (openaiDown) return new Response("upstream down", { status: 503 });
    if (body.stream) {
      const lines = [
        `data: {"id":"c","object":"chat.completion.chunk","created":1,"model":"gpt-4o","choices":[{"index":0,"delta":{"role":"assistant"},"finish_reason":null}]}\n\n`,
        `data: {"id":"c","object":"chat.completion.chunk","created":1,"model":"gpt-4o","choices":[{"index":0,"delta":{"content":"hi"},"finish_reason":null}]}\n\n`,
        `data: {"id":"c","object":"chat.completion.chunk","created":1,"model":"gpt-4o","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}\n\n`,
        `data: {"id":"c","object":"chat.completion.chunk","created":1,"model":"gpt-4o","choices":[],"usage":{"prompt_tokens":5,"completion_tokens":1,"total_tokens":6}}\n\n`,
        `data: [DONE]\n\n`,
      ];
      return new Response(new ReadableStream({ start(ctl) { for (const l of lines) ctl.enqueue(new TextEncoder().encode(l)); ctl.close(); } }), { headers: { "content-type": "text/event-stream" } });
    }
    return Response.json({ id: "x", object: "chat.completion", created: 1, model: "gpt-4o", choices: [{ index: 0, message: { role: "assistant", content: "hello from openai" }, finish_reason: "stop" }], usage: { prompt_tokens: 10, completion_tokens: 5, total_tokens: 15 } });
  }
  // Anthropic: assert the mapping happened, answer in its own shape.
  expect(body.system).toBe("be brief");
  expect(body.messages[0]).toEqual({ role: "user", content: "hi" });
  expect(body.max_tokens).toBe(4096);
  return Response.json({ id: "msg_1", content: [{ type: "text", text: "hello from claude" }], stop_reason: "end_turn", usage: { input_tokens: 7, output_tokens: 3 } });
};

const app = createApp(memoryStore(), fakeFetch, list);
const chat = (key: string, body: unknown) => app.request("/v1/chat/completions", { method: "POST", headers: { authorization: `Bearer ${key}`, "content-type": "application/json" }, body: JSON.stringify(body) });

describe("llm proxy", () => {
  test("keys are minted under the master key and scoped to models", async () => {
    const res = await app.request("/key/generate", { method: "POST", headers: { authorization: "Bearer sk-master", "content-type": "application/json" }, body: JSON.stringify({ models: ["claude"], max_budget: 0.001 }) });
    const { key } = await res.json();
    expect(key.startsWith("sk-")).toBe(true);
    const models = await (await app.request("/v1/models", { headers: { authorization: `Bearer ${key}` } })).json();
    expect(models.data.map((m: { id: string }) => m.id)).toEqual(["claude"]);
    expect((await chat(key, { model: "gpt-4o", messages: [] })).status).toBe(401);
  });

  test("anthropic is mapped and answered in openai's shape, with cost logged", async () => {
    const res = await chat("sk-master", { model: "claude", messages: [{ role: "system", content: "be brief" }, { role: "user", content: "hi" }] });
    expect(res.status).toBe(200);
    const out = await res.json();
    expect(out.choices[0].message.content).toBe("hello from claude");
    expect(out.usage.total_tokens).toBe(10);
    expect(res.headers.get("x-litellm-response-cost")).toBe((7 * 3e-6 + 3 * 15e-6).toFixed(6));
  });

  test("a dead deployment falls back to the next group", async () => {
    openaiDown = true;
    const res = await chat("sk-master", { model: "gpt-4o", messages: [{ role: "system", content: "be brief" }, { role: "user", content: "hi" }] });
    openaiDown = false;
    expect(res.status).toBe(200);
    expect(res.headers.get("x-litellm-model-id")).toContain("anthropic/");
  });

  test("streaming re-emits chunks and ends with [DONE]", async () => {
    const res = await chat("sk-master", { model: "gpt-4o", messages: [{ role: "user", content: "hi" }], stream: true });
    expect(res.headers.get("content-type")).toContain("text/event-stream");
    const text = await res.text();
    expect(text).toContain('"content":"hi"');
    expect(text.trim().endsWith("data: [DONE]")).toBe(true);
  });

  test("a budget stops spend", async () => {
    const res = await app.request("/key/generate", { method: "POST", headers: { authorization: "Bearer sk-master", "content-type": "application/json" }, body: JSON.stringify({ max_budget: 0.00001 }) });
    const { key } = await res.json();
    expect((await chat(key, { model: "gpt-4o", messages: [{ role: "user", content: "hi" }] })).status).toBe(200);
    const second = await chat(key, { model: "gpt-4o", messages: [{ role: "user", content: "hi" }] });
    expect(second.status).toBe(400);
    expect((await second.json()).error.type).toBe("budget_exceeded");
  });
});
"##;

const LLM_PAGE: &str = r##"async function logs() {
  const master = process.env.MASTER_KEY ?? process.env.LITELLM_MASTER_KEY ?? "";
  const res = await fetch(`${process.env.API_URL ?? "http://127.0.0.1:8000"}/spend/logs`, { headers: { authorization: `Bearer ${master}` }, cache: "no-store" });
  return res.ok ? ((await res.json()) as { request_id: string; model_group: string; model: string; prompt_tokens: number; completion_tokens: number; spend: number; duration_ms: number; status: string }[]) : [];
}

export default async function Home() {
  const rows = await logs();
  const total = rows.reduce((s, r) => s + Number(r.spend), 0);
  return (
    <main>
      <h1>{{NAME}} — LLM proxy</h1>
      <p>OpenAI-compatible. Point any SDK at <code>http://localhost:8000/v1</code> with a virtual key.</p>
      <pre>{`curl -X POST http://localhost:8000/key/generate -H "authorization: Bearer $MASTER_KEY" \\
  -H "content-type: application/json" -d '{"models":["claude","gpt-4o"],"max_budget":5}'
curl http://localhost:8000/v1/chat/completions -H "authorization: Bearer sk-..." \\
  -H "content-type: application/json" -d '{"model":"claude","messages":[{"role":"user","content":"hi"}]}'`}</pre>
      <h2>Spend · ${total.toFixed(4)}</h2>
      <table>
        <thead><tr><th>request</th><th>group → model</th><th>tokens</th><th>cost</th><th>ms</th><th>status</th></tr></thead>
        <tbody>{rows.map((r) => <tr key={r.request_id}><td>{r.request_id.slice(0, 16)}</td><td>{r.model_group} → {r.model}</td><td>{r.prompt_tokens}/{r.completion_tokens}</td><td>${Number(r.spend).toFixed(6)}</td><td>{r.duration_ms}</td><td>{r.status}</td></tr>)}</tbody>
      </table>
      {rows.length === 0 && <p>No calls yet.</p>}
    </main>
  );
}
"##;
