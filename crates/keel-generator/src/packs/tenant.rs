//! tenant: a multi-tenant SaaS, like Cal.com — orgs, memberships, a scoped data layer, invitations, plans, Stripe

pub fn files(name: &str) -> Vec<(&'static str, String)> {
    let f = |s: &str| s.replace("{{NAME}}", name);
    vec![
        ("backend/src/app.ts", TENANT_APP.into()),
        ("backend/src/store.ts", TENANT_STORE.into()),
        ("backend/src/app.test.ts", TENANT_TEST.into()),
        ("backend/migrations/0002_tenant.sql", TENANT_SQL.into()),
        ("frontend/app/page.tsx", f(TENANT_PAGE)),
        ("CLAUDE.md", f(TENANT_CLAUDE)),
        ("AGENTS.md", f(TENANT_AGENTS)),
        ("README.md", f(TENANT_README)),
        (".claude/agents/tenant-isolation.md", TENANT_REVIEWER.into()),
    ]
}

const TENANT_APP: &str = r##"import { Hono, type Context, type MiddlewareHandler } from "hono";
import { z } from "zod";
import { createHmac, randomBytes, createHash, timingSafeEqual } from "node:crypto";
import { deleteCookie, getCookie, setCookie } from "hono/cookie";
import { pgStore, type Org, type Plan, type Role, type Store, type User } from "./store";

// Cal.com's shape: an organisation with a slug, memberships with owner/admin/member roles,
// invitations by email that expire, plan limits checked before the write, and Stripe's
// customer.subscription.* webhooks as the only thing that changes the plan. Every route under
// /api/orgs/:slug resolves the membership first; a non-member sees 404, not 403 — the org's
// existence is tenant data too.
export type SendEmail = (to: string, link: string) => Promise<void>;
type Clock = () => Date;
type Env = { Variables: { user: User; org: Org; role: Role } };

export const PLANS: Record<Plan, { projects: number; members: number }> = {
  free: { projects: 3, members: 2 },
  pro: { projects: 50, members: 25 },
  enterprise: { projects: Infinity, members: Infinity },
};
export const INVITE_TTL_MS = 7 * 24 * 3600_000;
const RANK: Record<Role, number> = { member: 0, admin: 1, owner: 2 };
const sha = (s: string) => createHash("sha256").update(s).digest("hex");
const COOKIE = "session";

// Identity is the smallest thing that works here: a signed cookie carrying the email, so the
// tenant logic can be exercised end to end. Swap `whoami` for the auth pack's session lookup.
const sessionSecret = () => process.env.SESSION_SECRET ?? "dev-secret-change-me";
const sign = (email: string) => `${Buffer.from(email).toString("base64url")}.${createHmac("sha256", sessionSecret()).update(email).digest("base64url")}`;
const verify = (cookie: string | undefined): string | null => {
  const [b, sig] = (cookie ?? "").split(".");
  if (!b || !sig) return null;
  const email = Buffer.from(b, "base64url").toString();
  const want = Buffer.from(createHmac("sha256", sessionSecret()).update(email).digest("base64url"));
  const got = Buffer.from(sig);
  return want.length === got.length && timingSafeEqual(want, got) ? email : null;
};

// Stripe-Signature: t=<unix>,v1=<hmac-sha256 of "<t>.<raw body>"> with the endpoint secret.
export function verifyStripe(raw: string, header: string | undefined, secret: string, now: Date, toleranceS = 300): boolean {
  const parts = Object.fromEntries((header ?? "").split(",").map((kv) => kv.split("=") as [string, string]));
  const t = Number(parts.t), v1 = parts.v1 ?? "";
  if (!Number.isFinite(t) || Math.abs(now.getTime() / 1000 - t) > toleranceS) return false;
  const want = Buffer.from(createHmac("sha256", secret).update(`${t}.${raw}`).digest("hex"));
  const got = Buffer.from(v1);
  return want.length === got.length && timingSafeEqual(want, got);
}

export const consoleEmail: SendEmail = async (to, link) => {
  if (process.env.NODE_ENV === "production") throw new Error("no email sender configured");
  console.log(JSON.stringify({ level: "info", msg: "invitation (dev only)", to, link }));
};

