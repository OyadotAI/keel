//! mcpgateway: one MCP endpoint over many servers, like mcp-proxy / Metorial ═════════════

pub fn files(name: &str) -> Vec<(&'static str, String)> {
    let f = |s: &str| s.replace("{{NAME}}", name);
    let _ = &f;
    vec![
        ("backend/src/app.ts", MCP_APP.into()),
        ("backend/src/store.ts", MCP_STORE.into()),
        ("backend/src/upstream.ts", MCP_UPSTREAM.into()),
        ("backend/src/app.test.ts", MCP_TEST.into()),
        ("backend/migrations/0002_mcpgateway.sql", MCP_SQL.into()),
        ("frontend/app/page.tsx", f(MCP_PAGE)),
        ("CLAUDE.md", f(MCP_CLAUDE)),
        ("AGENTS.md", f(MCP_AGENTS)),
        ("README.md", f(MCP_README)),
        (
            ".claude/agents/protocol-conformance.md",
            MCP_REV_PROTOCOL.into(),
        ),
        (".claude/agents/tool-scoping.md", MCP_REV_SCOPING.into()),
    ]
}

const MCP_APP: &str = r##"import { Hono } from "hono";
import { z } from "zod";
import { createHash, randomBytes } from "node:crypto";
import { pgStore, type Server, type Store, type Tool } from "./store";
import { callTool, forgetSession, listTools, PROTOCOL, UpstreamError, type Fetch } from "./upstream";

// mcp-proxy / Metorial's shape: one MCP endpoint (streamable HTTP, spec 2025-06-18) at /mcp
// that aggregates upstream servers from a registry. Tools are namespaced <server>__<tool>;
// a caller's scopes ("github:*", "db:query") decide what tools/list shows and tools/call
// accepts; schemas are cached per server and a server that stops answering has its tools
// hidden until it is healthy again. Every call is audited with an args hash — never the args.

const sha = (s: string) => createHash("sha256").update(s).digest("hex");
export const MAX_ARGS_BYTES = 64 * 1024, MAX_RESULT_BYTES = 512 * 1024, CALL_TIMEOUT_MS = 30_000, LIST_TIMEOUT_MS = 10_000, SCHEMA_TTL_MS = 60_000;
export const SUPPORTED = ["2025-06-18", "2025-03-26"];
const SEP = "__";

const rpcErr = (id: unknown, code: number, message: string, data?: unknown) => ({ jsonrpc: "2.0", id: id ?? null, error: { code, message, ...(data !== undefined ? { data } : {}) } });
const rpcOk = (id: unknown, result: unknown) => ({ jsonrpc: "2.0", id, result });

export function allowed(scopes: string[], server: string, tool: string): boolean {
  return scopes.includes(`${server}:*`) || scopes.includes(`${server}:${tool}`) || scopes.includes("*");
}

const rpcSchema = z.object({ jsonrpc: z.literal("2.0"), id: z.union([z.string(), z.number()]).optional(), method: z.string(), params: z.unknown().optional() });

export function createApp(store: Store, fetchImpl: Fetch = fetch) {
  const adminOk = (auth: string | undefined) => { const t = process.env.ADMIN_TOKEN; return !!t && auth === `Bearer ${t}`; };
  const unauthorized = { message: "admin token required" };

  /// Refresh a server's tool cache. Failure marks it unhealthy — its tools disappear from the
  /// aggregate list — and the cached schemas are kept for the console to show what was there.
  const refresh = async (s: Server): Promise<Server> => {
    try {
      const tools = await listTools(s, fetchImpl, LIST_TIMEOUT_MS);
      await store.setHealth(s.name, true, tools, null);
      return { ...s, healthy: true, tools, error: null, checked_at: new Date().toISOString() };
    } catch (e) {
      forgetSession(s.url);
      await store.setHealth(s.name, false, s.tools, (e as Error).message);
      return { ...s, healthy: false, error: (e as Error).message, checked_at: new Date().toISOString() };
    }
  };
  const fresh = async (): Promise<Server[]> => Promise.all((await store.servers()).map((s) => (!s.checked_at || Date.now() - Date.parse(s.checked_at) > SCHEMA_TTL_MS ? refresh(s) : s)));

  const app = new Hono()
    .get("/api/health", (c) => c.json({ status: "ok" }))
    .get("/api/health/ready", (c) => c.json({ status: "ok", db: "ok" as const }))

    // ── admin: servers, callers, audit ──────────────────────────────────────────────────
    .get("/api/admin/servers", async (c) => {
      if (!adminOk(c.req.header("authorization"))) return c.json(unauthorized, 401);
      return c.json({ data: await store.servers() });
    })
    .post("/api/admin/servers", async (c) => {
      if (!adminOk(c.req.header("authorization"))) return c.json(unauthorized, 401);
      const p = z.object({ name: z.string().regex(/^[a-z0-9-]+$/), url: z.string().url(), auth_env: z.string().regex(/^[A-Z0-9_]+$/).nullable().default(null) }).safeParse(await c.req.json().catch(() => ({})));
      if (!p.success) return c.json({ message: "invalid server", details: p.error.flatten() }, 400);
      await store.putServer({ ...p.data, healthy: true, tools: [], error: null, checked_at: null });
      const s = await refresh({ ...p.data, healthy: true, tools: [], error: null, checked_at: null });
      return c.json(s, 201);
    })
    .delete("/api/admin/servers/:name", async (c) => {
      if (!adminOk(c.req.header("authorization"))) return c.json(unauthorized, 401);
      return (await store.deleteServer(c.req.param("name"))) ? c.body(null, 204) : c.json({ message: "not found" }, 404);
    })
    .post("/api/admin/callers", async (c) => {
      if (!adminOk(c.req.header("authorization"))) return c.json(unauthorized, 401);
      const p = z.object({ name: z.string().min(1), scopes: z.array(z.string().regex(/^(\*|[a-z0-9-]+:(\*|[\w.-]+))$/)).default([]) }).safeParse(await c.req.json().catch(() => ({})));
      if (!p.success) return c.json({ message: "invalid caller", details: p.error.flatten() }, 400);
      const raw = "mcp_" + randomBytes(24).toString("hex");
      const caller = await store.createCaller(p.data.name, sha(raw), p.data.scopes);
      // The plaintext exists exactly once, in this response.
      return c.json({ id: caller.id, name: caller.name, key: raw, scopes: caller.scopes }, 201);
    })
    .get("/api/admin/audit", async (c) => {
      if (!adminOk(c.req.header("authorization"))) return c.json(unauthorized, 401);
      return c.json({ data: await store.audits(Math.min(500, Number(c.req.query("limit") ?? 100))) });
    })

    // ── the MCP endpoint ────────────────────────────────────────────────────────────────
    // No server-initiated messages, so GET has no stream to offer (405, per spec) and sessions
    // are stateless: a session id is issued so clients that expect one are happy.
    .get("/mcp", (c) => c.text("Method Not Allowed", 405))
    .delete("/mcp", (c) => c.body(null, 204))
    .post("/mcp", async (c) => {
      const raw = c.req.header("authorization")?.replace(/^Bearer /, "");
      const caller = raw ? await store.callerByHash(sha(raw)) : null;
      if (!caller) return c.json(rpcErr(null, -32001, "unauthorized: a caller key is required"), 401);
      const pv = c.req.header("mcp-protocol-version");
      if (pv && !SUPPORTED.includes(pv)) return c.json(rpcErr(null, -32600, `unsupported MCP-Protocol-Version ${pv}`, { supported: SUPPORTED }), 400);
      const text = await c.req.text();
      if (text.length > MAX_ARGS_BYTES) return c.json(rpcErr(null, -32600, `request over ${MAX_ARGS_BYTES} bytes`), 413);
      const parsed = rpcSchema.safeParse(JSON.parse(text || "null"));
      if (!parsed.success) return c.json(rpcErr(null, -32600, "invalid JSON-RPC request"), 400);
      const { id, method, params } = parsed.data;
      if (id === undefined) return c.body(null, 202); // a notification; accepted, nothing to say

      switch (method) {
        case "initialize": {
          const p = (params ?? {}) as { protocolVersion?: string };
          const version = SUPPORTED.includes(p.protocolVersion ?? "") ? p.protocolVersion! : PROTOCOL;
          c.header("Mcp-Session-Id", crypto.randomUUID());
          return c.json(rpcOk(id, { protocolVersion: version, capabilities: { tools: { listChanged: false } }, serverInfo: { name: "mcp-gateway", version: "1.0.0" }, instructions: `Tools are named <server>${SEP}<tool>. You see only what your key is scoped to.` }));
        }
        case "ping": return c.json(rpcOk(id, {}));
        case "tools/list": {
          const tools: Tool[] = [];
          for (const s of await fresh()) {
            if (!s.healthy) continue;
            for (const t of s.tools) if (allowed(caller.scopes, s.name, t.name)) tools.push({ ...t, name: `${s.name}${SEP}${t.name}` });
          }
          return c.json(rpcOk(id, { tools }));
        }
        case "tools/call": {
          const p = z.object({ name: z.string(), arguments: z.record(z.unknown()).optional() }).safeParse(params);
          if (!p.success) return c.json(rpcErr(id, -32602, "params must be { name, arguments? }"));
          const [serverName, ...rest] = p.data.name.split(SEP);
          const toolName = rest.join(SEP);
          const server = (await fresh()).find((s) => s.name === serverName);
          const tool = server?.tools.find((t) => t.name === toolName);
          // Unknown and unscoped look the same from outside: no probing what exists.
          if (!server || !tool || !allowed(caller.scopes, server.name, toolName)) return c.json(rpcErr(id, -32602, `Unknown tool: ${p.data.name}`));
          if (!server.healthy) return c.json(rpcErr(id, -32603, `upstream ${server.name} is unhealthy: ${server.error ?? "unknown"}`));
          const argsJson = JSON.stringify(p.data.arguments ?? {});
          const argsHash = sha(argsJson);
          const required = ((tool.inputSchema as { required?: string[] } | undefined)?.required ?? []).filter((r) => !(r in (p.data.arguments ?? {})));
          if (required.length) return c.json(rpcErr(id, -32602, `missing required arguments: ${required.join(", ")}`));
          const started = Date.now();
          try {
            const result = await callTool(server, fetchImpl, toolName, p.data.arguments, CALL_TIMEOUT_MS);
            const bytes = Buffer.byteLength(JSON.stringify(result));
            if (bytes > MAX_RESULT_BYTES) {
              await store.audit({ caller: caller.name, server: server.name, tool: toolName, args_hash: argsHash, result_bytes: bytes, duration_ms: Date.now() - started, ok: false, error: "result too large" });
              return c.json(rpcOk(id, { content: [{ type: "text", text: `result of ${bytes} bytes exceeds the gateway cap of ${MAX_RESULT_BYTES}` }], isError: true }));
            }
            await store.audit({ caller: caller.name, server: server.name, tool: toolName, args_hash: argsHash, result_bytes: bytes, duration_ms: Date.now() - started, ok: !result.isError, error: result.isError ? "tool error" : null });
            return c.json(rpcOk(id, result));
          } catch (e) {
            const msg = (e as Error).name === "TimeoutError" ? `timed out after ${CALL_TIMEOUT_MS}ms` : (e as Error).message;
            await store.audit({ caller: caller.name, server: server.name, tool: toolName, args_hash: argsHash, result_bytes: 0, duration_ms: Date.now() - started, ok: false, error: msg });
            if (e instanceof UpstreamError && e.rpc) return c.json(rpcErr(id, e.rpc.code, e.rpc.message, e.rpc.data));
            if (!(e instanceof UpstreamError && e.rpc)) { forgetSession(server.url); await store.setHealth(server.name, false, server.tools, msg); } // transport failure: hide it until it answers again
            return c.json(rpcOk(id, { content: [{ type: "text", text: `upstream ${server.name} failed: ${msg}` }], isError: true }));
          }
        }
        default: return c.json(rpcErr(id, -32601, `Method not found: ${method}`));
      }
    });

  return app;
}

