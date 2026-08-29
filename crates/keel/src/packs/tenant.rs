//! tenant: a multi-tenant SaaS, like Cal.com — orgs, memberships, a scoped data layer, invitations, plans, Stripe

pub fn files(name: &str) -> Vec<(&'static str, String)> {
    let f = |s: &str| s.replace("{{NAME}}", name);
    vec![
        ("backend/src/app.ts", TENANT_APP.into()),
        ("backend/src/store.ts", TENANT_STORE.into()),
        ("backend/src/app.test.ts", TENANT_TEST.into()),
        ("backend/migrations/0002_tenant.sql", TENANT_SQL.into()),
        ("frontend/app/page.tsx", f(TENANT_PAGE)),
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
