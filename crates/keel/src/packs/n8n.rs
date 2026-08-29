//! n8n: port from n8n — workflows in, routes, schedules and named functions out ═══════════════════════════════

pub fn files(name: &str) -> Vec<(&'static str, String)> {
    let f = |s: &str| s.replace("{{NAME}}", name);
    vec![
        ("backend/src/app.ts", N8N_APP.into()),
        ("backend/src/store.ts", N8N_STORE.into()),
        ("backend/src/engine.ts", N8N_ENGINE.into()),
        ("backend/src/scheduler.ts", N8N_SCHEDULER.into()),
        ("backend/src/import/n8n.ts", N8N_IMPORT.into()),
        ("backend/src/import/fixture.ts", N8N_FIXTURE.into()),
        ("backend/src/app.test.ts", N8N_TEST.into()),
        ("backend/migrations/0002_n8n.sql", N8N_SQL.into()),
        ("frontend/app/page.tsx", f(N8N_PAGE)),
    ]
}

const N8N_APP: &str = r##"import { Hono } from "hono";
import { z } from "zod";
import { parse, report, stubs, type Graph, type Trigger } from "./import/n8n";
import { run, cronMatches, UnsupportedNode, type Ctx, type Fetch, type Item } from "./engine";
import { pgStore, type Store } from "./store";

// n8n's surface, so what pointed at n8n keeps working: a Webhook node's path is served at
// /webhook/<path> with its method (n8n's production URL), answering `{"message":"Workflow was
// started"}` for responseMode onReceived and the last node's items for lastNode; a Schedule or
// Cron trigger fires on its cron line from the scheduler (`bun src/scheduler.ts`); a manual trigger
// runs from POST /api/workflows/<name>/run. POST /api/import takes the exported JSON and
// returns the mapping report. A node the engine does not support fails the run loudly and is
// listed up front, on the page and in MIGRATION.md.

export type Deps = { fetch?: Fetch; env?: Record<string, string | undefined>; now?: () => Date };

export function createApp(store: Store, deps: Deps = {}) {
  const ctx = (): Ctx => ({ fetch: deps.fetch ?? ((u, i) => fetch(u, i)), env: deps.env ?? process.env, now: deps.now ?? (() => new Date()), outputs: new Map() });

  const execute = async (g: Graph, t: Trigger, input: Item[]) => {
    const label = t.kind === "webhook" ? `webhook:${t.path}` : t.kind === "cron" ? `cron:${t.cron}` : "manual";
    try {
      const r = await run(g, t, input, ctx());
      return store.record({ workflow: g.name, trigger: label, status: "ok", steps: r.steps, input, output: r.output, error: null, finished_at: new Date().toISOString() });
    } catch (e) {
      const error = e instanceof Error ? e.message : String(e);
      const rec = await store.record({ workflow: g.name, trigger: label, status: "failed", steps: [], input, output: null, error, finished_at: new Date().toISOString() });
      console.error(JSON.stringify({ level: "error", msg: "workflow failed", workflow: g.name, trigger: label, error, unsupported: e instanceof UnsupportedNode }));
      return rec;
    }
  };

  const app = new Hono()
    .get("/api/health", (c) => c.json({ status: "ok" }))
    .get("/api/health/ready", (c) => c.json({ status: "ok", db: "ok" as const }))

    .post("/api/import", async (c) => {
      // An import defines routes and scheduled work; it is an operator action, under ADMIN_TOKEN.
      const admin = process.env.ADMIN_TOKEN;
      if (!admin || c.req.header("authorization") !== `Bearer ${admin}`) return c.json({ error: { message: "admin token required", code: "unauthorized" } }, 401);
      let raw: unknown;
      try {
        if (c.req.header("content-type")?.includes("multipart/form-data")) {
          const f = (await c.req.parseBody())["file"];
          if (!(f instanceof File)) return c.json({ error: { message: "multipart needs a `file` part", code: "invalid" } }, 400);
          raw = JSON.parse(await f.text());
        } else raw = await c.req.json();
      } catch { return c.json({ error: { message: "not JSON", code: "invalid" } }, 400); }
      let gs: Graph[];
      try { gs = parse(raw); } catch (e) { return c.json({ error: { message: e instanceof z.ZodError ? e.issues.map((i) => `${i.path.join(".")}: ${i.message}`).join("; ") : String(e), code: "invalid" } }, 400); }
      await store.save(gs);
      return c.json({ workflows: gs.map((g) => ({ name: g.name, nodes: g.nodes.length, triggers: g.triggers, unsupported: g.unsupported, env: g.env })), report: report(gs), stubs: Object.fromEntries(gs.map((g) => [g.name, stubs(g)])) }, 201);
    })

    .get("/api/workflows", async (c) => c.json({ workflows: (await store.workflows()).map((g) => ({ name: g.name, active: g.active, nodes: g.nodes.map((n) => ({ name: n.name, fn: n.fn, type: n.type, supported: n.kind !== null })), triggers: g.triggers, unsupported: g.unsupported, env: g.env })) }))
    .get("/api/runs", async (c) => c.json({ runs: await store.runs(50) }))

    .post("/api/workflows/:name/run", async (c) => {
      const g = (await store.workflows()).find((g) => g.name === c.req.param("name"));
      if (!g) return c.json({ error: { message: "no such workflow", code: "not_found" } }, 404);
      const t = g.triggers.find((t) => t.kind === "manual") ?? g.triggers[0];
      if (!t) return c.json({ error: { message: "workflow has no trigger node", code: "invalid" } }, 400);
      const body = await c.req.json().catch(() => ({}));
      const r = await execute(g, t, [{ json: body as Record<string, unknown> }]);
      return c.json({ run: r }, r.status === "ok" ? 200 : 500);
    })

    // n8n's production webhook URL, one route for every imported Webhook node.
    .all("/webhook/*", async (c) => {
      const path = "/" + c.req.path.slice("/webhook/".length).replace(/^\/+/, "");
      for (const g of await store.workflows()) {
        const t = g.triggers.find((t): t is Extract<Trigger, { kind: "webhook" }> => t.kind === "webhook" && t.path === path && (t.method === c.req.method || t.method === "*"));
        if (!t) continue;
        if (!g.active) return c.json({ error: { message: "workflow is inactive", code: "inactive" } }, 404);
        const ct = c.req.header("content-type") ?? "";
        const body = ct.includes("json") ? await c.req.json().catch(() => ({})) : ct.includes("form") ? await c.req.parseBody() : await c.req.text();
        const item: Item = { json: { headers: Object.fromEntries(c.req.raw.headers), params: {}, query: c.req.query(), body } };
        const r = await execute(g, t, [item]);
        if (r.status !== "ok") return c.json({ error: { message: r.error, code: "workflow_failed", run: r.id } }, 500);
        if (t.responseMode === "lastNode") { const out = r.output as Item[]; return c.json(out.length === 1 ? out[0].json : out.map((i) => i.json)); }
        return c.json({ message: "Workflow was started" });
      }
      return c.json({ error: { message: `no webhook registered for ${c.req.method} ${path}`, code: "not_found" } }, 404);
    });

  // Fire every cron trigger whose line matches this minute, once across replicas.
  const tick = async (now = (deps.now ?? (() => new Date()))()) => {
    const minute = new Date(Math.floor(now.getTime() / 60_000) * 60_000);
    const fired: string[] = [];
    for (const g of await store.workflows()) {
      if (!g.active) continue;
      for (const t of g.triggers) {
        if (t.kind !== "cron" || !t.cron.split(" | ").some((e) => cronMatches(e, minute))) continue;
        if (!(await store.claimMinute(g.name, t.node, minute))) continue;
        await execute(g, t, [{ json: { timestamp: minute.toISOString() } }]);
        fired.push(`${g.name}/${t.node}`);
      }
    }
    return fired;
  };

  return Object.assign(app, { tick });
}

