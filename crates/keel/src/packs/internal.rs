//! internal: an admin tool, like Appsmith — a table with sort/filter/CSV, schema-driven forms, approvals, audit, roles

pub fn files(name: &str) -> Vec<(&'static str, String)> {
    let f = |s: &str| s.replace("{{NAME}}", name);
    vec![
        ("backend/src/app.ts", INTERNAL_APP.into()),
        ("backend/src/schema.ts", INTERNAL_SCHEMA.into()),
        ("backend/src/store.ts", INTERNAL_STORE.into()),
        ("backend/src/app.test.ts", INTERNAL_TEST.into()),
        ("backend/migrations/0002_internal.sql", INTERNAL_SQL.into()),
        ("frontend/app/page.tsx", f(INTERNAL_PAGE)),
    ]
}

const INTERNAL_APP: &str = r##"import { Hono, type MiddlewareHandler } from "hono";
import { z } from "zod";
import { createHmac, timingSafeEqual } from "node:crypto";
import { deleteCookie, getCookie, setCookie } from "hono/cookie";
import { ACTIONS, COLUMNS, EDITABLE, RANK, ROLES, RequestInput, STATES, TRANSITIONS, fields, type Action, type Role } from "./schema";
import { pgStore, sortable, type Store, type User } from "./store";

// Appsmith's shape, as code instead of a canvas: one table with a query API (sort, filter,
// search, CSV), a form driven by the schema the API validates with, a state machine for
// approvals, an audit row per change written in the change's transaction, and three roles.
type Env = { Variables: { user: User } };
const COOKIE = "session";

// Identity is the smallest thing that works: a signed cookie carrying the email, first user
// is admin, admins set roles. Swap `whoami` for the auth pack's session lookup.
const secret = () => process.env.SESSION_SECRET ?? "dev-secret-change-me";
const sign = (email: string) => `${Buffer.from(email).toString("base64url")}.${createHmac("sha256", secret()).update(email).digest("base64url")}`;
const verify = (cookie: string | undefined): string | null => {
  const [b, sig] = (cookie ?? "").split(".");
  if (!b || !sig) return null;
  const email = Buffer.from(b, "base64url").toString();
  const want = Buffer.from(createHmac("sha256", secret()).update(email).digest("base64url")), got = Buffer.from(sig);
  return want.length === got.length && timingSafeEqual(want, got) ? email : null;
};

// RFC 4180: quote every field, double the quotes inside. Excel and Sheets both read it.
export const csv = (rows: Record<string, unknown>[], cols: string[]) =>
  [cols, ...rows.map((r) => cols.map((c) => r[c]))].map((line) => line.map((v) => `"${String(v ?? "").replaceAll('"', '""')}"`).join(",")).join("\r\n") + "\r\n";