const app = createApp(pgStore());
export type AppType = typeof app;
export default app;
"##;
const MCP_STORE: &str = r##"import { db } from "./db";

// The registry and the audit trail. Postgres in the process, memory in the tests.
export type Tool = { name: string; title?: string; description?: string; inputSchema: unknown; outputSchema?: unknown; annotations?: unknown };
export type Server = { name: string; url: string; auth_env: string | null; healthy: boolean; tools: Tool[]; error: string | null; checked_at: string | null };
export type Caller = { id: number; name: string; key_hash: string; scopes: string[] };
export type Audit = { caller: string; server: string; tool: string; args_hash: string; result_bytes: number; duration_ms: number; ok: boolean; error: string | null };

export interface Store {
  servers(): Promise<Server[]>;
  putServer(s: Server): Promise<void>;
  deleteServer(name: string): Promise<boolean>;
  setHealth(name: string, healthy: boolean, tools: Tool[], error: string | null): Promise<void>;
  callerByHash(hash: string): Promise<Caller | null>;
  createCaller(name: string, hash: string, scopes: string[]): Promise<Caller>;
  audit(a: Audit): Promise<void>;
  audits(limit: number): Promise<(Audit & { at: string })[]>;
}

export function pgStore(): Store {
  return {
    servers: async () => db<Server[]>`select name, url, auth_env, healthy, tools, error, checked_at from mcp_servers order by name`,
    putServer: async (s) => { await db`insert into mcp_servers (name, url, auth_env, healthy, tools, error, checked_at) values (${s.name}, ${s.url}, ${s.auth_env}, ${s.healthy}, ${db.json(s.tools as never)}, ${s.error}, ${s.checked_at}) on conflict (name) do update set url = excluded.url, auth_env = excluded.auth_env, healthy = excluded.healthy, tools = excluded.tools, error = excluded.error, checked_at = excluded.checked_at`; },
    deleteServer: async (name) => (await db`delete from mcp_servers where name = ${name}`).count > 0,
    setHealth: async (name, healthy, tools, error) => { await db`update mcp_servers set healthy = ${healthy}, tools = ${db.json(tools as never)}, error = ${error}, checked_at = now() where name = ${name}`; },
    callerByHash: async (h) => (await db<Caller[]>`select id, name, key_hash, scopes from mcp_callers where key_hash = ${h}`)[0] ?? null,
    createCaller: async (name, key_hash, scopes) => (await db<Caller[]>`insert into mcp_callers (name, key_hash, scopes) values (${name}, ${key_hash}, ${scopes}) returning id, name, key_hash, scopes`)[0],
    audit: async (a) => { await db`insert into mcp_audit ${db(a, "caller", "server", "tool", "args_hash", "result_bytes", "duration_ms", "ok", "error")}`; },
    audits: async (limit) => db<(Audit & { at: string })[]>`select caller, server, tool, args_hash, result_bytes, duration_ms, ok, error, at from mcp_audit order by at desc limit ${limit}`,
  };
}

export function memoryStore(): Store {
  const servers = new Map<string, Server>(), callers: Caller[] = [], audits: (Audit & { at: string })[] = [];
  return {
    servers: async () => [...servers.values()],
    putServer: async (s) => { servers.set(s.name, { ...s }); },
    deleteServer: async (name) => servers.delete(name),
    setHealth: async (name, healthy, tools, error) => { const s = servers.get(name); if (s) Object.assign(s, { healthy, tools, error, checked_at: new Date().toISOString() }); },
    callerByHash: async (h) => callers.find((c) => c.key_hash === h) ?? null,
    createCaller: async (name, key_hash, scopes) => { const c = { id: callers.length + 1, name, key_hash, scopes }; callers.push(c); return c; },
    audit: async (a) => { audits.push({ ...a, at: new Date().toISOString() }); },
    audits: async (limit) => audits.slice(-limit).reverse(),
  };
}
"##;
const MCP_UPSTREAM: &str = r##"import type { Server, Tool } from "./store";

// A minimal MCP client over streamable HTTP (spec 2025-06-18): every message is a POST with
// both Accept types; the answer is one JSON object or an SSE stream that carries the response.
// Sessions: the upstream may hand back Mcp-Session-Id at initialize; we keep it per server
// and re-initialize once on a 404, as the spec tells clients to.

export type Fetch = (url: string | URL, init?: RequestInit) => Promise<Response>;
export const PROTOCOL = "2025-06-18";
export type RpcError = { code: number; message: string; data?: unknown };
export type CallResult = { content: { type: string; text?: string; [k: string]: unknown }[]; structuredContent?: unknown; isError?: boolean };

export class UpstreamError extends Error { constructor(msg: string, public rpc?: RpcError) { super(msg); } }

// ponytail: per-process session map; a shared one belongs in Redis when replicas multiply.
const sessions = new Map<string, string>();
let nextId = 1;