const app = createApp(pgStore());
export type AppType = typeof app;
export default app;
"##;

const N8N_STORE: &str = r##"import { db } from "./db";
import type { Graph } from "./import/n8n";
import type { Item } from "./engine";

export type Run = { id: number; workflow: string; trigger: string; status: "ok" | "failed"; steps: string[]; input: unknown; output: unknown; error: string | null; started_at: string; finished_at: string | null };

export interface Store {
  save(graphs: Graph[]): Promise<void>;
  workflows(): Promise<Graph[]>;
  record(r: Omit<Run, "id" | "started_at">): Promise<Run>;
  runs(limit: number): Promise<Run[]>;
  // True once per (workflow, node, minute) across every replica — the cron leader election.
  claimMinute(workflow: string, node: string, minute: Date): Promise<boolean>;
}

export function pgStore(): Store {
  return {
    save: (graphs) => db.begin(async (tx) => { for (const g of graphs) await tx`insert into workflows (name, graph, active) values (${g.name}, ${tx.json(g as never)}, ${g.active}) on conflict (name) do update set graph = excluded.graph, active = excluded.active, imported_at = now()`; }),
    workflows: async () => (await db<{ graph: Graph }[]>`select graph from workflows order by name`).map((r) => r.graph),
    record: async (r) => (await db<Run[]>`insert into runs (workflow, trigger, status, steps, input, output, error, finished_at) values (${r.workflow}, ${r.trigger}, ${r.status}, ${db.json(r.steps)}, ${db.json(r.input as never)}, ${db.json(r.output as never)}, ${r.error}, now()) returning *`)[0],
    runs: (limit) => db<Run[]>`select * from runs order by id desc limit ${limit}`,
    claimMinute: async (w, n, m) => (await db`insert into cron_fired (workflow, node, minute) values (${w}, ${n}, ${m}) on conflict do nothing`).count > 0,
  };
}

export function memoryStore(): Store {
  const graphs = new Map<string, Graph>(); const runs: Run[] = []; const fired = new Set<string>();
  return {
    save: async (gs) => { for (const g of gs) graphs.set(g.name, g); },
    workflows: async () => [...graphs.values()],
    record: async (r) => { const run = { ...r, id: runs.length + 1, started_at: new Date().toISOString() }; runs.push(run); return run; },
    runs: async (limit) => runs.slice(-limit).reverse(),
    claimMinute: async (w, n, m) => { const k = `${w}|${n}|${m.toISOString()}`; if (fired.has(k)) return false; fired.add(k); return true; },
  };
}
export type { Item };
"##;

const N8N_ENGINE: &str = r##"import type { Graph, Trigger } from "./import/n8n";

// The item is n8n's unit of data: { json, binary? }. A node takes the items on its inputs and
// returns one item list per output (an If node returns [true, false]). Expressions are the
// subset that reads cleanly without n8n's runtime: `={{ $json.a.b }}`, `{{ $env.X }}`,
// `$now`, `$input.first().json.x`, `$('Node').item.json.x`; anything else throws, naming the
// node, rather than evaluating half of it.

export type Item = { json: Record<string, unknown>; binary?: Record<string, unknown> };
export type Fetch = (url: string | URL, init?: RequestInit) => Promise<Response>;
export type Ctx = { env: Record<string, string | undefined>; fetch: Fetch; now: () => Date; outputs: Map<string, Item[]> };
export type NodeSpec = { name: string; type: string; typeVersion: number; params: Record<string, unknown> };

export class UnsupportedNode extends Error { constructor(public node: string, public type: string) { super(`unsupported n8n node ${type} ("${node}"): port it by hand`); } }
export class BadExpression extends Error { constructor(node: string, expr: string) { super(`cannot evaluate expression in "${node}": ${expr}`); } }

const path = (o: unknown, p: string): unknown => p.split(".").filter(Boolean).reduce<unknown>((v, k) => (v == null ? undefined : (v as Record<string, unknown>)[k]), o);