export function createApp(store: Store) {
  const requireUser: MiddlewareHandler<Env> = async (c, next) => {
    const email = verify(getCookie(c, COOKIE));
    if (!email) return c.json({ error: { message: "not signed in", code: "unauthorized" } }, 401);
    c.set("user", await store.ensureUser(email));
    return next();
  };
  const requireRole = (min: Role): MiddlewareHandler<Env> => async (c, next) =>
    RANK[c.get("user").role] < RANK[min] ? c.json({ error: { message: `${min} role required`, code: "forbidden" } }, 403) : next();
  const notFound = (c: Parameters<MiddlewareHandler<Env>>[0]) => c.json({ error: { message: "not found", code: "not_found" } }, 404);

  const app = new Hono<Env>()
    .get("/api/health", (c) => c.json({ status: "ok" }))
    .get("/api/health/ready", (c) => c.json({ status: "ok", db: "ok" as const }))

    .post("/api/session", async (c) => {
      const p = z.object({ email: z.string().email() }).safeParse(await c.req.json().catch(() => ({})));
      if (!p.success) return c.json({ error: { message: "invalid body", code: "invalid" } }, 400);
      setCookie(c, COOKIE, sign(p.data.email.toLowerCase()), { httpOnly: true, sameSite: "Lax", path: "/", secure: process.env.NODE_ENV === "production" });
      return c.json({ email: p.data.email.toLowerCase() });
    })
    .delete("/api/session", (c) => { deleteCookie(c, COOKIE, { path: "/" }); return c.json({ status: true }); })
    .get("/api/me", requireUser, (c) => c.json({ user: c.get("user") }))

    // The frontend builds its form and its action buttons from this, not from a copy.
    .get("/api/schema", (c) => c.json({ fields: fields(), states: STATES, transitions: TRANSITIONS, editable: EDITABLE, roles: ROLES }))

    .use("/api/records", requireUser)
    .use("/api/records/*", requireUser)
    .get("/api/records", async (c) => {
      const sort = c.req.query("sort") ?? "updated_at", dir = c.req.query("dir") === "asc" ? "asc" : "desc";
      const state = c.req.query("state"), q = c.req.query("q") || undefined;
      if (!sortable(sort)) return c.json({ error: { message: `sort must be one of ${[...COLUMNS, "state", "created_at", "updated_at"].join(", ")}`, code: "invalid" } }, 400);
      if (state && !STATES.includes(state as never)) return c.json({ error: { message: "unknown state", code: "invalid" } }, 400);
      const rows = await store.list({ sort, dir, state: state as never, q });
      if (c.req.query("format") === "csv") {
        c.header("content-type", "text/csv; charset=utf-8");
        c.header("content-disposition", `attachment; filename="requests-${new Date().toISOString().slice(0, 10)}.csv"`);
        return c.body(csv(rows, ["id", ...COLUMNS, "state", "created_by", "created_at", "updated_at"]));
      }
      return c.json({ records: rows });
    })
    .post("/api/records", requireRole("editor"), async (c) => {
      const p = RequestInput.safeParse(await c.req.json().catch(() => ({})));
      if (!p.success) return c.json({ error: { message: "invalid body", code: "invalid", details: p.error.flatten() } }, 400);
      return c.json(await store.create(p.data, c.get("user").email), 201);
    })
    .get("/api/records/:id", async (c) => { const r = await store.get(c.req.param("id")); return r ? c.json(r) : notFound(c); })
    .get("/api/records/:id/audit", async (c) => c.json({ audit: await store.auditFor(c.req.param("id")) }))
    .patch("/api/records/:id", requireRole("editor"), async (c) => {
      const p = RequestInput.partial().strict().safeParse(await c.req.json().catch(() => ({})));
      if (!p.success) return c.json({ error: { message: "invalid body", code: "invalid", details: p.error.flatten() } }, 400);
      const cur = await store.get(c.req.param("id"));
      if (!cur) return notFound(c);
      if (!EDITABLE.includes(cur.state)) return c.json({ error: { message: `a ${cur.state} request cannot be edited`, code: "conflict" } }, 409);
      return c.json(await store.update(cur.id, p.data, c.get("user").email));
    })
    .post("/api/records/:id/:action", async (c) => {
      const action = c.req.param("action") as Action;
      if (!ACTIONS.includes(action)) return notFound(c);
      const cur = await store.get(c.req.param("id"));
      if (!cur) return notFound(c);
      const t = TRANSITIONS[cur.state][action];
      if (!t) return c.json({ error: { message: `cannot ${action} a ${cur.state} request`, code: "conflict", allowed: Object.keys(TRANSITIONS[cur.state]) } }, 409);
      if (RANK[c.get("user").role] < RANK[t.role]) return c.json({ error: { message: `${t.role} role required to ${action}`, code: "forbidden" } }, 403);
      const r = await store.transition(cur.id, cur.state, t.to, action, c.get("user").email);
      // Null here means the row moved between our read and the write: someone else acted first.
      return r ? c.json(r) : c.json({ error: { message: "the request changed; reload", code: "conflict" } }, 409);
    })

    .get("/api/users", requireUser, requireRole("admin"), async (c) => c.json({ users: await store.listUsers() }))
    .patch("/api/users/:id/role", requireUser, requireRole("admin"), async (c) => {
      const p = z.object({ role: z.enum(ROLES) }).safeParse(await c.req.json().catch(() => ({})));
      if (!p.success) return c.json({ error: { message: "invalid body", code: "invalid" } }, 400);
      return (await store.setRole(c.req.param("id"), p.data.role)) ? c.json({ id: c.req.param("id"), role: p.data.role }) : notFound(c);
    });

  return app;
}

