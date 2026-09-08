//! bubble: port from Bubble.io — the Data API's meta + rows in, tables, schemas and CRUD out ═══════════════════════════════

pub fn files(name: &str) -> Vec<(&'static str, String)> {
    let f = |s: &str| s.replace("{{NAME}}", name);
    vec![
        ("backend/src/app.ts", BUBBLE_APP.into()),
        ("backend/src/store.ts", BUBBLE_STORE.into()),
        ("backend/src/import/bubble.ts", BUBBLE_IMPORT.into()),
        ("backend/src/import/fixture.ts", BUBBLE_FIXTURE.into()),
        ("backend/src/app.test.ts", BUBBLE_TEST.into()),
        ("backend/migrations/0002_bubble.sql", BUBBLE_SQL.into()),
        ("frontend/app/page.tsx", f(BUBBLE_PAGE)),
        ("CLAUDE.md", f(BUBBLE_CLAUDE_MD)),
        ("AGENTS.md", f(BUBBLE_AGENTS_MD)),
        ("README.md", f(BUBBLE_README_MD)),
        (
            ".claude/agents/data-mapping.md",
            BUBBLE_AGENT_MAPPING.into(),
        ),
    ]
}

const BUBBLE_APP: &str = r##"import { Hono } from "hono";
import { z } from "zod";
import { parse, report, sql, zodSource, normaliseRow, runtimeSchema, type Model } from "./import/bubble";
import { pgStore, type Store } from "./store";

// Bubble's surface, kept so the port is a swap and not a rewrite: the Data API's routes
// (/api/1.1/obj/<type>, GET list with cursor/limit answered as { response: { cursor, results,
// count, remaining } }, GET/PATCH/DELETE one, POST one returning { id }) live here as
// /api/obj/<type>, generated at runtime from the imported types and validated with the same
// mapping the migration was written from. POST /api/import takes the export (JSON body or a
// multipart `file`) and returns the mapping report; the SQL and zod it would write are in
// the response too, so the agent can read them before running the CLI.

export function createApp(store: Store) {
  // The current model, loaded from the store on first use so a restart keeps serving.
  let model: Model | null = null;
  const current = async () => model ?? (model = (await store.lastImport())?.model ?? null);
  const typeOf = async (id: string) => (await current())?.types.find((t) => t.id === id) ?? null;
  const schemaOf = async (id: string) => { const m = await current(); const t = m?.types.find((t) => t.id === id); return t && m ? runtimeSchema(t, m.optionSets) : null; };
  const notFound = (c: { json: (b: unknown, s: 404) => Response }) => c.json({ error: { message: "no such type; import an export first", code: "not_found" } }, 404);

  const app = new Hono()
    .get("/api/health", (c) => c.json({ status: "ok" }))
    .get("/api/health/ready", (c) => c.json({ status: "ok", db: "ok" as const }))

    .post("/api/import", async (c) => {
      let raw: unknown;
      try {
        if (c.req.header("content-type")?.includes("multipart/form-data")) {
          const f = (await c.req.parseBody())["file"];
          if (!(f instanceof File)) return c.json({ error: { message: "multipart needs a `file` part", code: "invalid" } }, 400);
          raw = JSON.parse(await f.text());
        } else raw = await c.req.json();
      } catch { return c.json({ error: { message: "not JSON", code: "invalid" } }, 400); }
      let m: Model;
      try { m = parse(raw); } catch (e) { return c.json({ error: { message: e instanceof z.ZodError ? e.issues.map((i) => `${i.path.join(".")}: ${i.message}`).join("; ") : String(e), code: "invalid" } }, 400); }
      const rows: Record<string, Record<string, unknown>[]> = {};
      for (const t of m.types) if (m.rows[t.id]) rows[t.id] = m.rows[t.id].map((r) => normaliseRow(t, r));
      const md = report(m);
      await store.saveImport(m, md, rows);
      model = m;
      return c.json({ app: m.app, types: m.types.length, optionSets: m.optionSets.length, workflows: m.workflows.length, pages: m.pages.length, rows: Object.fromEntries(Object.entries(rows).map(([k, v]) => [k, v.length])), unmapped: m.unmapped, report: md, sql: sql(m), zod: zodSource(m) }, 201);
    })

    .get("/api/import", async (c) => {
      const last = await store.lastImport();
      if (!last) return c.json({ imported: false as const, counts: {} as Record<string, number> });
      const { model: m, report: md, imported_at } = last;
      return c.json({ imported: true as const, imported_at, app: m.app, report: md, counts: await store.counts(), types: m.types.map((t) => ({ id: t.id, display: t.display, table: t.table, fields: t.fields.length })), optionSets: m.optionSets, workflows: m.workflows.map((w) => ({ endpoint: w.endpoint, route: w.route, params: w.params.length })), pages: m.pages, unmapped: m.unmapped });
    })

    // The Data API, over the jsonb table, for whatever the export declared.
    .get("/api/obj/:type", async (c) => {
      if (!(await typeOf(c.req.param("type")))) return notFound(c);
      const q = z.object({ cursor: z.coerce.number().int().min(0).default(0), limit: z.coerce.number().int().min(1).max(100).default(100) }).safeParse(c.req.query());
      if (!q.success) return c.json({ error: { message: "bad cursor/limit", code: "invalid" } }, 400);
      return c.json({ response: await store.list(c.req.param("type"), q.data.cursor, q.data.limit) });
    })
    .get("/api/obj/:type/:id", async (c) => {
      if (!(await typeOf(c.req.param("type")))) return notFound(c);
      const t = await store.get(c.req.param("type"), c.req.param("id"));
      return t ? c.json({ response: t }) : c.json({ error: { message: "no such thing", code: "not_found" } }, 404);
    })
    .post("/api/obj/:type", async (c) => {
      const s = await schemaOf(c.req.param("type"));
      if (!s) return notFound(c);
      const p = s.strict().safeParse(await c.req.json().catch(() => null));
      if (!p.success) return c.json({ error: { message: p.error.issues.map((i) => `${i.path.join(".")}: ${i.message}`).join("; "), code: "invalid" } }, 400);
      const t = await store.create(c.req.param("type"), crypto.randomUUID(), p.data);
      return c.json({ id: t._id }, 201);
    })
    .patch("/api/obj/:type/:id", async (c) => {
      const s = await schemaOf(c.req.param("type"));
      if (!s) return notFound(c);
      const p = s.strict().partial().safeParse(await c.req.json().catch(() => null));
      if (!p.success) return c.json({ error: { message: p.error.issues.map((i) => `${i.path.join(".")}: ${i.message}`).join("; "), code: "invalid" } }, 400);
      const t = await store.update(c.req.param("type"), c.req.param("id"), p.data);
      return t ? c.json({ response: t }) : c.json({ error: { message: "no such thing", code: "not_found" } }, 404);
    })
    .delete("/api/obj/:type/:id", async (c) => {
      if (!(await typeOf(c.req.param("type")))) return notFound(c);
      return (await store.remove(c.req.param("type"), c.req.param("id"))) ? c.body(null, 204) : c.json({ error: { message: "no such thing", code: "not_found" } }, 404);
    });

  return app;
}

const app = createApp(pgStore());
export type AppType = typeof app;
export default app;
"##;

const BUBBLE_STORE: &str = r##"import { db } from "./db";
import type { Model } from "./import/bubble";

export type Thing = { _id: string; created_date: string; modified_date: string } & Record<string, unknown>;
export type Page = { cursor: number; results: Thing[]; count: number; remaining: number };

export interface Store {
  saveImport(model: Model, report: string, rows: Record<string, Record<string, unknown>[]>): Promise<void>;
  lastImport(): Promise<{ model: Model; report: string; imported_at: string } | null>;
  counts(): Promise<Record<string, number>>;
  list(type: string, cursor: number, limit: number): Promise<Page>;
  get(type: string, id: string): Promise<Thing | null>;
  create(type: string, id: string, data: Record<string, unknown>): Promise<Thing>;
  update(type: string, id: string, patch: Record<string, unknown>): Promise<Thing | null>;
  remove(type: string, id: string): Promise<boolean>;
}

const ROW_ID = (r: Record<string, unknown>) => (typeof r._id === "string" ? r._id : crypto.randomUUID());
const shape = (r: { _id: string; data: unknown; created_date: string | Date; modified_date: string | Date }): Thing =>
  ({ ...(r.data as Record<string, unknown>), _id: r._id, created_date: new Date(r.created_date).toISOString(), modified_date: new Date(r.modified_date).toISOString() });