export function evaluate(node: string, raw: unknown, item: Item, ctx: Ctx): unknown {
  if (typeof raw !== "string") return raw;
  const s = raw.startsWith("=") ? raw.slice(1) : raw;
  const whole = /^\{\{\s*(.*?)\s*\}\}$/.exec(s);
  const one = (expr: string): unknown => {
    let m: RegExpExecArray | null;
    if ((m = /^\$json((?:\.[A-Za-z_$][\w$]*)*)$/.exec(expr))) return path(item.json, m[1]);
    if ((m = /^\$env\.([A-Za-z_][\w]*)$/.exec(expr))) return ctx.env[m[1]];
    if (expr === "$now" || expr === "$now.toISO()" || expr === "$today") return ctx.now().toISOString();
    if ((m = /^\$input\.(?:first|item)\(\)\.json((?:\.[A-Za-z_$][\w$]*)*)$/.exec(expr))) return path(item.json, m[1]);
    if ((m = /^\$\(['"](.+?)['"]\)\.(?:item|first\(\))\.json((?:\.[A-Za-z_$][\w$]*)*)$/.exec(expr))) return path(ctx.outputs.get(m[1])?.[0]?.json, m[2]);
    if (/^-?\d+(\.\d+)?$/.test(expr)) return Number(expr);
    if (/^(['"]).*\1$/.test(expr)) return expr.slice(1, -1);
    if (expr === "true" || expr === "false") return expr === "true";
    throw new BadExpression(node, expr);
  };
  if (whole) return one(whole[1]);
  if (!s.includes("{{")) return s;
  return s.replace(/\{\{\s*(.*?)\s*\}\}/g, (_, e) => String(one(e) ?? ""));
}

const compare = (op: { type?: string; operation?: string }, l: unknown, r: unknown): boolean => {
  const o = op.operation ?? "equals";
  const t = op.type ?? "string";
  if (o === "exists") return l != null; if (o === "notExists") return l == null;
  if (o === "true") return l === true; if (o === "false") return l === false;
  if (o === "isEmpty") return l == null || l === ""; if (o === "isNotEmpty") return l != null && l !== "";
  const [a, b] = t === "number" ? [Number(l), Number(r)] : t === "dateTime" ? [new Date(String(l)).getTime(), new Date(String(r)).getTime()] : [String(l ?? ""), String(r ?? "")];
  switch (o) {
    case "equals": case "equal": return a === b;
    case "notEquals": case "notEqual": return a !== b;
    case "contains": return String(a).includes(String(b));
    case "notContains": return !String(a).includes(String(b));
    case "startsWith": return String(a).startsWith(String(b));
    case "endsWith": return String(a).endsWith(String(b));
    case "gt": case "larger": case "after": return a > b;
    case "gte": case "largerEqual": case "afterOrEquals": return a >= b;
    case "lt": case "smaller": case "before": return a < b;
    case "lte": case "smallerEqual": case "beforeOrEquals": return a <= b;
    default: throw new Error(`unsupported If operation ${o}`);
  }
};

// One node, one step. Returns the items per output.
export async function step(n: NodeSpec, items: Item[], ctx: Ctx): Promise<Item[][]> {
  const kind = n.type.replace(/^n8n-nodes-base\./, "");
  const p = n.params as Record<string, unknown>;
  switch (kind) {
    case "webhook": case "scheduleTrigger": case "cron": case "manualTrigger": case "noOp": case "merge":
      return [items];
    case "set": {
      // v3+ `assignments.assignments[{name, value, type}]`; v1-2 `values.{string,number,boolean}[{name,value}]`.
      const a = (p as { assignments?: { assignments?: { name: string; value: unknown; type?: string }[] } }).assignments?.assignments
        ?? Object.entries((p as { values?: Record<string, { name: string; value: unknown }[]> }).values ?? {}).flatMap(([type, vs]) => vs.map((v) => ({ ...v, type })));
      const keep = (p as { includeOtherFields?: boolean; options?: { include?: string } }).includeOtherFields ?? (p as { keepOnlySet?: boolean }).keepOnlySet === false;
      return [items.map((it) => {
        const json: Record<string, unknown> = keep ? { ...it.json } : {};
        for (const x of a ?? []) { const v = evaluate(n.name, x.value, it, ctx); json[x.name] = x.type === "number" ? Number(v) : x.type === "boolean" ? v === true || v === "true" : v; }
        return { json };
      })];
    }
    case "if": {
      const c = (p as { conditions?: { combinator?: string; conditions?: { leftValue: unknown; rightValue?: unknown; operator: { type?: string; operation?: string } }[] } }).conditions;
      const yes: Item[] = [], no: Item[] = [];
      for (const it of items) {
        const rs = (c?.conditions ?? []).map((k) => compare(k.operator, evaluate(n.name, k.leftValue, it, ctx), evaluate(n.name, k.rightValue, it, ctx)));
        ((c?.combinator === "or" ? rs.some(Boolean) : rs.every(Boolean)) ? yes : no).push(it);
      }
      return [yes, no];
    }
    case "httpRequest": {
      const q = p as { url: string; method?: string; sendBody?: boolean; jsonBody?: unknown; bodyParameters?: { parameters?: { name: string; value: unknown }[] }; headerParameters?: { parameters?: { name: string; value: unknown }[] }; queryParameters?: { parameters?: { name: string; value: unknown }[] }; options?: { response?: { response?: { fullResponse?: boolean } } } };
      const out: Item[] = [];
      for (const it of items) {
        const url = new URL(String(evaluate(n.name, q.url, it, ctx)));
        for (const kv of q.queryParameters?.parameters ?? []) url.searchParams.set(kv.name, String(evaluate(n.name, kv.value, it, ctx)));
        const headers: Record<string, string> = {};
        for (const kv of q.headerParameters?.parameters ?? []) headers[kv.name] = String(evaluate(n.name, kv.value, it, ctx));
        let body: string | undefined;
        if (q.sendBody) {
          headers["content-type"] ??= "application/json";
          const b = q.jsonBody !== undefined ? evaluate(n.name, q.jsonBody, it, ctx) : Object.fromEntries((q.bodyParameters?.parameters ?? []).map((kv) => [kv.name, evaluate(n.name, kv.value, it, ctx)]));
          body = typeof b === "string" ? b : JSON.stringify(b);
        }
        const res = await ctx.fetch(url, { method: q.method ?? "GET", headers, body });
        const text = await res.text();
        let json: unknown; try { json = JSON.parse(text); } catch { json = text; }
        if (!res.ok) throw new Error(`"${n.name}": ${q.method ?? "GET"} ${url.origin}${url.pathname} → ${res.status}`);
        out.push({ json: q.options?.response?.response?.fullResponse ? { body: json, headers: Object.fromEntries(res.headers), statusCode: res.status } : (json && typeof json === "object" && !Array.isArray(json) ? (json as Record<string, unknown>) : { data: json }) });
      }
      return [out];
    }
    case "code":
      // n8n ran jsCode in a sandbox. Evaluating it here would be arbitrary code from an
      // uploaded file running in this process, so it is never executed: the importer writes
      // the original source into the node's stub for a person to port into a tested function.
      throw new UnsupportedNode(n.name, `${n.type} (port the jsCode in src/workflows by hand)`);
    case "stickyNote": return [items];
    default: throw new UnsupportedNode(n.name, n.type);
  }
}

// Walk the graph from a trigger, breadth-first along the connections, feeding each node the
// items that reached it. A node with several inputs (Merge) runs once per delivery — enough
// for the linear and branching workflows this covers. Returns the last node's output.
export async function run(g: Graph, trigger: Trigger, input: Item[], ctx: Ctx): Promise<{ output: Item[]; steps: string[] }> {
  const byName = new Map(g.nodes.map((n) => [n.name, n]));
  const steps: string[] = [];
  let last: Item[] = input;
  const queue: { name: string; items: Item[] }[] = [{ name: trigger.node, items: input }];
  while (queue.length) {
    const { name, items } = queue.shift()!;
    const n = byName.get(name)!;
    if (!n.kind) throw new UnsupportedNode(n.name, n.type);
    if (n.kind === "stickyNote") continue;
    const outs = await step({ name: n.name, type: n.type, typeVersion: n.typeVersion, params: n.params }, items, ctx);
    steps.push(n.fn);
    ctx.outputs.set(n.name, outs[0] ?? []);
    last = outs[0] ?? [];
    for (const e of g.edges) if (e.from === name && (outs[e.output] ?? []).length) queue.push({ name: e.to, items: outs[e.output] });
  }
  return { output: last, steps };
}

// Five-field cron: `*`, `n`, `a,b`, `a-b`, `*/n`. Weekday 0 and 7 are both Sunday.
export function cronMatches(expr: string, d: Date): boolean {
  const f = expr.trim().split(/\s+/);
  if (f.length !== 5) return false;
  const vals = [d.getUTCMinutes(), d.getUTCHours(), d.getUTCDate(), d.getUTCMonth() + 1, d.getUTCDay()];
  return f.every((spec, i) => spec.split(",").some((part) => {
    const [range, stepS] = part.split("/");
    const stepN = stepS ? Number(stepS) : 1;
    let lo = 0, hi = [59, 23, 31, 12, 7][i];
    if (range !== "*") { const [a, b] = range.split("-").map(Number); lo = a; hi = b ?? a; }
    const v = i === 4 && vals[i] === 0 && lo === 7 ? 7 : vals[i];
    return v >= lo && v <= hi && (v - lo) % stepN === 0;
  }));
}
"##;

const N8N_SCHEDULER: &str = r##"import app from "./app";

// The scheduler: one tick a minute, aligned to the minute, in its own process (`bun run
// scheduler`) so it scales apart from HTTP. Several replicas are safe: the store hands each
// (workflow, node, minute) to exactly one of them.
console.log(JSON.stringify({ level: "info", msg: "scheduler started" }));
let stopping = false;
process.on("SIGTERM", () => { stopping = true; });
while (!stopping) {
  const fired = await app.tick();
  if (fired.length) console.log(JSON.stringify({ level: "info", msg: "fired", fired }));
  await new Promise((r) => setTimeout(r, 60_000 - (Date.now() % 60_000)));
}
"##;

const N8N_IMPORT: &str = r##"import { z } from "zod";

// n8n's export, as the editor's "Download" and the REST API write it: `nodes` with `name`,
// `type` (n8n-nodes-base.webhook, .scheduleTrigger, .cron, .httpRequest, .set, .if, .code,
// .noOp, .merge, …), `typeVersion`, `parameters` (per-type; versions differ) and `credentials`
// ({ httpHeaderAuth: { id, name } } — never a secret, the secret lives in n8n's own store), and
// `connections`: { [fromNodeName]: { main: [ [ { node, type, index } ] ] } } — the outer array is
// the from-node's output index (an If node has two), the inner array its targets. A file may be
// one workflow, an array of them, or { workflows: [...] }.

const Conn = z.object({ node: z.string(), type: z.string().default("main"), index: z.number().int().default(0) });
const Node = z.object({
  name: z.string(),
  type: z.string(),
  typeVersion: z.number().optional(),
  parameters: z.record(z.unknown()).default({}),
  credentials: z.record(z.object({ id: z.string().nullable().optional(), name: z.string().optional() })).nullish(),
  disabled: z.boolean().optional(),
});
export const Workflow = z.object({
  id: z.string().optional(),
  name: z.string(),
  active: z.boolean().nullish(),
  nodes: z.array(Node),
  connections: z.record(z.object({ main: z.array(z.array(Conn).nullable()).optional() }).passthrough()).default({}),
  settings: z.record(z.unknown()).optional(),
});
export const N8nExport = z.union([z.array(Workflow), z.object({ workflows: z.array(Workflow) }), Workflow]);
export type N8nNode = z.infer<typeof Node>;

// What the engine and the stubs know how to do. Anything else is imported, named in the report
// and on the page, and fails the run loudly when reached.
export const SUPPORTED = ["webhook", "scheduleTrigger", "cron", "manualTrigger", "httpRequest", "set", "if", "noOp", "merge", "stickyNote"] as const;
export type Kind = (typeof SUPPORTED)[number];

export type Trigger = { node: string; fn: string } & ({ kind: "webhook"; method: string; path: string; responseMode: "onReceived" | "lastNode" } | { kind: "cron"; cron: string } | { kind: "manual" });
export type Edge = { from: string; output: number; to: string; input: number };
export type Graph = {
  name: string;
  active: boolean;
  nodes: { name: string; fn: string; type: string; kind: Kind | null; typeVersion: number; params: Record<string, unknown>; credentials: { type: string; name: string; env: string }[] }[];
  edges: Edge[];
  triggers: Trigger[];
  unsupported: { node: string; type: string }[];
  env: string[];
};

// Node names become function names, so "Get new accessToken" ↔ get_new_accessToken lines up
// in MIGRATION.md and in a stack trace.
export const fnName = (s: string) => { const id = s.replace(/[^A-Za-z0-9_$]+/g, "_").replace(/^_+|_+$/g, ""); return /^[A-Za-z_$]/.test(id) ? id : "n_" + id; };
export const envName = (s: string) => s.toUpperCase().replace(/[^A-Z0-9]+/g, "_").replace(/^_+|_+$/g, "");
const kindOf = (type: string): Kind | null => { const k = type.replace(/^n8n-nodes-base\./, ""); return (SUPPORTED as readonly string[]).includes(k) ? (k as Kind) : null; };

// n8n's two schedule shapes to one cron line. Schedule Trigger: rule.interval[{field, …Interval,
// triggerAtHour, triggerAtMinute, expression}]; the older Cron node: triggerTimes.item[{mode, hour, minute, …}].
export function cronOf(n: Pick<N8nNode, "name" | "type" | "parameters">): string {
  const p = n.parameters as { rule?: { interval?: Record<string, unknown>[] }; triggerTimes?: { item?: Record<string, unknown>[] } };
  const one = (i: Record<string, unknown>): string => {
    const num = (k: string, d: number) => (typeof i[k] === "number" ? (i[k] as number) : d);
    const field = (i.field as string | undefined) ?? (i.mode as string | undefined) ?? "days";
    const h = num("triggerAtHour", num("hour", 0)), m = num("triggerAtMinute", num("minute", 0));
    switch (field) {
      case "cronExpression": case "custom": return String(i.expression ?? i.cronExpression ?? "* * * * *").split(" ").slice(-5).join(" ");
      case "seconds": return "* * * * *";
      case "minutes": case "everyMinute": return `*/${num("minutesInterval", 1)} * * * *`;
      case "hours": case "everyHour": return `${m} */${num("hoursInterval", 1)} * * *`;
      case "days": case "everyDay": return `${m} ${h} */${num("daysInterval", 1)} * *`;
      case "weeks": case "everyWeek": return `${m} ${h} * * ${((i.triggerAtDay as number[] | undefined) ?? [num("weekday", 1)]).join(",")}`;
      case "months": case "everyMonth": return `${m} ${h} ${num("triggerAtDayOfMonth", num("dayOfMonth", 1))} */${num("monthsInterval", 1)} *`;
      default: return "* * * * *";
    }
  };
  const items = p.rule?.interval ?? p.triggerTimes?.item ?? [{}];
  return items.map(one).join(" | ");
}

export function parseWorkflow(raw: unknown): Graph {
  const w = Workflow.parse(raw);
  const nodes = w.nodes.filter((n) => !n.disabled).map((n) => ({
    name: n.name, fn: fnName(n.name), type: n.type, kind: kindOf(n.type), typeVersion: n.typeVersion ?? 1, params: n.parameters,
    credentials: Object.entries(n.credentials ?? {}).map(([type, c]) => ({ type, name: c.name ?? type, env: envName(c.name ?? type) })),
  }));
  const names = new Set(nodes.map((n) => n.name));
  const edges: Edge[] = [];
  for (const [from, outs] of Object.entries(w.connections)) for (const [output, targets] of (outs.main ?? []).entries()) for (const t of targets ?? []) if (names.has(from) && names.has(t.node)) edges.push({ from, output, to: t.node, input: t.index });
  const triggers: Trigger[] = [];
  for (const n of nodes) {
    if (n.kind === "webhook") {
      const p = n.params as { path?: string; httpMethod?: string; responseMode?: string };
      triggers.push({ kind: "webhook", node: n.name, fn: n.fn, method: (p.httpMethod ?? "GET").toUpperCase(), path: "/" + String(p.path ?? n.fn).replace(/^\/+/, ""), responseMode: p.responseMode === "lastNode" ? "lastNode" : "onReceived" });
    } else if (n.kind === "scheduleTrigger" || n.kind === "cron") triggers.push({ kind: "cron", node: n.name, fn: n.fn, cron: cronOf({ name: n.name, type: n.type, parameters: n.params }) });
    else if (n.kind === "manualTrigger") triggers.push({ kind: "manual", node: n.name, fn: n.fn });
  }
  const unsupported = nodes.filter((n) => !n.kind).map((n) => ({ node: n.name, type: n.type }));
  const env = [...new Set([...nodes.flatMap((n) => n.credentials.map((c) => c.env)), ...[...JSON.stringify(w.nodes).matchAll(/\$env\.([A-Za-z_][A-Za-z0-9_]*)/g)].map((m) => m[1])])].sort();
  return { name: w.name, active: w.active ?? false, nodes, edges, triggers, unsupported, env };
}

export function parse(raw: unknown): Graph[] {
  const x = N8nExport.parse(raw);
  const list = Array.isArray(x) ? x : "workflows" in x ? x.workflows : [x];
  return list.map(parseWorkflow);
}

// Function stubs: one per node, named for it, typed on n8n's item shape. Supported kinds get
// a body that calls the engine's step; the rest throw so a port cannot silently skip them.
export function stubs(g: Graph): string {
  const L = [
    `// Generated from the n8n workflow "${g.name}". Regenerate with: bun src/import/n8n.ts <workflows.json>`,
    `import { step, type Item, type Ctx } from "../engine";`,
    ``,
    `// Credentials by name, never by value. Set these in backend/.env:`,
    ...g.env.map((e) => `//   ${e}=`),
    ``,
  ];
  for (const n of g.nodes) {
    if (n.kind === "stickyNote") continue;
    L.push(`/** n8n node "${n.name}" (${n.type} v${n.typeVersion})${n.credentials.length ? ` — credentials: ${n.credentials.map((c) => `${c.type} → $${c.env}`).join(", ")}` : ""} */`);
    L.push(`export async function ${n.fn}(items: Item[], ctx: Ctx): Promise<Item[][]> {`);
    if (n.type.endsWith(".code")) L.push(`  // Original jsCode (n8n ran this in a sandbox; port it, never eval it):`, ...String(n.params.jsCode ?? "").split("\n").map((l) => `  //   ${l}`));
    if (n.kind) L.push(`  return step(${JSON.stringify({ name: n.name, type: n.type, typeVersion: n.typeVersion, params: n.params })}, items, ctx);`);
    else L.push(`  throw new Error(${JSON.stringify(`unsupported n8n node ${n.type} ("${n.name}"): port it by hand`)});`);
    L.push(`}`, ``);
  }
  return L.join("\n");
}

export function report(gs: Graph[]): string {
  const L = [`# Migration from n8n`, ``];
  for (const g of gs) {
    L.push(`## ${g.name}${g.active ? "" : " (inactive in n8n)"}`, ``, `| Node | Type | Code |`, `|---|---|---|`);
    for (const n of g.nodes) {
      const t = g.triggers.find((t) => t.node === n.name);
      const where = t?.kind === "webhook" ? `\`${t.method} /webhook${t.path}\` route → \`${n.fn}()\`` : t?.kind === "cron" ? `schedule \`${t.cron}\` → \`${n.fn}()\`` : n.kind === "stickyNote" ? "note, dropped" : n.kind ? `\`${n.fn}()\`` : `**unsupported** — \`${n.fn}()\` throws until ported`;
      L.push(`| ${n.name} | ${n.type} | ${where}${n.credentials.length ? ` (env: ${n.credentials.map((c) => c.env).join(", ")})` : ""} |`);
    }
    L.push(``, `Edges: ${g.edges.map((e) => `${e.from}[${e.output}] → ${e.to}`).join(", ") || "none"}`, ``);
    if (g.env.length) L.push(`Environment: ${g.env.map((e) => `\`${e}\``).join(", ")}`, ``);
  }
  return L.join("\n");
}

// `bun src/import/n8n.ts <workflows.json>` writes one stub module per workflow and MIGRATION.md.
if (import.meta.main) {
  const { readFile, writeFile, mkdir } = await import("node:fs/promises");
  const file = process.argv[2];
  if (!file) { console.error("usage: bun src/import/n8n.ts <n8n-workflows.json>"); process.exit(2); }
  const gs = parse(JSON.parse(await readFile(file, "utf8")));
  await mkdir("src/workflows", { recursive: true });
  for (const g of gs) await writeFile(`src/workflows/${fnName(g.name).toLowerCase()}.ts`, stubs(g));
  await writeFile("../MIGRATION.md", report(gs));
  console.log(`${gs.length} workflows, ${gs.reduce((n, g) => n + g.nodes.length, 0)} nodes, ${gs.reduce((n, g) => n + g.unsupported.length, 0)} unsupported → src/workflows/*.ts, MIGRATION.md`);
}
"##;

const N8N_FIXTURE: &str = r##"// Two exported n8n workflows as the editor downloads them: a webhook → set → if → http flow
// with a credential and a Code node, and a schedule → unsupported Slack node.
export const FIXTURE = [
  {
    name: "Lead intake",
    active: true,
    nodes: [
      { parameters: { httpMethod: "POST", path: "lead", responseMode: "lastNode", options: {} }, name: "Webhook", type: "n8n-nodes-base.webhook", typeVersion: 2, position: [0, 0], webhookId: "a1" },
      { parameters: { assignments: { assignments: [
        { id: "1", name: "email", value: "={{ $json.body.email }}", type: "string" },
        { id: "2", name: "score", value: "={{ $json.body.score }}", type: "number" },
        { id: "3", name: "source", value: "webhook", type: "string" },
      ] }, includeOtherFields: false, options: {} }, name: "Shape lead", type: "n8n-nodes-base.set", typeVersion: 3.4, position: [200, 0] },
      { parameters: { conditions: { options: { caseSensitive: true, leftValue: "", typeValidation: "strict", version: 2 }, conditions: [
        { id: "c1", leftValue: "={{ $json.score }}", rightValue: 50, operator: { type: "number", operation: "gte" } },
      ], combinator: "and" }, options: {} }, name: "Is hot?", type: "n8n-nodes-base.if", typeVersion: 2.2, position: [400, 0] },
      { parameters: { method: "POST", url: "={{ $env.CRM_URL }}/leads", authentication: "genericCredentialType", genericAuthType: "httpHeaderAuth", sendBody: true, specifyBody: "json", jsonBody: "={{ $json }}", options: {} }, name: "Create in CRM", type: "n8n-nodes-base.httpRequest", typeVersion: 4.2, position: [600, -100], credentials: { httpHeaderAuth: { id: "7", name: "CRM API key" } } },
      { parameters: { jsCode: "return $input.all().map(i => ({ json: { ...i.json, archived: true } }));" }, name: "Archive cold", type: "n8n-nodes-base.code", typeVersion: 2, position: [600, 100] },
      { parameters: { content: "Leads: hot ones go to the CRM.", height: 80, width: 200 }, name: "Sticky Note", type: "n8n-nodes-base.stickyNote", typeVersion: 1, position: [0, -200] },
    ],
    connections: {
      Webhook: { main: [[{ node: "Shape lead", type: "main", index: 0 }]] },
      "Shape lead": { main: [[{ node: "Is hot?", type: "main", index: 0 }]] },
      "Is hot?": { main: [[{ node: "Create in CRM", type: "main", index: 0 }], [{ node: "Archive cold", type: "main", index: 0 }]] },
    },
    settings: { executionOrder: "v1" },
  },
  {
    name: "Daily digest",
    active: true,
    nodes: [
      { parameters: { rule: { interval: [{ field: "days", daysInterval: 1, triggerAtHour: 9, triggerAtMinute: 30 }] } }, name: "Every morning", type: "n8n-nodes-base.scheduleTrigger", typeVersion: 1.2, position: [0, 0] },
      { parameters: { select: "channel", channelId: { value: "C1" }, text: "digest" }, name: "Post to Slack", type: "n8n-nodes-base.slack", typeVersion: 2.2, position: [200, 0], credentials: { slackApi: { id: "3", name: "Slack bot" } } },
    ],
    connections: { "Every morning": { main: [[{ node: "Post to Slack", type: "main", index: 0 }]] } },
  },
];
"##;

const N8N_TEST: &str = r##"import { describe, expect, test } from "bun:test";
import { createApp } from "./app";
import { memoryStore } from "./store";
import { parse, stubs, report, cronOf } from "./import/n8n";
import { cronMatches } from "./engine";
import { FIXTURE } from "./import/fixture";

process.env.ADMIN_TOKEN = "admin";
const json = (body: unknown, method = "POST") => ({ method, headers: { "content-type": "application/json", authorization: "Bearer admin" }, body: JSON.stringify(body) });

describe("n8n import", () => {
  test("nodes become functions named for them; credentials become env names, never values", () => {
    const [lead, digest] = parse(FIXTURE);
    expect(lead.nodes.map((n) => n.fn)).toEqual(["Webhook", "Shape_lead", "Is_hot", "Create_in_CRM", "Archive_cold", "Sticky_Note"]);
    expect(lead.triggers).toEqual([{ kind: "webhook", node: "Webhook", fn: "Webhook", method: "POST", path: "/lead", responseMode: "lastNode" }]);
    expect(lead.edges).toContainEqual({ from: "Is hot?", output: 1, to: "Archive cold", input: 0 });
    expect(lead.env).toEqual(["CRM_API_KEY", "CRM_URL"]);
    // A Code node is never evaluated: it is listed for porting, with its source in the stub.
    expect(lead.unsupported).toEqual([{ node: "Archive cold", type: "n8n-nodes-base.code" }]);
    expect(digest.triggers).toEqual([{ kind: "cron", node: "Every morning", fn: "Every_morning", cron: "30 9 */1 * *" }]);
    expect(digest.unsupported).toEqual([{ node: "Post to Slack", type: "n8n-nodes-base.slack" }]);
    const src = stubs(lead);
    expect(src).toContain("export async function Create_in_CRM(items: Item[], ctx: Ctx)");
    expect(src).toContain("httpHeaderAuth → $CRM_API_KEY");
    expect(src).toContain("Original jsCode");
    expect(src).not.toContain('"id": "7"');
    expect(stubs(digest)).toContain('throw new Error("unsupported n8n node n8n-nodes-base.slack (\\"Post to Slack\\"): port it by hand")');
    const md = report([lead, digest]);
    expect(md).toContain("| Webhook | n8n-nodes-base.webhook | `POST /webhook/lead` route → `Webhook()` |");
    expect(md).toContain("| Post to Slack | n8n-nodes-base.slack | **unsupported**");
    expect(cronOf({ name: "c", type: "n8n-nodes-base.cron", parameters: { triggerTimes: { item: [{ mode: "everyHour", minute: 15 }] } } })).toBe("15 */1 * * *");
    expect(cronOf({ name: "c", type: "x", parameters: { rule: { interval: [{ field: "cronExpression", expression: "0 */6 * * 1-5" }] } } })).toBe("0 */6 * * 1-5");
  });

  test("a webhook route runs set → if → http with the mapped steps and answers the last node", async () => {
    const calls: { url: string; init?: RequestInit }[] = [];
    const app = createApp(memoryStore(), {
      env: { CRM_URL: "https://crm.example", CRM_API_KEY: "shh" },
      fetch: async (url, init) => { calls.push({ url: String(url), init }); return new Response(JSON.stringify({ id: "L1" }), { headers: { "content-type": "application/json" } }); },
    });
    expect((await app.request("/api/import", json(FIXTURE))).status).toBe(201);
    const hot = await app.request("/webhook/lead", json({ email: "a@b.c", score: 80 }));
    expect(hot.status).toBe(200);
    expect(await hot.json()).toEqual({ id: "L1" });
    expect(calls[0].url).toBe("https://crm.example/leads");
    expect(JSON.parse(calls[0].init!.body as string)).toEqual({ email: "a@b.c", score: 80, source: "webhook" });
    // The cold branch reaches the Code node, which fails loudly rather than running uploaded JS.
    const cold = await app.request("/webhook/lead", json({ email: "z@b.c", score: 5 }));
    expect(cold.status).toBe(500);
    expect((await cold.json()).error.message).toContain("Archive cold");
    expect(calls.length).toBe(1);
    const runs = (await (await app.request("/api/runs")).json()).runs;
    expect(runs[0].status).toBe("failed");
    expect(runs[1].steps).toEqual(["Webhook", "Shape_lead", "Is_hot", "Create_in_CRM"]);
    expect((await app.request("/webhook/nope", { method: "POST" })).status).toBe(404);
    expect((await app.request("/webhook/lead")).status).toBe(404); // GET is not the node's method
  });

  test("an unsupported node fails the run loudly instead of skipping", async () => {
    const app = createApp(memoryStore(), { now: () => new Date("2024-03-04T09:30:10Z") });
    await app.request("/api/import", json(FIXTURE));
    const res = await app.request("/api/workflows/Daily%20digest/run", json({}));
    expect(res.status).toBe(500);
    expect((await res.json()).run.error).toBe('unsupported n8n node n8n-nodes-base.slack ("Post to Slack"): port it by hand');
    const listed = (await (await app.request("/api/workflows")).json()).workflows.find((w: { name: string }) => w.name === "Daily digest");
    expect(listed.unsupported).toEqual([{ node: "Post to Slack", type: "n8n-nodes-base.slack" }]);
  });

  test("a cron trigger fires once per matching minute across ticks and replicas", async () => {
    const store = memoryStore();
    const a = createApp(store, { now: () => new Date("2024-03-04T09:30:10Z") });
    const b = createApp(store, { now: () => new Date("2024-03-04T09:30:50Z") });
    await a.request("/api/import", json(FIXTURE));
    expect(await a.tick()).toEqual(["Daily digest/Every morning"]);
    expect(await b.tick()).toEqual([]); // same minute, other replica
    expect(await a.tick(new Date("2024-03-04T09:31:00Z"))).toEqual([]);
    expect(await a.tick(new Date("2024-03-05T09:30:00Z"))).toEqual(["Daily digest/Every morning"]);
    const runs = await store.runs(10);
    expect(runs.length).toBe(2);
    expect(runs[0].status).toBe("failed"); // the Slack node, loudly
    expect(cronMatches("*/15 * * * 1-5", new Date("2024-03-04T10:45:00Z"))).toBe(true);
    expect(cronMatches("*/15 * * * 1-5", new Date("2024-03-03T10:45:00Z"))).toBe(false);
  });

  test("an expression the engine cannot evaluate names the node", async () => {
    const app = createApp(memoryStore());
    await app.request("/api/import", json([{ name: "w", active: true, nodes: [
      { name: "Manual", type: "n8n-nodes-base.manualTrigger", parameters: {} },
      { name: "Odd", type: "n8n-nodes-base.set", typeVersion: 3.4, parameters: { assignments: { assignments: [{ name: "x", value: "={{ $json.a.map(v => v * 2) }}", type: "string" }] } } },
    ], connections: { Manual: { main: [[{ node: "Odd", type: "main", index: 0 }]] } } }]));
    const res = await app.request("/api/workflows/w/run", json({ a: [1] }));
    expect(res.status).toBe(500);
    expect((await res.json()).run.error).toBe('cannot evaluate expression in "Odd": $json.a.map(v => v * 2)');
  });
});
"##;

const N8N_SQL: &str = r##"-- Imported workflows as parsed graphs, and every run of one: which trigger, what came in,
-- what the last node produced or which node failed. The run row is the audit n8n's
-- executions list gave you.
create table if not exists workflows (
  name text primary key,
  graph jsonb not null,
  active boolean not null default true,
  imported_at timestamptz not null default now()
);
create table if not exists runs (
  id bigserial primary key,
  workflow text not null references workflows(name) on delete cascade,
  trigger text not null,          -- webhook:/path | cron:<expr> | manual
  status text not null,           -- ok | failed
  steps jsonb not null default '[]',
  input jsonb,
  output jsonb,
  error text,
  started_at timestamptz not null default now(),
  finished_at timestamptz
);
create index if not exists runs_recent on runs (started_at desc);
-- Which minute each cron trigger last fired, so two replicas do not both fire it.
create table if not exists cron_fired (
  workflow text not null,
  node text not null,
  minute timestamptz not null,
  primary key (workflow, node, minute)
);
"##;

const N8N_PAGE: &str = r##"const API = process.env.API_URL ?? "http://127.0.0.1:8000";
type Trigger = { kind: "webhook"; method: string; path: string } | { kind: "cron"; cron: string } | { kind: "manual" };
type Workflow = { name: string; active: boolean; nodes: { name: string; fn: string; type: string; supported: boolean }[]; triggers: (Trigger & { node: string })[]; unsupported: { node: string; type: string }[]; env: string[] };
type Run = { id: number; workflow: string; trigger: string; status: string; steps: string[]; error: string | null; started_at: string };

async function load() {
  const [w, r] = await Promise.all([fetch(`${API}/api/workflows`, { cache: "no-store" }), fetch(`${API}/api/runs`, { cache: "no-store" })]);
  return { workflows: (await w.json()).workflows as Workflow[], runs: (await r.json()).runs as Run[] };
}

// Every imported workflow: its triggers as the routes and schedules they became, the env
// names its credentials need, and the nodes that still need a hand — then the recent runs.
export default async function Home() {
  const { workflows, runs } = await load();
  return (
    <main>
      <h1>{{NAME}} — port from n8n</h1>
      <p>Import: <code>{`curl -X POST http://localhost:8000/api/import -H "authorization: Bearer $ADMIN_TOKEN" -F file=@workflows.json`}</code> — webhooks answer at <code>/webhook/&lt;path&gt;</code>, schedules run from <code>bun src/scheduler.ts</code>, and <code>bun src/import/n8n.ts workflows.json</code> in backend/ writes the function stubs and MIGRATION.md.</p>
      {workflows.length === 0 && <p>Nothing imported yet.</p>}
      {workflows.map((w) => (
        <section key={w.name}>
          <h2>{w.name}{w.active ? "" : " (inactive)"}</h2>
          <ul>
            {w.triggers.map((t) => <li key={t.node}>{t.node}: {t.kind === "webhook" ? <code>{t.method} /webhook{t.path}</code> : t.kind === "cron" ? <>schedule <code>{t.cron}</code></> : "manual (POST /api/workflows/" + w.name + "/run)"}</li>)}
          </ul>
          <table>
            <thead><tr><th>node</th><th>type</th><th>function</th></tr></thead>
            <tbody>{w.nodes.map((n) => <tr key={n.name}><td>{n.name}</td><td>{n.type}</td><td>{n.supported ? <code>{n.fn}()</code> : <b>unsupported — port by hand</b>}</td></tr>)}</tbody>
          </table>
          {w.env.length > 0 && <p>Environment: {w.env.map((e) => <code key={e}>{e} </code>)}</p>}
        </section>
      ))}
      <h2>Recent runs</h2>
      <table>
        <thead><tr><th>id</th><th>workflow</th><th>trigger</th><th>status</th><th>steps</th><th>error</th></tr></thead>
        <tbody>{runs.map((r) => <tr key={r.id}><td>{r.id}</td><td>{r.workflow}</td><td>{r.trigger}</td><td>{r.status}</td><td>{r.steps.join(" → ")}</td><td>{r.error ?? ""}</td></tr>)}</tbody>
      </table>
    </main>
  );
}
"##;