const app = createApp(pgStore());
export type AppType = typeof app;
export default app;
"##;

const INTERNAL_SCHEMA: &str = r##"import { z } from "zod";

// The one schema. The backend validates with it; the frontend imports its types and renders
// forms from `fields()`, which is derived from it — there is no second copy to drift.
export const RequestInput = z.object({
  title: z.string().min(1).max(120),
  amount: z.number().min(0).max(1_000_000),
  category: z.enum(["travel", "hardware", "software", "other"]),
  notes: z.string().max(2000).default(""),
});
export type RequestInput = z.infer<typeof RequestInput>;
export const COLUMNS = Object.keys(RequestInput.shape) as (keyof RequestInput)[];

export type Field = { name: string; type: "text" | "number" | "select"; required: boolean; options?: string[] };
export function fields(): Field[] {
  return Object.entries(RequestInput.shape).map(([name, s]) => {
    const inner = s instanceof z.ZodDefault ? s._def.innerType : s;
    const type = inner instanceof z.ZodNumber ? "number" : inner instanceof z.ZodEnum ? "select" : "text";
    return { name, type, required: !(s instanceof z.ZodDefault) && !s.isOptional(), ...(inner instanceof z.ZodEnum ? { options: inner.options as string[] } : {}) };
  });
}

// The approval workflow as data: a state, an action, the state it leads to and the role that
// may take it. Anything not listed here is a 409, and the transition is applied with a
// `where state = from` so two people racing on the same row cannot both win.
export const ROLES = ["admin", "editor", "viewer"] as const;
export type Role = (typeof ROLES)[number];
export const STATES = ["draft", "submitted", "approved", "rejected"] as const;
export type State = (typeof STATES)[number];
export const ACTIONS = ["submit", "approve", "reject", "withdraw", "revise"] as const;
export type Action = (typeof ACTIONS)[number];
export const TRANSITIONS: Record<State, Partial<Record<Action, { to: State; role: Role }>>> = {
  draft: { submit: { to: "submitted", role: "editor" } },
  submitted: { approve: { to: "approved", role: "admin" }, reject: { to: "rejected", role: "admin" }, withdraw: { to: "draft", role: "editor" } },
  rejected: { revise: { to: "draft", role: "editor" } },
  approved: {},
};
/// States in which the record's fields may still be edited.
export const EDITABLE: State[] = ["draft", "rejected"];
export const RANK: Record<Role, number> = { viewer: 0, editor: 1, admin: 2 };
"##;

const INTERNAL_STORE: &str = r##"import { db } from "./db";
import { COLUMNS, type Action, type RequestInput, type Role, type State } from "./schema";

export type User = { id: string; email: string; role: Role };
export type Rec = RequestInput & { id: string; state: State; created_by: string; created_at: string; updated_at: string };
export type Audit = { id: number; record_id: string; actor: string; action: string; before: Rec | null; after: Rec | null; at: string };
export type Query = { sort: keyof Rec; dir: "asc" | "desc"; state?: State; q?: string };

// Every write that changes a record also writes its audit row, inside the same transaction:
// there is no code path that changes a row and can fail to log it, or logs a change that
// was rolled back.
export interface Store {
  ensureUser(email: string): Promise<User>;
  listUsers(): Promise<User[]>;
  setRole(id: string, role: Role): Promise<boolean>;
  list(q: Query): Promise<Rec[]>;
  get(id: string): Promise<Rec | null>;
  create(input: RequestInput, actor: string): Promise<Rec>;
  update(id: string, patch: Partial<RequestInput>, actor: string): Promise<Rec | null>;
  /// Applies `action` only if the row is still in `from`; null means it was not.
  transition(id: string, from: State, to: State, action: Action, actor: string): Promise<Rec | null>;
  auditFor(recordId: string): Promise<Audit[]>;
}