export function pgStore(): Store {
  const one = async (type: string, id: string) => (await db<{ _id: string; data: unknown; created_date: string; modified_date: string }[]>`select _id, data, created_date, modified_date from bubble_things where type = ${type} and _id = ${id}`)[0];
  return {
    // The model and every row in one transaction: a half-imported app is worse than none.
    saveImport: (model, report, rows) => db.begin(async (tx) => {
      await tx`insert into bubble_imports (model, report) values (${tx.json(model as never)}, ${report})`;
      for (const [type, list] of Object.entries(rows)) for (const r of list) {
        const { _id, created_date, modified_date, ...data } = r as Record<string, unknown>;
        await tx`insert into bubble_things (type, _id, data, created_date, modified_date) values (${type}, ${ROW_ID(r)}, ${tx.json(data as never)}, ${(created_date as string) ?? new Date()}, ${(modified_date as string) ?? new Date()})
          on conflict (type, _id) do update set data = excluded.data, modified_date = excluded.modified_date`;
      }
    }),
    lastImport: async () => (await db<{ model: Model; report: string; imported_at: string }[]>`select model, report, imported_at from bubble_imports order by id desc limit 1`)[0] ?? null,
    counts: async () => Object.fromEntries((await db<{ type: string; n: string }[]>`select type, count(*) as n from bubble_things group by type`).map((r) => [r.type, Number(r.n)])),
    list: async (type, cursor, limit) => {
      const [{ n }] = await db<{ n: string }[]>`select count(*) as n from bubble_things where type = ${type}`;
      const rows = await db<{ _id: string; data: unknown; created_date: string; modified_date: string }[]>`select _id, data, created_date, modified_date from bubble_things where type = ${type} order by created_date, _id offset ${cursor} limit ${limit}`;
      return { cursor, results: rows.map(shape), count: rows.length, remaining: Math.max(0, Number(n) - cursor - rows.length) };
    },
    get: async (type, id) => { const r = await one(type, id); return r ? shape(r) : null; },
    create: async (type, id, data) => shape((await db<{ _id: string; data: unknown; created_date: string; modified_date: string }[]>`insert into bubble_things (type, _id, data) values (${type}, ${id}, ${db.json(data as never)}) returning _id, data, created_date, modified_date`)[0]),
    update: async (type, id, patch) => { const r = (await db<{ _id: string; data: unknown; created_date: string; modified_date: string }[]>`update bubble_things set data = data || ${db.json(patch as never)}, modified_date = now() where type = ${type} and _id = ${id} returning _id, data, created_date, modified_date`)[0]; return r ? shape(r) : null; },
    remove: async (type, id) => (await db`delete from bubble_things where type = ${type} and _id = ${id}`).count > 0,
  };
}

export function memoryStore(): Store {
  const things = new Map<string, Map<string, Thing>>();
  let last: { model: Model; report: string; imported_at: string } | null = null;
  const bucket = (t: string) => things.get(t) ?? (things.set(t, new Map()), things.get(t)!);
  return {
    saveImport: async (model, report, rows) => {
      last = { model, report, imported_at: new Date().toISOString() };
      for (const [type, list] of Object.entries(rows)) for (const r of list) { const id = ROW_ID(r); bucket(type).set(id, { created_date: new Date().toISOString(), modified_date: new Date().toISOString(), ...r, _id: id } as Thing); }
    },
    lastImport: async () => last,
    counts: async () => Object.fromEntries([...things].map(([t, m]) => [t, m.size])),
    list: async (type, cursor, limit) => { const all = [...bucket(type).values()]; const results = all.slice(cursor, cursor + limit); return { cursor, results, count: results.length, remaining: Math.max(0, all.length - cursor - results.length) }; },
    get: async (type, id) => bucket(type).get(id) ?? null,
    create: async (type, id, data) => { const t = { ...data, _id: id, created_date: new Date().toISOString(), modified_date: new Date().toISOString() } as Thing; bucket(type).set(id, t); return t; },
    update: async (type, id, patch) => { const t = bucket(type).get(id); if (!t) return null; Object.assign(t, patch, { modified_date: new Date().toISOString() }); return t; },
    remove: async (type, id) => bucket(type).delete(id),
  };
}
"##;

const BUBBLE_IMPORT: &str = r##"import { z } from "zod";

// Bubble's own description of an app, as its Data API returns it from GET /api/1.1/meta:
// `types` (display + fields with id/display/type), `post` (backend API workflows with typed
// parameters) and `app_data`. Field types are exactly the strings Bubble emits: text, number,
// date, boolean, geographic_address, file, image, user, custom.<type>, option.<set>,
// api.<call>, and list.<any of those>. Two optional extras the editor's .bubble download adds
// and meta does not: `option_sets` (values) and `pages` (names). Row data is what
// GET /api/1.1/obj/<type> returns — `{ response: { results: [...] } }` — keyed by type under
// `data`. Every shape is tolerant on purpose: an export is read, never authored, here.

const Field = z.object({ id: z.string(), display: z.string(), type: z.string() });
const Type = z.object({ display: z.string().optional(), fields: z.array(Field) });
const Param = z.object({ key: z.string(), value: z.string(), optional: z.boolean().optional() });
const Workflow = z.object({ endpoint: z.string(), method: z.string().optional(), parameters: z.array(Param).default([]), auth_unecessary: z.boolean().optional() });
const OptionSet = z.union([
  z.array(z.string()),
  z.object({ display: z.string().optional(), options: z.union([z.array(z.union([z.string(), z.object({ display: z.string() })])), z.record(z.object({ display: z.string() }))]) }),
]);
const Rows = z.union([z.array(z.record(z.unknown())), z.object({ response: z.object({ results: z.array(z.record(z.unknown())) }) })]);

export const BubbleExport = z.object({
  app_data: z.object({ appname: z.string().optional() }).passthrough().optional(),
  get: z.array(z.string()).optional(),
  post: z.array(Workflow).optional(),
  types: z.record(Type),
  option_sets: z.record(OptionSet).optional(),
  pages: z.union([z.array(z.string()), z.record(z.unknown())]).optional(),
  data: z.record(Rows).optional(),
});
export type BubbleExport = z.infer<typeof BubbleExport>;

// Bubble's built-ins every type carries. `_id` becomes the primary key; the rest are columns.
export const BUILTIN = new Set(["_id", "Created Date", "Modified Date", "Created By", "Slug"]);

export type FieldMap = { id: string; display: string; column: string; bubble: string; pg: string; zod: string; ref?: string; list: boolean };
export type TypeMap = { id: string; display: string; table: string; fields: FieldMap[] };
export type Model = {
  app: string;
  types: TypeMap[];
  optionSets: { name: string; enumName: string; values: string[] }[];
  workflows: { endpoint: string; route: string; method: string; params: { key: string; type: string; optional: boolean; zod: string }[] }[];
  pages: { name: string; route: string }[];
  rows: Record<string, Record<string, unknown>[]>;
  unmapped: string[];
};

// Postgres reserved words get a suffix: a Bubble app almost always has a `user` type, and
// `create table user` is a syntax error, not a table.
const RESERVED = new Set(["user", "order", "group", "table", "select", "from", "where", "default", "check", "constraint", "primary", "references", "column", "grant", "limit", "offset", "union", "case", "when", "then", "else", "end", "null", "true", "false", "all", "any", "some", "in", "is", "not", "and", "or", "as", "on", "using", "into", "values", "create", "drop", "alter", "index", "session", "role", "type", "function", "trigger", "view", "with", "window", "over", "partition", "distinct", "only", "both", "leading", "trailing", "cast", "collate", "current_date", "current_time", "current_user", "localtime", "returning", "symmetric", "asymmetric", "array", "analyse", "analyze", "authorization", "binary", "cross", "join", "inner", "outer", "left", "right", "full", "natural", "having", "intersect", "except", "fetch", "for", "to", "do", "desc", "asc", "placing", "similar", "verbose", "variadic", "lateral", "freeze", "ilike", "like", "isnull", "notnull", "overlaps", "tablesample", "concurrently", "deferrable", "initially", "foreign", "unique", "between"]);
const rawIdent = (s: string) => s.toLowerCase().replace(/[^a-z0-9]+/g, "_").replace(/^_+|_+$/g, "").replace(/^(\d)/, "_$1") || "x";
export const ident = (s: string) => { const i = rawIdent(s); return RESERVED.has(i) ? i + "_" : i; };
const enumName = (s: string) => rawIdent(s) + "_enum";