async function post(s: Server, fetchImpl: Fetch, msg: unknown, timeoutMs: number): Promise<Response> {
  const headers: Record<string, string> = { "content-type": "application/json", accept: "application/json, text/event-stream", "mcp-protocol-version": PROTOCOL };
  const token = s.auth_env ? process.env[s.auth_env] : undefined;
  if (token) headers.authorization = `Bearer ${token}`;
  const sid = sessions.get(s.url);
  if (sid) headers["mcp-session-id"] = sid;
  return fetchImpl(s.url, { method: "POST", headers, body: JSON.stringify(msg), signal: AbortSignal.timeout(timeoutMs) });
}

/// Read one JSON-RPC response for `id` out of a JSON or SSE body.
async function readResponse(res: Response, id: number): Promise<{ result?: unknown; error?: RpcError }> {
  const ct = res.headers.get("content-type") ?? "";
  if (ct.includes("text/event-stream")) {
    for (const line of (await res.text()).split("\n")) {
      if (!line.startsWith("data:")) continue;
      const msg = JSON.parse(line.slice(5).trim());
      if (msg.id === id) return msg;
    }
    throw new UpstreamError("stream closed without a response");
  }
  return res.json();
}

async function request(s: Server, fetchImpl: Fetch, method: string, params: unknown, timeoutMs: number, retried = false): Promise<unknown> {
  if (method !== "initialize" && !sessions.has(s.url)) await initialize(s, fetchImpl, timeoutMs);
  const id = nextId++;
  const res = await post(s, fetchImpl, { jsonrpc: "2.0", id, method, params }, timeoutMs);
  if (res.status === 404 && !retried) { sessions.delete(s.url); return request(s, fetchImpl, method, params, timeoutMs, true); }
  if (!res.ok) throw new UpstreamError(`upstream ${res.status}`);
  if (method === "initialize") sessions.set(s.url, res.headers.get("mcp-session-id") ?? "");
  const msg = await readResponse(res, id);
  if (msg.error) throw new UpstreamError(msg.error.message, msg.error);
  return msg.result;
}

async function initialize(s: Server, fetchImpl: Fetch, timeoutMs: number) {
  await request(s, fetchImpl, "initialize", { protocolVersion: PROTOCOL, capabilities: {}, clientInfo: { name: "mcp-gateway", version: "1.0.0" } }, timeoutMs);
  await post(s, fetchImpl, { jsonrpc: "2.0", method: "notifications/initialized" }, timeoutMs);
}

export async function listTools(s: Server, fetchImpl: Fetch, timeoutMs: number): Promise<Tool[]> {
  const tools: Tool[] = [];
  let cursor: string | undefined;
  do {
    const r = (await request(s, fetchImpl, "tools/list", cursor ? { cursor } : {}, timeoutMs)) as { tools: Tool[]; nextCursor?: string };
    tools.push(...r.tools); cursor = r.nextCursor;
  } while (cursor);
  return tools;
}

export async function callTool(s: Server, fetchImpl: Fetch, name: string, args: unknown, timeoutMs: number): Promise<CallResult> {
  return (await request(s, fetchImpl, "tools/call", { name, arguments: args ?? {} }, timeoutMs)) as CallResult;
}

export function forgetSession(url: string) { sessions.delete(url); }
"##;
const MCP_TEST: &str = r##"import { beforeAll, describe, expect, test } from "bun:test";
import { Hono } from "hono";
import { createApp, MAX_ARGS_BYTES } from "./app";
import { memoryStore } from "./store";
import type { Fetch } from "./upstream";

// The gateway between a fake MCP client and a fake upstream MCP server (streamable HTTP, JSON
// answers with a session id). Covers the lifecycle shapes, scoping, forwarding with audit,
// the size caps, and hiding an upstream that stops answering.
process.env.ADMIN_TOKEN = "admin";

let echoDown = false, slow = false;
const upstreamCalls: { method: string; session: string | null; params: unknown }[] = [];
const fakeUpstream = new Hono().post("/mcp", async (c) => {
  if (echoDown) return c.text("gone", 502);
  const msg = await c.req.json();
  upstreamCalls.push({ method: msg.method, session: c.req.header("mcp-session-id") ?? null, params: msg.params });
  if (msg.id === undefined) return c.body(null, 202);
  if (msg.method === "initialize") { c.header("Mcp-Session-Id", "sess-1"); return c.json({ jsonrpc: "2.0", id: msg.id, result: { protocolVersion: "2025-06-18", capabilities: { tools: {} }, serverInfo: { name: "echo", version: "0" } } }); }
  if (c.req.header("mcp-session-id") !== "sess-1") return c.text("no session", 404);
  if (msg.method === "tools/list") return c.json({ jsonrpc: "2.0", id: msg.id, result: { tools: [
    { name: "echo", description: "Echo text", inputSchema: { type: "object", properties: { text: { type: "string" } }, required: ["text"] } },
    { name: "secret", description: "Admin only", inputSchema: { type: "object" } },
  ] } });
  if (msg.method === "tools/call") {
    if (slow) await new Promise((_, rej) => c.req.raw.signal.addEventListener("abort", () => rej(Object.assign(new Error("aborted"), { name: "TimeoutError" }))));
    if (msg.params.name === "secret") return c.json({ jsonrpc: "2.0", id: msg.id, result: { content: [{ type: "text", text: "s3cret" }] } });
    // Answer over SSE, the other shape the transport allows.
    return c.body(`event: message\ndata: ${JSON.stringify({ jsonrpc: "2.0", id: msg.id, result: { content: [{ type: "text", text: msg.params.arguments.text }] } })}\n\n`, 200, { "content-type": "text/event-stream" });
  }
  return c.json({ jsonrpc: "2.0", id: msg.id, error: { code: -32601, message: "Method not found" } });
});
const fetchImpl: Fetch = async (url, init) => fakeUpstream.request(String(url), init);

const app = createApp(memoryStore(), fetchImpl);
const adminPost = (path: string, body: unknown) => app.request(path, { method: "POST", headers: { authorization: "Bearer admin", "content-type": "application/json" }, body: JSON.stringify(body) });
let key = "", adminKey = "", rpcId = 0;
const rpc = (k: string, method: string, params?: unknown) => app.request("/mcp", { method: "POST", headers: { authorization: `Bearer ${k}`, "content-type": "application/json", accept: "application/json, text/event-stream", "mcp-protocol-version": "2025-06-18" }, body: JSON.stringify({ jsonrpc: "2.0", id: ++rpcId, method, params }) });

beforeAll(async () => {
  const s = await adminPost("/api/admin/servers", { name: "echo", url: "http://echo.internal/mcp", auth_env: "ECHO_TOKEN" });
  expect(s.status).toBe(201);
  expect((await s.json()).tools.map((t: { name: string }) => t.name)).toEqual(["echo", "secret"]);
  key = (await (await adminPost("/api/admin/callers", { name: "agent", scopes: ["echo:echo"] })).json()).key;
  adminKey = (await (await adminPost("/api/admin/callers", { name: "ops", scopes: ["echo:*"] })).json()).key;
});