const SORTABLE = new Set<string>([...COLUMNS, "state", "created_at", "updated_at"]);
export const sortable = (s: string): s is keyof Rec => SORTABLE.has(s);
const REC = "id, title, amount::float8 as amount, category, notes, state, created_by, created_at, updated_at";

export function pgStore(): Store {
  const audit = (tx: typeof db, action: string, actor: string, before: Rec | null, after: Rec | null) =>
    tx`insert into audit_log (record_id, actor, action, before, after) values (${(after ?? before)!.id}, ${actor}, ${action}, ${before ? tx.json(before as never) : null}, ${after ? tx.json(after as never) : null})`;
  return {
    ensureUser: async (email) => db.begin(async (tx) => {
      await tx`lock table users in share row exclusive mode`;
      const found = (await tx<User[]>`select id, email, role from users where email = ${email}`)[0];
      if (found) return found;
      const [{ n }] = await tx<{ n: number }[]>`select count(*)::int as n from users`;
      return (await tx<User[]>`insert into users (id, email, role) values (${crypto.randomUUID()}, ${email}, ${n === 0 ? "admin" : "viewer"}) returning id, email, role`)[0];
    }),
    listUsers: async () => db<User[]>`select id, email, role from users order by created_at`,
    setRole: async (id, role) => (await db`update users set role = ${role} where id = ${id}`).count > 0,
    list: async (q) => db<Rec[]>`select ${db.unsafe(REC)} from requests
      where (${q.state ?? null}::text is null or state = ${q.state ?? null})
        and (${q.q ?? null}::text is null or title ilike ${"%" + (q.q ?? "") + "%"} or notes ilike ${"%" + (q.q ?? "") + "%"})
      order by ${db.unsafe(String(q.sort))} ${db.unsafe(q.dir === "desc" ? "desc" : "asc")}, id limit 1000`,
    get: async (id) => (await db<Rec[]>`select ${db.unsafe(REC)} from requests where id = ${id}`)[0] ?? null,
    create: async (i, actor) => db.begin(async (tx) => {
      const [r] = await tx<Rec[]>`insert into requests (id, title, amount, category, notes, created_by) values (${crypto.randomUUID()}, ${i.title}, ${i.amount}, ${i.category}, ${i.notes}, ${actor}) returning ${tx.unsafe(REC)}`;
      await audit(tx as unknown as typeof db, "create", actor, null, r);
      return r;
    }),
    update: async (id, patch, actor) => db.begin(async (tx) => {
      const before = (await tx<Rec[]>`select ${tx.unsafe(REC)} from requests where id = ${id} for update`)[0];
      if (!before) return null;
      const next = { ...before, ...patch };
      const [after] = await tx<Rec[]>`update requests set title = ${next.title}, amount = ${next.amount}, category = ${next.category}, notes = ${next.notes}, updated_at = now() where id = ${id} returning ${tx.unsafe(REC)}`;
      await audit(tx as unknown as typeof db, "update", actor, before, after);
      return after;
    }),
    transition: async (id, from, to, action, actor) => db.begin(async (tx) => {
      const before = (await tx<Rec[]>`select ${tx.unsafe(REC)} from requests where id = ${id} and state = ${from} for update`)[0];
      if (!before) return null;
      const [after] = await tx<Rec[]>`update requests set state = ${to}, updated_at = now() where id = ${id} returning ${tx.unsafe(REC)}`;
      await audit(tx as unknown as typeof db, action, actor, before, after);
      return after;
    }),
    auditFor: async (rid) => db<Audit[]>`select id, record_id, actor, action, before, after, at from audit_log where record_id = ${rid} order by id desc`,
  };
}