// One Bubble type → one Postgres type and one zod expression. Lists are arrays of the scalar;
// a custom reference is the other table's id (text, Bubble's ids are not numeric); an option
// is its enum; an api.* type has no table and stays jsonb.
export function mapType(t: string, sets: Set<string>): { pg: string; zod: string; ref?: string; list: boolean; unmapped?: string } {
  const list = t.startsWith("list.");
  const base = list ? t.slice(5) : t;
  const scalar = ((): { pg: string; zod: string; ref?: string; unmapped?: string } => {
    switch (base) {
      case "text": return { pg: "text", zod: "z.string()" };
      case "number": return { pg: "double precision", zod: "z.number()" };
      case "date": return { pg: "timestamptz", zod: "z.string().datetime({ offset: true })" };
      case "boolean": return { pg: "boolean", zod: "z.boolean()" };
      case "geographic_address": return { pg: "jsonb", zod: "z.object({ address: z.string(), lat: z.number().optional(), lng: z.number().optional() }).or(z.string())" };
      case "file": case "image": return { pg: "text", zod: "z.string().url().or(z.string().startsWith('//'))" };
      case "user": return { pg: "text", zod: "z.string()", ref: "user" };
    }
    if (base.startsWith("custom.")) return { pg: "text", zod: "z.string()", ref: base.slice(7) };
    if (base.startsWith("option.")) {
      const set = base.slice(7);
      return sets.has(set) ? { pg: enumName(set), zod: `${rawIdent(set)}Enum` } : { pg: "text", zod: "z.string()", unmapped: `option set ${set} has no values in the export; stored as text` };
    }
    if (base.startsWith("api.")) return { pg: "jsonb", zod: "z.unknown()", unmapped: `${base} is an API Connector type; kept as jsonb` };
    return { pg: "jsonb", zod: "z.unknown()", unmapped: `unknown Bubble type ${t}; kept as jsonb` };
  })();
  return list ? { ...scalar, pg: scalar.pg + "[]", zod: `z.array(${scalar.zod})`, list } : { ...scalar, list };
}

export function parse(raw: unknown): Model {
  const x = BubbleExport.parse(raw);
  const unmapped: string[] = [];
  const optionSets = Object.entries(x.option_sets ?? {}).map(([name, v]) => {
    const values = Array.isArray(v) ? v : Array.isArray(v.options) ? v.options.map((o) => (typeof o === "string" ? o : o.display)) : Object.values(v.options).map((o) => o.display);
    return { name, enumName: enumName(name), values };
  });
  const sets = new Set(optionSets.map((s) => s.name));
  const types = Object.entries(x.types).map(([id, t]) => ({
    id,
    display: t.display ?? id,
    table: ident(id),
    fields: t.fields.filter((f) => !BUILTIN.has(f.id)).map((f) => {
      const m = mapType(f.type, sets);
      if (m.unmapped) unmapped.push(`${id}.${f.display}: ${m.unmapped}`);
      return { id: f.id, display: f.display, column: ident(f.display), bubble: f.type, pg: m.pg, zod: m.zod, ref: m.ref, list: m.list };
    }),
  }));
  const known = new Set(types.map((t) => t.id));
  for (const t of types) for (const f of t.fields) if (f.ref && f.ref !== "user" && !known.has(f.ref)) unmapped.push(`${t.id}.${f.display}: references type ${f.ref} which is not in the export`);
  const workflows = (x.post ?? []).map((w) => ({
    endpoint: w.endpoint,
    route: `/api/wf/${w.endpoint}`,
    method: (w.method ?? "post").toUpperCase(),
    params: w.parameters.map((p) => ({ key: p.key, type: p.value, optional: p.optional ?? false, zod: mapType(p.value, sets).zod + (p.optional ? ".optional()" : "") })),
  }));
  const pageNames = Array.isArray(x.pages) ? x.pages : Object.keys(x.pages ?? {});
  const pages = pageNames.map((name) => ({ name, route: name === "index" ? "/" : `/${ident(name).replace(/_/g, "-")}` }));
  const rows: Model["rows"] = {};
  for (const [type, r] of Object.entries(x.data ?? {})) {
    if (!known.has(type)) { unmapped.push(`data.${type}: rows for a type not in the export; skipped`); continue; }
    rows[type] = Array.isArray(r) ? r : r.response.results;
  }
  return { app: x.app_data?.appname ?? "app", types, optionSets, workflows, pages, rows, unmapped };
}

// Bubble's Data API answers rows keyed by field *display* (use_captions_for_get) or by field id.
// Normalise to our columns so either export reads the same.
export function normaliseRow(t: TypeMap, row: Record<string, unknown>): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  for (const f of t.fields) {
    const v = row[f.display] ?? row[f.id] ?? row[f.column];
    if (v !== undefined) out[f.column] = v;
  }
  for (const b of BUILTIN) if (row[b] !== undefined) out[b === "_id" ? b : ident(b)] = row[b];
  return out;
}

export function sql(m: Model): string {
  const lines: string[] = [`-- Generated from the Bubble export of ${m.app}. One table per data type; Bubble's ids are text.`];
  for (const s of m.optionSets) lines.push(`do $$ begin create type ${s.enumName} as enum (${s.values.map((v) => `'${v.replace(/'/g, "''")}'`).join(", ")}); exception when duplicate_object then null; end $$;`);
  for (const t of m.types) {
    const cols = [
      "  _id text primary key",
      "  created_date timestamptz not null default now()",
      "  modified_date timestamptz not null default now()",
      "  created_by text",
      "  slug text",
      ...t.fields.map((f) => `  ${f.column} ${f.pg}${f.ref && !f.list && f.ref !== "user" && m.types.some((o) => o.id === f.ref) ? ` references ${ident(f.ref)}(_id) deferrable initially deferred` : ""}`),
    ];
    lines.push(`create table if not exists ${t.table} ( -- Bubble type "${t.display}"\n${cols.join(",\n")}\n);`);
  }
  return lines.join("\n") + "\n";
}

// The zod module the port compiles against: one enum per option set, one schema per type,
// one per backend workflow's parameters.
export function zodSource(m: Model): string {
  const out = [`import { z } from "zod";`, `// Generated from the Bubble export of ${m.app}. Regenerate with: bun src/import/bubble.ts <export.json>`, ""];
  for (const s of m.optionSets) out.push(`export const ${rawIdent(s.name)}Enum = z.enum([${s.values.map((v) => JSON.stringify(v)).join(", ")}]);`);
  for (const t of m.types) {
    out.push(`export const ${ident(t.id)} = z.object({`);
    for (const f of t.fields) out.push(`  ${JSON.stringify(f.column)}: ${f.zod}.nullish(), // ${f.display}: ${f.bubble}`);
    out.push(`});`);
  }
  for (const w of m.workflows) {
    out.push(`export const wf_${ident(w.endpoint)} = z.object({`);
    for (const p of w.params) out.push(`  ${JSON.stringify(p.key)}: ${p.zod}, // ${p.type}`);
    out.push(`});`);
  }
  return out.join("\n") + "\n";
}

export function report(m: Model): string {
  const L: string[] = [`# Migration from Bubble: ${m.app}`, ""];
  L.push(`| Bubble | Kind | Where it went |`, `|---|---|---|`);
  for (const t of m.types) {
    L.push(`| ${t.display} | data type | table \`${t.table}\`, schema \`${ident(t.id)}\`, routes \`/api/obj/${t.id}\` (${m.rows[t.id]?.length ?? 0} rows imported) |`);
    for (const f of t.fields) L.push(`| ${t.display}.${f.display} | field \`${f.bubble}\` | column \`${t.table}.${f.column}\` \`${f.pg}\`${f.ref ? ` → ${f.ref}` : ""} |`);
  }
  for (const s of m.optionSets) L.push(`| ${s.name} | option set | enum \`${s.enumName}\` (${s.values.join(", ")}) |`);
  for (const w of m.workflows) L.push(`| ${w.endpoint} | backend workflow | \`${w.method} ${w.route}\`, params \`wf_${ident(w.endpoint)}\` — body to write |`);
  for (const p of m.pages) L.push(`| ${p.name} | page | \`frontend/app${p.route === "/" ? "" : p.route}/page.tsx\` — to write |`);
  L.push("", '## Not mapped', "");
  L.push(...(m.unmapped.length ? m.unmapped.map((u) => `- ${u}`) : ["- nothing"]));
  return L.join("\n") + "\n";
}