describe("mcp gateway", () => {
  test("initialize answers the spec shape with a session id; notifications are 202; GET is 405", async () => {
    expect((await app.request("/mcp", { method: "POST", body: "{}" })).status).toBe(401);
    const res = await rpc(key, "initialize", { protocolVersion: "2025-06-18", capabilities: {}, clientInfo: { name: "test", version: "1" } });
    expect(res.status).toBe(200);
    expect(res.headers.get("mcp-session-id")).not.toBeNull();
    const body = await res.json();
    expect(body).toMatchObject({ jsonrpc: "2.0", id: rpcId, result: { protocolVersion: "2025-06-18", capabilities: { tools: { listChanged: false } }, serverInfo: { name: "mcp-gateway" } } });
    const note = await app.request("/mcp", { method: "POST", headers: { authorization: `Bearer ${key}` }, body: JSON.stringify({ jsonrpc: "2.0", method: "notifications/initialized" }) });
    expect(note.status).toBe(202);
    expect((await app.request("/mcp")).status).toBe(405);
    expect((await (await rpc(key, "resources/list")).json()).error.code).toBe(-32601);
  });

  test("tools/list is namespaced and filtered by scope", async () => {
    const mine = (await (await rpc(key, "tools/list")).json()).result.tools;
    expect(mine.map((t: { name: string }) => t.name)).toEqual(["echo__echo"]);
    expect(mine[0].inputSchema.required).toEqual(["text"]);
    const all = (await (await rpc(adminKey, "tools/list")).json()).result.tools.map((t: { name: string }) => t.name);
    expect(all).toEqual(["echo__echo", "echo__secret"]);
  });

  test("tools/call is forwarded (SSE answer read), upstream session reused, and audited", async () => {
    upstreamCalls.length = 0;
    const res = await (await rpc(key, "tools/call", { name: "echo__echo", arguments: { text: "hello" } })).json();
    expect(res.result).toEqual({ content: [{ type: "text", text: "hello" }] });
    expect(upstreamCalls.map((u) => u.method)).toEqual(["tools/call"]); // initialized once, at registration
    expect(upstreamCalls[0].session).toBe("sess-1");
    const audit = (await (await app.request("/api/admin/audit", { headers: { authorization: "Bearer admin" } })).json()).data[0];
    expect(audit).toMatchObject({ caller: "agent", server: "echo", tool: "echo", ok: true });
    expect(audit.args_hash).toHaveLength(64);
    expect(audit.result_bytes).toBeGreaterThan(0);
    expect(JSON.stringify(audit)).not.toContain("hello");
  });

  test("out-of-scope and unknown tools are the same -32602; required arguments are checked", async () => {
    expect((await (await rpc(key, "tools/call", { name: "echo__secret", arguments: {} })).json()).error.code).toBe(-32602);
    expect((await (await rpc(key, "tools/call", { name: "nope__x", arguments: {} })).json()).error.code).toBe(-32602);
    const missing = (await (await rpc(key, "tools/call", { name: "echo__echo", arguments: {} })).json()).error;
    expect(missing.code).toBe(-32602);
    expect(missing.message).toContain("text");
    expect((await (await rpc(adminKey, "tools/call", { name: "echo__secret", arguments: {} })).json()).result.content[0].text).toBe("s3cret");
  });

  test("oversized arguments are refused before forwarding", async () => {
    upstreamCalls.length = 0;
    const res = await rpc(key, "tools/call", { name: "echo__echo", arguments: { text: "x".repeat(MAX_ARGS_BYTES) } });
    expect(res.status).toBe(413);
    expect(upstreamCalls.length).toBe(0);
  });

  test("an upstream that stops answering has its tools hidden until it recovers", async () => {
    echoDown = true;
    const res = await (await rpc(key, "tools/call", { name: "echo__echo", arguments: { text: "?" } })).json();
    expect(res.result.isError).toBe(true);
    expect((await (await rpc(key, "tools/list")).json()).result.tools).toEqual([]);
    const servers = (await (await app.request("/api/admin/servers", { headers: { authorization: "Bearer admin" } })).json()).data;
    expect(servers[0].healthy).toBe(false);
    expect(servers[0].tools.length).toBe(2); // schemas kept for the console
    echoDown = false;
    // Recovery: the next refresh re-initializes (the old session is forgotten) and lists again.
    await adminPost("/api/admin/servers", { name: "echo", url: "http://echo.internal/mcp" });
    expect((await (await rpc(key, "tools/list")).json()).result.tools.length).toBe(1);
  });
});
"##;
const MCP_SQL: &str = r##"create table if not exists mcp_servers (
  name text primary key,                -- tool names are exposed as <name>__<tool>
  url text not null,                    -- the upstream's MCP endpoint (streamable HTTP)
  auth_env text,                        -- env var holding its bearer token; never the token
  healthy boolean not null default true,
  tools jsonb not null default '[]',    -- cached tools/list result
  error text,
  checked_at timestamptz,
  created_at timestamptz not null default now()
);
create table if not exists mcp_callers (
  id bigserial primary key,
  name text not null,
  key_hash text not null unique,
  scopes text[] not null default '{}',  -- "server:*" or "server:tool"; empty = nothing
  created_at timestamptz not null default now()
);
create table if not exists mcp_audit (
  id bigserial primary key,
  caller text not null,
  server text not null,
  tool text not null,
  args_hash text not null,
  result_bytes int not null default 0,
  duration_ms int not null default 0,
  ok boolean not null,
  error text,
  at timestamptz not null default now()
);
create index if not exists mcp_audit_at on mcp_audit (at desc);
"##;
const MCP_PAGE: &str = r##"// The console: upstream servers with their health and cached tools, then the audit trail.
// Reads the admin API with ADMIN_TOKEN from the environment — never sent to the browser.
const base = process.env.API_URL ?? "http://127.0.0.1:8000";
const headers = { authorization: `Bearer ${process.env.ADMIN_TOKEN ?? ""}` };

type Server = { name: string; url: string; auth_env: string | null; healthy: boolean; tools: { name: string; description?: string }[]; error: string | null; checked_at: string | null };
type Audit = { caller: string; server: string; tool: string; args_hash: string; result_bytes: number; duration_ms: number; ok: boolean; error: string | null; at: string };

async function get<T>(path: string, fallback: T): Promise<T> {
  const res = await fetch(base + path, { headers, cache: "no-store" }).catch(() => null);
  return res?.ok ? ((await res.json()).data as T) : fallback;
}

export default async function Home() {
  const [servers, audit] = await Promise.all([get<Server[]>("/api/admin/servers", []), get<Audit[]>("/api/admin/audit?limit=50", [])]);
  return (
    <main>
      <h1>{{NAME}} — MCP gateway</h1>
      <p>One MCP endpoint at <code>/mcp</code> (streamable HTTP). Tools are <code>server__tool</code>; a caller sees only what its scopes allow; every call is audited by args hash.</p>
      <pre>{`curl -X POST http://localhost:8000/api/admin/servers -H "authorization: Bearer $ADMIN_TOKEN" -H "content-type: application/json" \\
  -d '{"name":"github","url":"https://api.githubcopilot.com/mcp/","auth_env":"GITHUB_TOKEN"}'
curl -X POST http://localhost:8000/api/admin/callers -H "authorization: Bearer $ADMIN_TOKEN" -H "content-type: application/json" \\
  -d '{"name":"agent","scopes":["github:search_repositories"]}'
curl -X POST http://localhost:8000/mcp -H "authorization: Bearer mcp_..." -H "content-type: application/json" \\
  -H "accept: application/json, text/event-stream" -d '{"jsonrpc":"2.0","id":1,"method":"tools/list"}'`}</pre>
      <h2>Servers</h2>
      <table>
        <thead><tr><th>name</th><th>url</th><th>auth</th><th>health</th><th>tools</th><th>checked</th></tr></thead>
        <tbody>{servers.map((s) => (
          <tr key={s.name}>
            <td>{s.name}</td><td>{s.url}</td><td>{s.auth_env ? `$${s.auth_env}` : "none"}</td>
            <td>{s.healthy ? "healthy" : `unhealthy: ${s.error ?? ""}`}</td>
            <td>{s.tools.map((t) => `${s.name}__${t.name}`).join(", ") || "—"}</td>
            <td>{s.checked_at ? new Date(s.checked_at).toLocaleTimeString() : "—"}</td>
          </tr>
        ))}</tbody>
      </table>
      {servers.length === 0 && <p>No servers registered yet.</p>}
      <h2>Recent calls</h2>
      <table>
        <thead><tr><th>at</th><th>caller</th><th>tool</th><th>args</th><th>result bytes</th><th>ms</th><th>ok</th><th>error</th></tr></thead>
        <tbody>{audit.map((a, i) => (
          <tr key={i}><td>{new Date(a.at).toLocaleTimeString()}</td><td>{a.caller}</td><td>{a.server}__{a.tool}</td><td>{a.args_hash.slice(0, 8)}</td><td>{a.result_bytes}</td><td>{a.duration_ms}</td><td>{a.ok ? "yes" : "no"}</td><td>{a.error ?? ""}</td></tr>
        ))}</tbody>
      </table>
      {audit.length === 0 && <p>No calls yet.</p>}
    </main>
  );
}
"##;

const MCP_CLAUDE: &str = r##"# {{NAME}} — MCP gateway

{{NAME}} is one MCP endpoint over many MCP servers, in the shape of mcp-proxy and Metorial: a
client connects to `/mcp` (streamable HTTP, spec 2025-06-18) with a caller key, sees the tools
of every registered upstream as `<server>__<tool>` — only those its scopes allow — and calls
them through the gateway, which holds the upstream credentials, caches the schemas, hides a
server that stops answering, and audits every call by argument hash. "Done" here means an
MCP client that speaks the spec works against `/mcp` without knowing there are several servers
behind it, a caller cannot see or call outside its scopes, an upstream outage is visible in the
console and invisible to other servers' tools, and `make check` proves it against a fake
upstream with no database and no network.

## Architecture