export function memoryStore(): Store {
  const users: User[] = [], recs = new Map<string, Rec>(), log: Audit[] = [];
  const audit = (action: string, actor: string, before: Rec | null, after: Rec | null) =>
    log.unshift({ id: log.length + 1, record_id: (after ?? before)!.id, actor, action, before, after, at: new Date().toISOString() });
  return {
    ensureUser: async (email) => { let u = users.find((u) => u.email === email); if (!u) { u = { id: crypto.randomUUID(), email, role: users.length === 0 ? "admin" : "viewer" }; users.push(u); } return { ...u }; },
    listUsers: async () => users.map((u) => ({ ...u })),
    setRole: async (id, role) => { const u = users.find((u) => u.id === id); if (!u) return false; u.role = role; return true; },
    list: async (q) => {
      const needle = q.q?.toLowerCase();
      return [...recs.values()]
        .filter((r) => (!q.state || r.state === q.state) && (!needle || r.title.toLowerCase().includes(needle) || r.notes.toLowerCase().includes(needle)))
        .sort((a, b) => { const x = a[q.sort], y = b[q.sort]; const c = x < y ? -1 : x > y ? 1 : a.id.localeCompare(b.id); return q.dir === "desc" ? -c : c; });
    },
    get: async (id) => recs.get(id) ?? null,
    create: async (i, actor) => { const t = new Date().toISOString(); const r: Rec = { ...i, id: crypto.randomUUID(), state: "draft", created_by: actor, created_at: t, updated_at: t }; recs.set(r.id, r); audit("create", actor, null, r); return { ...r }; },
    update: async (id, patch, actor) => { const before = recs.get(id); if (!before) return null; const after = { ...before, ...patch, updated_at: new Date().toISOString() }; recs.set(id, after); audit("update", actor, before, after); return { ...after }; },
    transition: async (id, from, to, action, actor) => { const before = recs.get(id); if (!before || before.state !== from) return null; const after = { ...before, state: to, updated_at: new Date().toISOString() }; recs.set(id, after); audit(action, actor, before, after); return { ...after }; },
    auditFor: async (rid) => log.filter((a) => a.record_id === rid),
  };
}
"##;

const INTERNAL_TEST: &str = r##"import { describe, expect, test } from "bun:test";
import { createApp } from "./app";
import { fields, TRANSITIONS } from "./schema";
import { memoryStore } from "./store";

// The tool tested as HTTP against the memory store. The first user to sign in is admin.
function harness() {
  const store = memoryStore();
  const app = createApp(store);
  const as = async (email: string) => {
    const res = await app.request("/api/session", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ email }) });
    const cookie = res.headers.get("set-cookie")!.split(";")[0]!;
    const call = (path: string, method = "GET", body?: unknown) =>
      app.request(path, { method, headers: { "content-type": "application/json", cookie }, body: body === undefined ? undefined : JSON.stringify(body) });
    await call("/api/me"); // the account exists from the first authenticated request
    return call;
  };
  return { app, store, as };
}
const req = (title: string, amount = 10) => ({ title, amount, category: "travel" });