// The same mapping as a live zod schema, so imported types are validated at runtime without
// a code generation step. Every field is optional: Bubble never required one.
export function runtimeSchema(t: TypeMap, sets: Model["optionSets"]): z.ZodObject<Record<string, z.ZodTypeAny>> {
  const scalar = (f: FieldMap): z.ZodTypeAny => {
    const base = f.list ? f.bubble.slice(5) : f.bubble;
    if (base === "number") return z.number();
    if (base === "boolean") return z.boolean();
    if (base === "date") return z.string().datetime({ offset: true });
    if (base.startsWith("option.")) { const s = sets.find((s) => s.name === base.slice(7)); return s && s.values.length ? z.enum(s.values as [string, ...string[]]) : z.string(); }
    if (base.startsWith("api.") || base === "geographic_address") return z.unknown();
    return z.string();
  };
  const shape: Record<string, z.ZodTypeAny> = {};
  for (const f of t.fields) shape[f.column] = (f.list ? z.array(scalar(f)) : scalar(f)).nullish();
  return z.object(shape);
}

// `bun src/import/bubble.ts <export.json>` writes the three artifacts next to the code.
if (import.meta.main) {
  const { readFile, writeFile, mkdir } = await import("node:fs/promises");
  const file = process.argv[2];
  if (!file) { console.error("usage: bun src/import/bubble.ts <bubble-export.json>"); process.exit(2); }
  const m = parse(JSON.parse(await readFile(file, "utf8")));
  await mkdir("migrations", { recursive: true });
  await writeFile("migrations/0003_bubble_types.sql", sql(m));
  await writeFile("src/import/bubble.schemas.ts", zodSource(m));
  await writeFile("../MIGRATION.md", report(m));
  console.log(`${m.types.length} types, ${m.optionSets.length} option sets, ${m.workflows.length} workflows, ${m.pages.length} pages, ${m.unmapped.length} unmapped → migrations/0003_bubble_types.sql, src/import/bubble.schemas.ts, MIGRATION.md`);
}
"##;

const BUBBLE_FIXTURE: &str = r##"// A realistic Bubble export: /api/1.1/meta plus the editor's option sets and pages, plus rows
// from GET /api/1.1/obj/<type> (keyed by field display, as use_captions_for_get returns them).
export const FIXTURE = {
  app_data: { appname: "bookings", app_version: "live", use_captions_for_get: true },
  get: ["venue", "booking", "user"],
  post: [
    { endpoint: "confirm_booking", method: "post", auth_unecessary: false, parameters: [{ key: "booking", value: "custom.booking", optional: false, param_in: "body" }, { key: "note", value: "text", optional: true, param_in: "body" }] },
  ],
  types: {
    venue: { display: "Venue", fields: [
      { id: "name_text", display: "Name", type: "text" },
      { id: "capacity_number", display: "Capacity", type: "number" },
      { id: "address_geographic_address", display: "Address", type: "geographic_address" },
      { id: "photos_list_image", display: "Photos", type: "list.image" },
      { id: "owner_user", display: "Owner", type: "user" },
      { id: "Created Date", display: "Created Date", type: "date" }, { id: "Modified Date", display: "Modified Date", type: "date" }, { id: "Created By", display: "Created By", type: "user" }, { id: "_id", display: "unique ID", type: "text" }, { id: "Slug", display: "Slug", type: "text" },
    ] },
    booking: { display: "Booking", fields: [
      { id: "venue_custom_venue", display: "Venue", type: "custom.venue" },
      { id: "starts_date", display: "Starts", type: "date" },
      { id: "guests_number", display: "Guests", type: "number" },
      { id: "status_option_booking_status", display: "Status", type: "option.booking_status" },
      { id: "tags_list_text", display: "Tags", type: "list.text" },
      { id: "paid_boolean", display: "Paid?", type: "boolean" },
      { id: "invoice_api_stripe_invoice", display: "Invoice", type: "api.stripe.Invoice" },
      { id: "_id", display: "unique ID", type: "text" }, { id: "Created Date", display: "Created Date", type: "date" },
    ] },
    user: { display: "User", fields: [
      { id: "email", display: "email", type: "text" },
      { id: "role_option_role", display: "Role", type: "option.role" },
      { id: "_id", display: "unique ID", type: "text" },
    ] },
  },
  option_sets: {
    booking_status: ["Pending", "Confirmed", "Cancelled"],
    role: { display: "Role", options: { a: { display: "admin" }, b: { display: "member" } } },
  },
  pages: ["index", "venue_detail", "my_bookings"],
  data: {
    venue: { response: { cursor: 0, count: 2, remaining: 0, results: [
      { _id: "1700000000000x100", "Created Date": "2023-11-14T22:13:20.000Z", "Modified Date": "2023-11-14T22:13:20.000Z", "Created By": "1700000000000x1", Name: "Hall A", Capacity: 120, Address: { address: "1 Main St", lat: 51.5, lng: -0.1 }, Photos: ["//s3.amazonaws.com/appforest_uf/f1/a.png"] },
      { _id: "1700000000000x101", "Created Date": "2023-11-15T09:00:00.000Z", Name: "Garden", Capacity: 40 },
    ] } },
    booking: [
      { _id: "1700000000000x200", Venue: "1700000000000x100", Starts: "2024-01-01T10:00:00.000Z", Guests: 30, Status: "Confirmed", Tags: ["vip"], "Paid?": true },
    ],
  },
};
"##;

const BUBBLE_TEST: &str = r##"import { describe, expect, test } from "bun:test";
import { ident } from "./import/bubble";
import { createApp } from "./app";
import { memoryStore } from "./store";
import { parse, sql, zodSource, report } from "./import/bubble";
import { FIXTURE } from "./import/fixture";

const json = (body: unknown, method = "POST") => ({ method, headers: { "content-type": "application/json" }, body: JSON.stringify(body) });

