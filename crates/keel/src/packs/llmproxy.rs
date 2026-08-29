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
        ("CLAUDE.md", f(LLM_CLAUDE)),
        ("AGENTS.md", f(LLM_AGENTS)),
        ("README.md", f(LLM_README)),
        (
            ".claude/agents/provider-mapping.md",
            LLM_REVIEWER_MAPPING.into(),
        ),
        (".claude/agents/key-and-spend.md", LLM_REVIEWER_SPEND.into()),
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

const LLM_CLAUDE: &str = r##"# {{NAME}} — working agreement

{{NAME}} is an OpenAI-compatible LLM proxy modelled on LiteLLM's proxy server: one
`POST /v1/chat/completions` in front of OpenAI and Anthropic, virtual keys minted under a master
key with model allowlists and budgets, a spend log per call with cost from a price table, and
ordered fallbacks between deployments of the same model group. Provider keys live in this
process's environment and never reach a client. "Done" here means: the request shape in and out
is OpenAI's (a client SDK pointed at `/v1` cannot tell), every provider path is exercised by
`app.test.ts` through an injected `fetch` with no network, spend is logged, and `make check` is
green.

## Architecture

| File | Owns |
|---|---|
| `backend/src/app.ts` | `MODEL_LIST` (the router table), `FALLBACKS`, the routes: `/key/generate`, `/v1/models`, `/v1/chat/completions`, `/spend/logs`; the auth + budget middleware on `/v1/*`; the exported `AppType` |
| `backend/src/providers.ts` | The OpenAI types (`ChatRequest`, `ChatResponse`, `Chunk`), `Provider` interface, `openai` (pass-through) and `anthropic` (mapped both ways, streaming re-emitted as OpenAI chunks), `PRICES`, `cost()`, `sse()`, `ProviderError` |
| `backend/src/store.ts` | `Store` — `key`, `createKey`, `addSpend`, `log`, `logs` — as `pgStore()` and `memoryStore()` |
| `backend/src/db.ts` | The one `postgres` pool, from `DATABASE_URL` |
| `backend/src/server.ts` | HTTP on `PORT`, SIGTERM drain |
| `backend/src/migrate.ts` | Applies `backend/migrations/*.sql` in order |
| `backend/src/seed.ts` | Sample `notes` rows from the base scaffold; unrelated to the proxy |
| `backend/src/app.test.ts` | Keys and allowlists, the Anthropic mapping, fallback, streaming `[DONE]`, budgets — against a fake `fetch` |
| `backend/migrations/0002_llm.sql` | `virtual_keys`, `spend_logs` |
| `frontend/app/page.tsx` | The spend table from `/spend/logs` under the master key, with the total |
| `frontend/lib/api.ts` | `hc<AppType>` — the typed client |

### The request path

1. `.use("/v1/*")` reads `Authorization: Bearer`. Equal to `MASTER_KEY` (or `LITELLM_MASTER_KEY`): `token = "master"`. Otherwise `store.key(sha256(raw))`; unknown: 401 `auth_error`. Over budget (`spend >= max_budget`): 400 `budget_exceeded` — LiteLLM's code.
2. `POST /v1/chat/completions` parses the body as `ChatRequest`. A virtual key with a non-empty `models` list that does not include `req.model` is 401.
3. Candidate deployments are `MODEL_LIST` rows whose `model_name` is `req.model`, then those of each `FALLBACKS[req.model]` group, in order. None: 404 `model_not_found`.
4. For each deployment: the provider is chosen from the `provider/` prefix of `d.model`; the upstream key is `process.env[d.api_key_env]` (unset: skip to the next).
5. Non-streaming: `provider.complete()` → `ChatResponse`; the spend log row is written, `spend` is added to the virtual key, and the response goes back with `id` replaced by the proxy's `chatcmpl-…` and `model` set to the group name, plus `x-litellm-model-id`, `x-litellm-call-id`, `x-litellm-response-cost`.
6. Streaming: the first chunk is awaited **before** headers go out so an upstream error can still fall through to the next deployment; then chunks are re-emitted as `data:` lines, `data: [DONE]` ends the stream, and the usage from the last chunk that carried one is logged after.
7. Any thrown error moves to the next deployment. When all fail, a `status: "failure"` log row is written with zero tokens and the last error's status (a `ProviderError` keeps the upstream code; otherwise 502).
8. `GET /spend/logs` returns the caller's rows (master: everyone's), newest first, 200 max.