| File | Owns |
|---|---|
| `backend/src/app.ts` | `createApp(store, fetch)`: admin API, `refresh`/`fresh` (schema cache with TTL and health), the `/mcp` JSON-RPC dispatcher (`initialize`, `ping`, `tools/list`, `tools/call`), `allowed()`, the size caps |
| `backend/src/upstream.ts` | A minimal streamable-HTTP MCP client: `post`, `readResponse` (JSON or SSE), `request` with session handling and one re-initialise on 404, `initialize`, `listTools` (paginated), `callTool`, `forgetSession` |
| `backend/src/store.ts` | `Store`: servers with cached tools and health, callers with hashed keys and scopes, the audit; `pgStore()` and `memoryStore()` |
| `backend/src/app.test.ts` | A fake upstream (a Hono app answering JSON and SSE with a session id) and a fake client: lifecycle, scoping, forwarding, caps, health |
| `backend/migrations/0002_mcpgateway.sql` | `mcp_servers`, `mcp_callers`, `mcp_audit` |
| `backend/src/server.ts`, `db.ts` | From the stack: listen on `PORT`, drain on SIGTERM, the pool |
| `frontend/app/page.tsx` | Console: servers with health and cached tools, the last 50 audit rows. Reads the admin API with `ADMIN_TOKEN` |

### Request path (`POST /mcp`)

1. Caller auth: `Authorization: Bearer mcp_…`, hashed, looked up in `mcp_callers`. Missing or unknown is HTTP 401 with JSON-RPC error `-32001`.
2. `MCP-Protocol-Version`, if sent, must be in `SUPPORTED` (`2025-06-18`, `2025-03-26`), else 400 `-32600` with the supported list in `data`.
3. Body over `MAX_ARGS_BYTES` (64 KiB) is 413 `-32600` before parsing. The body must be a JSON-RPC 2.0 object (`rpcSchema`); else 400 `-32600`. Batches are not accepted.
4. No `id` means a notification: 202 with no body, whatever the method.
5. `initialize`: echoes the client's `protocolVersion` if supported, else `PROTOCOL`; sets `Mcp-Session-Id` (random, not stored — sessions are stateless); returns `capabilities: {tools: {listChanged: false}}`, `serverInfo`, and `instructions` explaining the naming.
6. `tools/list`: `fresh()` refreshes any server whose `checked_at` is older than `SCHEMA_TTL_MS` (60 s) — a failed refresh marks it unhealthy and forgets its upstream session. Healthy servers' tools that pass `allowed(scopes, server, tool)` are returned with `name` prefixed `<server>__`.
7. `tools/call`: the name is split on the first `__`; server and tool are looked up in the fresh registry. Unknown server, unknown tool, or out of scope all answer the same `-32602 Unknown tool` (no probing). An unhealthy server is `-32603`. Required arguments per the cached `inputSchema.required` are checked; missing is `-32602` naming them.
8. `callTool` forwards over the upstream's session with `CALL_TIMEOUT_MS` (30 s). A result over `MAX_RESULT_BYTES` (512 KiB) becomes `isError: true` with an explanatory text. A JSON-RPC error from the upstream is passed through with its code; a transport failure or timeout marks the server unhealthy, forgets its session, and answers `isError: true`. Every outcome writes one `mcp_audit` row with `sha256(arguments)`, `result_bytes`, `duration_ms`, `ok`, `error`.
9. Any other method is `-32601 Method not found`. `GET /mcp` is 405 (no server-initiated stream), `DELETE /mcp` is 204.

### Data model

| Table | Column | Why |
|---|---|---|
| `mcp_servers` | `name` (pk, `[a-z0-9-]+`) | The prefix in `<name>__<tool>`; `POST` upserts by name |
| | `url` | The upstream's streamable-HTTP endpoint |
| | `auth_env` | The *name* of the env var holding its bearer token; the token itself is never stored |
| | `healthy`, `error`, `checked_at` | Health from the last refresh or call; `checked_at` drives the 60 s schema TTL |
| | `tools jsonb` | The cached `tools/list`; kept when unhealthy so the console shows what was there |
| `mcp_callers` | `key_hash` (unique) | sha256 of the `mcp_…` plaintext, returned once at creation |
| | `scopes text[]` | `*`, `server:*`, or `server:tool`; empty grants nothing |
| `mcp_audit` | `args_hash` | Proves what was called without storing arguments |
| | `result_bytes`, `duration_ms`, `ok`, `error` | The cap and the timeout, observable |

## Invariants

1. **Arguments and results are never stored.** The audit row has `args_hash` and `result_bytes`; nothing else. Guarded by "tools/call is forwarded…" (`JSON.stringify(audit)` does not contain the argument text).
2. **Unknown and out-of-scope are indistinguishable** — both `-32602 Unknown tool: <name>`. Why: a caller must not be able to enumerate tools it cannot use. Guarded by "out-of-scope and unknown tools are the same -32602".
3. **`tools/list` is filtered by scope and only lists healthy servers.** Guarded by "tools/list is namespaced and filtered by scope" and "an upstream that stops answering has its tools hidden until it recovers".
4. **Upstream credentials are env-var names, never values.** `auth_env` is validated `^[A-Z0-9_]+$`; `upstream.ts` reads `process.env[auth_env]` at call time. Guarded by the `beforeAll` registration (`auth_env: "ECHO_TOKEN"`) and the schema; grep `auth_env` before adding a field that could carry a token.
5. **Size caps apply before forwarding.** Body over 64 KiB is refused before `JSON.parse`; a result over 512 KiB is refused before it reaches the client. Guarded by "oversized arguments are refused before forwarding" (`upstreamCalls.length === 0`).
6. **A transport failure hides the server; a JSON-RPC error from the upstream does not.** The upstream saying "bad argument" is its answer, not an outage. Guarded by "an upstream that stops answering…" (502 → unhealthy, tools hidden, schemas kept).
7. **Sessions are stateless on the client side** — `Mcp-Session-Id` is issued at `initialize` and never checked. Why: no server-initiated messages, so there is nothing per session to hold, and any replica can answer. Guarded by "initialize answers the spec shape with a session id…" (the header is present) and every later test not sending it.
8. **The upstream session is reused and re-established once on 404**, as the spec tells clients. Guarded by "tools/call is forwarded…" (`upstreamCalls[0].session === "sess-1"`, initialise not repeated) and the recovery half of the health test.
9. **Notifications are 202, `GET` is 405, unknown methods are `-32601`.** Guarded by "initialize answers the spec shape…".
10. **Required arguments are checked from the cached schema before forwarding.** Why: a clear `-32602` here beats a provider-specific error later, and it is free. Guarded by "out-of-scope and unknown tools…" (`missing.message` contains `text`).
11. **The schema cache is at most 60 s stale and is refreshed synchronously on registration.** `POST /api/admin/servers` returns the tools it found. Guarded by `beforeAll` (`tools` in the 201 body) and `SCHEMA_TTL_MS` in `fresh()`.
12. **The admin API is closed when `ADMIN_TOKEN` is unset.** Guarded by the first line of the first test (401 without a key on `/mcp`) and `adminOk`.

## Extending it

**Add an upstream transport (stdio, SSE-only legacy servers).** `upstream.ts` is the seam: `listTools` and `callTool` take a `Server` and a `Fetch`. Add a `transport` column to `mcp_servers` (`0003_`, default `'http'`), branch in `request()`. For stdio, a child process per server in the gateway pod is a different operational shape — say so in the console. Test: a fake for the new transport, the same six tests.

**Forward resources and prompts.** Today only `tools/*` is aggregated. Add `resources/list`, `resources/read`, `prompts/list`, `prompts/get` cases in the dispatcher, namespace URIs and prompt names the same way, and extend `allowed()` to a third segment (`server:resource:*`). Announce them in `initialize`'s `capabilities`. Test: the fake upstream answers `resources/list`; a scoped caller sees only its server's.

**Add a caller scope shape** (e.g. deny lists, or `*:search_*`). Only `allowed()` and the regex in the callers schema change. Test: three lines in "tools/list is namespaced and filtered by scope".

**Add per-caller rate limits or quotas.** A `hit()` on the store as in the other gateways, keyed by caller id, checked after auth in `POST /mcp`; answer `-32000` with HTTP 429. Test: a caller with `rpm: 2` gets 429 on the third call.

**Support server-initiated messages (progress, sampling, `listChanged`).** That needs `GET /mcp` to open an SSE stream per session and a session store that survives across replicas. It is the one change that makes sessions stateful; do it only for a concrete client need, and move `sessions` (both directions) to Redis first.