describe("bubble import", () => {
  test("every Bubble field type lands as a Postgres column and a zod rule", () => {
    expect(ident("user")).toBe("user_"); expect(ident("Order Item")).toBe("order_item");
    const m = parse(FIXTURE);
    const ddl = sql(m);
    expect(ddl).toContain("create type booking_status_enum as enum ('Pending', 'Confirmed', 'Cancelled')");
    expect(ddl).toContain("capacity double precision");
    expect(ddl).toContain("photos text[]");
    expect(ddl).toContain("address jsonb");
    expect(ddl).toContain("venue text references venue(_id)");
    expect(ddl).toContain("status booking_status_enum");
    expect(ddl).toContain("invoice jsonb");
    expect(ddl).not.toContain("unique_id"); // built-ins are fixed columns, not fields
    const zod = zodSource(m);
    expect(zod).toContain('export const roleEnum = z.enum(["admin", "member"])');
    expect(zod).toContain('"tags": z.array(z.string()).nullish()');
    expect(zod).toContain('export const wf_confirm_booking = z.object({\n  "booking": z.string(), // custom.booking\n  "note": z.string().optional(), // text');
    expect(m.unmapped).toEqual(["booking.Invoice: api.stripe.Invoice is an API Connector type; kept as jsonb"]);
    const md = report(m);
    expect(md).toContain("| venue_detail | page | `frontend/app/venue-detail/page.tsx` — to write |");
    expect(md).toContain("| confirm_booking | backend workflow | `POST /api/wf/confirm_booking`");
  });

  test("an export that is not Bubble's shape is refused with the reason", async () => {
    const app = createApp(memoryStore());
    const res = await app.request("/api/import", json({ types: { venue: { fields: [{ id: "x" }] } } }));
    expect(res.status).toBe(400);
    expect((await res.json()).error.message).toContain("types.venue.fields.0.display");
    expect((await app.request("/api/obj/venue")).status).toBe(404);
  });

  test("upload → report, and the rows answer in the Data API's shape", async () => {
    const app = createApp(memoryStore());
    const up = await app.request("/api/import", json(FIXTURE));
    expect(up.status).toBe(201);
    const body = await up.json();
    expect(body.rows).toEqual({ venue: 2, booking: 1 });
    expect(body.report).toContain("# Migration from Bubble: bookings");
    const list = await (await app.request("/api/obj/venue?limit=1")).json();
    expect(list.response).toMatchObject({ cursor: 0, count: 1, remaining: 1 });
    expect(list.response.results[0]).toMatchObject({ _id: "1700000000000x100", name: "Hall A", capacity: 120, created_by: "1700000000000x1" });
    const page2 = await (await app.request("/api/obj/venue?cursor=1&limit=1")).json();
    expect(page2.response.results[0].name).toBe("Garden");
    expect(page2.response.remaining).toBe(0);
    const one = await (await app.request("/api/obj/booking/1700000000000x200")).json();
    expect(one.response).toMatchObject({ venue: "1700000000000x100", status: "Confirmed", paid: true, tags: ["vip"] });
    const summary = await (await app.request("/api/import")).json();
    expect(summary.counts).toEqual({ venue: 2, booking: 1 });
    expect(summary.unmapped.length).toBe(1);
  });

  test("multipart upload works the way a form does", async () => {
    const app = createApp(memoryStore());
    const fd = new FormData();
    fd.append("file", new File([JSON.stringify(FIXTURE)], "bookings.json", { type: "application/json" }));
    const res = await app.request("/api/import", { method: "POST", body: fd });
    expect(res.status).toBe(201);
    expect((await res.json()).types).toBe(3);
  });

  test("CRUD is validated with the imported types: options, numbers, unknown fields", async () => {
    const app = createApp(memoryStore());
    await app.request("/api/import", json(FIXTURE));
    const bad = await app.request("/api/obj/booking", json({ status: "Maybe", guests: "ten" }));
    expect(bad.status).toBe(400);
    const msg = (await bad.json()).error.message;
    expect(msg).toContain("status:");
    expect(msg).toContain("guests:");
    expect((await app.request("/api/obj/booking", json({ nope: 1 }))).status).toBe(400);
    const ok = await app.request("/api/obj/booking", json({ status: "Pending", guests: 4, venue: "1700000000000x101" }));
    expect(ok.status).toBe(201);
    const { id } = await ok.json();
    const patched = await app.request(`/api/obj/booking/${id}`, json({ status: "Confirmed" }, "PATCH"));
    expect((await patched.json()).response.status).toBe("Confirmed");
    expect((await app.request(`/api/obj/booking/${id}`, { method: "DELETE" })).status).toBe(204);
    expect((await app.request(`/api/obj/booking/${id}`)).status).toBe(404);
    expect((await app.request("/api/obj/nothing")).status).toBe(404);
  });
});
"##;

const BUBBLE_SQL: &str = r##"-- The import lands here before the per-type tables exist: one jsonb row per Bubble thing,
-- keyed by Bubble's own _id, so the data is queryable the minute the export is uploaded.
-- `bun src/import/bubble.ts <export>` then writes 0003_bubble_types.sql with the real tables.
create table if not exists bubble_things (
  type text not null,
  _id text not null,
  data jsonb not null default '{}',
  created_date timestamptz not null default now(),
  modified_date timestamptz not null default now(),
  primary key (type, _id)
);
create index if not exists bubble_things_type on bubble_things (type, created_date);
-- The last import's parsed model and report, so the API and the page survive a restart.
create table if not exists bubble_imports (
  id bigserial primary key,
  model jsonb not null,
  report text not null,
  imported_at timestamptz not null default now()
);
"##;

const BUBBLE_PAGE: &str = r##"async function summary() {
  const res = await fetch(`${process.env.API_URL ?? "http://127.0.0.1:8000"}/api/import`, { cache: "no-store" });
  return (await res.json()) as {
    imported: boolean; imported_at?: string; app?: string; report?: string; counts: Record<string, number>;
    types?: { id: string; display: string; table: string; fields: number }[];
    optionSets?: { name: string; values: string[] }[];
    workflows?: { endpoint: string; route: string; params: number }[];
    pages?: { name: string; route: string }[];
    unmapped?: string[];
  };
}

// The mapping report: what the export declared, where each piece went, how many rows are
// already queryable, and what could not be mapped — the list the port works through.
export default async function Home() {
  const s = await summary();
  return (
    <main>
      <h1>{{NAME}} — port from Bubble</h1>
      <p>Upload the export: <code>{`curl -X POST http://localhost:8000/api/import -F file=@bookings.json`}</code> — then query <code>GET /api/obj/&lt;type&gt;</code> like Bubble's Data API, and run <code>bun src/import/bubble.ts bookings.json</code> in backend/ to write the migration, schemas and MIGRATION.md.</p>
      {!s.imported ? <p>Nothing imported yet.</p> : (
        <>
          <p>App <b>{s.app}</b>, imported {s.imported_at}.</p>
          <h2>Data types</h2>
          <table>
            <thead><tr><th>Bubble type</th><th>table</th><th>fields</th><th>rows</th><th>route</th></tr></thead>
            <tbody>{s.types!.map((t) => <tr key={t.id}><td>{t.display}</td><td><code>{t.table}</code></td><td>{t.fields}</td><td>{s.counts[t.id] ?? 0}</td><td><code>/api/obj/{t.id}</code></td></tr>)}</tbody>
          </table>
          <h2>Option sets</h2>
          <ul>{s.optionSets!.map((o) => <li key={o.name}><code>{o.name}</code>: {o.values.join(", ")}</li>)}</ul>
          <h2>Backend workflows</h2>
          <ul>{s.workflows!.map((w) => <li key={w.endpoint}>{w.endpoint} → <code>POST {w.route}</code> ({w.params} params)</li>)}</ul>
          <h2>Pages</h2>
          <ul>{s.pages!.map((p) => <li key={p.name}>{p.name} → <code>{p.route}</code></li>)}</ul>
          <h2>Not mapped</h2>
          {s.unmapped!.length === 0 ? <p>Everything mapped.</p> : <ul>{s.unmapped!.map((u) => <li key={u}>{u}</li>)}</ul>}
          <h2>MIGRATION.md</h2>
          <pre>{s.report}</pre>
        </>
      )}
    </main>
  );
}
"##;

const BUBBLE_CLAUDE_MD: &str = r##"# {{NAME}} — working agreement

{{NAME}} is the landing zone for an application leaving Bubble.io. It reads Bubble's own
description of the app — `GET /api/1.1/meta` plus the rows from `GET /api/1.1/obj/<type>` — and
turns it into three things the port compiles against: a Postgres migration (one table per data
type, one enum per option set), a zod module (one schema per type, one per backend workflow's
parameters), and `MIGRATION.md`, the list of what mapped and what a person still has to write.
Until the typed tables exist, every imported row is already queryable through the Data API's own
routes, served from one jsonb table. "Done" for the import means: the report has no surprises in
*Not mapped*, `make migrate` has applied `0003_bubble_types.sql`, and the rows have been moved
from `bubble_things` into the typed tables. "Done" for the migration is in `README.md`.

## Architecture

| File | Owns |
|---|---|
| `backend/src/app.ts` | `POST/GET /api/import` and the Data API routes `/api/obj/:type[/:id]`, validated at runtime by `runtimeSchema`. Exports `AppType`. |
| `backend/src/store.ts` | `Store`: `saveImport` (model + rows in one transaction), `lastImport`, `counts`, and CRUD over `bubble_things`; `pgStore` and `memoryStore`. |
| `backend/src/import/bubble.ts` | `BubbleExport` (zod, tolerant), `mapType` (Bubble type → pg + zod), `parse` → `Model`, `normaliseRow`, `sql`, `zodSource`, `report`, `runtimeSchema`, and the CLI (`bun src/import/bubble.ts <export.json>`). |
| `backend/src/import/fixture.ts` | A realistic export (`bookings`: venue, booking, user; two option sets; one backend workflow; three pages; rows keyed by display name). |
| `backend/src/app.test.ts` | Five tests: the mapping, refusal, upload → report → Data API, multipart, validated CRUD. |
| `backend/migrations/0002_bubble.sql` | `bubble_things` (the landing table) and `bubble_imports`. |
| `frontend/app/page.tsx` | The report: types → tables, option sets, workflows, pages, *Not mapped*, and `MIGRATION.md` inline. Server-rendered from `GET /api/import`. |

### Request path (import)