### Data model

| Table | Columns that matter | Why |
|---|---|---|
| `virtual_keys` | `token text primary key` (sha256) | The plaintext `sk-…` is returned once from `/key/generate` and never stored |
| | `models text[]` | Empty means any model; non-empty is an allowlist on the group name |
| | `max_budget numeric`, `spend numeric` | The budget check is `spend >= max_budget` at request time; `addSpend` is an atomic `spend = spend + $1` |
| | `rpm_limit int` | Stored, returned, **not enforced** (see Ceilings) |
| `spend_logs` | `request_id` (the `chatcmpl-…` id the client saw) | Lets a client's complaint be matched to a row |
| | `api_key` (hash or `master`), `model_group`, `model`, `provider` | What was asked for vs what answered — the fallback trail |
| | `prompt_tokens`, `completion_tokens`, `spend`, `duration_ms`, `status` | The bill and the latency, per call |

## Invariants

1. **Provider keys never leave the process.** `OPENAI_API_KEY`/`ANTHROPIC_API_KEY` are read
   by name from `d.api_key_env` at call time and appear only in the upstream request headers.
   No route echoes `MODEL_LIST` with env values. Grep for `api_key_env` when reviewing.
2. **Virtual keys are stored hashed.** `createKey` receives `sha(key)`; `/key/generate` is
   the only place the plaintext exists. Guarded by "keys are minted under the master key and
   scoped to models" (the test can only use what it was handed).
3. **`/key/generate` requires the master key**, compared as the exact `Bearer <master>` header.
   Unset master: nobody can mint keys. Same test.
4. **The response is OpenAI's shape regardless of provider.** `id` is `chatcmpl-…`, `object`
   is `chat.completion`, `choices[0].message` and `usage` are present; Anthropic's
   `stop_reason` becomes `finish_reason` via `finish()` (`max_tokens → length`,
   `tool_use → tool_calls`, else `stop`). Guarded by "anthropic is mapped and answered in
   openai's shape, with cost logged".
5. **The Anthropic mapping is LiteLLM's.** `system` messages are lifted and joined; `tool`
   messages become `user` turns with a `tool_result` block; assistant `tool_calls` become
   `tool_use` blocks; consecutive same-role text turns are merged (Anthropic requires
   alternation); `max_tokens` defaults to 4096 because Anthropic requires it. The fake fetch in
   the test asserts `system`, `messages[0]` and `max_tokens`.
6. **A streamed response ends with `data: [DONE]`** and carries `content-type:
   text/event-stream`. Guarded by "streaming re-emits chunks and ends with [DONE]".
7. **Fallback happens before headers.** The first chunk is awaited outside `streamSSE`;
   a 503 from the first deployment therefore surfaces as a `ProviderError` and the loop tries
   the next. Guarded by "a dead deployment falls back to the next group"
   (`x-litellm-model-id` contains `anthropic/`).
8. **Every completed call writes a spend row and increments the key's spend**, master calls
   log but do not increment. The `x-litellm-response-cost` header equals
   `cost(d, usage).toFixed(6)`. Guarded by the mapping test's cost assertion.
9. **Budget is enforced on the next request, not the current one.** The check is in the
   middleware, so a key with `0.00001` left completes one call and is refused on the second
   with `type: "budget_exceeded"`. Guarded by "a budget stops spend".
10. **Model allowlists are checked against the requested group, not the fallback.** A key
    allowed `["gpt-4o"]` may be served by `claude` through `FALLBACKS`. This is the current
    behaviour, documented rather than hidden; tighten it if allowlists are a cost boundary
    (see Ceilings).
11. **Errors are OpenAI's envelope**: `{error:{message, type, param, code}}` with
    `auth_error`, `budget_exceeded`, `invalid_request_error`, `api_error`.
12. **Tests never touch the network.** `createApp(store, fetchImpl, list)` takes the fetch and
    the router table; `app.test.ts` passes a fake for both. A test that imports `MODEL_LIST` or
    uses the global `fetch` is wrong.