describe("internal", () => {
  test("the form schema is the validation schema", async () => {
    const h = harness();
    const admin = await h.as("admin@x.com");
    const f = fields();
    expect(f.find((x) => x.name === "category")).toEqual({ name: "category", type: "select", required: true, options: ["travel", "hardware", "software", "other"] });
    expect(f.find((x) => x.name === "notes")?.required).toBe(false);
    expect((await (await h.app.request("/api/schema")).json()).fields).toEqual(f);
    const bad = await admin("/api/records", "POST", { title: "", amount: -1, category: "boats" });
    expect(bad.status).toBe(400);
    expect(Object.keys((await bad.json()).error.details.fieldErrors).sort()).toEqual(["amount", "category", "title"]);
    expect((await admin("/api/records", "POST", req("ok"))).status).toBe(201);
  });

  test("the state machine refuses what the table does not list, and roles gate each step", async () => {
    const h = harness();
    const admin = await h.as("admin@x.com"), editor = await h.as("ed@x.com");
    const users = (await (await admin("/api/users")).json()).users as { id: string; email: string }[];
    await admin(`/api/users/${users.find((u) => u.email === "ed@x.com")!.id}/role`, "PATCH", { role: "editor" });
    const r = await (await editor("/api/records", "POST", req("laptop", 1200))).json();
    // Not from draft: 409 names what is allowed. Editor cannot approve: 403.
    const early = await editor(`/api/records/${r.id}/approve`, "POST");
    expect(early.status).toBe(409);
    expect((await early.json()).error.allowed).toEqual(["submit"]);
    expect((await editor(`/api/records/${r.id}/submit`, "POST")).status).toBe(200);
    expect((await editor(`/api/records/${r.id}/approve`, "POST")).status).toBe(403);
    expect((await editor(`/api/records/${r.id}`, "PATCH", { amount: 1 })).status).toBe(409);
    expect((await admin(`/api/records/${r.id}/reject`, "POST")).status).toBe(200);
    expect((await editor(`/api/records/${r.id}/revise`, "POST")).status).toBe(200);
    expect((await editor(`/api/records/${r.id}/submit`, "POST")).status).toBe(200);
    expect((await (await admin(`/api/records/${r.id}/approve`, "POST")).json()).state).toBe("approved");
    // Approved is terminal, for everyone.
    expect(TRANSITIONS.approved).toEqual({});
    expect((await admin(`/api/records/${r.id}/reject`, "POST")).status).toBe(409);
    expect((await admin(`/api/records/${r.id}/nonsense`, "POST")).status).toBe(404);
  });

  test("every change has an audit row with before and after; a refused change has none", async () => {
    const h = harness();
    const admin = await h.as("admin@x.com");
    const r = await (await admin("/api/records", "POST", req("chair", 300))).json();
    await admin(`/api/records/${r.id}`, "PATCH", { amount: 350 });
    await admin(`/api/records/${r.id}/submit`, "POST");
    await admin(`/api/records/${r.id}/withdraw`, "POST");
    await admin(`/api/records/${r.id}/approve`, "POST"); // 409: draft cannot be approved
    const audit = (await (await admin(`/api/records/${r.id}/audit`)).json()).audit as { action: string; actor: string; before: { amount: number; state: string } | null; after: { amount: number; state: string } }[];
    expect(audit.map((a) => a.action)).toEqual(["withdraw", "submit", "update", "create"]);
    expect(audit[2]).toMatchObject({ actor: "admin@x.com", before: { amount: 300 }, after: { amount: 350 } });
    expect(audit[0]).toMatchObject({ before: { state: "submitted" }, after: { state: "draft" } });
    expect(audit[3]!.before).toBeNull();
  });

  test("viewers read, and only read", async () => {
    const h = harness();
    const admin = await h.as("admin@x.com"), viewer = await h.as("v@x.com");
    const r = await (await admin("/api/records", "POST", req("desk"))).json();
    expect((await viewer("/api/records")).status).toBe(200);
    expect((await viewer(`/api/records/${r.id}`)).status).toBe(200);
    expect((await viewer("/api/records", "POST", req("no"))).status).toBe(403);
    expect((await viewer(`/api/records/${r.id}`, "PATCH", { title: "no" })).status).toBe(403);
    expect((await viewer(`/api/records/${r.id}/submit`, "POST")).status).toBe(403);
    expect((await viewer("/api/users")).status).toBe(403);
    expect((await h.app.request("/api/records")).status).toBe(401);
  });

  test("list sorts on allowed columns, filters, searches, and exports CSV that survives commas and quotes", async () => {
    const h = harness();
    const admin = await h.as("admin@x.com");
    for (const [t, a] of [['Monitor, 27"', 400], ["Cable", 5], ["Dock", 150]] as const) await admin("/api/records", "POST", req(t, a));
    const cable = (await (await admin("/api/records?q=cable")).json()).records[0];
    await admin(`/api/records/${cable.id}/submit`, "POST");
    const byAmount = (await (await admin("/api/records?sort=amount&dir=desc")).json()).records.map((r: { amount: number }) => r.amount);
    expect(byAmount).toEqual([400, 150, 5]);
    expect((await (await admin("/api/records?state=submitted")).json()).records.map((r: { title: string }) => r.title)).toEqual(["Cable"]);
    expect((await admin("/api/records?sort=created_by;drop")).status).toBe(400);
    expect((await admin("/api/records?state=lost")).status).toBe(400);
    const res = await admin("/api/records?format=csv&sort=amount&dir=asc");
    expect(res.headers.get("content-type")).toContain("text/csv");
    const lines = (await res.text()).split("\r\n");
    expect(lines[0]).toBe('"id","title","amount","category","notes","state","created_by","created_at","updated_at"');
    expect(lines[3]).toContain('"Monitor, 27"""');
    expect(lines.length).toBe(5);
  });
});
"##;