1. `POST /api/import` — JSON body or multipart `file`. Not JSON is `400`.
2. `parse(raw)`: `BubbleExport.parse` (a zod error becomes `400` with the path, e.g. `types.venue.fields.0.display: Required`), then per type: drop Bubble's built-ins (`_id`, `Created Date`, `Modified Date`, `Created By`, `Slug`), map each field with `mapType`, collect `unmapped`.
3. Rows under `data.<type>` — either an array or Bubble's `{response: {results}}` — are kept only for types in the export; `normaliseRow` reads each field by display name, id or column so `use_captions_for_get` on or off both land.
4. `store.saveImport(model, report, rows)` — one transaction: a `bubble_imports` row and an upsert per thing on `(type, _id)`.
5. `201` with counts, `unmapped`, the report, and the SQL and zod the CLI would write, so an agent can read them before touching disk.

### Request path (Data API)

1. `/api/obj/:type` checks the type against the last import's model (`404 no such type; import an export first`).
2. `GET` lists with `cursor`/`limit` (≤ 100) → `{response: {cursor, results, count, remaining}}`, Bubble's shape; `GET /:id` → `{response: thing}`.
3. `POST` validates with `runtimeSchema(type).strict()` — unknown fields, wrong option values and wrong scalar types are `400` with `field: message` — and returns `201 {id}`; `PATCH` uses `.strict().partial()`; `DELETE` is `204`.
4. Rows are stored as `data jsonb` under Bubble's `_id`; `created_date` and `modified_date` are columns; `created_by` and `slug` stay inside `data`.

### Data model

| Table | Column | Why |
|---|---|---|
| `bubble_things` | `(type, _id)` pk | Bubble's ids (`1700000000000x100`) are text and unique per app; keeping them makes references between rows valid on arrival. |
| | `data jsonb` | The row minus `_id`/dates, keyed by our column names (`ident(display)`). The move to typed tables reads from here. |
| | `created_date`, `modified_date` | Bubble's; `modified_date` is bumped by `PATCH`. |
| `bubble_imports` | `model jsonb`, `report text` | The parsed `Model`, so a restart (and the page) serve the same types without re-uploading. `lastImport` is the newest by `id`. |

## Invariants

1. **Every Bubble field type maps to a pg type and a zod rule, or is listed as unmapped.** `mapType` has a case for each type Bubble emits; the fallthrough is `jsonb` + `z.unknown()` + an `unmapped` entry — never a silent drop. Guarded by `every Bubble field type lands as a Postgres column and a zod rule`.
2. **Built-ins are fixed columns, not fields.** `BUILTIN` is filtered out of `fields`; the DDL has `_id text primary key`, the two dates, `created_by`, `slug`, and never `unique_id`. Same test (`not.toContain("unique_id")`).
3. **Bubble ids stay Bubble ids.** `_id` is `text`, references are `text references <table>(_id) deferrable initially deferred` so rows can be inserted in any order within a transaction. Same test (`venue text references venue(_id)`).
4. **A reference to a type not in the export is reported**, not made a foreign key. `parse` adds `… references type X which is not in the export` and `sql` only emits `references` when the target table exists.
5. **An export that is not Bubble's shape is refused with the path.** Guarded by `an export that is not Bubble's shape is refused with the reason`.
6. **Import is atomic.** Model and every row in one `db.begin`; a half-imported app is worse than none. `pgStore.saveImport`.
7. **The Data API answers in Bubble's shape**, `{response: {cursor, results, count, remaining}}`, so a client written against Bubble's `obj` routes needs only a base URL change. Guarded by `upload → report, and the rows answer in the Data API's shape`.
8. **CRUD is validated with the imported types** — option values against the set, numbers as numbers, unknown fields refused. Guarded by `CRUD is validated with the imported types: options, numbers, unknown fields`.
9. **Every field is optional.** Bubble never required one; `zodSource` emits `.nullish()` per field and `runtimeSchema` does the same. A `not null` is a decision made after the port, by migration.
10. **Rows keyed by display or by id both land.** `normaliseRow` reads `row[display] ?? row[id] ?? row[column]`. The fixture uses display names (captions on); the test asserts `name`, `capacity`, `created_by` arrived.
11. **`ident()` is the one naming rule.** Tables, columns, enums, schema exports and page routes all come from it, so `MIGRATION.md`, the DDL and the zod agree by construction.

## Extending it

**Map a Bubble type differently** (e.g. `geographic_address` to PostGIS). Change the case in `mapType` and the matching branch in `runtimeSchema`; update the DDL assertion in the first test. Regenerate `0003` with the CLI — it is a generated file until you edit it, after which it is yours.

**Add a Bubble type that appears in a new export.** It arrives as `unknown Bubble type …; kept as jsonb`. Add a case to `mapType` and to `runtimeSchema`, and a field of that type to `fixture.ts` so the test covers it.

**Generate route handlers per type.** Today `/api/obj/:type` is generic over jsonb. To serve the typed tables, add a route group per type in `app.ts` using `bubble.schemas.ts` (import the generated module; keep the route table one chained expression so `AppType` stays typed). Test with `memoryStore` extended to hold typed rows, or against Postgres in an opt-in file.

**Write a backend workflow.** `MIGRATION.md` lists `POST /api/wf/<endpoint>` with `wf_<endpoint>` as its parameter schema. Add the route by hand; nothing generates its body — the workflow's logic never left Bubble's editor.

**Add constraints / search.** Bubble's `constraints` query parameter is not implemented. Add a zod schema for `[{key, constraint_type, value}]` and translate to `where data->>key …` for the landing table, or to typed columns once moved.

**Add auth.** There is none; Bubble had privacy rules and an API token. See README → Production before exposing `/api/obj`.

## Operating it

| Variable | Required | Meaning |
|---|---|---|
| `DATABASE_URL` | yes | Postgres. |
| `API_URL` | frontend | Where the page fetches `/api/import`; default `http://127.0.0.1:8000`. |
| `PORT` | no | API port, default 8000. |

**Processes.** One: `backend`. No worker. The CLI runs on a developer machine or in CI, not in the cluster.

**What is per-replica.** The current model is cached in `createApp`'s closure after first use; a new import on replica A is not seen by replica B until B restarts or first loads. Do the import against one replica, then roll — or have `current()` re-read `lastImport` on a miss.

**Failure modes.**

| What | User sees |
|---|---|
| Export not Bubble-shaped | `400` with the zod path. |
| Rows for a type not in `types` | Imported? No — skipped and listed under *Not mapped*. |
| Option field with a value outside the set | `400 status: Invalid enum value` on POST/PATCH; on import the row lands as-is (import is tolerant, CRUD is strict). |
| `GET /api/obj/x` before any import | `404 no such type; import an export first`. |
| Two imports | The second replaces the model and upserts rows by `_id`; rows deleted in Bubble since are not deleted here. |

**Metrics and logs.** `GET /api/import` → `counts` per type against Bubble's own counts is the import check. Watch `bubble_things` row counts after the move — they should stop changing once clients point at typed routes.

## Ceilings