## Extending it

**Add a deployment**: a row in `MODEL_LIST` (or in `MODEL_LIST_JSON`) with `model_name` (the
group clients ask for), `model` (`openai/…` or `anthropic/…`), `api_key_env`, and the two
per-token costs (add the model to `PRICES` if it is new). A second row with the same
`model_name` is tried second. Test: extend the `list` in `app.test.ts` and assert
`x-litellm-model-id`. No migration.

**Add a provider** (say Gemini):
1. `providers.ts` — a `Provider` with `complete` and `stream`, mapping `ChatRequest` to the
   provider's request and its response back to `ChatResponse`/`Chunk`. Reuse `sse()` for
   the body and `ProviderError` for non-2xx.
2. `providerOf()` — recognise the `gemini/` prefix; return type widens.
3. `app.ts` — the `provider =` selection becomes a lookup by `providerOf`.
4. `app.test.ts` — a branch in `fakeFetch` for the provider's host that asserts the mapped
   request and answers in the provider's shape; a test that the answer comes back as OpenAI's.
5. `PRICES` and a `MODEL_LIST` row.

**Add a fallback**: `FALLBACKS[group] = [other groups…]`. Order matters. Test: mark the first
group's fake as down (`openaiDown` is the pattern) and assert the model header.

**Add an endpoint** (`/v1/embeddings`): the middleware already covers `/v1/*`; add a route in
the chain, a `Provider.embed()` method for each provider, and log spend the same way with
`completion_tokens: 0`. Add the schema to the OpenAI types in `providers.ts`.

**Enforce `rpm_limit`**: `store.hit(token, now)` like the api pack (sliding window), checked
in the middleware after the budget; 429 with `type: "rate_limit_error"`. Per replica in
memory, Redis when there are two.

**Key management routes** (`/key/info`, `/key/delete`, `/key/update` in LiteLLM): add `Store`
methods and master-only routes; `virtual_keys` already has the columns. Test each with a
generated key.

**Tool calling end to end**: the mapping exists in `toAnthropic` and the response mapper;
add a test whose fake Anthropic returns a `tool_use` block and assert
`choices[0].message.tool_calls[0].function.arguments` is a JSON string and
`finish_reason === "tool_calls"`.

## Operating it

| Variable | Required | Meaning |
|---|---|---|
| `MASTER_KEY` or `LITELLM_MASTER_KEY` | yes | Mints keys; bypasses allowlists and budgets; reads all spend |
| `OPENAI_API_KEY` | per deployment | Named by `api_key_env` in `MODEL_LIST`; unset deployments are skipped |
| `ANTHROPIC_API_KEY` | per deployment | Same |
| `MODEL_LIST_JSON` | no | JSON array of deployments replacing the built-in `MODEL_LIST`; read once at startup |
| `DATABASE_URL` | yes (prod) | Keys and spend |
| `PORT` | no | Default `8000` |
| `API_URL` | frontend | Where the page fetches `/spend/logs` |

**Scaling.** Stateless: every replica reads keys and writes spend in Postgres. The budget
check is read-then-call, so N concurrent requests on a nearly exhausted key can all pass;
acceptable for a budget, not for a hard cap. Upstream connections are per request; there is no
connection pool to size beyond Node's default agent.

**Failure modes.**
- Upstream 5xx/timeout: next deployment in the list, then the next fallback group; the client
  sees the last error's status when all fail, and one `failure` row is logged.
- Upstream 4xx (bad request, context too long): also falls through — a malformed request is
  re-sent to every deployment before the 4xx reaches the client. See Ceilings.
- Provider key unset: that deployment is skipped with `X is not set` as the last error.
- Client disconnects mid-stream: the `streamSSE` callback aborts and `record()` never runs, so
  that call's tokens are not billed to the key.
- Postgres down: readiness still says `db: "ok"`; `/v1/*` fails at the key lookup with a 500.

**What to watch.** `spend_logs`: `sum(spend)` per `api_key` per day; `status = 'failure'`
rate per `model_group`; `model != model_group`'s primary deployment as a fallback-rate signal;
p95 `duration_ms` per provider. Alert on failure rate and on a key's spend velocity, not on CPU.