export function createApp(store: Store, sendEmail: SendEmail = consoleEmail, now: Clock = () => new Date()) {
  const requireUser: MiddlewareHandler<Env> = async (c, next) => {
    const email = verify(getCookie(c, COOKIE));
    if (!email) return c.json({ error: { message: "not signed in", code: "unauthorized" } }, 401);
    c.set("user", await store.ensureUser(email));
    return next();
  };
  // The tenant boundary: resolve org and membership, or 404. Nothing below sees an org id
  // that did not come through here.
  const requireOrg: MiddlewareHandler<Env> = async (c, next) => {
    const org = await store.orgBySlug(c.req.param("slug") ?? "");
    const role = org ? await store.membership(org.id, c.get("user").id) : null;
    if (!org || !role) return c.json({ error: { message: "not found", code: "not_found" } }, 404);
    c.set("org", org); c.set("role", role);
    return next();
  };
  const requireRole = (min: Role): MiddlewareHandler<Env> => async (c, next) =>
    RANK[c.get("role")] < RANK[min] ? c.json({ error: { message: `${min} role required`, code: "forbidden" } }, 403) : next();
  // Plan limits, checked in middleware before the handler so the rule lives in one place.
  const withinPlan = (kind: "projects" | "members"): MiddlewareHandler<Env> => async (c, next) => {
    const t = store.tenant(c.get("org").id);
    const used = kind === "projects" ? await t.countProjects() : await t.countMembers();
    const limit = PLANS[c.get("org").plan][kind];
    if (used >= limit) return c.json({ error: { message: `${c.get("org").plan} plan allows ${limit} ${kind}`, code: "plan_limit", upgrade: "/billing" } }, 402);
    return next();
  };
  const bad = (c: Context<Env>, e: z.ZodError) => c.json({ error: { message: "invalid body", code: "invalid", details: e.flatten() } }, 400);

  const app = new Hono<Env>()
    .get("/api/health", (c) => c.json({ status: "ok" }))
    .get("/api/health/ready", (c) => c.json({ status: "ok", db: "ok" as const }))

    // ── identity ────────────────────────────────────────────────────────────────────────
    .post("/api/session", async (c) => {
      const p = z.object({ email: z.string().email() }).safeParse(await c.req.json().catch(() => ({})));
      if (!p.success) return bad(c, p.error);
      setCookie(c, COOKIE, sign(p.data.email.toLowerCase()), { httpOnly: true, sameSite: "Lax", path: "/", secure: process.env.NODE_ENV === "production" });
      return c.json({ email: p.data.email.toLowerCase() });
    })
    .delete("/api/session", (c) => { deleteCookie(c, COOKIE, { path: "/" }); return c.json({ status: true }); })
    .get("/api/me", requireUser, async (c) => c.json({ user: c.get("user"), orgs: await store.orgsFor(c.get("user").id) }))

    // ── orgs ────────────────────────────────────────────────────────────────────────────
    .post("/api/orgs", requireUser, async (c) => {
      const p = z.object({ slug: z.string().regex(/^[a-z0-9-]{2,40}$/), name: z.string().min(1).max(100) }).safeParse(await c.req.json().catch(() => ({})));
      if (!p.success) return bad(c, p.error);
      if (await store.orgBySlug(p.data.slug)) return c.json({ error: { message: "slug taken", code: "conflict" } }, 409);
      return c.json(await store.createOrg(p.data.slug, p.data.name, c.get("user").id), 201);
    })
    .use("/api/orgs/:slug/*", requireUser, requireOrg)
    .get("/api/orgs/:slug", requireUser, requireOrg, async (c) => {
      const t = store.tenant(c.get("org").id);
      return c.json({ org: c.get("org"), role: c.get("role"), limits: PLANS[c.get("org").plan], usage: { projects: await t.countProjects(), members: await t.countMembers() } });
    })
    .get("/api/orgs/:slug/members", async (c) => c.json({ members: await store.members(c.get("org").id) }))

    // ── invitations ─────────────────────────────────────────────────────────────────────
    .get("/api/orgs/:slug/invitations", requireRole("admin"), async (c) => c.json({ invitations: await store.listInvitations(c.get("org").id, now()) }))
    .post("/api/orgs/:slug/invitations", requireRole("admin"), withinPlan("members"), async (c) => {
      const p = z.object({ email: z.string().email(), role: z.enum(["admin", "member"]).default("member") }).safeParse(await c.req.json().catch(() => ({})));
      if (!p.success) return bad(c, p.error);
      const token = randomBytes(32).toString("base64url");
      const inv = { id: crypto.randomUUID(), org_id: c.get("org").id, email: p.data.email.toLowerCase(), role: p.data.role, expires_at: new Date(now().getTime() + INVITE_TTL_MS).toISOString(), accepted_at: null };
      await store.putInvitation({ ...inv, token_hash: sha(token) });
      await sendEmail(inv.email, `${process.env.APP_URL ?? "http://localhost:3000"}/?invite=${token}`);
      return c.json(inv, 201);
    })
    .post("/api/invitations/accept", requireUser, async (c) => {
      const p = z.object({ token: z.string().min(10) }).safeParse(await c.req.json().catch(() => ({})));
      if (!p.success) return bad(c, p.error);
      const inv = await store.acceptInvitation(sha(p.data.token), c.get("user"), now());
      return inv ? c.json(inv) : c.json({ error: { message: "invitation invalid, expired, or for another address", code: "invalid_token" } }, 401);
    })

    // ── tenant data ─────────────────────────────────────────────────────────────────────
    .get("/api/orgs/:slug/projects", async (c) => c.json({ projects: await store.tenant(c.get("org").id).listProjects() }))
    .post("/api/orgs/:slug/projects", withinPlan("projects"), async (c) => {
      const p = z.object({ name: z.string().min(1).max(100) }).safeParse(await c.req.json().catch(() => ({})));
      if (!p.success) return bad(c, p.error);
      return c.json(await store.tenant(c.get("org").id).createProject(p.data.name), 201);
    })
    .get("/api/orgs/:slug/projects/:id", async (c) => {
      const p = await store.tenant(c.get("org").id).getProject(c.req.param("id"));
      return p ? c.json(p) : c.json({ error: { message: "not found", code: "not_found" } }, 404);
    })
    .delete("/api/orgs/:slug/projects/:id", requireRole("admin"), async (c) =>
      (await store.tenant(c.get("org").id).deleteProject(c.req.param("id"))) ? c.body(null, 204) : c.json({ error: { message: "not found", code: "not_found" } }, 404))

    // ── billing ─────────────────────────────────────────────────────────────────────────
    // Stripe is the source of truth for the plan. The org is found by customer id (set at
    // checkout via metadata or here on the first event carrying `metadata.org_id`).
    .post("/api/webhooks/stripe", async (c) => {
      const raw = await c.req.text();
      const secret = process.env.STRIPE_WEBHOOK_SECRET ?? "";
      if (!secret || !verifyStripe(raw, c.req.header("stripe-signature"), secret, now())) return c.json({ error: { message: "bad signature", code: "invalid_signature" } }, 400);
      const ev = z.object({
        id: z.string(), type: z.string(),
        data: z.object({ object: z.object({ customer: z.string().nullish(), status: z.string().optional(), metadata: z.record(z.string()).optional(), items: z.object({ data: z.array(z.object({ price: z.object({ lookup_key: z.string().nullish() }) })) }).optional() }) }),
      }).safeParse(JSON.parse(raw || "{}"));
      if (!ev.success) return bad(c, ev.error);
      if (!(await store.recordStripeEvent(ev.data.id, ev.data.type))) return c.json({ received: true, duplicate: true });
      if (ev.data.type.startsWith("customer.subscription.")) {
        const o = ev.data.data.object;
        const org = (o.customer && (await store.orgByCustomer(o.customer))) ?? (o.metadata?.org_id ? await store.orgBySlug(o.metadata.org_id) : null);
        if (org) {
          const live = ev.data.type !== "customer.subscription.deleted" && ["active", "trialing"].includes(o.status ?? "");
          const key = o.items?.data[0]?.price.lookup_key ?? o.metadata?.plan;
          const plan: Plan = live && (key === "pro" || key === "enterprise") ? key : "free";
          await store.setBilling(org.id, { plan, subscription_status: o.status ?? null, stripe_customer_id: o.customer ?? undefined });
        }
      }
      return c.json({ received: true });
    });

  return app;
}

const app = createApp(pgStore());
export type AppType = typeof app;
export default app;
"##;

const TENANT_STORE: &str = r##"import { db } from "./db";

// Two layers. The global one knows about users, orgs, memberships, invitations and billing.
// The tenant one is only reachable through `tenant(orgId)`, and every query it runs carries
// that org id — there is no way to ask for a project without saying whose it is.
export type Role = "owner" | "admin" | "member";
export type Plan = "free" | "pro" | "enterprise";
export type User = { id: string; email: string };
export type Org = { id: string; slug: string; name: string; plan: Plan; stripe_customer_id: string | null; subscription_status: string | null };
export type Member = { user_id: string; email: string; role: Role };
export type Invitation = { id: string; org_id: string; email: string; role: Exclude<Role, "owner">; expires_at: string; accepted_at: string | null };
export type Project = { org_id: string; id: string; name: string; created_at: string };

export interface Tenant {
  listProjects(): Promise<Project[]>;
  getProject(id: string): Promise<Project | null>;
  createProject(name: string): Promise<Project>;
  deleteProject(id: string): Promise<boolean>;
  countProjects(): Promise<number>;
  countMembers(): Promise<number>;
}

export interface Store {
  ensureUser(email: string): Promise<User>;
  createOrg(slug: string, name: string, ownerId: string): Promise<Org>;
  orgBySlug(slug: string): Promise<Org | null>;
  orgByCustomer(customerId: string): Promise<Org | null>;
  orgsFor(userId: string): Promise<(Org & { role: Role })[]>;
  membership(orgId: string, userId: string): Promise<Role | null>;
  members(orgId: string): Promise<Member[]>;
  addMember(orgId: string, userId: string, role: Role): Promise<void>;
  setBilling(orgId: string, b: { plan: Plan; stripe_customer_id?: string; subscription_status: string | null }): Promise<void>;
  putInvitation(inv: Invitation & { token_hash: string }): Promise<void>;
  listInvitations(orgId: string, now: Date): Promise<Invitation[]>;
  /// Marks the invitation accepted and adds the member in one transaction. Null when the
  /// token is unknown, used, expired, or was sent to a different address.
  acceptInvitation(tokenHash: string, user: User, now: Date): Promise<Invitation | null>;
  /// Returns false when the event id was already seen.
  recordStripeEvent(id: string, type: string): Promise<boolean>;
  tenant(orgId: string): Tenant;
}

const ORG = "id, slug, name, plan, stripe_customer_id, subscription_status";