- **List pagination is `offset`** (Bubble's `cursor` is one too). Fine to ~100 k rows per type; keyset on `(created_date, _id)` after.
- **One import model, one app.** `lastImport` is global; importing a second Bubble app replaces the first. Add an `app` column and route prefix if you need both.
- **Model cached per process** (above).
- **No `constraints`, no `sort_field`, no search** on the list route.
- **The typed tables are generated but not served.** `/api/obj` reads `bubble_things`; after the move, the routes over typed tables are yours to write.
- **No auth, no privacy rules.** Every imported row is readable by anyone who can reach `:8000`.
- **Files and images are URLs into Bubble's S3.** Nothing copies them.
- **Reserved words get a trailing underscore.** Bubble's `user` type becomes `user_`, a field called `order` becomes `order_`; enum names are untouched. If you prefer other names, rename in the generated `0003` (and the matching zod export) before applying — the CLI will not do it.

The stack rules — gate, typed seam, production checklist, deploy — are in `docs/PRODUCTION.md`. They apply.
"##;

const BUBBLE_AGENTS_MD: &str = r##"# {{NAME}} — for agents

See `CLAUDE.md` for the rules. This is how to run it.

## Run

    make demo        # postgres + redis, migrate (0001, 0002), seed, API :8000, page :3000
    make check       # typecheck both halves, bun test the backend

## The importer, two ways

Over HTTP (lands rows, serves them, returns the report and the artifacts as strings):

    curl -s -X POST localhost:8000/api/import -F file=@bookings.json | jq '{app, types, rows, unmapped}'
    # {"app":"bookings","types":3,"rows":{"venue":2,"booking":1},"unmapped":["booking.Invoice: api.stripe.Invoice is an API Connector type; kept as jsonb"]}
    curl -s -X POST localhost:8000/api/import -H 'content-type: application/json' --data-binary @bookings.json   # same, JSON body
    curl -s localhost:8000/api/import | jq '{imported, counts, types: [.types[].table], workflows, pages}'

On disk (writes the files the port compiles against):

    cd backend && bun src/import/bubble.ts ../bookings.json
    # 3 types, 2 option sets, 1 workflows, 3 pages, 1 unmapped → migrations/0003_bubble_types.sql, src/import/bubble.schemas.ts, MIGRATION.md

The fixture is a valid export: `bun -e 'import {FIXTURE} from "./src/import/fixture"; console.log(JSON.stringify(FIXTURE))' > ../bookings.json` from `backend/`.

## The Data API

    curl -s 'localhost:8000/api/obj/venue?cursor=0&limit=1'
    # {"response":{"cursor":0,"results":[{"_id":"1700000000000x100","name":"Hall A","capacity":120,"address":{…},"photos":["//s3…/a.png"],"created_by":"1700000000000x1","created_date":"2023-11-14T22:13:20.000Z","modified_date":"…"}],"count":1,"remaining":1}}
    curl -s localhost:8000/api/obj/booking/1700000000000x200
    # {"response":{"_id":"1700000000000x200","venue":"1700000000000x100","starts":"2024-01-01T10:00:00.000Z","guests":30,"status":"Confirmed","tags":["vip"],"paid":true,…}}

    curl -s -X POST localhost:8000/api/obj/booking -H 'content-type: application/json' -d '{"status":"Pending","guests":4,"venue":"1700000000000x101"}'
    # 201 {"id":"6d2f…"}
    curl -s -X POST localhost:8000/api/obj/booking -H 'content-type: application/json' -d '{"status":"Maybe","guests":"ten","nope":1}'
    # 400 {"error":{"message":"status: Invalid enum value. Expected 'Pending' | 'Confirmed' | 'Cancelled', received 'Maybe'; guests: Expected number, received string; : Unrecognized key(s) in object: 'nope'","code":"invalid"}}
    curl -s -X PATCH localhost:8000/api/obj/booking/6d2f… -H 'content-type: application/json' -d '{"status":"Confirmed"}'   # {"response":{…}}
    curl -s -X DELETE localhost:8000/api/obj/booking/6d2f…    # 204
    curl -s localhost:8000/api/obj/nothing                     # 404 no such type; import an export first

## Tests

`backend/src/app.test.ts`, `bun test`:

- **`FIXTURE`** is the input everywhere; a new Bubble shape you meet in the wild goes into it, not into a second fixture.
- **`parse`/`sql`/`zodSource`/`report` are pure**, so the mapping test asserts on strings of DDL and TypeScript.
- **`memoryStore()`** replaces Postgres for the upload and CRUD tests; `createApp(memoryStore())` then `app.request(...)`.
- Multipart is tested with a real `FormData` and `File`.

To add a test: extend `FIXTURE` (a field, a type, a row), then assert the DDL line, the zod line, the `report` row, and — if rows are involved — what `GET /api/obj/<type>` returns. Do not test the generated `0003` by running it here; `make migrate` against the dev database is that test.
"##;

const BUBBLE_README_MD: &str = r##"# {{NAME}}

Leave Bubble.io with your data queryable on day one and a typed target to port the rest into.

## What you get

- `POST /api/import` takes Bubble's export (the `meta` document, plus rows from the Data API) and answers with a mapping report: every data type, field, option set, backend workflow and page, and where each one went.
- The rows land immediately in `bubble_things` and are served at `/api/obj/<type>` in Bubble's Data API shape — list with `cursor`/`limit`, get, create, patch, delete — validated against the imported types.
- `bun src/import/bubble.ts export.json` writes `migrations/0003_bubble_types.sql` (one table per type, one enum per option set, Bubble's ids as keys, references as deferrable foreign keys), `src/import/bubble.schemas.ts` (a zod schema per type and per backend workflow), and `MIGRATION.md`.
- Nothing is silently dropped: a type the importer cannot map is stored as `jsonb` and listed under *Not mapped*.
- Tests without a database; typed end to end; the stack's deploy, migrations and secrets.

## Five minutes

    make demo
    cd backend && bun -e 'import {FIXTURE} from "./src/import/fixture"; console.log(JSON.stringify(FIXTURE))' > ../bookings.json && cd ..

    curl -s -X POST localhost:8000/api/import -F file=@bookings.json | jq '{types, rows, unmapped}'
    # {"types":3,"rows":{"venue":2,"booking":1},"unmapped":["booking.Invoice: api.stripe.Invoice is an API Connector type; kept as jsonb"]}

    curl -s 'localhost:8000/api/obj/venue?limit=1' | jq '.response.results[0] | {name, capacity, photos}'
    # {"name":"Hall A","capacity":120,"photos":["//s3.amazonaws.com/appforest_uf/f1/a.png"]}

    curl -s -X POST localhost:8000/api/obj/booking -H 'content-type: application/json' -d '{"status":"Maybe"}'
    # 400 … status: Invalid enum value. Expected 'Pending' | 'Confirmed' | 'Cancelled' …

    cd backend && bun src/import/bubble.ts ../bookings.json && head -20 migrations/0003_bubble_types.sql

Open `http://localhost:3000` for the report.

## The migration, in order

This is the part Bubble's export does not do for you. Each step has a check; do not skip to the next without it.

**1. Export from Bubble.**
- `GET https://<app>.bubbleapps.io/api/1.1/meta` (Settings → API must have the Data API on) → the `types`, `post` (backend workflows) and `app_data` sections.
- For each type, page through `GET /api/1.1/obj/<type>?cursor=N&limit=100` with an API token that can read everything (privacy rules apply to the token's user) and put the concatenated `results` under `data.<type>`. Turn on `use_captions_for_get` or not — both land.
- From the editor, add `option_sets` (name → values) and `pages` (names). The `meta` document does not carry them; without them option fields become `text` and pages are not listed.
- *Check:* counts per type match what the Bubble editor's Data tab shows.

**2. Upload.** `POST /api/import`. Read *Not mapped* on the page. Everything there is a decision: an `api.*` field stays `jsonb`; an option set with no values becomes `text`; a reference to a type you did not export has no foreign key.
- *Check:* `GET /api/import` → `counts` equals step 1's counts.

**3. Generate.** `bun src/import/bubble.ts export.json`. Read `0003_bubble_types.sql` and `bubble.schemas.ts` — they are yours now. Rename columns you dislike here, once, before anything depends on them — and rename anything that is a Postgres reserved word: Bubble's `user` type generates `create table user`, which will not apply; use `app_user`.
- *Check:* `make migrate` applies `0003` cleanly; `make check` compiles the schemas.

**4. Move the data** from the landing table into the typed tables, one statement per type, in one transaction so the deferred foreign keys check at commit. The pattern, from the fixture:

    begin;
    insert into venue (_id, created_date, modified_date, created_by, slug, name, capacity, address, photos, owner)
    select _id, created_date, modified_date, data->>'created_by', data->>'slug',
           data->>'name', (data->>'capacity')::double precision, data->'address',
           array(select jsonb_array_elements_text(data->'photos')), data->>'owner'
    from bubble_things where type = 'venue';
    insert into booking (_id, created_date, modified_date, created_by, slug, venue, starts, guests, status, tags, paid, invoice)
    select _id, created_date, modified_date, data->>'created_by', data->>'slug',
           data->>'venue', (data->>'starts')::timestamptz, (data->>'guests')::double precision,
           (data->>'status')::booking_status_enum, array(select jsonb_array_elements_text(data->'tags')),
           (data->>'paid')::boolean, data->'invoice'
    from bubble_things where type = 'booking';
    commit;

  Casts: `text` → `->>`; `number` → `::double precision`; `date` → `::timestamptz`; `boolean` → `::boolean`; `option.X` → `::x_enum`; `list.*` → `array(select jsonb_array_elements_text(...))`; `geographic_address`, `api.*` → `->` (jsonb). Save it as `migrations/0004_bubble_move.sql` so it runs once, everywhere.
- *Check:* an orphan query before you commit — `select b._id from bubble_things b where b.type = 'booking' and not exists (select 1 from bubble_things v where v.type = 'venue' and v._id = b.data->>'venue')` — must be empty, or the commit fails on the deferred key. Fix the data or null the reference; do not drop the constraint.
- *Check:* `select count(*) from venue` equals `counts.venue`.

**5. Serve the typed tables.** Write routes per type in `app.ts` using the generated schemas (keep the route table one chained expression). Point clients at them, then retire `/api/obj/<type>` for that type. Until you do, `/api/obj` still reads `bubble_things`, which is now stale — do not write to both.

**6. Backend workflows.** Each `post` endpoint is in `MIGRATION.md` as `POST /api/wf/<endpoint>` with `wf_<endpoint>` validating its parameters. The body is not exported by Bubble; open the workflow in the editor and write it as a handler, one test each.

**7. Pages.** `MIGRATION.md` lists `frontend/app/<route>/page.tsx` per page. Bubble's element tree, page workflows and conditionals are not in any export; these are rewritten, against the typed client.

**8. Users and privacy rules.** Bubble's `user` type arrives as rows (email, custom fields) but **not passwords** — they are never exported. Plan a password-reset-on-first-login flow. Privacy rules become authorisation in your handlers; nothing here enforces them. Do not expose `/api/obj` publicly before this step (see Production).

**9. Files.** `file` and `image` fields are URLs into Bubble's S3 (`//s3.amazonaws.com/appforest_uf/…`). Copy them to your own bucket and rewrite the columns before the Bubble plan ends.

**10. Cut over.** Freeze writes in Bubble, re-export and re-import (rows upsert by `_id`), re-run step 4 for the delta, switch DNS. Keep `bubble_things` for a month, then drop it.

## API

| Method | Path | Auth | What |
|---|---|---|---|
| GET | `/api/health`, `/api/health/ready` | none | probes |
| POST | `/api/import` | none | JSON or multipart `file` → `201` report, counts, `sql`, `zod`, `unmapped` |
| GET | `/api/import` | none | last import: counts, types, option sets, workflows, pages, unmapped, report |
| GET | `/api/obj/:type` | none | `cursor`, `limit` ≤ 100 → `{response: {cursor, results, count, remaining}}` |
| GET | `/api/obj/:type/:id` | none | `{response: thing}` |
| POST | `/api/obj/:type` | none | validated, strict → `201 {id}` |
| PATCH | `/api/obj/:type/:id` | none | partial, strict → `{response: thing}` |
| DELETE | `/api/obj/:type/:id` | none | `204` |

## Compared with Bubble's Data API

**Same shape.** Route layout `obj/<type>[/<id>]`; list response `{response: {cursor, results, count, remaining}}`; `_id`, `Created Date`, `Modified Date`, `Created By`, `Slug` as the built-ins (lower-cased); `POST` answering `{id}`; rows keyed by caption or by field id both accepted on import.

**Better here, specifically.**
- Types are real: an enum column refuses `"Maybe"`, a number column refuses `"ten"`, an unknown field is a 400. Bubble accepted all three.
- A schema you can read: one SQL file and one zod module, generated once, then edited like any code.
- Every reference is a foreign key (deferrable, so bulk loads work) — Bubble had no referential integrity.
- Tests, CI, migrations, environments, a cluster — the whole stack.

**Not here yet.**
- **Workflows.** Bubble's page workflows and the bodies of backend workflows are not in any export. The importer lists backend endpoints with typed parameters; the logic is rewritten by hand.
- **Pages / UI.** Listed by name only.
- **Privacy rules and users' passwords.** Not exported; authorisation and password reset are yours.
- **`constraints`, `sort_field`, `descending`** on the list route — no search or sort.
- **Bulk create (`/bulk`)**, Bubble's Workflow API (`/api/1.1/wf/<name>`) — not served.
- **Files** — URLs, not copied.
- **Auth on the API.** Bubble required a token; this requires nothing.

## Production

- **Environments.** `DATABASE_URL`, `API_URL` (frontend). Separate databases for dev and prod; do the import into each.
- **Processes.** One `backend`; the CLI runs from a workstation or CI.
- **Probes.** `/api/health`, `/api/health/ready`.
- **Migrations.** `0002_bubble.sql` ships; `0003` (generated) and `0004` (your move) are committed and applied by the migrate init container in order.
- **Secrets.** None specific. Do not commit the export JSON — it contains user rows.
- **Model cache.** The parsed model is cached per process after first use; import, then restart or roll the backend.
- **Auth.** None on any route. Until step 8 is done, keep the API inside the cluster network or behind ingress auth; every row of every type is readable.
- **What pages you.** `bubble_things` changing after cut-over (a client still on the old routes); `/api/obj` 400 rate (clients sending what Bubble tolerated).

## Roadmap

- Keyset pagination and `constraints`/`sort_field` on the list route.
- Generated typed route groups per type, so step 5 is a command.
- File copy from Bubble's S3 into your bucket.
- A re-import diff (deleted-in-Bubble rows).
- Multiple apps per instance.
- Auth and a privacy-rule translation table.
"##;

const BUBBLE_AGENT_MAPPING: &str = r##"---
name: data-mapping
description: Run on any change to import/bubble.ts, the Data API routes, or a generated 0003/0004 migration. Checks that no Bubble field can be dropped or mistyped on the way to Postgres, that the moved data equals the landed data, and that the Data API still answers in Bubble's shape.
tools: Read, Grep, Glob, Bash
---

You are the person who will be blamed when a customer's booking has the wrong status after the
migration. Report only what loses, mistypes, or misroutes data — each as
`path:line — what — the Bubble field or row that triggers it — the fix`.

Check:
1. **`mapType` is total.** Every branch returns `pg` and `zod`; the fallthrough returns `jsonb`/`z.unknown()` *and* an `unmapped` message. A new `case` that returns without both is a silent drop.
2. **`runtimeSchema` agrees with `mapType`.** Same base types, same option handling, same list wrapping. They are two functions describing one mapping; a field the DDL types as `double precision` must be `z.number()` at the API.
3. **Built-ins.** `BUILTIN` filtered from `fields`; `normaliseRow` maps them to `_id`, `created_date`, `modified_date`, `created_by`, `slug`; `sql` emits those five columns on every table; the move SQL selects `created_by` and `slug` from `data`, the dates from columns.
4. **Ids and references.** `_id text primary key`; `references` only when the target type is in the export, only for non-list refs, never for `user`; `deferrable initially deferred` present. A type named `user` generates `create table user`, which Postgres refuses (reserved word) — the generated DDL must be renamed (`app_user`) before `make migrate`. A `list.custom.X` is `text[]` with no key — say so if a change claims otherwise.
5. **Option sets.** Enum values are the option *display* strings in both `sql` (quoted with `''` escaping) and `zodSource`; a set present in `types` but absent from `option_sets` falls back to `text` and is reported.
6. **Row normalisation.** `row[f.display] ?? row[f.id] ?? row[f.column]` — a change to the order silently prefers one export style. Rows for unknown types are skipped *and* reported.
7. **Import atomicity.** `saveImport` is one `db.begin`; the upsert is `on conflict (type, _id) do update` on `data` and `modified_date` only (never `created_date`).
8. **Move SQL (0004).** For every column in `0003`: the cast matches the pg type (`::double precision`, `::timestamptz`, `::boolean`, `::<set>_enum`, `array(select jsonb_array_elements_text(...))` for lists, `->` for jsonb). Run the orphan query for each reference before the insert. Row counts per table equal `bubble_things` counts per type.
9. **Data API shape.** List returns `{response: {cursor, results, count, remaining}}` with `remaining = total - cursor - count`, clamped at 0; `limit` ≤ 100; get/patch return `{response}`; post returns `{id}`; delete 204.
10. **Strictness.** `POST` uses `.strict()`, `PATCH` uses `.strict().partial()`; an unknown key is 400. The import path is *not* strict — confirm nothing made it so, or real exports with extra keys will be refused.
11. **Type existence check.** Every `/api/obj/:type*` route calls `typeOf`/`schemaOf` first; a route that reaches the store for an unknown type returns rows from an unrelated import.
12. **Model cache.** `current()` reads `lastImport` once per process; a change that caches `null` forever after an empty first call breaks the first import on a fresh replica.
13. **Secrets in exports.** Nothing logs a row body. The fixture contains no real ids, emails or tokens.

Run `cd backend && bun test`. End with `data-mapping: N findings` and, if 0, which of the above you read.
"##;