## Ceilings

- **`rpm_limit` is stored and not enforced.** Upgrade: sliding window in the middleware
  (recipe above).
- **Budget overshoot by one call per concurrent request.** Upgrade: `addSpend` returning the
  new total and refusing when over, or a reservation before the call.
- **Fallback retries on every error, including 4xx.** Upgrade: only fall through on
  `ProviderError.status >= 500`, 429, or network errors; return 4xx immediately.
- **Allowlists do not constrain fallbacks.** Upgrade: filter `groups` by `k.models` before
  building `deployments`.
- **Streaming spend is lost on client disconnect.** Upgrade: record in a `finally` with the
  usage seen so far, or count tokens locally.
- **No upstream timeout.** `fetchImpl` is called without `AbortSignal.timeout`. Upgrade: wrap
  with a deadline per deployment shorter than the client's.
- **`PRICES` is a static table**; a new model with no entry costs 0. Upgrade: refuse to route
  a deployment with no price, or load LiteLLM's `model_prices_and_context_window.json`.
- **No `/v1/embeddings`, `/v1/completions`, images, audio.** Chat only.
- **No key lifecycle routes** (info, update, delete, list) and no teams/users/orgs.
- **`MODEL_LIST_JSON` is read at import**; a change needs a restart.
- **Master key compared with `!==`**, not constant time; fine for a long random key, worth
  `timingSafeEqual` if the key ever becomes guessable.
- **Anthropic streaming tool-call deltas carry `id: ""` and no `index`**; SDKs that
  reassemble arguments by index will need the index added to `Chunk`.

The stack rules — gate, typed seam, production checklist, deploy — are in `docs/PRODUCTION.md`. They apply.
"##;

const LLM_AGENTS: &str = r##"# {{NAME}} — for agents

`CLAUDE.md` has the rules. This is how to run and test the proxy.

## Run

    make demo                     # postgres + redis, migrate, seed, proxy :8000, spend page :3000
    make check                    # the gate: typecheck both halves, bun test in backend/
    cd backend && bun test        # the proxy tests alone; no network, no database

`backend/.env` needs `MASTER_KEY=sk-master-…` and at least one of `OPENAI_API_KEY` /
`ANTHROPIC_API_KEY`. A deployment whose key is unset is skipped, not an error.

## Every route, by hand

Mint a virtual key (plaintext returned once):

    curl -s -X POST localhost:8000/key/generate \
      -H "authorization: Bearer $MASTER_KEY" -H "content-type: application/json" \
      -d '{"key_alias":"team-a","models":["claude","fast"],"max_budget":5}'
    # {"key":"sk-9a1f…","models":["claude","fast"],"max_budget":5,"spend":0,"key_alias":"team-a"}
    export VKEY=sk-9a1f…

What this key may use:

    curl -s localhost:8000/v1/models -H "authorization: Bearer $VKEY"
    # {"object":"list","data":[{"id":"claude","object":"model",…},{"id":"fast",…}]}

A completion (Anthropic behind an OpenAI request):

    curl -si -X POST localhost:8000/v1/chat/completions \
      -H "authorization: Bearer $VKEY" -H "content-type: application/json" \
      -d '{"model":"claude","messages":[{"role":"system","content":"be brief"},{"role":"user","content":"hi"}]}'
    # x-litellm-model-id: anthropic/claude-sonnet-4-20250514
    # x-litellm-call-id: chatcmpl-…
    # x-litellm-response-cost: 0.000066
    # {"id":"chatcmpl-…","object":"chat.completion","model":"claude","choices":[{"index":0,"message":{"role":"assistant","content":"Hello."},"finish_reason":"stop"}],"usage":{"prompt_tokens":12,"completion_tokens":3,"total_tokens":15}}

Streaming:

    curl -sN -X POST localhost:8000/v1/chat/completions -H "authorization: Bearer $VKEY" \
      -H "content-type: application/json" \
      -d '{"model":"fast","messages":[{"role":"user","content":"count to 3"}],"stream":true}'
    # data: {"id":"chatcmpl-…","object":"chat.completion.chunk","choices":[{"index":0,"delta":{"role":"assistant"},"finish_reason":null}]}
    # data: {…"delta":{"content":"1"}…}
    # …
    # data: {…"finish_reason":"stop"…,"usage":{"prompt_tokens":11,"completion_tokens":7,"total_tokens":18}}
    # data: [DONE]

