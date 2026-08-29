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