export function pgStore(): Store {
  return {
    ensureUser: async (email) => (await db<User[]>`insert into users (id, email) values (${crypto.randomUUID()}, ${email}) on conflict (email) do update set email = excluded.email returning id, email`)[0],
    createOrg: async (slug, name, ownerId) => db.begin(async (tx) => {
      const [org] = await tx<Org[]>`insert into orgs (id, slug, name) values (${crypto.randomUUID()}, ${slug}, ${name}) returning ${tx.unsafe(ORG)}`;
      await tx`insert into memberships (org_id, user_id, role) values (${org.id}, ${ownerId}, 'owner')`;
      return org;
    }),
    orgBySlug: async (slug) => (await db<Org[]>`select ${db.unsafe(ORG)} from orgs where slug = ${slug}`)[0] ?? null,
    orgByCustomer: async (cid) => (await db<Org[]>`select ${db.unsafe(ORG)} from orgs where stripe_customer_id = ${cid}`)[0] ?? null,
    orgsFor: async (uid) => db`select o.id, o.slug, o.name, o.plan, o.stripe_customer_id, o.subscription_status, m.role from orgs o join memberships m on m.org_id = o.id where m.user_id = ${uid} order by o.created_at`,
    membership: async (orgId, uid) => (await db<{ role: Role }[]>`select role from memberships where org_id = ${orgId} and user_id = ${uid}`)[0]?.role ?? null,
    members: async (orgId) => db`select m.user_id, u.email, m.role from memberships m join users u on u.id = m.user_id where m.org_id = ${orgId} order by m.created_at`,
    addMember: async (orgId, uid, role) => { await db`insert into memberships (org_id, user_id, role) values (${orgId}, ${uid}, ${role}) on conflict do nothing`; },
    setBilling: async (orgId, b) => { await db`update orgs set plan = ${b.plan}, subscription_status = ${b.subscription_status}, stripe_customer_id = coalesce(${b.stripe_customer_id ?? null}, stripe_customer_id) where id = ${orgId}`; },
    putInvitation: async (i) => { await db`insert into invitations (id, org_id, email, role, token_hash, expires_at) values (${i.id}, ${i.org_id}, ${i.email}, ${i.role}, ${i.token_hash}, ${i.expires_at})`; },
    listInvitations: async (orgId, now) => db`select id, org_id, email, role, expires_at, accepted_at from invitations where org_id = ${orgId} and accepted_at is null and expires_at > ${now} order by expires_at`,
    acceptInvitation: async (h, user, now) => db.begin(async (tx) => {
      const [inv] = await tx<Invitation[]>`update invitations set accepted_at = ${now} where token_hash = ${h} and email = ${user.email} and accepted_at is null and expires_at > ${now} returning id, org_id, email, role, expires_at, accepted_at`;
      if (!inv) return null;
      await tx`insert into memberships (org_id, user_id, role) values (${inv.org_id}, ${user.id}, ${inv.role}) on conflict do nothing`;
      return inv;
    }),
    recordStripeEvent: async (id, type) => (await db`insert into stripe_events (id, type) values (${id}, ${type}) on conflict (id) do nothing`).count > 0,
    tenant: (org) => ({
      listProjects: () => db<Project[]>`select org_id, id, name, created_at from projects where org_id = ${org} order by created_at`,
      getProject: async (id) => (await db<Project[]>`select org_id, id, name, created_at from projects where org_id = ${org} and id = ${id}`)[0] ?? null,
      createProject: async (name) => (await db<Project[]>`insert into projects (org_id, id, name) values (${org}, ${crypto.randomUUID()}, ${name}) returning org_id, id, name, created_at`)[0],
      deleteProject: async (id) => (await db`delete from projects where org_id = ${org} and id = ${id}`).count > 0,
      countProjects: async () => (await db<{ n: number }[]>`select count(*)::int as n from projects where org_id = ${org}`)[0].n,
      countMembers: async () => (await db<{ n: number }[]>`select count(*)::int as n from memberships where org_id = ${org}`)[0].n,
    }),
  };
}

export function memoryStore(): Store {
  const users: User[] = [], orgs: Org[] = [], members: { org_id: string; user_id: string; role: Role }[] = [];
  const invitations: (Invitation & { token_hash: string })[] = [], projects: Project[] = [];
  const events = new Set<string>();
  return {
    ensureUser: async (email) => { let u = users.find((u) => u.email === email); if (!u) { u = { id: crypto.randomUUID(), email }; users.push(u); } return { ...u }; },
    createOrg: async (slug, name, ownerId) => { const o: Org = { id: crypto.randomUUID(), slug, name, plan: "free", stripe_customer_id: null, subscription_status: null }; orgs.push(o); members.push({ org_id: o.id, user_id: ownerId, role: "owner" }); return { ...o }; },
    orgBySlug: async (slug) => orgs.find((o) => o.slug === slug) ?? null,
    orgByCustomer: async (cid) => orgs.find((o) => o.stripe_customer_id === cid) ?? null,
    orgsFor: async (uid) => members.filter((m) => m.user_id === uid).map((m) => ({ ...orgs.find((o) => o.id === m.org_id)!, role: m.role })),
    membership: async (orgId, uid) => members.find((m) => m.org_id === orgId && m.user_id === uid)?.role ?? null,
    members: async (orgId) => members.filter((m) => m.org_id === orgId).map((m) => ({ user_id: m.user_id, email: users.find((u) => u.id === m.user_id)!.email, role: m.role })),
    addMember: async (org_id, user_id, role) => { if (!members.some((m) => m.org_id === org_id && m.user_id === user_id)) members.push({ org_id, user_id, role }); },
    setBilling: async (orgId, b) => { const o = orgs.find((o) => o.id === orgId)!; o.plan = b.plan; o.subscription_status = b.subscription_status; o.stripe_customer_id = b.stripe_customer_id ?? o.stripe_customer_id; },
    putInvitation: async (i) => { invitations.push(i); },
    listInvitations: async (orgId, now) => invitations.filter((i) => i.org_id === orgId && !i.accepted_at && new Date(i.expires_at) > now).map(({ token_hash: _, ...i }) => i),
    acceptInvitation: async (h, user, now) => {
      const i = invitations.find((i) => i.token_hash === h);
      if (!i || i.email !== user.email || i.accepted_at || new Date(i.expires_at) <= now) return null;
      i.accepted_at = now.toISOString();
      if (!members.some((m) => m.org_id === i.org_id && m.user_id === user.id)) members.push({ org_id: i.org_id, user_id: user.id, role: i.role });
      const { token_hash: _, ...pub } = i; return pub;
    },
    recordStripeEvent: async (id) => { if (events.has(id)) return false; events.add(id); return true; },
    tenant: (org) => ({
      listProjects: async () => projects.filter((p) => p.org_id === org),
      getProject: async (id) => projects.find((p) => p.org_id === org && p.id === id) ?? null,
      createProject: async (name) => { const p = { org_id: org, id: crypto.randomUUID(), name, created_at: new Date().toISOString() }; projects.push(p); return p; },
      deleteProject: async (id) => { const i = projects.findIndex((p) => p.org_id === org && p.id === id); if (i < 0) return false; projects.splice(i, 1); return true; },
      countProjects: async () => projects.filter((p) => p.org_id === org).length,
      countMembers: async () => members.filter((m) => m.org_id === org).length,
    }),
  };
}
"##;

const TENANT_TEST: &str = r##"import { describe, expect, test } from "bun:test";
import { createHmac } from "node:crypto";
import { createApp, INVITE_TTL_MS } from "./app";
import { memoryStore } from "./store";

// Tenancy tested as HTTP against the memory store, with email and the clock injected.
process.env.STRIPE_WEBHOOK_SECRET = "whsec_test";
function harness() {
  const sent: { to: string; link: string }[] = [];
  let t = Date.now();
  const store = memoryStore();
  const app = createApp(store, async (to, link) => { sent.push({ to, link }); }, () => new Date(t));
  const as = async (email: string) => {
    const res = await app.request("/api/session", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ email }) });
    const cookie = res.headers.get("set-cookie")!.split(";")[0]!;
    const call = (path: string, method = "GET", body?: unknown) =>
      app.request(path, { method, headers: { "content-type": "application/json", cookie }, body: body === undefined ? undefined : JSON.stringify(body) });
    return { call, org: async (slug: string) => { const r = await call("/api/orgs", "POST", { slug, name: slug }); expect(r.status).toBe(201); return r.json(); } };
  };
  const stripe = (event: object) => {
    const raw = JSON.stringify(event), ts = Math.floor(t / 1000);
    const v1 = createHmac("sha256", "whsec_test").update(`${ts}.${raw}`).digest("hex");
    return app.request("/api/webhooks/stripe", { method: "POST", headers: { "stripe-signature": `t=${ts},v1=${v1}` }, body: raw });
  };
  return { app, store, as, sent, stripe, advance: (ms: number) => { t += ms; } };
}