A model the key is not allowed:

    curl -s -X POST localhost:8000/v1/chat/completions -H "authorization: Bearer $VKEY" \
      -H "content-type: application/json" -d '{"model":"gpt-4o","messages":[]}'
    # 401 {"error":{"message":"key not allowed to use model gpt-4o","type":"auth_error","param":null,"code":401}}

Over budget (on the request after the one that crossed it):

    # 400 {"error":{"message":"ExceededBudget: spend 5.0012 >= max_budget 5","type":"budget_exceeded","param":null,"code":400}}

Tool calling (mapped to Anthropic `tool_use` and back):

    curl -s -X POST localhost:8000/v1/chat/completions -H "authorization: Bearer $VKEY" \
      -H "content-type: application/json" -d '{"model":"claude","messages":[{"role":"user","content":"weather in Oslo?"}],
        "tools":[{"type":"function","function":{"name":"weather","parameters":{"type":"object","properties":{"city":{"type":"string"}}}}}]}'
    # …"message":{"role":"assistant","content":null,"tool_calls":[{"id":"toolu_…","type":"function","function":{"name":"weather","arguments":"{\"city\":\"Oslo\"}"}}]},"finish_reason":"tool_calls"…

Spend (own rows with a virtual key, everyone's with the master):

    curl -s localhost:8000/spend/logs -H "authorization: Bearer $MASTER_KEY" | jq '.[0]'
    # {"request_id":"chatcmpl-…","api_key":"<sha256>","model":"anthropic/claude-sonnet-4-20250514","model_group":"claude","provider":"anthropic","prompt_tokens":12,"completion_tokens":3,"spend":0.000066,"duration_ms":812,"status":"success",…}

Point an SDK at it:

    OPENAI_BASE_URL=http://localhost:8000/v1 OPENAI_API_KEY=$VKEY python -c 'import openai; print(openai.OpenAI().chat.completions.create(model="claude", messages=[{"role":"user","content":"hi"}]).choices[0].message.content)'

Health: `curl localhost:8000/api/health` and `/api/health/ready`.

## How the tests work

`app.test.ts` calls `createApp(memoryStore(), fakeFetch, list)`:

- `memoryStore()` holds keys and spend rows in maps, so budgets and logs are asserted without
  Postgres.
- `fakeFetch` is the entire upstream: it branches on the URL host. The OpenAI branch answers
  in OpenAI's shape (JSON or a hand-built SSE `ReadableStream`), and flips to 503 when
  `openaiDown` is set. The Anthropic branch **asserts on the mapped request** (`system`,
  `messages[0]`, `max_tokens`) before answering in Anthropic's shape — the mapping is tested
  from the provider's side of the wire.
- `list` is a two-row router table with round costs (`1e-6`/`2e-6`, `3e-6`/`15e-6`) so cost
  assertions are exact.
- `process.env.MASTER_KEY` and the two provider keys are set at the top; the values are
  never sent anywhere real.

## Adding a test

    test("a 4xx from the first deployment still reaches the client", async () => {
      const bad: Fetch = async () => new Response('{"error":{"message":"context too long"}}', { status: 400 });
      const app = createApp(memoryStore(), bad, list);
      const res = await app.request("/v1/chat/completions", { method: "POST", headers: { authorization: "Bearer sk-master", "content-type": "application/json" }, body: JSON.stringify({ model: "gpt-4o", messages: [] }) });
      expect(res.status).toBe(400);
    });

Build a new `createApp` when the test needs its own fetch; share the module-level `app` when
it only needs the default fakes. Assert on the headers (`x-litellm-*`) to see which deployment
answered.

## Migrations

`backend/migrations/0003_name.sql`, `make migrate` locally; the `migrate` init container in
the cluster. `virtual_keys` and `spend_logs` are append-mostly; add columns nullable.
"##;

const LLM_README: &str = r##"# {{NAME}}

An OpenAI-compatible proxy in front of OpenAI and Anthropic — virtual keys, budgets, per-call
spend, fallbacks — the core of LiteLLM's proxy as one TypeScript service you can read and own.

## What you get

- `POST /v1/chat/completions` with OpenAI's request and response shape; any OpenAI SDK works by changing `base_url`.
- Anthropic behind the same endpoint: system prompts, tool calls and tool results mapped both ways; streaming re-emitted as OpenAI chunks ending in `[DONE]`.
- `POST /key/generate` under a master key: `sk-…` virtual keys with a model allowlist, a budget and an alias; only the hash is stored.
- A spend row per call — tokens, cost from a price table, latency, which deployment answered — and `x-litellm-response-cost` on the response.
- `MODEL_LIST`: a model group can have several deployments tried in order; `FALLBACKS` routes a dead group to another.
- Provider keys stay in the environment of this process; clients never see them.
- Tests that cover the mapping, fallback, streaming and budgets against a fake `fetch` — no network, no database.

## Five minutes

    cat >> backend/.env <<'E'
    MASTER_KEY=sk-master-change-me
    ANTHROPIC_API_KEY=sk-ant-…
    E
    make demo

Then:

    VKEY=$(curl -s -X POST localhost:8000/key/generate -H "authorization: Bearer sk-master-change-me" \
      -H "content-type: application/json" -d '{"models":["claude"],"max_budget":1}' | jq -r .key)

    curl -s -X POST localhost:8000/v1/chat/completions -H "authorization: Bearer $VKEY" \
      -H "content-type: application/json" \
      -d '{"model":"claude","messages":[{"role":"user","content":"one word: hello"}]}' | jq .choices[0].message.content
    # "Hello"

    curl -s localhost:8000/v1/models -H "authorization: Bearer $VKEY" | jq '.data[].id'
    # "claude"

    curl -s localhost:8000/spend/logs -H "authorization: Bearer sk-master-change-me" | jq '.[0] | {model, spend, duration_ms}'
    # {"model":"anthropic/claude-sonnet-4-20250514","spend":0.000081,"duration_ms":640}

Open http://localhost:3000 for the spend table.

## API

| Method | Path | Auth | What |
|---|---|---|---|
| GET | `/api/health` | none | Liveness |
| GET | `/api/health/ready` | none | Readiness |
| POST | `/key/generate` | master | `{models?, max_budget?, key_alias?, rpm_limit?}` → `{key, …}` |
| GET | `/v1/models` | key | Model groups this key may use |
| POST | `/v1/chat/completions` | key | OpenAI chat, `stream: true` for SSE |
| GET | `/spend/logs` | key | This key's last 200 calls (master: all keys) |

Errors are OpenAI's `{"error":{"message","type","param","code"}}`; `type` is one of
`auth_error`, `budget_exceeded`, `invalid_request_error`, `api_error`.

## Compared with LiteLLM

**Same shapes, so its docs and every OpenAI SDK apply**

- `/v1/chat/completions`, `/v1/models`, `/key/generate`, `/spend/logs` — the same paths and bodies.
- `model_list` semantics: `model_name` is the public group, `model` is `provider/name`, repeated groups are ordered deployments; `fallbacks` by group.
- `x-litellm-model-id`, `x-litellm-call-id`, `x-litellm-response-cost` headers.
- `ExceededBudget` / `budget_exceeded` on a spent key; empty `models` means any model.
- The Anthropic ↔ OpenAI mapping rules (system lifting, `tool_use`/`tool_result`, `stop_reason` renames, `max_tokens` default).

**Better here**

- ~450 lines of TypeScript, two providers, one store; the routing logic is code you can step through, not a YAML schema with hundreds of keys.
- Typed end to end: the request/response types are in `providers.ts` and the frontend client is built from the route table.
- Tests run in-process against a fake upstream that asserts on the mapped request, so a mapping regression fails `make check` rather than a customer.
- Fallback decisions happen before response headers are sent, including for streams.
- Ships with Dockerfile, compose, kustomize overlays, migrations and CI; LiteLLM gives you a container and a Postgres URL.

**Not here yet**

- Two providers (OpenAI, Anthropic). LiteLLM has 100+, plus Bedrock/Vertex/Azure auth flows.
- Chat only: no `/v1/embeddings`, `/v1/completions`, images, audio, batches, files, assistants.
- `rpm_limit` is accepted and stored but not enforced; no TPM limits, no per-key concurrency.
- No teams, users, organisations, key expiry, or key info/update/delete routes.
- No caching (Redis/semantic), no load balancing strategies beyond ordered deployments, no cooldowns or health checks on deployments.
- No admin UI beyond the spend page; no Prometheus endpoint; no OpenTelemetry.
- A static price table; unknown models cost 0.
- Fallback re-sends on 4xx as well as 5xx.
- A key's allowlist does not constrain which fallback group serves it.
- No guardrails, prompt injection checks, or content moderation hooks.

## Production

- **Environments.** `MASTER_KEY`, provider keys named by `MODEL_LIST`, optional `MODEL_LIST_JSON`, `DATABASE_URL`, `PORT`; from `backend/.env` locally, from the `app-secrets` Secret in the cluster (`backend/.env.age` committed).
- **Scaling.** Stateless; HPA 2–5 pods on CPU. All state is in Postgres. Budgets are read-then-call, so treat them as soft under concurrency.
- **Probes.** `/api/health` liveness, `/api/health/ready` readiness (does not yet query Postgres).
- **Migrations.** `backend/migrations/*.sql` via the `migrate` init container.
- **Secrets.** Provider keys and the master key never appear in logs, responses or the image; rotate by re-rendering the Secret.
- **Overlay.** `kubectl apply -k k8s/overlays/prod`; `k8s/README.md` explains the manifests.
- **What pages you.** `status = 'failure'` rate per `model_group`; spend velocity per key; p95 `duration_ms` per provider; requests answered by a fallback deployment.

## Roadmap

- Enforce `rpm_limit`; add TPM.
- Fallback only on 5xx/429/network; constrain fallbacks by the key's allowlist.
- Upstream timeouts with `AbortSignal`.
- Record streaming spend on disconnect.
- Key info/update/delete; expiry.
- `/v1/embeddings`.
- More providers behind the `Provider` interface.
- Prices loaded from a file, refusing unknown models.
"##;

const LLM_REVIEWER_MAPPING: &str = r##"---
name: provider-mapping
description: "Run on any change to providers.ts, the Anthropic or OpenAI adapters, streaming, or the OpenAI types. Checks that what goes out to a provider is what its API requires and what comes back is byte-for-byte OpenAI's shape, the way an SDK maintainer would."
tools: Read, Grep, Glob
---

You maintain an OpenAI client SDK and your users are pointing it at this proxy. Read the change
as that person. Report only shape or semantic breaks, each as
`path:line — what — what a client sees — the fix`.

Check:
1. **Response envelope.** `id` starts with `chatcmpl-`, `object` is `chat.completion`,
   `created` is seconds, `model` is the requested group, `choices[0].message.role` is
   `assistant`, `usage` has all three token counts.
2. **`finish_reason` set.** Only `stop`, `length`, `tool_calls` come out of `finish()`; a new
   Anthropic `stop_reason` maps to one of them.
3. **System lifting.** All `system` messages are joined into `system`; none remain in
   `messages`; an empty system yields `undefined`, not `""`.
4. **Alternation.** Consecutive same-role string turns are merged; a `tool` message becomes a
   `user` turn with `tool_result`; an assistant turn with `tool_calls` becomes `tool_use`
   blocks with `input` parsed from `arguments` (`"{}"` when empty).
5. **Tools.** `function.parameters` becomes `input_schema`, defaulting to
   `{type:"object"}`; `tool_choice` maps `auto/required/none/{function}` to
   `auto/any/none/tool`.
6. **`max_tokens`** is always sent to Anthropic (default 4096); `stop` becomes
   `stop_sequences` as an array.
7. **Streaming chunks.** `object` is `chat.completion.chunk`; the first chunk carries
   `delta.role`; text is `delta.content`; tool-call deltas are `delta.tool_calls[]`; the
   final chunk carries `finish_reason` and `usage`; the stream ends with `data: [DONE]` and
   nothing after.
8. **SSE parsing.** `sse()` splits on `\n`, handles `data:` with and without a space, and
   buffers partial lines across reads; `[DONE]` from OpenAI is not forwarded as JSON.
9. **Errors.** Non-2xx upstream throws `ProviderError(status, body)`; the body is truncated
   to 400 chars and never includes the upstream key; `res.body` null on a stream throws.
10. **Headers.** OpenAI: `authorization: Bearer`; Anthropic: `x-api-key` +
    `anthropic-version: 2023-06-01`. `api_base` overrides the host without a trailing slash
    doubling.
11. **Model unwrapping.** `bare()` strips exactly one `provider/` prefix; a model name with
    a `/` of its own survives.
12. **Fallback safety.** For streams, the first `it.next()` is awaited before `streamSSE` so
    an upstream error still triggers the next deployment; a change that starts writing
    before the first chunk breaks fallback.
13. **Test parity.** `fakeFetch` in `app.test.ts` asserts on the mapped request for any field
    whose mapping changed.

End with one line: `provider-mapping: N findings`, and if 0, what you checked.
"##;

const LLM_REVIEWER_SPEND: &str = r##"---
name: key-and-spend
description: "Run on any change to app.ts middleware, /key/generate, budgets, the spend log, MODEL_LIST/FALLBACKS or store.ts. Checks who can spend whose money on which model, the way the person who gets the provider invoice would."
tools: Read, Grep, Glob, Bash
---

You pay the OpenAI and Anthropic bills for this proxy. Read the change as that person. Report
only what lets spend go unattributed, unbounded, or to the wrong model, each as
`path:line — what — how it costs money — the fix`.

Check:
1. **Master key gate.** `/key/generate` compares the full `Bearer <master>` header; an unset
   master refuses (`!master ||`). No new route mints or lists keys without it.
2. **Hashing.** `store.key(sha(raw))` is the only lookup; `createKey` receives a hash; the
   plaintext appears only in the `/key/generate` response.
3. **Budget check order.** Auth → budget (`spend >= max_budget`) → allowlist → route. Master
   bypasses budget and allowlist and is logged as `api_key: "master"`.
4. **Allowlist.** `k.models.length && !k.models.includes(req.model)` is 401; empty means any.
   `/v1/models` filters the same way. Note: fallbacks are not filtered — flag any change that
   widens `FALLBACKS` for a group with restricted keys.
5. **Every success logs once.** `record()` runs exactly once per completed call (non-stream:
   before the response; stream: after `[DONE]`), writes `spend_logs` and then `addSpend` for
   non-master keys, and `x-litellm-response-cost` matches `cost(d, usage)`.
6. **Every total failure logs once**, with `status: "failure"`, zero tokens, the first
   deployment's model, and the last error's status.
7. **Cost inputs.** `cost()` uses the deployment's per-token prices, not `PRICES` directly; a
   new `MODEL_LIST` row has both costs and a `PRICES` entry; `MODEL_LIST_JSON` rows are
   validated for the same fields (they are not today — flag any route that trusts them).
8. **Usage source.** Non-stream: the provider's `usage`; stream: the last chunk carrying
   `usage` (OpenAI needs `stream_options.include_usage`; Anthropic's is built from
   `message_start` + `message_delta`). A provider change that drops usage bills zero.
9. **Provider key isolation.** `process.env[d.api_key_env]` is read at call time and passed
   only to the provider; no log line, error message or response includes it. `ProviderError`
   bodies are upstream text, not our request.
10. **Fallback cost.** A fallback to a pricier group is legitimate but visible: the log row
    records `model_group` (asked) and `model` (served). A change that logs only one of them
    hides the bill.
11. **`/spend/logs` scoping.** A virtual key sees only `api_key = sha(token)`; master sees all;
    no key sees another's rows; the limit stays bounded (200).
12. **Concurrency.** `addSpend` is `spend = spend + $1` in SQL (atomic); the budget check is
    read-then-call and can overshoot by one call per concurrent request — say so if a change
    makes budgets a hard guarantee.
13. **Timeouts.** Upstream calls have no deadline; a change that adds retries without one
    multiplies spend on slow providers.

End with one line: `key-and-spend: N findings`, and if 0, what you checked.
"##;