const INTERNAL_SQL: &str = r##"create table if not exists users (
  id uuid primary key,
  email text not null unique,
  role text not null default 'viewer' check (role in ('admin', 'editor', 'viewer')),
  created_at timestamptz not null default now()
);
-- The table the tool is over. Columns mirror RequestInput in schema.ts; the state column is
-- only ever changed through the transition table there.
create table if not exists requests (
  id uuid primary key,
  title text not null,
  amount numeric(12, 2) not null,
  category text not null,
  notes text not null default '',
  state text not null default 'draft' check (state in ('draft', 'submitted', 'approved', 'rejected')),
  created_by text not null,
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now()
);
create index if not exists requests_state on requests (state, updated_at desc);
-- One row per change, written in the same transaction as the change. before/after are the
-- whole record, so the log reads without the table.
create table if not exists audit_log (
  id bigserial primary key,
  record_id uuid not null,
  actor text not null,
  action text not null,
  before jsonb,
  after jsonb,
  at timestamptz not null default now()
);
create index if not exists audit_record on audit_log (record_id, id desc);
"##;

const INTERNAL_PAGE: &str = r##""use client";
import { useEffect, useState } from "react";
import type { Field, RequestInput, State, Action } from "@backend/schema";

type Rec = RequestInput & { id: string; state: State; created_by: string; updated_at: string };
type Schema = { fields: Field[]; states: State[]; transitions: Record<State, Partial<Record<Action, { to: State; role: string }>>>; editable: State[] };
type Audit = { id: number; action: string; actor: string; at: string; before: Rec | null; after: Rec | null };