describe("tenant", () => {
  test("a cross-tenant read fails, in the store and over HTTP", async () => {
    const h = harness();
    const alice = await h.as("alice@a.com"), bob = await h.as("bob@b.com");
    const a = await alice.org("acme"), b = await bob.org("bobco");
    const p = await (await alice.call("/api/orgs/acme/projects", "POST", { name: "secret" })).json();
    // The store cannot answer without an org, and the wrong org gets nothing.
    expect(await h.store.tenant(b.id).getProject(p.id)).toBeNull();
    expect(await h.store.tenant(a.id).getProject(p.id)).toMatchObject({ name: "secret" });
    // Bob is not a member of acme: the org does not exist for him, project or not.
    expect((await bob.call(`/api/orgs/acme/projects/${p.id}`)).status).toBe(404);
    expect((await bob.call("/api/orgs/acme")).status).toBe(404);
    expect((await bob.call(`/api/orgs/bobco/projects/${p.id}`)).status).toBe(404);
    expect((await h.app.request("/api/orgs/acme")).status).toBe(401);
  });

  test("plan limits are enforced before the write, and a Stripe subscription lifts them", async () => {
    const h = harness();
    const alice = await h.as("alice@a.com");
    await alice.org("acme");
    for (const n of ["a", "b", "c"]) expect((await alice.call("/api/orgs/acme/projects", "POST", { name: n })).status).toBe(201);
    const over = await alice.call("/api/orgs/acme/projects", "POST", { name: "d" });
    expect(over.status).toBe(402);
    expect((await over.json()).error.code).toBe("plan_limit");
    const ev = { id: "evt_1", type: "customer.subscription.created", data: { object: { customer: "cus_1", status: "active", metadata: { org_id: "acme" }, items: { data: [{ price: { lookup_key: "pro" } }] } } } };
    expect((await h.stripe(ev)).status).toBe(200);
    expect((await (await alice.call("/api/orgs/acme")).json()).org.plan).toBe("pro");
    expect((await alice.call("/api/orgs/acme/projects", "POST", { name: "d" })).status).toBe(201);
    // Cancelled: found by customer id this time, back to free.
    await h.stripe({ id: "evt_2", type: "customer.subscription.deleted", data: { object: { customer: "cus_1", status: "canceled" } } });
    expect((await (await alice.call("/api/orgs/acme")).json()).org.plan).toBe("free");
  });

  test("the Stripe webhook refuses bad signatures and stale timestamps, and replays are no-ops", async () => {
    const h = harness();
    const raw = JSON.stringify({ id: "evt_x", type: "ping", data: { object: {} } });
    expect((await h.app.request("/api/webhooks/stripe", { method: "POST", headers: { "stripe-signature": "t=1,v1=nope" }, body: raw })).status).toBe(400);
    expect((await h.app.request("/api/webhooks/stripe", { method: "POST", body: raw })).status).toBe(400);
    const ts = Math.floor(Date.now() / 1000) - 600;
    const v1 = createHmac("sha256", "whsec_test").update(`${ts}.${raw}`).digest("hex");
    expect((await h.app.request("/api/webhooks/stripe", { method: "POST", headers: { "stripe-signature": `t=${ts},v1=${v1}` }, body: raw })).status).toBe(400);
    const ev = { id: "evt_dup", type: "ping", data: { object: {} } };
    expect(await (await h.stripe(ev)).json()).toEqual({ received: true });
    expect(await (await h.stripe(ev)).json()).toEqual({ received: true, duplicate: true });
  });

  test("invitations: admins only, matching address only, and they expire", async () => {
    const h = harness();
    const alice = await h.as("alice@a.com"), bob = await h.as("bob@b.com"), eve = await h.as("eve@e.com");
    await alice.org("acme");
    const inv = await alice.call("/api/orgs/acme/invitations", "POST", { email: "bob@b.com", role: "member" });
    expect(inv.status).toBe(201);
    const token = new URL(h.sent[0]!.link).searchParams.get("invite")!;
    expect((await eve.call("/api/invitations/accept", "POST", { token })).status).toBe(401);
    expect((await bob.call("/api/invitations/accept", "POST", { token })).status).toBe(200);
    expect((await bob.call("/api/orgs/acme")).status).toBe(200);
    expect((await bob.call("/api/invitations/accept", "POST", { token })).status).toBe(401);
    // A member cannot invite; and the free plan holds two seats, so the owner's next invite is
    // refused before any email goes out.
    expect((await bob.call("/api/orgs/acme/invitations", "POST", { email: "carol@c.com" })).status).toBe(403);
    expect((await alice.call("/api/orgs/acme/invitations", "POST", { email: "carol@c.com" })).status).toBe(402);
    expect(h.sent.length).toBe(1);
  });

  test("an expired invitation is refused", async () => {
    const h = harness();
    const alice = await h.as("alice@a.com"), bob = await h.as("bob@b.com");
    await alice.org("acme");
    await alice.call("/api/orgs/acme/invitations", "POST", { email: "bob@b.com" });
    h.advance(INVITE_TTL_MS + 1);
    expect((await bob.call("/api/invitations/accept", "POST", { token: new URL(h.sent[0]!.link).searchParams.get("invite")! })).status).toBe(401);
    expect((await (await alice.call("/api/orgs/acme/invitations")).json()).invitations).toEqual([]);
  });
});
"##;

const TENANT_SQL: &str = r##"-- Every tenant-owned table carries org_id and is only ever read through the scoped layer in
-- store.ts. The composite primary key on projects makes the id meaningless without the org.
create table if not exists users (
  id uuid primary key,
  email text not null unique,
  created_at timestamptz not null default now()
);
create table if not exists orgs (
  id uuid primary key,
  slug text not null unique,
  name text not null,
  plan text not null default 'free' check (plan in ('free', 'pro', 'enterprise')),
  stripe_customer_id text unique,
  subscription_status text,
  created_at timestamptz not null default now()
);
create table if not exists memberships (
  org_id uuid not null references orgs (id) on delete cascade,
  user_id uuid not null references users (id) on delete cascade,
  role text not null check (role in ('owner', 'admin', 'member')),
  created_at timestamptz not null default now(),
  primary key (org_id, user_id)
);
create table if not exists invitations (
  id uuid primary key,
  org_id uuid not null references orgs (id) on delete cascade,
  email text not null,
  role text not null check (role in ('admin', 'member')),
  token_hash text not null unique,
  expires_at timestamptz not null,
  accepted_at timestamptz
);
create table if not exists projects (
  org_id uuid not null references orgs (id) on delete cascade,
  id uuid not null,
  name text not null,
  created_at timestamptz not null default now(),
  primary key (org_id, id)
);
-- Stripe retries until it sees a 2xx; the event id is the idempotency key.
create table if not exists stripe_events (
  id text primary key,
  type text not null,
  received_at timestamptz not null default now()
);
"##;

const TENANT_PAGE: &str = r##""use client";
import { useEffect, useState } from "react";

type Org = { id: string; slug: string; name: string; plan: string; role: string };
type Detail = { org: Org; role: string; limits: { projects: number; members: number }; usage: { projects: number; members: number } };
type Project = { id: string; name: string; created_at: string };
type Member = { user_id: string; email: string; role: string };