**Add a server field** (headers, timeout override). In order: the zod schema in `POST /api/admin/servers`, the `Server` type, `0003_*.sql` `add column … default`, both `pgStore` queries (`servers`, `putServer`), then use it in `upstream.ts`. Test: register with the field and assert on what the fake upstream received.

**Health checks on a timer.** Today health is refreshed lazily on `tools/list` and `tools/call`. A `setInterval` in `server.ts` calling `refresh` for every server keeps the console current with no traffic; keep it out of `createApp`.

## Operating it

| Variable | Required | Meaning |
|---|---|---|
| `ADMIN_TOKEN` | yes | Bearer for `/api/admin/*`; unset means closed |
| `DATABASE_URL` | yes | Registry, callers, audit |
| `<auth_env>` per server | when set on the server | e.g. `GITHUB_TOKEN`; sent as `Authorization: Bearer` to that upstream |
| `PORT` | no | Default 8000 |
| `API_URL` | frontend | Where the console reaches the backend |

**Per replica today:** the upstream session map (`sessions` in `upstream.ts`) and the JSON-RPC id counter. Each replica initialises its own session with each upstream; that is correct, just N× the sessions. Health and the schema cache are in Postgres, so a server marked unhealthy by one replica is hidden by all within a `tools/list`. Nothing about the *client* side is per replica.

**Failure modes:**

| Situation | HTTP | JSON-RPC |
|---|---|---|
| No / unknown caller key | 401 | `-32001 unauthorized` |
| Unsupported protocol version header | 400 | `-32600`, `data.supported` |
| Body over 64 KiB | 413 | `-32600` |
| Not JSON-RPC 2.0 | 400 | `-32600` |
| Unknown method | 200 | `-32601` |
| Unknown tool / out of scope / bad params / missing required args | 200 | `-32602` |
| Upstream unhealthy (cached) | 200 | `-32603 upstream X is unhealthy: …` |
| Upstream JSON-RPC error | 200 | passed through with its code |
| Upstream timeout / transport failure | 200 | `result.isError: true`, text `upstream X failed: …`; server marked unhealthy |
| Result over 512 KiB | 200 | `result.isError: true`, text with the size |
| Postgres down | 500 | none |

**What to watch:** `mcp_audit` — `ok = false` rate by `server` and `error`; p95 `duration_ms` by `server__tool`; `result_bytes` near the cap; calls per `caller` per hour (an agent in a loop). `mcp_servers.healthy = false` is the page. The upstream's own 401 shows as `upstream 401` in `error`: the env var named in `auth_env` is missing or expired.

## Ceilings

| Ceiling | Where | Upgrade |
|---|---|---|
| Upstream session map per process | `upstream.ts` `sessions` (`ponytail:` comment) | Redis when replicas multiply — or leave it: N sessions is fine for most upstreams |
| No server-initiated messages, `listChanged: false` | `GET /mcp` 405 | SSE stream per session plus a shared session store |
| Only `tools/*` aggregated | dispatcher | resources and prompts, same pattern |
| Schema refresh is inline on the request that finds it stale | `fresh()` | A timer in `server.ts`; a slow upstream currently delays that one `tools/list` by up to `LIST_TIMEOUT_MS` |
| Refresh runs for all stale servers in parallel with no cap | `fresh()` | `p-limit` when servers number in the hundreds |
| SSE responses are read whole, not streamed | `readResponse` | Stream through when an upstream sends progress notifications you want to relay |
| No batch requests | `rpcSchema` | The 2025-06-18 spec dropped batching; nothing to do unless a 2025-03-26 client needs it |
| Argument validation is `required` only | `tools/call` step 7 | Full JSON Schema via `ajv` if you want to reject before forwarding |
| Readiness does not check Postgres | `/api/health/ready` | `select 1` |

The stack rules — gate, typed seam, production checklist, deploy — are in `docs/PRODUCTION.md`. They apply.
"##;
const MCP_AGENTS: &str = r##"# {{NAME}} — for agents

`CLAUDE.md` has the rules and the architecture. This is how to run and test it.

## Run

    cp .env.example backend/.env
    printf 'ADMIN_TOKEN=admin\nGITHUB_TOKEN=ghp_…\n' >> backend/.env   # one var per upstream that needs one
    ADMIN_TOKEN=admin make demo   # postgres + redis, migrate, seed, API on :8000, console on :3000
    make check                    # typecheck both halves, backend tests — no database, no upstream

Pieces: `make dev`, `make backend`, `make frontend`, `make migrate`, `make up` (nginx on :8080).
No worker in this pack.

## Routes, with bodies

Admin (`authorization: Bearer $ADMIN_TOKEN`):

    # register (or update) an upstream; the gateway initialises it and lists its tools now
    curl -s -X POST localhost:8000/api/admin/servers -H "authorization: Bearer $ADMIN_TOKEN" \
      -H "content-type: application/json" \
      -d '{"name":"github","url":"https://api.githubcopilot.com/mcp/","auth_env":"GITHUB_TOKEN"}'
    # 201 {"name":"github","url":"…","auth_env":"GITHUB_TOKEN","healthy":true,"tools":[{"name":"search_repositories",…},…],"error":null,"checked_at":"…"}
    # or, if it could not be reached: "healthy":false,"error":"upstream 401","tools":[]

    curl -s localhost:8000/api/admin/servers -H "authorization: Bearer $ADMIN_TOKEN"
    # {"data":[{…}]}

    curl -s -X DELETE localhost:8000/api/admin/servers/github -H "authorization: Bearer $ADMIN_TOKEN"
    # 204 | 404

    # mint a caller; the plaintext is in this response and nowhere else
    curl -s -X POST localhost:8000/api/admin/callers -H "authorization: Bearer $ADMIN_TOKEN" \
      -H "content-type: application/json" -d '{"name":"agent","scopes":["github:search_repositories","github:get_file_contents"]}'
    # 201 {"id":1,"name":"agent","key":"mcp_…","scopes":[…]}
    # scopes: "*" | "<server>:*" | "<server>:<tool>"

    curl -s "localhost:8000/api/admin/audit?limit=20" -H "authorization: Bearer $ADMIN_TOKEN"
    # {"data":[{"caller":"agent","server":"github","tool":"search_repositories","args_hash":"…64 hex…","result_bytes":2310,"duration_ms":412,"ok":true,"error":null,"at":"…"}]}

The MCP endpoint (`authorization: Bearer mcp_…`; send `accept: application/json, text/event-stream`
and `mcp-protocol-version: 2025-06-18` as a spec client does):

    M() { curl -si -X POST localhost:8000/mcp -H "authorization: Bearer $KEY" -H "content-type: application/json" \
          -H "accept: application/json, text/event-stream" -H "mcp-protocol-version: 2025-06-18" -d "$1"; }

    M '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"curl","version":"0"}}}'
    # 200, Mcp-Session-Id: <uuid>
    # {"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-06-18","capabilities":{"tools":{"listChanged":false}},"serverInfo":{"name":"mcp-gateway","version":"1.0.0"},"instructions":"Tools are named <server>__<tool>. …"}}

    M '{"jsonrpc":"2.0","method":"notifications/initialized"}'        # 202, empty
    M '{"jsonrpc":"2.0","id":2,"method":"ping"}'                       # {"jsonrpc":"2.0","id":2,"result":{}}

    M '{"jsonrpc":"2.0","id":3,"method":"tools/list"}'
    # {"jsonrpc":"2.0","id":3,"result":{"tools":[{"name":"github__search_repositories","description":"…","inputSchema":{…}},…]}}

    M '{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"github__search_repositories","arguments":{"query":"hono"}}}'
    # {"jsonrpc":"2.0","id":4,"result":{"content":[{"type":"text","text":"…"}]}}

    M '{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"github__delete_repository","arguments":{}}}'
    # {"jsonrpc":"2.0","id":5,"error":{"code":-32602,"message":"Unknown tool: github__delete_repository"}}   — out of scope looks unknown

    M '{"jsonrpc":"2.0","id":6,"method":"resources/list"}'
    # {"jsonrpc":"2.0","id":6,"error":{"code":-32601,"message":"Method not found: resources/list"}}

Point a real client at it — Claude Code: `claude mcp add --transport http {{NAME}} http://localhost:8000/mcp --header "Authorization: Bearer $KEY"`.

## Tests