// One table over the requests: click a header to sort, filter by state, search, edit a draft
// cell in place, act on a row with the buttons its state allows, export what you see as CSV.
// The form below is rendered from /api/schema — the same zod object the API validates with.
export default function Home() {
  const [me, setMe] = useState<{ email: string; role: string } | null>(null);
  const [email, setEmail] = useState("");
  const [schema, setSchema] = useState<Schema | null>(null);
  const [rows, setRows] = useState<Rec[]>([]);
  const [sort, setSort] = useState<{ by: string; dir: "asc" | "desc" }>({ by: "updated_at", dir: "desc" });
  const [state, setState] = useState("");
  const [q, setQ] = useState("");
  const [form, setForm] = useState<Record<string, string>>({});
  const [error, setError] = useState<string | null>(null);
  const [audit, setAudit] = useState<{ id: string; rows: Audit[] } | null>(null);

  const j = (path: string, method = "GET", body?: unknown) => fetch(path, { method, headers: { "content-type": "application/json" }, body: body === undefined ? undefined : JSON.stringify(body) });
  const query = `sort=${sort.by}&dir=${sort.dir}${state ? `&state=${state}` : ""}${q ? `&q=${encodeURIComponent(q)}` : ""}`;
  const load = async () => { const r = await j(`/api/records?${query}`); if (r.ok) setRows((await r.json()).records); };
  const act = async (res: Promise<Response>) => { const r = await res; setError(r.ok ? null : ((await r.json()).error?.message ?? r.statusText)); void load(); };

  useEffect(() => { void j("/api/schema").then((r) => r.json()).then(setSchema); void j("/api/me").then(async (r) => setMe(r.ok ? (await r.json()).user : null)); }, []);
  useEffect(() => { if (me) void load(); }, [me, sort, state, q]); // eslint-disable-line react-hooks/exhaustive-deps

  const submit = async () => {
    const body = Object.fromEntries((schema?.fields ?? []).map((f) => [f.name, f.type === "number" ? Number(form[f.name] ?? 0) : (form[f.name] ?? "")]));
    await act(j("/api/records", "POST", body)); setForm({});
  };
  const edit = (r: Rec, field: string, value: string) => {
    const f = schema?.fields.find((x) => x.name === field);
    if (String(r[field as keyof Rec]) !== value) void act(j(`/api/records/${r.id}`, "PATCH", { [field]: f?.type === "number" ? Number(value) : value }));
  };
  const head = (col: string) => <th key={col} onClick={() => setSort({ by: col, dir: sort.by === col && sort.dir === "asc" ? "desc" : "asc" })} style={{ cursor: "pointer" }}>{col}{sort.by === col ? (sort.dir === "asc" ? " ▲" : " ▼") : ""}</th>;

  if (!me) return (
    <main><h1>{{NAME}} — requests</h1>
      <p><input type="email" placeholder="you@example.com" value={email} onChange={(e) => setEmail(e.target.value)} /> <button onClick={() => j("/api/session", "POST", { email }).then(() => j("/api/me")).then(async (r) => setMe((await r.json()).user))}>Sign in</button> — the first account is admin.</p>
    </main>);
  const cols = schema?.fields.map((f) => f.name) ?? [];
  return (
    <main>
      <h1>{{NAME}} — requests</h1>
      <p><code>{me.email}</code> · {me.role} <button onClick={() => j("/api/session", "DELETE").then(() => setMe(null))}>Sign out</button> · <a href={`/api/records?${query}&format=csv`}>Export CSV</a></p>
      <pre>{`curl -b session=... "/api/records?sort=amount&dir=desc&state=submitted&format=csv"`}</pre>
      <p><select value={state} onChange={(e) => setState(e.target.value)}><option value="">all states</option>{schema?.states.map((s) => <option key={s}>{s}</option>)}</select> <input placeholder="search" value={q} onChange={(e) => setQ(e.target.value)} /></p>
      {error && <p style={{ color: "crimson" }}>{error}</p>}
      <table><thead><tr>{cols.map(head)}{head("state")}<th>by</th>{head("updated_at")}<th>actions</th></tr></thead>
        <tbody>{rows.map((r) => (
          <tr key={r.id}>
            {cols.map((c) => <td key={c}>{schema?.editable.includes(r.state) && me.role !== "viewer"
              ? <input defaultValue={String(r[c as keyof Rec])} onBlur={(e) => edit(r, c, e.target.value)} size={c === "title" ? 24 : 8} />
              : String(r[c as keyof Rec])}</td>)}
            <td><strong>{r.state}</strong></td><td>{r.created_by}</td><td>{r.updated_at.slice(0, 16)}</td>
            <td>{Object.keys(schema?.transitions[r.state] ?? {}).map((a) => <button key={a} onClick={() => act(j(`/api/records/${r.id}/${a}`, "POST"))}>{a}</button>)}
              <button onClick={async () => setAudit(audit?.id === r.id ? null : { id: r.id, rows: (await (await j(`/api/records/${r.id}/audit`)).json()).audit })}>log</button></td>
          </tr>))}</tbody></table>
      {audit && <ul>{audit.rows.map((a) => <li key={a.id}><code>{a.at.slice(0, 19)}</code> {a.actor} <strong>{a.action}</strong>{a.before && a.after && a.action === "update" && ` ${cols.filter((c) => a.before![c as keyof Rec] !== a.after![c as keyof Rec]).map((c) => `${c}: ${String(a.before![c as keyof Rec])} → ${String(a.after![c as keyof Rec])}`).join(", ")}`}{a.before && a.after && a.before.state !== a.after.state && ` ${a.before.state} → ${a.after.state}`}</li>)}</ul>}
      {me.role !== "viewer" && schema && (
        <section><h2>New request</h2>
          {schema.fields.map((f) => <p key={f.name}><label>{f.name}{f.required ? " *" : ""}{" "}
            {f.type === "select" ? <select value={form[f.name] ?? ""} onChange={(e) => setForm({ ...form, [f.name]: e.target.value })}><option value="">—</option>{f.options?.map((o) => <option key={o}>{o}</option>)}</select>
              : <input type={f.type} value={form[f.name] ?? ""} onChange={(e) => setForm({ ...form, [f.name]: e.target.value })} />}</label></p>)}
          <button onClick={submit}>Create</button>
        </section>)}
    </main>
  );
}
"##;