// Org settings: switch org, see plan usage against limits, members and pending invitations,
// projects (the tenant data). An `?invite=` in the URL is accepted for the signed-in address.
export default function Home() {
  const [me, setMe] = useState<{ user: { email: string }; orgs: Org[] } | null>(null);
  const [email, setEmail] = useState("");
  const [slug, setSlug] = useState<string | null>(null);
  const [d, setD] = useState<Detail | null>(null);
  const [projects, setProjects] = useState<Project[]>([]);
  const [members, setMembers] = useState<Member[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [newSlug, setNewSlug] = useState("");
  const [name, setName] = useState("");
  const [invite, setInvite] = useState("");

  const j = (path: string, method = "GET", body?: unknown) => fetch(path, { method, headers: { "content-type": "application/json" }, body: body === undefined ? undefined : JSON.stringify(body) });
  const refresh = async () => { const r = await j("/api/me"); setMe(r.ok ? await r.json() : null); };
  const open = async (s: string) => {
    setSlug(s); setError(null);
    setD(await (await j(`/api/orgs/${s}`)).json());
    setProjects((await (await j(`/api/orgs/${s}/projects`)).json()).projects);
    setMembers((await (await j(`/api/orgs/${s}/members`)).json()).members);
  };
  useEffect(() => { void refresh(); }, []);
  useEffect(() => {
    const token = new URLSearchParams(window.location.search).get("invite");
    if (me && token) void j("/api/invitations/accept", "POST", { token }).then(() => { window.history.replaceState(null, "", "/"); void refresh(); });
  }, [me]);

  const act = async (res: Promise<Response>) => { const r = await res; if (!r.ok) setError((await r.json()).error?.message ?? r.statusText); else setError(null); if (slug) void open(slug); };

  return (
    <main>
      <h1>{{NAME}} — organisations</h1>
      <p>Orgs, memberships, a tenant-scoped data layer, expiring invitations, plan limits, Stripe subscriptions.</p>
      {!me ? (
        <p><input type="email" placeholder="you@example.com" value={email} onChange={(e) => setEmail(e.target.value)} /> <button onClick={() => j("/api/session", "POST", { email }).then(refresh)}>Sign in</button></p>
      ) : (
        <>
          <p><code>{me.user.email}</code> <button onClick={() => j("/api/session", "DELETE").then(() => { setMe(null); setSlug(null); })}>Sign out</button></p>
          <p>{me.orgs.map((o) => <button key={o.id} onClick={() => open(o.slug)} disabled={o.slug === slug}>{o.name} ({o.role})</button>)}
            {" "}<input placeholder="new-org-slug" value={newSlug} onChange={(e) => setNewSlug(e.target.value)} /> <button onClick={() => act(j("/api/orgs", "POST", { slug: newSlug, name: newSlug })).then(refresh)}>Create org</button></p>
          {error && <p style={{ color: "crimson" }}>{error}</p>}
          {d && slug && (
            <section>
              <h2>{d.org.name} · plan <strong>{d.org.plan}</strong></h2>
              <p>Projects {d.usage.projects}/{d.limits.projects} · members {d.usage.members}/{d.limits.members}. Plan changes arrive from Stripe:</p>
              <pre>{`stripe listen --forward-to localhost:8000/api/webhooks/stripe   # STRIPE_WEBHOOK_SECRET in backend/.env
stripe trigger customer.subscription.created --override 'subscription:metadata.org_id=${slug}'`}</pre>
              <h3>Projects</h3>
              <p><input placeholder="project name" value={name} onChange={(e) => setName(e.target.value)} /> <button onClick={() => act(j(`/api/orgs/${slug}/projects`, "POST", { name }))}>Add</button></p>
              <ul>{projects.map((p) => <li key={p.id}>{p.name} <button onClick={() => act(j(`/api/orgs/${slug}/projects/${p.id}`, "DELETE"))}>delete</button></li>)}</ul>
              <h3>Members</h3>
              <table><thead><tr><th>email</th><th>role</th></tr></thead><tbody>{members.map((m) => <tr key={m.user_id}><td>{m.email}</td><td>{m.role}</td></tr>)}</tbody></table>
              {d.role !== "member" && <p><input placeholder="invite@example.com" value={invite} onChange={(e) => setInvite(e.target.value)} /> <button onClick={() => act(j(`/api/orgs/${slug}/invitations`, "POST", { email: invite }))}>Invite</button> — the link is printed by the backend in dev.</p>}
            </section>
          )}
        </>
      )}
    </main>
  );
}
"##;

const TENANT_CLAUDE: &str = r##"# {{NAME}} — multi-tenant SaaS

This service is the organisation layer of a SaaS, modelled on Cal.com's teams and organisations:
an org with a slug, memberships with `owner`/`admin`/`member` roles, invitations by email that
expire, plan limits checked before the write, and Stripe's `customer.subscription.*` webhooks as
the only thing that changes a plan. "Done" here means: no query on a tenant-owned table exists
outside `store.tenant(orgId)`, a non-member still sees 404 for an org that exists, the plan limit
is enforced before the side effect, and `make check` is green with a test over HTTP.

## Architecture

| File | Owns |
|---|---|
| `backend/src/app.ts` | Routes; `requireUser` (signed email cookie), `requireOrg` (the tenant boundary), `requireRole`, `withinPlan`; `PLANS`; `verifyStripe`; the Stripe webhook; `AppType` |
| `backend/src/store.ts` | `Store` (global: users, orgs, memberships, invitations, billing, Stripe event ids) and `Tenant` (org-scoped: projects and counts), as `pgStore` and `memoryStore` |
| `backend/src/app.test.ts` | Tenancy over HTTP against the memory store, with email, clock and a Stripe signer |
| `backend/migrations/0002_tenant.sql` | `users`, `orgs`, `memberships`, `invitations`, `projects`, `stripe_events` |
| `backend/src/server.ts`, `db.ts`, `migrate.ts` | Node server and SIGTERM drain, the pool, SQL migrations (from the stack) |
| `frontend/app/page.tsx` | Org switcher, usage against limits, projects, members, invite; accepts `?invite=` for the signed-in address |

The request path for `POST /api/orgs/:slug/projects`:

1. `.use("/api/orgs/:slug/*", requireUser, requireOrg)` runs first. `requireUser` verifies the HMAC on the `session` cookie and `ensureUser`s the email (a row per address, upserted).
2. `requireOrg` loads the org by slug and the caller's membership. **Either missing is a 404**, not a 403: whether an org exists is tenant data too.
3. `c.set("org")` and `c.set("role")`; nothing below this line receives an org id from the request.
4. `withinPlan("projects")` counts through `store.tenant(org.id)` and compares with `PLANS[org.plan].projects`; at or over the limit it answers 402 with `code: "plan_limit"` and `upgrade: "/billing"`, before any write.
5. The handler validates the body with zod and calls `store.tenant(org.id).createProject(name)`. The `Tenant` object has no method that takes an org id — it was bound at construction.
6. Postgres inserts with `org_id` as part of the composite primary key `(org_id, id)`; a project id means nothing without its org.
7. The response is the row; `AppType` carries its type to the frontend.

The billing path is separate: Stripe POSTs to `/api/webhooks/stripe`; the raw body is verified against `Stripe-Signature` (`t=…,v1=…`, 300 s tolerance, constant-time compare); the event id is inserted into `stripe_events` (a duplicate answers `{received, duplicate}` and does nothing); for `customer.subscription.*` the org is found by `customer` or, failing that, by `metadata.org_id` (an org **slug**); the plan becomes `pro`/`enterprise` from the first price's `lookup_key` (or `metadata.plan`) if the status is `active`/`trialing` and the event is not `…deleted`, otherwise `free`.

Data model:

| Table | Why the columns that matter |
|---|---|
| `users` | `email` unique; identity is nothing more than an address here (see Ceilings) |
| `orgs` | `slug` unique and URL-safe (`^[a-z0-9-]{2,40}$`); `plan` check-constrained; `stripe_customer_id` unique so a customer maps to one org; `subscription_status` is Stripe's word, stored for display |
| `memberships` | PK `(org_id, user_id)`; `role` check-constrained; cascade from both sides. Created for the owner in the same transaction as the org |
| `invitations` | `token_hash` unique (the plaintext is only in the email); `email` is matched against the acceptor; `expires_at` (7 days); `accepted_at` makes acceptance single-use; `role` cannot be `owner` |
| `projects` | The tenant data. PK `(org_id, id)`; every query in `Tenant` carries `org_id` |
| `stripe_events` | `id` PK: Stripe retries until 2xx, so the event id is the idempotency key |

## Invariants

1. **Every tenant query goes through `store.tenant(orgId)`.** `Tenant` has no method accepting an org id; `pgStore.tenant` adds `where org_id = ${org}` to every statement. Why: cross-tenant reads are the one bug a SaaS cannot ship. Guarded by "a cross-tenant read fails, in the store and over HTTP" — `store.tenant(b.id).getProject(p.id)` is null for a project of `a`.
2. **A non-member sees 404, never 403 or the org name.** `requireOrg` answers `not_found` when the org is missing *or* the membership is. Why: "this org exists but you are not in it" is information. Same test: Bob gets 404 for `/api/orgs/acme` and for acme's project id under his own slug.
3. **The org id never comes from the request.** Handlers read `c.get("org").id`; the slug is resolved once in `requireOrg`. Why: a handler that trusts a body or query field for the tenant is invariant 1 with extra steps. Reviewer check, no dedicated test — add one if a route starts taking an id.
4. **Plan limits are checked in middleware before the write, and before the email.** `withinPlan` runs before the handler for `POST projects` and `POST invitations`. Why: an over-limit invite that still sends a link is a limit that does not exist. Guarded by "plan limits are enforced before the write…" (4th project is 402) and "invitations: admins only…" (`h.sent.length` stays 1 after the 402).
5. **Stripe is the only writer of `plan`.** No route sets a plan; `setBilling` is called only from the webhook. Why: a plan is a fact about money, and the source of that fact is the processor. Guarded by "plan limits…": `pro` arrives by event, `free` returns on `…deleted`.
6. **The webhook verifies before it parses.** `verifyStripe(raw, header, secret, now)` runs on the raw text; a missing secret or header, a bad `v1`, or a timestamp more than 300 s off is a 400. Why: an unverified webhook is an unauthenticated plan-upgrade endpoint. Guarded by "the Stripe webhook refuses bad signatures and stale timestamps, and replays are no-ops".
7. **Webhook replays are no-ops.** `recordStripeEvent` inserts `on conflict do nothing` and the handler returns early on a duplicate. Same test (`duplicate: true`).
8. **An invitation is accepted by its address only, once, within 7 days.** `acceptInvitation` matches `email = ${user.email}`, `accepted_at is null`, `expires_at > now`, and inserts the membership in the same transaction. Why: a forwarded invite link must not admit whoever holds it. Guarded by "invitations: admins only, matching address only, and they expire" (Eve is 401, Bob's second accept is 401) and "an expired invitation is refused".
9. **Only admins and owners invite, and nobody is invited as owner.** `requireRole("admin")` on the invitation routes; the body's `role` enum is `admin | member`. Why: ownership is created with the org and never granted by email. Guarded by "invitations: …" (a member inviting is 403).
10. **The owner membership is created with the org, atomically.** `createOrg` inserts the org and the owner row in one `db.begin`. Why: an org with no owner cannot be administered or billed. Asserted indirectly by every test that creates an org and then acts as its owner; the transaction itself is Postgres-only.
11. **Slugs are unique and validated at the boundary.** `^[a-z0-9-]{2,40}$`, 409 on conflict. Why: the slug is in every URL and in Stripe metadata; it must be safe and stable. No test for the regex — add one before loosening it.
12. **Nothing is read from the session cookie except an email.** `verify` returns the email only if the HMAC matches in constant time; the user row is loaded fresh on every request. Why: roles and org lists must not be cached in a cookie a person can keep after being removed.

## Extending it

**Add a tenant-owned resource** (e.g. `documents`). Migration `0003_documents.sql` with `org_id uuid not null references orgs (id) on delete cascade` and `primary key (org_id, id)`. Add the type and the methods to `Tenant` in `store.ts` — every SQL statement `where org_id = ${org}` — and mirror them in `memoryStore.tenant` with `.filter((d) => d.org_id === org)`. Add routes under `/api/orgs/:slug/documents` in the chain **after** the `.use("/api/orgs/:slug/*", …)` line. If it counts against a plan, add a key to `PLANS` and a `countDocuments` used by `withinPlan`. Test: extend "a cross-tenant read fails" with the new resource — the store call with the wrong org returns null and the HTTP call from a non-member is 404.

**Add a plan or change a limit.** `PLANS` in `app.ts` and the check constraint in a new migration (`alter table orgs drop constraint orgs_plan_check, add constraint … check (plan in (…))`). Create the Stripe price with `lookup_key` equal to the plan name — the webhook maps on that. Test: a subscription event with the new `lookup_key` and the limit that follows.

**Add an org-level role action** (remove a member, change a role). `Store.removeMember(orgId, userId)` and `setMembership(orgId, userId, role)`; routes under `/api/orgs/:slug/members/:userId` with `requireRole("admin")`; refuse touching the last owner. Test: an admin removes a member, the member's next `/api/orgs/:slug` is 404; the owner cannot be removed.

**Replace the identity cookie with real sign-in.** Generate the `auth` pack beside this one and lift its `requireUser` (session table, magic links) into `app.ts`, keeping `c.set("user", …)` as the contract; drop `sign`/`verify` and `POST /api/session`. `User` gains `role` only if you want deployment-wide admins; org roles stay in `memberships`. Tests: replace `h.as(email)` with the auth pack's `signIn`.

**Start a Stripe subscription from the app** (Checkout). Add `stripe` as a dependency, a `POST /api/orgs/:slug/billing/checkout` under `requireRole("owner")` that creates a Checkout Session with `customer` or `metadata.org_id = slug` and `line_items[0].price` looked up by `lookup_key`, and return the URL. Do not set the plan there — the webhook will. Test with `fetch` injected: assert the request shape.

**Revoke an invitation.** `Store.deleteInvitation(orgId, id)` (scoped by org), `DELETE /api/orgs/:slug/invitations/:id` under `requireRole("admin")`. Test: after deletion the token is 401 on accept.

**Handle another Stripe event type.** Add a branch in the webhook after the `customer.subscription.` one; record whatever it changes with `store.setBilling` or a new `Store` method; never trust `metadata` for money. Extend the zod event shape only with fields you read. Test with `h.stripe(event)`.

## Operating it

| Env var | Required | Meaning |
|---|---|---|
| `DATABASE_URL` | yes | Postgres |
| `PORT` | no | API port, default 8000 |
| `SESSION_SECRET` | **yes in production** | HMAC key for the identity cookie. Defaults to `dev-secret-change-me` and **nothing refuses to start without it** — set it, and rotate it to sign everyone out |
| `STRIPE_WEBHOOK_SECRET` | yes for billing | The endpoint's `whsec_…`; empty means every webhook is 400 and plans never change |
| `APP_URL` | in production | Origin used in invitation links (`${APP_URL}/?invite=…`) |
| `NODE_ENV` | in production | `production` makes the cookie `Secure` and makes `consoleEmail` throw |

**Per replica today:** nothing. Every read and write is Postgres; replicas share plans, memberships and the Stripe idempotency table. The plan check is read-then-write, so two concurrent creates can exceed a limit by one (see Ceilings).

**Scaling knobs:** pool `max: 10` in `db.ts` per replica. `countProjects`/`countMembers` are `count(*)` per write on the org's rows — fine until an org has millions of projects, at which point keep a counter on `orgs`.

**Failure modes and what the person sees:**

| Failure | Seen as |
|---|---|
| `STRIPE_WEBHOOK_SECRET` unset or wrong | Stripe's dashboard shows 400s and retries; orgs stay `free` after paying. Alert on webhook 4xx |
| Checkout created without `customer` and without `metadata.org_id` = slug | The event verifies, is recorded, and matches no org; the plan never changes and the event cannot be replayed (its id is stored). Fix the data and use Stripe's "resend" with a new event, or delete the row from `stripe_events` |
| Email sender down | `POST invitations` returns 500 after the invitation row exists; the list shows it pending with no email sent. Re-invite creates a new token |
| `SESSION_SECRET` changed | Every cookie fails `verify` → 401 everywhere; everyone signs in again |
| Database down | 500 on everything except `/api/health`; `/api/health/ready` still says `db: ok` — wire it before trusting it |

**What to watch:** 402 rate by org (people hitting limits are upgrade leads and support tickets), webhook 400s (misconfiguration), `stripe_events` rows whose type is `customer.subscription.*` but matched no org (log it — today it is silent), invitations expiring unaccepted, and `memberships` count per org against `PLANS`.

## Ceilings

- `ponytail: identity is a signed email cookie` — anyone can claim any address. It exists so the tenant logic is testable end to end; it is not sign-in. Replace with the `auth` pack's session before any real user.
- `ponytail: plan check is read-then-write` — `withinPlan` counts, then the handler inserts. Two concurrent requests can both pass at limit−1. A `select … for update` on the org row inside the create transaction when a limit must be exact.
- Pending invitations do not count as seats. `withinPlan("members")` counts memberships only, so an admin can send more invites than seats remain and acceptance does not re-check. Count `invitations where accepted_at is null and expires_at > now` in `countMembers` when seats are billed.
- No Stripe API calls: no Checkout, no customer portal, no seat sync. Plans change only by webhook, which means the first subscription must be created in Stripe with `metadata.org_id` set to the slug.
- `PLANS` is code. A price change is a deploy; move limits to a table when marketing owns them.
- `metadata.org_id` is a slug, not an id; renaming an org's slug breaks matching for events without `customer`. Prefer setting `stripe_customer_id` early.
- Project lists are unbounded; add keyset pagination on `(created_at, id)` before an org can have thousands.
- `/api/health/ready` does not query the database.
- No org deletion, owner transfer, member removal, role change or invitation revocation routes yet (recipes above).

The stack rules — gate, typed seam, production checklist, deploy — are in `docs/PRODUCTION.md`. They apply.
"##;

const TENANT_AGENTS: &str = r##"# {{NAME}} — for agents

`CLAUDE.md` has the rules. This is how to run it and prove it.

## Run

    make demo        # postgres + redis, migrate, seed the stack's sample table, start both halves
    make check       # frontend typecheck, backend typecheck + tests
    make backend     # API alone on :8000
    make frontend    # Next.js on :3000, /api proxied

No worker, no importer. Invitation links are printed by the backend in dev
(`"msg":"invitation (dev only)"`). Stripe locally:

    stripe listen --forward-to localhost:8000/api/webhooks/stripe     # copy whsec_… into backend/.env as STRIPE_WEBHOOK_SECRET
    stripe trigger customer.subscription.created --override 'subscription:metadata.org_id=acme'

## Every route, with a body that works

Identity (dev-grade: the cookie is a signed email):

    curl -s -c jar localhost:3000/api/session -H 'content-type: application/json' -d '{"email":"alice@a.com"}'
    # {"email":"alice@a.com"}
    curl -s -b jar localhost:3000/api/me
    # {"user":{"id":"…","email":"alice@a.com"},"orgs":[]}

Orgs:

    curl -s -b jar localhost:3000/api/orgs -H 'content-type: application/json' -d '{"slug":"acme","name":"Acme"}'
    # 201 {"id":"…","slug":"acme","name":"Acme","plan":"free","stripe_customer_id":null,"subscription_status":null}
    curl -s -b jar localhost:3000/api/orgs/acme
    # {"org":{…},"role":"owner","limits":{"projects":3,"members":2},"usage":{"projects":0,"members":1}}
    curl -s -b jar localhost:3000/api/orgs/acme/members
    # {"members":[{"user_id":"…","email":"alice@a.com","role":"owner"}]}

Projects (the tenant data):

    curl -s -b jar localhost:3000/api/orgs/acme/projects -H 'content-type: application/json' -d '{"name":"Website"}'
    # 201 {"org_id":"…","id":"…","name":"Website","created_at":"…"}
    curl -s -b jar localhost:3000/api/orgs/acme/projects
    # {"projects":[…]}
    curl -s -b jar localhost:3000/api/orgs/acme/projects/<id>
    # the project, or 404
    curl -si -b jar -X DELETE localhost:3000/api/orgs/acme/projects/<id>     # admin+
    # HTTP/1.1 204
    # the 4th project on free:
    # 402 {"error":{"message":"free plan allows 3 projects","code":"plan_limit","upgrade":"/billing"}}

Invitations:

    curl -s -b jar localhost:3000/api/orgs/acme/invitations -H 'content-type: application/json' -d '{"email":"bob@b.com","role":"member"}'
    # 201 {"id":"…","org_id":"…","email":"bob@b.com","role":"member","expires_at":"…","accepted_at":null}
    #   backend log: "link":"http://localhost:3000/?invite=<token>"
    curl -s -b jar localhost:3000/api/orgs/acme/invitations
    # {"invitations":[…pending, unexpired…]}
    # as bob (own cookie):
    curl -s -b bob localhost:3000/api/invitations/accept -H 'content-type: application/json' -d '{"token":"<token>"}'
    # 200 the invitation with accepted_at set; 401 {"code":"invalid_token"} if wrong address, used, or expired

Stripe webhook (signed; see the test harness for how `v1` is computed):

    # {"id":"evt_1","type":"customer.subscription.created","data":{"object":{"customer":"cus_1","status":"active","metadata":{"org_id":"acme"},"items":{"data":[{"price":{"lookup_key":"pro"}}]}}}}
    # → {"received":true}; a second delivery → {"received":true,"duplicate":true}; bad signature → 400 {"code":"invalid_signature"}

Errors: `{"error":{"message","code",…}}` with `unauthorized` 401, `not_found` 404 (also for an org you are not in), `forbidden` 403, `invalid` 400, `conflict` 409 (slug), `plan_limit` 402, `invalid_token` 401, `invalid_signature` 400.

## How the tests work

`backend/src/app.test.ts` builds `createApp(memoryStore(), captureEmail, clock)` and sets
`STRIPE_WEBHOOK_SECRET=whsec_test` for the process.

- `h.as(email)` posts to `/api/session`, keeps the cookie, and returns `call(path, method, body)`
  plus `org(slug)` which creates an org and asserts 201.
- `h.sent` holds every invitation link; the token is `new URL(link).searchParams.get("invite")`.
- `h.stripe(event)` signs the JSON with the test secret and the harness clock and POSTs it.
- `h.advance(ms)` moves the clock for expiry.
- `h.store` is exposed so a test can assert the store itself refuses a cross-tenant read, not
  only the HTTP layer.

## Add a test

Inside `describe("tenant")`: create two principals with `h.as`, an org each with `.org(slug)`, and
assert both the positive path and the cross-tenant path — every resource test should include a
call from the other org expecting 404. For billing, build the event object and send it through
`h.stripe`; for anything time-based, `h.advance`. Postgres-only properties (the composite key,
the `createOrg` transaction, `on conflict do nothing`) are not exercised by the memory store —
say so in a comment when a change relies on one.
"##;

const TENANT_README: &str = r##"# {{NAME}}

Organisations, memberships, invitations, plan limits and Stripe subscriptions — the multi-tenant
skeleton of a SaaS, as one Hono service with a scoped data layer that cannot ask for a row without
saying whose it is.

## What you get

- Orgs with URL slugs; the creator is `owner`; `admin` and `member` roles per org.
- A tenant boundary in middleware: `/api/orgs/:slug/*` resolves the membership first, and a non-member sees 404 for everything under it.
- A `Tenant` data layer bound to one org id, with `projects` as the worked example (composite key `(org_id, id)`).
- Invitations by email, 7-day expiry, accepted only by the invited address, once.
- Plan limits (`free` 3 projects / 2 members, `pro` 50 / 25, `enterprise` unlimited) enforced in middleware with a 402 before the write.
- Stripe `customer.subscription.*` webhooks, signature-verified, replay-safe, as the only writer of the plan.
- Tests over HTTP in memory with email, clock and a Stripe signer injected — including the cross-tenant read that must fail.
- Migrations, images, kustomize overlays, deploy workflows.

## Five minutes

    make demo

Then:

    curl -s -c a localhost:3000/api/session -H 'content-type: application/json' -d '{"email":"alice@a.com"}'
    curl -s -b a localhost:3000/api/orgs -H 'content-type: application/json' -d '{"slug":"acme","name":"Acme"}'
    # 201 {"slug":"acme","plan":"free",…}

    for n in a b c d; do curl -s -b a localhost:3000/api/orgs/acme/projects -H 'content-type: application/json' -d "{\"name\":\"$n\"}"; echo; done
    # three 201s, then: {"error":{"message":"free plan allows 3 projects","code":"plan_limit","upgrade":"/billing"}}

    curl -s -c b localhost:3000/api/session -H 'content-type: application/json' -d '{"email":"bob@b.com"}'
    curl -si -b b localhost:3000/api/orgs/acme | head -1
    # HTTP/1.1 404 Not Found      — not a member, so the org does not exist for Bob

    stripe trigger customer.subscription.created --override 'subscription:metadata.org_id=acme'   # with `stripe listen` running
    curl -s -b a localhost:3000/api/orgs/acme | grep -o '"plan":"[a-z]*"'
    # "plan":"pro"   (if the price has lookup_key "pro")

## API

| Method | Path | Auth | What |
|---|---|---|---|
| POST | `/api/session` | none | `{email}` → signed identity cookie (dev-grade; see Compared) |
| DELETE | `/api/session` | none | Clears it |
| GET | `/api/me` | user | `{user, orgs: [{…org, role}]}` |
| POST | `/api/orgs` | user | `{slug, name}` → 201; 409 if the slug is taken; caller becomes owner |
| GET | `/api/orgs/:slug` | member | `{org, role, limits, usage}` |
| GET | `/api/orgs/:slug/members` | member | Members with roles |
| GET | `/api/orgs/:slug/invitations` | admin | Pending, unexpired |
| POST | `/api/orgs/:slug/invitations` | admin | `{email, role?}` → 201 and an email; 402 at the seat limit |
| POST | `/api/invitations/accept` | user | `{token}` → membership; 401 if wrong address, used, expired |
| GET | `/api/orgs/:slug/projects` | member | The org's projects |
| POST | `/api/orgs/:slug/projects` | member | `{name}` → 201; 402 at the plan limit |
| GET | `/api/orgs/:slug/projects/:id` | member | One project, or 404 |
| DELETE | `/api/orgs/:slug/projects/:id` | admin | 204 |
| POST | `/api/webhooks/stripe` | Stripe signature | `customer.subscription.*` → plan; replays are no-ops |
| GET | `/api/health`, `/api/health/ready` | none | Probes |

Non-members get 404, not 403, for anything under `/api/orgs/:slug`.

## Compared with Cal.com (teams and organisations)

**Same.** The shape: an organisation with a slug, memberships with owner/admin/member, email
invitations that expire and must be accepted by the invited address, plan limits that answer with
an upgrade pointer, and Stripe subscription webhooks driving the plan. Stripe's own signature
scheme and event idempotency, so Stripe's docs and CLI apply unchanged.

**Better here.**
- The tenant boundary is a type: `Tenant` methods have no org parameter, so a cross-tenant query is not expressible in application code. Cal.com scopes with `where: { teamId }` per query, and its history has the bugs that pattern produces.
- Non-existence for non-members (404) is the default, not a per-route decision.
- Plan limits are one table (`PLANS`) enforced in one middleware, not spread across handlers.
- Tests without a database or Stripe: the webhook is signed by the harness, the clock is injected, and the cross-tenant read is asserted at the store *and* at the HTTP layer.
- ~500 lines you own, typed end to end into the frontend through `AppType`.
- Migrations, images, kustomize overlays and deploy workflows are in the repository (`docs/PRODUCTION.md`).

**Not here yet.**
- Real sign-in. Identity is a signed email cookie so the tenancy is testable; combine with the `auth` pack (magic links, sessions, tokens) before any real user.
- Stripe Checkout, the customer portal, per-seat quantity sync, invoices, trials management — this service only consumes webhooks; the first subscription is created in Stripe with `metadata.org_id` set to the slug.
- Removing members, changing a member's role, transferring ownership, deleting an org, revoking an invitation.
- Sub-teams inside an org, org-level settings, custom domains, SSO/SAML, SCIM.
- Pending invitations counting as seats.
- Pagination on projects.
- Everything Cal.com does that is not tenancy: scheduling, availability, event types, the app store.

## Production

- **Environments.** `k8s/overlays/dev` and `k8s/overlays/prod`; `git push` to `main` deploys dev, `make release` deploys prod.
- **Config.** `DATABASE_URL`, `PORT`, `SESSION_SECRET` (required — the default is a dev string and nothing refuses it), `STRIPE_WEBHOOK_SECRET`, `APP_URL`, `NODE_ENV=production`. From `backend/.env` → Secret via `k8s/scripts/env-to-secrets.sh`; committed as `backend/.env.age`.
- **Stripe.** One webhook endpoint per environment, each with its own secret; prices with `lookup_key` equal to the plan name (`pro`, `enterprise`); subscriptions created with `metadata.org_id = <slug>` or with the org's `stripe_customer_id` already stored.
- **Email.** No provider is wired; implement `SendEmail` with a timeout and pass it to `createApp` (recipe in `CLAUDE.md`).
- **Scaling.** Stateless; nothing is per replica. Pool 10 per replica. Counts are `count(*)` per write on the org's rows.
- **Probes.** `/api/health` and `/api/health/ready`; readiness does not query the database yet.
- **Migrations.** `backend/migrations/*.sql` via the init container; expand/contract; every new tenant table has `org_id` in its primary key.
- **What pages you.** Webhook 4xx in Stripe's dashboard; subscription events that matched no org (add the log line — today it is silent); 5xx on invitations (email); Postgres connections near the limit.

## Roadmap

- Replace the identity cookie with the `auth` pack.
- Atomic plan checks (`for update` on the org row) and pending invitations as seats.
- Stripe Checkout and portal routes; a seat-quantity sync.
- Member removal, role change, owner transfer, invitation revocation.
- Keyset pagination on tenant lists; a readiness probe that asks the database.
"##;

const TENANT_REVIEWER: &str = r##"---
name: tenant-isolation
description: Run on any change to app.ts, store.ts or a migration that adds a table — routes, the Tenant layer, plan limits, invitations, the Stripe webhook. Reads the change as a member of one org trying to see or change another's data, and reports only what lets them.
tools: Read, Grep, Glob, Bash
---

You review tenancy. Assume the change lets one organisation touch another's rows until the code
shows otherwise.

Report only what crosses a tenant, bypasses a limit, changes a plan without Stripe, or admits
someone an invitation was not for — each as `path:line — what — how it is exploited — the fix`.
End with `tenant-isolation: N findings` and, if 0, what you checked.

Check:
1. **Every query on a tenant-owned table carries `org_id`.** In `pgStore.tenant`, every statement has `where org_id = ${org}` (or `insert … (org_id, …) values (${org}, …)`); in `memoryStore.tenant`, every array access filters on `org`. Grep `from projects`, `into projects`, and any new table; a statement without the org predicate is a finding.
2. **`Tenant` has no method that accepts an org id.** The id is bound in `store.tenant(orgId)` and nowhere else. A method signature like `getProject(orgId, id)` is a finding — it invites the caller to pass the wrong one.
3. **Handlers get the org from the context.** Every handler under `/api/orgs/:slug` uses `c.get("org").id`; none reads an org id or slug from the body or query. Grep `org_id` in `app.ts`: it should appear only in the webhook's `metadata.org_id` branch.
4. **The mount line is intact and new routes are below it.** `.use("/api/orgs/:slug/*", requireUser, requireOrg)` precedes every `/api/orgs/:slug/…` route in the chain; the exact-path `GET /api/orgs/:slug` carries the middleware explicitly because `/*` does not match it. A route registered above the `.use` runs without `requireOrg`.
5. **404, not 403, for non-members**, and the 404 body is the same as for a missing org. A response that differs between "no such org" and "not your org" is a finding.
6. **New tables have `org_id` in the primary key** (`primary key (org_id, id)`) and `references orgs (id) on delete cascade`. An `id uuid primary key` on a tenant table is a finding: the row becomes addressable without its owner.
7. **Plan checks are middleware, before effects.** `withinPlan(kind)` is on every route that creates a limited thing, and it runs before `sendEmail` or any insert. A limit checked inside the handler after a side effect, or a new limited resource without `withinPlan`, is a finding.
8. **Only the webhook writes `plan`.** Grep `setBilling`: one call site, inside `/api/webhooks/stripe`, after `verifyStripe` and after `recordStripeEvent`. Any other writer is a finding.
9. **The webhook verifies the raw body**, not a re-serialised one: `c.req.text()` then `verifyStripe(raw, …)`, tolerance 300 s, `timingSafeEqual` with a length check. Parsing before verifying is a finding.
10. **Replay is a no-op.** `recordStripeEvent` returns false on a seen id and the handler returns before any effect.
11. **Invitations are bound to the address.** `acceptInvitation` matches `email = ${user.email}` and `accepted_at is null` and `expires_at > now`, and adds the membership in the same transaction; the token is hashed before lookup. The `role` enum for invitations excludes `owner`.
12. **Invitation routes are admin-gated**, and the invitation list is scoped by `org_id` (an admin of org A cannot list org B's pending invites).
13. **Roles come from `memberships`, per org**, read on every request in `requireOrg`; nothing caches a role in the cookie, and `requireRole` compares `c.get("role")`, not a user-level field.
14. **The identity cookie is HMAC-verified in constant time** and yields only an email; a change that adds fields to the cookie (role, org) is a finding.
"##;