`backend/src/app.test.ts` has two fakes. `fakeUpstream` is a Hono app at `/mcp` that behaves
like a streamable-HTTP server: hands out `Mcp-Session-Id: sess-1` at `initialize`, answers 404
without it, lists two tools (`echo` with a required `text`, `secret`), answers `tools/call` for
`echo` **over SSE** (so `readResponse`'s SSE path is covered) and for `secret` as JSON, returns
502 while `echoDown`, and waits for the abort signal while `slow`. `upstreamCalls` records
method, session header and params. `fetchImpl` routes every gateway fetch into it.

The gateway is `createApp(memoryStore(), fetchImpl)`. `beforeAll` registers `echo` and mints two
callers: `key` scoped `echo:echo` and `adminKey` scoped `echo:*`. `rpc(key, method, params)` is
the client: right headers, incrementing `id`.

Adding a test:

1. Use `rpc(key, …)` for a scoped caller or `rpc(adminKey, …)` for one that sees everything; mint another with `adminPost("/api/admin/callers", …)` if you need different scopes.
2. Add a tool to the fake's `tools/list` answer if the test needs one; keep `echo` and `secret` as they are — four tests depend on them.
3. Toggle `echoDown`/`slow` and reset them in the same test; re-register the server (`adminPost("/api/admin/servers", …)`) to force a refresh, as the recovery test does.
4. Assert on the JSON-RPC envelope (`error.code`, `result.isError`) rather than HTTP status — the transport is 200 for application errors — and on `/api/admin/audit` for anything that should be observable.
5. `cd backend && bun test`, or `make check`.

Exported for tests: `createApp`, `allowed`, `MAX_ARGS_BYTES`, `MAX_RESULT_BYTES`, `CALL_TIMEOUT_MS`, `LIST_TIMEOUT_MS`, `SCHEMA_TTL_MS`, `SUPPORTED`, and from `upstream.ts` `PROTOCOL`, `UpstreamError`, `forgetSession`.
"##;
const MCP_README: &str = r##"# {{NAME}}

One MCP endpoint for all your MCP servers: register upstreams, mint scoped keys for your agents,
and every tool call goes through a gateway that holds the credentials, enforces scopes and
leaves an audit trail.

## What you get

- `POST /mcp` — a streamable-HTTP MCP server (spec 2025-06-18, 2025-03-26 accepted) that any MCP client can use: Claude Code, Claude Desktop, Cursor, the official SDKs.
- A registry of upstream MCP servers; tools appear as `<server>__<tool>` with their real schemas, refreshed at most every 60 s.
- Caller keys (`mcp_…`) with scopes — `*`, `github:*`, `db:query` — that filter `tools/list` and gate `tools/call`. Out of scope and non-existent look the same.
- Upstream credentials as environment-variable names on the server record; the gateway sends them, the caller never sees them.
- A 64 KiB request cap, a 512 KiB result cap, a 30 s call timeout and a 10 s list timeout; required-argument checking from the cached schema.
- Health: an upstream that stops answering has its tools hidden from every caller until it answers again; its last schemas stay in the console.
- An audit row per call: caller, server, tool, `sha256(arguments)`, result size, duration, ok/error. Arguments and results are never stored.
- A console on :3000: servers with health and tools, recent calls.
- `make check`: typecheck both halves and run the gateway between a fake client and a fake upstream (JSON and SSE answers, sessions, outages). No Postgres, no network.

## Five minutes

    cp .env.example backend/.env && echo ADMIN_TOKEN=admin >> backend/.env
    ADMIN_TOKEN=admin make demo

In another shell, with any streamable-HTTP MCP server to hand — e.g. `npx -y @modelcontextprotocol/server-everything streamableHttp` on :3001:

    curl -s -X POST localhost:8000/api/admin/servers -H "authorization: Bearer admin" \
      -H "content-type: application/json" -d '{"name":"every","url":"http://localhost:3001/mcp"}' | jq '{healthy, tools: [.tools[].name]}'
    # {"healthy": true, "tools": ["echo", "add", "longRunningOperation", …]}

    KEY=$(curl -s -X POST localhost:8000/api/admin/callers -H "authorization: Bearer admin" \
      -H "content-type: application/json" -d '{"name":"me","scopes":["every:echo","every:add"]}' | jq -r .key)

    curl -s -X POST localhost:8000/mcp -H "authorization: Bearer $KEY" -H "content-type: application/json" \
      -H "accept: application/json, text/event-stream" \
      -d '{"jsonrpc":"2.0","id":1,"method":"tools/list"}' | jq '[.result.tools[].name]'
    # ["every__echo", "every__add"]          ← two of the many, per the scopes

    curl -s -X POST localhost:8000/mcp -H "authorization: Bearer $KEY" -H "content-type: application/json" \
      -H "accept: application/json, text/event-stream" \
      -d '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"every__add","arguments":{"a":2,"b":3}}}' | jq .result
    # {"content":[{"type":"text","text":"The sum of 2 and 3 is 5."}]}

    curl -s "localhost:8000/api/admin/audit?limit=1" -H "authorization: Bearer admin" | jq '.data[0] | {caller, tool, args_hash, result_bytes, duration_ms, ok}'

Then give it to an agent: `claude mcp add --transport http {{NAME}} http://localhost:8000/mcp --header "Authorization: Bearer $KEY"`.

## API

| Method | Path | Auth | What |
|---|---|---|---|
| GET | `/api/health` | none | liveness |
| GET | `/api/health/ready` | none | readiness |
| GET | `/api/admin/servers` | admin | registry with health and cached tools |
| POST | `/api/admin/servers` | admin | register or update by `name`; initialises and lists now |
| DELETE | `/api/admin/servers/:name` | admin | remove |
| POST | `/api/admin/callers` | admin | mint a caller key with scopes; plaintext returned once |
| GET | `/api/admin/audit?limit=` | admin | last N calls (max 500) |
| POST | `/mcp` | caller key | JSON-RPC: `initialize`, `ping`, `tools/list`, `tools/call`; notifications → 202 |
| GET | `/mcp` | — | 405 (no server-initiated stream) |
| DELETE | `/mcp` | — | 204 |

Admin auth is `Authorization: Bearer $ADMIN_TOKEN`; the caller key is `Authorization: Bearer mcp_…`.

## Compared with mcp-proxy and Metorial

**Same shapes** — spec clients and their docs apply:

- Streamable HTTP as the spec describes it: POST with both `Accept` types, `Mcp-Session-Id` on `initialize`, `MCP-Protocol-Version` header, 202 for notifications, 405 on GET when there is no stream, JSON-RPC 2.0 error codes (`-32600`, `-32601`, `-32602`, `-32603`).
- Tool namespacing as `<server>__<tool>` (mcp-proxy's separator), the upstream's `inputSchema` passed through unchanged.
- The upstream client does what the spec asks of clients: `initialize` → `notifications/initialized`, session header reuse, re-initialise on 404, paginated `tools/list`, JSON or SSE responses.

**Better here:**

- Scopes per caller key, down to a single tool, with no way to enumerate what is out of scope. mcp-proxy has no auth; Metorial's scoping is per deployment.
- Credentials never leave the gateway: a server record names an env var, not a secret, and the audit is an argument hash.
- Caps and timeouts on every call, and an audit row per call with size and duration — in Postgres, joinable to your own tables.
- Health is a property you can see: an outage hides one server's tools and nothing else, and the console shows the last error.
- Tested without an upstream: the fake server in the test suite answers over SSE and JSON, hands out sessions, goes down and comes back.
- One TypeScript codebase you can read in an afternoon, and Kubernetes manifests, migrations and secret rendering with it.

**Not here yet:**

- Only `tools/*`. Resources, prompts, completion, logging and sampling are not aggregated (`-32601`).
- No server-initiated messages: no `GET /mcp` stream, no progress notifications relayed, `listChanged: false`. A long-running tool is a 30 s timeout, not a progress bar.
- Only streamable-HTTP upstreams. No stdio servers (mcp-proxy's main job is stdio→HTTP), no legacy HTTP+SSE transport.
- No OAuth on the client side (the spec's authorization flow); a caller key is a bearer token you mint. No OAuth to upstreams either — a static token per upstream.
- No hosted deployment of upstream servers, no marketplace, no per-user upstream credentials (Metorial's model); one credential per server for all callers.
- No rate limits or quotas per caller; no batching; no argument validation beyond `required`.
- Sessions to upstreams are per replica; the schema cache is refreshed lazily, not on a timer.
- The console is read-only; registration and keys are `curl`.

## Production

- **Environments:** `backend/.env` holds `ADMIN_TOKEN`, `DATABASE_URL` and one variable per upstream that needs a token (the name you put in `auth_env`); `make encrypt-env` writes `backend/.env.age`. `_DEV`-suffixed keys win in the dev overlay — separate upstream tokens per environment.
- **Scaling:** stateless towards clients; each replica keeps its own upstream sessions, which is correct. Health and schemas are in Postgres and shared.
- **Probes:** `/api/health` liveness, `/api/health/ready` readiness. Readiness does not query Postgres yet — add `select 1` before relying on it. Do not make readiness depend on upstreams: one of them being down is exactly what the gateway is for.
- **Migrations:** `backend/migrations/*.sql`, `make migrate` locally, the `migrate` init container before a rollout. `mcp_servers.tools` is jsonb; a new server field is `add column … default`.
- **Secrets:** `k8s/scripts/env-to-secrets.sh` renders `backend/.env` into a Secret; upstream tokens exist only in the pod environment.
- **Overlays:** `k8s/overlays/dev` and `prod`. `git push` to `main` deploys dev; `make release` tags for prod. Put the ingress in front of `/mcp` with a body limit at or above 64 KiB and an idle timeout above 30 s, or calls will be cut before the gateway's own timeout.
- **What pages you:** `mcp_servers.healthy = false` for longer than a refresh interval; `ok = false` rate by server in `mcp_audit`; p95 `duration_ms` approaching 30 s; an `error` of `upstream 401` (an expired upstream token); calls per caller per minute far above its normal (an agent looping).

## Roadmap

1. Readiness that checks Postgres; a health-refresh timer in `server.ts`.
2. Resources and prompts aggregated with the same namespacing and scopes.
3. Per-caller rate limits (`hit()` on the store, then Redis).
4. `GET /mcp` stream and relayed progress notifications, with the session map in Redis.
5. Full JSON Schema validation of arguments before forwarding.
6. stdio upstreams as child processes, if a server you need has no HTTP transport.
7. OAuth for callers and per-user upstream credentials.
"##;
const MCP_REV_PROTOCOL: &str = r##"---
name: protocol-conformance
description: Run before a change to backend/src/app.ts (the /mcp dispatcher) or backend/src/upstream.ts ships. Reads it against the MCP streamable-HTTP spec (2025-06-18) as someone who has watched a client hang on a wrong status code or a missing header.
tools: Read, Grep, Glob, Bash
---

You check conformance to the Model Context Protocol, both as a server (to callers) and as a
client (to upstreams). Report only what will make a spec client or server misbehave — each as
`path:line — what — which client/server breaks — fix`.

Check, as a server on `POST /mcp`:
1. Envelope: every response is `{"jsonrpc":"2.0","id":…}` with exactly one of `result` or
   `error`; `id` echoes the request's, `null` when it could not be read. `error` has `code`
   and `message`, optional `data`.
2. Codes: `-32700` for unparsable JSON is not used (the handler answers `-32600`; note it, do
   not fail it); `-32600` invalid request; `-32601` unknown method; `-32602` bad params, unknown
   tool, missing required args; `-32603` internal (an unhealthy upstream). Application-level
   tool failures are `result.isError: true`, never a JSON-RPC error — SDKs surface the two
   differently to the model.
3. Notifications (no `id`) return 202 with no body, before the method switch. A request with
   `id: 0` is a request, not a notification (`=== undefined`, not falsy).
4. `initialize` returns `protocolVersion` equal to the client's if supported, else the
   gateway's; `capabilities.tools` is present; `serverInfo.name` and `version` are strings;
   `Mcp-Session-Id` is set on this response only.
5. `MCP-Protocol-Version` header: accepted when in `SUPPORTED`, 400 when not, absent means
   assume the older version — never 400 on absence.
6. `GET /mcp` is 405 while there is no server-initiated stream; if a stream is added it must
   be `text/event-stream` with `Mcp-Session-Id` validation. `DELETE /mcp` is 204 (or 405 —
   both allowed; 404 is not).
7. `tools/list` items keep the upstream's `inputSchema` verbatim (a client validates against
   it) and only `name` is rewritten; `title`, `description`, `annotations`, `outputSchema` pass
   through. `nextCursor` is not emitted (the whole list is returned) — fine, but a client that
   sends `cursor` must not get an error.
8. `tools/call` result keeps `content`, `structuredContent` and `isError` as the upstream sent
   them; the size-cap replacement is a valid `content` array.
9. HTTP status: 200 for every JSON-RPC-level outcome; 401/400/413 only for transport-level
   refusals, each still carrying a JSON-RPC error body.

Check, as a client in `upstream.ts`:
10. Every POST carries `Accept: application/json, text/event-stream`, `Content-Type:
    application/json`, `MCP-Protocol-Version`, and `Mcp-Session-Id` once known.
11. `initialize` is followed by `notifications/initialized` (a POST with no `id`, response
    ignored) before any other request; the session id is read from the `initialize`
    response's header (case-insensitive).
12. A 404 after a session was established clears the session and retries exactly once from
    `initialize`. Any other non-2xx is an `UpstreamError` with the status.
13. SSE responses: only `data:` lines are parsed; the message whose `id` matches is the
    answer; other messages (notifications, other ids) are ignored, not errors; a stream that
    ends without the answer is an error.
14. `tools/list` follows `nextCursor` until absent.
15. Request ids are unique per process (`nextId`); a change to a shared counter or a UUID is
    fine, a reset to 1 per call is not.
16. `forgetSession` is called on every transport failure so the next call re-initialises.

End with one line: `protocol-conformance: N findings`, and if 0, what you checked.
"##;
const MCP_REV_SCOPING: &str = r##"---
name: tool-scoping
description: Run before a change to allowed(), the callers schema, tools/list, tools/call, the audit, or anything that handles an upstream credential ships. Reads it as someone whose agent key leaked and wants to know exactly what the holder can see, call and learn.
tools: Read, Grep, Glob, Bash
---

You hold a stolen caller key and a curious mind. Report only what lets a caller see, call, or
infer beyond its scopes, or lets a secret or an argument escape — each as
`path:line — what leaks — the request that proves it — fix`.

Check:
1. `allowed()` is the only gate, used by both `tools/list` and `tools/call`; a new method that
   touches tools must call it. Failure: a tool visible through one path and not the other.
2. Scope grammar: `*`, `server:*`, `server:tool`; the callers schema regex rejects anything
   else, including an empty string and `server:` alone. Empty `scopes` grants nothing.
3. Name splitting: `tools/call` splits on the first `__`; a tool whose own name contains `__`
   still resolves; a name with no `__` resolves to no server. A server named so that
   `a__b` is ambiguous between server `a` and server `a__b` cannot exist (the name regex has
   no underscore).
4. Indistinguishability: unknown server, unknown tool, and out-of-scope all return the same
   code and message shape. A change that adds detail ("server exists but…") is an oracle.
5. Health leak: an unhealthy server's tools are absent from `tools/list` for everyone, and a
   call to one returns `-32603` only when the caller is in scope — check the order of the
   scope check and the health check.
6. Credentials: `auth_env` is a name matching `^[A-Z0-9_]+$`; the value is read in
   `upstream.ts` and sent only to that server's `url`. A server record's `url` change moves
   the token to a new host — say whether the admin API should be that trusting.
7. Admin surface: `GET /api/admin/servers` returns `auth_env` (a name) and never a value;
   `POST /api/admin/callers` returns the plaintext once and `mcp_callers` stores the hash.
8. Audit: `args_hash` is the hash of the canonical JSON of `arguments`; `error` strings come
   from the gateway or the upstream's error message and must not include argument values —
   check the upstream error path (`e.rpc.message` may echo arguments; it is passed to the
   caller, which is fine, and to the audit, which is not: confirm).
9. Size caps: the request cap is on the raw body before parsing; the result cap is on the
   serialised result. A change that parses before measuring is a memory DoS.
10. Required-argument check reads the cached schema, so a stale schema can refuse a valid call
    or admit an invalid one for up to `SCHEMA_TTL_MS`; the upstream still validates. Fine —
    unless the change starts trusting the gateway's check as the only one.
11. Session id: the `Mcp-Session-Id` the gateway issues carries no authority; nothing reads it
    back. If a change starts storing per-session state, the id must be bound to the caller.
12. Rate and quota: there is none per caller. A leaked key can call an in-scope tool at line
    speed until the upstream's own limits bite; say whether the change makes that worse.
13. The console: `frontend/app/page.tsx` renders `auth_env` and tool names, never keys; it
    reads with `ADMIN_TOKEN` on the server side and the token is not in any client bundle.

End with one line: `tool-scoping: N findings`, and if 0, what you checked.
"##;
