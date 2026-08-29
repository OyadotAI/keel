//! auth: the account layer, like better-auth / Lucia — magic links, sessions, roles, API tokens

pub fn files(name: &str) -> Vec<(&'static str, String)> {
    let f = |s: &str| s.replace("{{NAME}}", name);
    vec![
        ("backend/src/app.ts", AUTH_APP.into()),
        ("backend/src/store.ts", AUTH_STORE.into()),
        ("backend/src/app.test.ts", AUTH_TEST.into()),
        ("backend/migrations/0002_auth.sql", AUTH_SQL.into()),
        ("frontend/app/page.tsx", f(AUTH_PAGE)),
    ]
}

const AUTH_APP: &str = r##"import { Hono, type Context, type MiddlewareHandler } from "hono";
import { z } from "zod";
import { createHash, randomBytes } from "node:crypto";
import { deleteCookie, getCookie, setCookie } from "hono/cookie";
import { pgStore, type Role, type Store, type User } from "./store";

// better-auth's surface: POST /sign-in/magic-link, GET /magic-link/verify?token=&callbackURL=,
// GET /get-session, POST /sign-out, a session cookie that is HttpOnly + SameSite=Lax + Secure.
// Lucia's session model underneath: the cookie holds a random token, the table holds its
// sha256, and a session past half its life is rotated on use so a leaked id has a short life.
// No password anywhere; the email provider is a function passed in, so tests capture the link.
export type SendEmail = (to: string, link: string) => Promise<void>;
type Clock = () => Date;
type Env = { Variables: { user: User; auth: "session" | "token" } };

const sha = (s: string) => createHash("sha256").update(s).digest("hex");
const secret = (prefix: string) => prefix + randomBytes(32).toString("base64url");
const RANK: Record<Role, number> = { member: 0, admin: 1, owner: 2 };
export const SESSION_TTL_MS = 30 * 24 * 3600_000;
export const MAGIC_LINK_TTL_MS = 5 * 60_000;
export const COOKIE = "session_token";

// Dev default: print the link. Production must inject a real sender (Resend, SES, Postmark…).
export const consoleEmail: SendEmail = async (to, link) => {
  if (process.env.NODE_ENV === "production") throw new Error("no email sender configured");
  console.log(JSON.stringify({ level: "info", msg: "magic link (dev only)", to, link }));
};

export function createApp(store: Store, sendEmail: SendEmail = consoleEmail, now: Clock = () => new Date()) {
  const ip = (c: Context<Env>) => c.req.header("x-forwarded-for")?.split(",")[0]?.trim() ?? "local";
  const secure = process.env.NODE_ENV === "production";

  const issueSession = async (c: Context<Env>, user: User) => {
    const token = secret("ses_");
    const t = now();
    await store.putSession({ id_hash: sha(token), user_id: user.id, created_at: t.toISOString(), expires_at: new Date(t.getTime() + SESSION_TTL_MS).toISOString() });
    setCookie(c, COOKIE, token, { httpOnly: true, sameSite: "Lax", secure, path: "/", maxAge: SESSION_TTL_MS / 1000 });
  };

  // Session cookie or `Authorization: Bearer pat_…` — both resolve to a user, or 401.
  const requireUser: MiddlewareHandler<Env> = async (c, next) => {
    const bearer = c.req.header("authorization")?.replace(/^Bearer /, "");
    if (bearer) {
      const t = await store.apiTokenByHash(sha(bearer), now());
      if (!t) return c.json({ error: { message: "invalid token", code: "unauthorized" } }, 401);
      c.set("user", t.user); c.set("auth", "token");
      return next();
    }
    const cookie = getCookie(c, COOKIE);
    const s = cookie ? await store.sessionByHash(sha(cookie), now()) : null;
    if (!s) return c.json({ error: { message: "not signed in", code: "unauthorized" } }, 401);
    // Rotation: past half its life, the session gets a new id and the old one stops working.
    if (now().getTime() - new Date(s.created_at).getTime() > SESSION_TTL_MS / 2) {
      await store.deleteSession(s.id_hash);
      await issueSession(c, s.user);
    }
    c.set("user", s.user); c.set("auth", "session");
    return next();
  };
  const requireRole = (min: Role): MiddlewareHandler<Env> => async (c, next) => {
    if (RANK[c.get("user").role] < RANK[min]) return c.json({ error: { message: `${min} role required`, code: "forbidden" } }, 403);
    return next();
  };

  const app = new Hono<Env>()
    .get("/api/health", (c) => c.json({ status: "ok" }))
    .get("/api/health/ready", (c) => c.json({ status: "ok", db: "ok" as const }))

    // ── sign in ─────────────────────────────────────────────────────────────────────────
    .post("/api/auth/sign-in/magic-link", async (c) => {
      const p = z.object({ email: z.string().email().max(254), callbackURL: z.string().max(2000).default("/") }).safeParse(await c.req.json().catch(() => ({})));
      if (!p.success) return c.json({ error: { message: "invalid body", code: "invalid", details: p.error.flatten() } }, 400);
      const email = p.data.email.toLowerCase();
      const t = now().getTime();
      // Per address and per source: five a minute each. Enough for a fat-fingered inbox, not for
      // enumerating one.
      if ((await store.hit(`email:${email}`, t)) > 5 || (await store.hit(`ip:${ip(c)}`, t)) > 20) {
        await store.audit("sign_in.rate_limited", { email, ip: ip(c) });
        c.header("Retry-After", "60");
        return c.json({ error: { message: "too many requests", code: "rate_limited" } }, 429);
      }
      const token = secret("ml_");
      await store.putMagicLink(sha(token), email, new Date(t + MAGIC_LINK_TTL_MS));
      // Relative callbacks only: an absolute URL here would make the link an open redirect.
      const cb = p.data.callbackURL.startsWith("/") && !p.data.callbackURL.startsWith("//") ? p.data.callbackURL : "/";
      const base = process.env.APP_URL ?? "http://localhost:3000";
      await sendEmail(email, `${base}/api/auth/magic-link/verify?token=${token}&callbackURL=${encodeURIComponent(cb)}`);
      await store.audit("sign_in.requested", { email, ip: ip(c) });
      return c.json({ status: true });
    })
    .get("/api/auth/magic-link/verify", async (c) => {
      const token = c.req.query("token") ?? "";
      const email = token ? await store.redeemMagicLink(sha(token), now()) : null;
      if (!email) { await store.audit("sign_in.failed", { ip: ip(c) }); return c.json({ error: { message: "link invalid or expired", code: "invalid_token" } }, 401); }
      const user = await store.ensureUser(email);
      await issueSession(c, user);
      await store.audit("sign_in.completed", { user_id: user.id, email, ip: ip(c) });
      const cb = c.req.query("callbackURL") ?? "/";
      return c.redirect(cb.startsWith("/") && !cb.startsWith("//") ? cb : "/", 302);
    })
    .get("/api/auth/get-session", async (c) => {
      const cookie = getCookie(c, COOKIE);
      const s = cookie ? await store.sessionByHash(sha(cookie), now()) : null;
      if (!s) return c.json(null);
      if (now().getTime() - new Date(s.created_at).getTime() > SESSION_TTL_MS / 2) { await store.deleteSession(s.id_hash); await issueSession(c, s.user); }
      return c.json({ user: s.user, session: { expires_at: s.expires_at } });
    })
    .post("/api/auth/sign-out", async (c) => {
      const cookie = getCookie(c, COOKIE);
      if (cookie) { const s = await store.sessionByHash(sha(cookie), now()); await store.deleteSession(sha(cookie)); if (s) await store.audit("sign_out", { user_id: s.user_id, ip: ip(c) }); }
      deleteCookie(c, COOKIE, { path: "/" });
      return c.json({ status: true });
    })

    // ── the signed-in surface ───────────────────────────────────────────────────────────
    .use("/api/me/*", requireUser)
    .get("/api/me", requireUser, (c) => c.json({ user: c.get("user"), auth: c.get("auth") }))
    .get("/api/me/tokens", async (c) => c.json({ tokens: await store.listApiTokens(c.get("user").id) }))
    .post("/api/me/tokens", async (c) => {
      // API tokens are minted from a browser session only — a token must not mint tokens.
      if (c.get("auth") !== "session") return c.json({ error: { message: "sign in to create tokens", code: "forbidden" } }, 403);
      const p = z.object({ name: z.string().min(1).max(100) }).safeParse(await c.req.json().catch(() => ({})));
      if (!p.success) return c.json({ error: { message: "invalid body", code: "invalid", details: p.error.flatten() } }, 400);
      const raw = secret("pat_");
      const t = { id: crypto.randomUUID(), user_id: c.get("user").id, name: p.data.name, created_at: now().toISOString(), last_used_at: null };
      await store.putApiToken({ ...t, token_hash: sha(raw) });
      await store.audit("token.created", { user_id: t.user_id, ip: ip(c) });
      // The plaintext exists exactly once, in this response.
      return c.json({ ...t, token: raw }, 201);
    })
    .delete("/api/me/tokens/:id", async (c) => {
      const ok = await store.deleteApiToken(c.get("user").id, c.req.param("id"));
      if (ok) await store.audit("token.revoked", { user_id: c.get("user").id, ip: ip(c) });
      return ok ? c.body(null, 204) : c.json({ error: { message: "not found", code: "not_found" } }, 404);
    })

    // ── admin ───────────────────────────────────────────────────────────────────────────
    .use("/api/admin/*", requireUser, requireRole("admin"))
    .get("/api/admin/users", async (c) => c.json({ users: await store.listUsers() }))
    .get("/api/admin/audit", async (c) => c.json({ events: await store.listAudit(Math.min(200, Number(c.req.query("limit") ?? 50))) }))
    .patch("/api/admin/users/:id/role", requireRole("owner"), async (c) => {
      const p = z.object({ role: z.enum(["owner", "admin", "member"]) }).safeParse(await c.req.json().catch(() => ({})));
      if (!p.success) return c.json({ error: { message: "invalid body", code: "invalid" } }, 400);
      const target = await store.userById(c.req.param("id"));
      if (!target) return c.json({ error: { message: "not found", code: "not_found" } }, 404);
      if (target.id === c.get("user").id && p.data.role !== "owner") return c.json({ error: { message: "an owner cannot demote themselves", code: "forbidden" } }, 403);
      await store.setRole(target.id, p.data.role);
      await store.audit(`role.${p.data.role}`, { user_id: target.id, email: target.email, ip: ip(c) });
      return c.json({ id: target.id, role: p.data.role });
    });

  return app;
}

const app = createApp(pgStore());
export type AppType = typeof app;
export default app;
"##;

const AUTH_STORE: &str = r##"import { db } from "./db";

// Everything the account layer persists. Postgres in the process, memory in the tests. Every
// secret arrives here already hashed — the store never sees a magic-link token, a session id
// or an API token in plaintext.
export type Role = "owner" | "admin" | "member";
export type User = { id: string; email: string; role: Role; created_at: string };
export type Session = { id_hash: string; user_id: string; created_at: string; expires_at: string };
export type ApiToken = { id: string; user_id: string; name: string; created_at: string; last_used_at: string | null };
export type Audit = { id: number; event: string; user_id: string | null; email: string | null; ip: string | null; at: string };

export interface Store {
  userByEmail(email: string): Promise<User | null>;
  userById(id: string): Promise<User | null>;
  /// Creates the user if the email is new. The first user of the deployment is its owner.
  ensureUser(email: string): Promise<User>;
  setRole(id: string, role: Role): Promise<boolean>;
  listUsers(): Promise<User[]>;
  putMagicLink(tokenHash: string, email: string, expiresAt: Date): Promise<void>;
  /// Single use: marks the link used and returns its email, or null if unknown, used or expired.
  redeemMagicLink(tokenHash: string, now: Date): Promise<string | null>;
  putSession(s: Session): Promise<void>;
  sessionByHash(idHash: string, now: Date): Promise<(Session & { user: User }) | null>;
  deleteSession(idHash: string): Promise<void>;
  putApiToken(t: ApiToken & { token_hash: string }): Promise<void>;
  apiTokenByHash(tokenHash: string, now: Date): Promise<(ApiToken & { user: User }) | null>;
  listApiTokens(userId: string): Promise<ApiToken[]>;
  deleteApiToken(userId: string, id: string): Promise<boolean>;
  audit(event: string, meta: { user_id?: string; email?: string; ip?: string }): Promise<void>;
  listAudit(limit: number): Promise<Audit[]>;
  /// Sliding-window count for a bucket in the last minute; returns the count after increment.
  hit(bucket: string, nowMs: number): Promise<number>;
}

const cols = "id, email, role, created_at";

export function pgStore(): Store {
  const hits = new Map<string, number[]>();
  return {
    userByEmail: async (email) => (await db<User[]>`select ${db.unsafe(cols)} from users where email = ${email}`)[0] ?? null,
    userById: async (id) => (await db<User[]>`select ${db.unsafe(cols)} from users where id = ${id}`)[0] ?? null,
    ensureUser: async (email) => db.begin(async (tx) => {
      // Lock the table so two first sign-ins cannot both become owner.
      await tx`lock table users in share row exclusive mode`;
      const found = (await tx<User[]>`select ${tx.unsafe(cols)} from users where email = ${email}`)[0];
      if (found) return found;
      const [{ n }] = await tx<{ n: number }[]>`select count(*)::int as n from users`;
      const role: Role = n === 0 ? "owner" : "member";
      return (await tx<User[]>`insert into users (id, email, role) values (${crypto.randomUUID()}, ${email}, ${role}) returning ${tx.unsafe(cols)}`)[0];
    }),
    setRole: async (id, role) => (await db`update users set role = ${role} where id = ${id}`).count > 0,
    listUsers: async () => db<User[]>`select ${db.unsafe(cols)} from users order by created_at`,
    putMagicLink: async (h, email, exp) => { await db`insert into magic_links (token_hash, email, expires_at) values (${h}, ${email}, ${exp})`; },
    redeemMagicLink: async (h, now) =>
      (await db<{ email: string }[]>`update magic_links set used_at = ${now} where token_hash = ${h} and used_at is null and expires_at > ${now} returning email`)[0]?.email ?? null,
    putSession: async (s) => { await db`insert into sessions (id_hash, user_id, created_at, expires_at) values (${s.id_hash}, ${s.user_id}, ${s.created_at}, ${s.expires_at})`; },
    sessionByHash: async (h, now) => {
      const r = (await db<(Session & User & { user_id: string })[]>`select s.id_hash, s.user_id, s.created_at, s.expires_at, u.id, u.email, u.role, u.created_at as user_created_at
        from sessions s join users u on u.id = s.user_id where s.id_hash = ${h} and s.expires_at > ${now}`)[0];
      return r ? { id_hash: r.id_hash, user_id: r.user_id, created_at: r.created_at, expires_at: r.expires_at, user: { id: r.id, email: r.email, role: r.role, created_at: (r as unknown as { user_created_at: string }).user_created_at } } : null;
    },
    deleteSession: async (h) => { await db`delete from sessions where id_hash = ${h}`; },
    putApiToken: async (t) => { await db`insert into api_tokens (id, user_id, name, token_hash, created_at) values (${t.id}, ${t.user_id}, ${t.name}, ${t.token_hash}, ${t.created_at})`; },
    apiTokenByHash: async (h, now) => {
      const r = (await db<(ApiToken & { email: string; role: Role; user_created_at: string })[]>`update api_tokens t set last_used_at = ${now} from users u
        where u.id = t.user_id and t.token_hash = ${h} returning t.id, t.user_id, t.name, t.created_at, t.last_used_at, u.email, u.role, u.created_at as user_created_at`)[0];
      return r ? { id: r.id, user_id: r.user_id, name: r.name, created_at: r.created_at, last_used_at: r.last_used_at, user: { id: r.user_id, email: r.email, role: r.role, created_at: r.user_created_at } } : null;
    },
    listApiTokens: async (uid) => db<ApiToken[]>`select id, user_id, name, created_at, last_used_at from api_tokens where user_id = ${uid} order by created_at`,
    deleteApiToken: async (uid, id) => (await db`delete from api_tokens where user_id = ${uid} and id = ${id}`).count > 0,
    audit: async (event, m) => { await db`insert into auth_audit (event, user_id, email, ip) values (${event}, ${m.user_id ?? null}, ${m.email ?? null}, ${m.ip ?? null})`; },
    listAudit: async (limit) => db<Audit[]>`select id, event, user_id, email, ip, at from auth_audit order by at desc, id desc limit ${limit}`,
    // In-process window: fine per replica; move to Redis when replicas must share a limit.
    hit: async (bucket, now) => { const w = (hits.get(bucket) ?? []).filter((t) => t > now - 60_000); w.push(now); hits.set(bucket, w); return w.length; },
  };
}

export function memoryStore(): Store {
  const users: User[] = [];
  const links = new Map<string, { email: string; expires_at: Date; used: boolean }>();
  const sessions = new Map<string, Session>();
  const tokens = new Map<string, ApiToken & { token_hash: string }>();
  const audit: Audit[] = [];
  const hits = new Map<string, number[]>();
  const pub = (u: User) => ({ ...u });
  return {
    userByEmail: async (e) => users.find((u) => u.email === e) ?? null,
    userById: async (id) => users.find((u) => u.id === id) ?? null,
    ensureUser: async (email) => {
      const found = users.find((u) => u.email === email);
      if (found) return pub(found);
      const u: User = { id: crypto.randomUUID(), email, role: users.length === 0 ? "owner" : "member", created_at: new Date().toISOString() };
      users.push(u); return pub(u);
    },
    setRole: async (id, role) => { const u = users.find((u) => u.id === id); if (!u) return false; u.role = role; return true; },
    listUsers: async () => users.map(pub),
    putMagicLink: async (h, email, expires_at) => { links.set(h, { email, expires_at, used: false }); },
    redeemMagicLink: async (h, now) => { const l = links.get(h); if (!l || l.used || l.expires_at <= now) return null; l.used = true; return l.email; },
    putSession: async (s) => { sessions.set(s.id_hash, s); },
    sessionByHash: async (h, now) => { const s = sessions.get(h); if (!s || new Date(s.expires_at) <= now) return null; const user = users.find((u) => u.id === s.user_id); return user ? { ...s, user: pub(user) } : null; },
    deleteSession: async (h) => { sessions.delete(h); },
    putApiToken: async (t) => { tokens.set(t.token_hash, t); },
    apiTokenByHash: async (h, now) => { const t = tokens.get(h); if (!t) return null; t.last_used_at = now.toISOString(); const user = users.find((u) => u.id === t.user_id); return user ? { ...t, user: pub(user) } : null; },
    listApiTokens: async (uid) => [...tokens.values()].filter((t) => t.user_id === uid).map(({ token_hash: _, ...t }) => t),
    deleteApiToken: async (uid, id) => { for (const [h, t] of tokens) if (t.user_id === uid && t.id === id) return tokens.delete(h); return false; },
    audit: async (event, m) => { audit.unshift({ id: audit.length + 1, event, user_id: m.user_id ?? null, email: m.email ?? null, ip: m.ip ?? null, at: new Date().toISOString() }); },
    listAudit: async (limit) => audit.slice(0, limit),
    hit: async (bucket, now) => { const w = (hits.get(bucket) ?? []).filter((t) => t > now - 60_000); w.push(now); hits.set(bucket, w); return w.length; },
  };
}
"##;

const AUTH_TEST: &str = r##"import { describe, expect, test } from "bun:test";
import { createApp, MAGIC_LINK_TTL_MS, SESSION_TTL_MS } from "./app";
import { memoryStore } from "./store";

// The account layer tested as HTTP against the memory store, with the email sender and the
// clock injected: nothing leaves the process, and time moves when a test says so.
function harness() {
  const sent: { to: string; link: string }[] = [];
  let t = Date.now();
  const app = createApp(memoryStore(), async (to, link) => { sent.push({ to, link }); }, () => new Date(t));
  const post = (path: string, body: unknown, headers: Record<string, string> = {}) =>
    app.request(path, { method: "POST", headers: { "content-type": "application/json", ...headers }, body: JSON.stringify(body) });
  // Request a link, follow it, return the session cookie — the whole sign-in as one call.
  const signIn = async (email: string) => {
    expect((await post("/api/auth/sign-in/magic-link", { email })).status).toBe(200);
    const link = sent.at(-1)!.link;
    const res = await app.request(new URL(link).pathname + new URL(link).search);
    expect(res.status).toBe(302);
    const cookie = res.headers.get("set-cookie")!;
    expect(cookie).toContain("HttpOnly");
    return { cookie: cookie.split(";")[0]!, link };
  };
  return { app, post, signIn, sent, advance: (ms: number) => { t += ms; } };
}

describe("auth", () => {
  test("magic link signs in once, and only once; the token is never stored in plaintext", async () => {
    const h = harness();
    const { cookie, link } = await h.signIn("a@example.com");
    const me = await h.app.request("/api/me", { headers: { cookie } });
    expect((await me.json()).user).toMatchObject({ email: "a@example.com", role: "owner" });
    // The same link a second time is refused, and the sender saw the only plaintext copy.
    const again = await h.app.request(new URL(link).pathname + new URL(link).search);
    expect(again.status).toBe(401);
    expect(link).toContain("token=ml_");
  });

  test("an expired link is refused", async () => {
    const h = harness();
    await h.post("/api/auth/sign-in/magic-link", { email: "late@example.com" });
    h.advance(MAGIC_LINK_TTL_MS + 1);
    const link = new URL(h.sent[0]!.link);
    expect((await h.app.request(link.pathname + link.search)).status).toBe(401);
  });

  test("sessions rotate past half their life and sign-out ends them", async () => {
    const h = harness();
    const { cookie } = await h.signIn("r@example.com");
    h.advance(SESSION_TTL_MS / 2 + 1);
    const res = await h.app.request("/api/auth/get-session", { headers: { cookie } });
    expect((await res.json()).user.email).toBe("r@example.com");
    const fresh = res.headers.get("set-cookie")!.split(";")[0]!;
    expect(fresh).not.toBe(cookie);
    expect(await (await h.app.request("/api/auth/get-session", { headers: { cookie } })).json()).toBeNull();
    expect((await h.app.request("/api/me", { headers: { cookie: fresh } })).status).toBe(200);
    await h.post("/api/auth/sign-out", {}, { cookie: fresh });
    expect((await h.app.request("/api/me", { headers: { cookie: fresh } })).status).toBe(401);
  });

  test("roles: first user owns, members are refused, owners promote", async () => {
    const h = harness();
    const owner = (await h.signIn("owner@example.com")).cookie;
    const member = (await h.signIn("m@example.com")).cookie;
    expect((await h.app.request("/api/admin/users", { headers: { cookie: member } })).status).toBe(403);
    const users = (await (await h.app.request("/api/admin/users", { headers: { cookie: owner } })).json()).users as { id: string; email: string }[];
    const m = users.find((u) => u.email === "m@example.com")!;
    expect((await h.app.request(`/api/admin/users/${m.id}/role`, { method: "PATCH", headers: { "content-type": "application/json", cookie: owner }, body: JSON.stringify({ role: "admin" }) })).status).toBe(200);
    expect((await h.app.request("/api/admin/users", { headers: { cookie: member } })).status).toBe(200);
    // Admin is not owner: promoting is refused.
    expect((await h.app.request(`/api/admin/users/${m.id}/role`, { method: "PATCH", headers: { "content-type": "application/json", cookie: member }, body: JSON.stringify({ role: "owner" }) })).status).toBe(403);
  });

  test("API tokens: minted once from a session, hashed at rest, revocable", async () => {
    const h = harness();
    const { cookie } = await h.signIn("t@example.com");
    const made = await h.post("/api/me/tokens", { name: "ci" }, { cookie });
    expect(made.status).toBe(201);
    const { id, token } = await made.json();
    expect(token).toMatch(/^pat_/);
    const me = await h.app.request("/api/me", { headers: { authorization: `Bearer ${token}` } });
    expect((await me.json()).auth).toBe("token");
    expect((await h.post("/api/me/tokens", { name: "nope" }, { authorization: `Bearer ${token}` })).status).toBe(403);
    const listed = (await (await h.app.request("/api/me/tokens", { headers: { cookie } })).json()).tokens;
    expect(JSON.stringify(listed)).not.toContain(token);
    expect((await h.app.request(`/api/me/tokens/${id}`, { method: "DELETE", headers: { cookie } })).status).toBe(204);
    expect((await h.app.request("/api/me", { headers: { authorization: `Bearer ${token}` } })).status).toBe(401);
  });

  test("sign-in is rate limited per address and every step is audited", async () => {
    const h = harness();
    for (let i = 0; i < 5; i++) expect((await h.post("/api/auth/sign-in/magic-link", { email: "spam@example.com" })).status).toBe(200);
    const blocked = await h.post("/api/auth/sign-in/magic-link", { email: "spam@example.com" });
    expect(blocked.status).toBe(429);
    expect(blocked.headers.get("retry-after")).toBe("60");
    const { cookie } = await h.signIn("audit@example.com");
    const events = (await (await h.app.request("/api/admin/audit", { headers: { cookie } })).json()).events.map((e: { event: string }) => e.event);
    expect(events).toContain("sign_in.rate_limited");
    expect(events).toContain("sign_in.completed");
  });
});
"##;

const AUTH_SQL: &str = r##"-- No password column anywhere: sign-in is a magic link, and everything that grants access
-- (magic-link tokens, session ids, API tokens) is stored as a sha256 hash, never plaintext.
create table if not exists users (
  id uuid primary key,
  email text not null unique,
  role text not null default 'member' check (role in ('owner', 'admin', 'member')),
  created_at timestamptz not null default now()
);
create table if not exists magic_links (
  token_hash text primary key,
  email text not null,
  expires_at timestamptz not null,
  used_at timestamptz
);
create table if not exists sessions (
  id_hash text primary key,
  user_id uuid not null references users (id) on delete cascade,
  created_at timestamptz not null default now(),
  expires_at timestamptz not null
);
create index if not exists sessions_user on sessions (user_id);
create table if not exists api_tokens (
  id uuid primary key,
  user_id uuid not null references users (id) on delete cascade,
  name text not null,
  token_hash text not null unique,
  created_at timestamptz not null default now(),
  last_used_at timestamptz
);
create table if not exists auth_audit (
  id bigserial primary key,
  event text not null,
  user_id uuid,
  email text,
  ip text,
  at timestamptz not null default now()
);
create index if not exists auth_audit_at on auth_audit (at desc);
"##;

const AUTH_PAGE: &str = r##""use client";
import { useEffect, useState } from "react";

type Me = { user: { id: string; email: string; role: string } } | null;
type Token = { id: string; name: string; created_at: string; last_used_at: string | null };

// Account page: request a link, see who you are, mint and revoke API tokens. With no email
// provider configured the backend prints the link to its log — copy it from there in dev.
export default function Home() {
  const [me, setMe] = useState<Me>(null);
  const [email, setEmail] = useState("");
  const [sentTo, setSentTo] = useState<string | null>(null);
  const [tokens, setTokens] = useState<Token[]>([]);
  const [minted, setMinted] = useState<string | null>(null);
  const [name, setName] = useState("ci");

  const refresh = async () => {
    const s = (await (await fetch("/api/auth/get-session")).json()) as Me;
    setMe(s);
    if (s) setTokens((await (await fetch("/api/me/tokens")).json()).tokens);
  };
  useEffect(() => { void refresh(); }, []);

  const post = (path: string, body: unknown) => fetch(path, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body) });
  const signIn = async () => { await post("/api/auth/sign-in/magic-link", { email, callbackURL: "/" }); setSentTo(email); };
  const signOut = async () => { await post("/api/auth/sign-out", {}); setMinted(null); await refresh(); };
  const mint = async () => { setMinted((await (await post("/api/me/tokens", { name })).json()).token); await refresh(); };
  const revoke = async (id: string) => { await fetch(`/api/me/tokens/${id}`, { method: "DELETE" }); await refresh(); };

  return (
    <main>
      <h1>{{NAME}} — account</h1>
      <p>Magic-link sign-in, HttpOnly session cookies that rotate, roles (owner · admin · member), hashed API tokens, an audit table. No passwords.</p>
      {!me ? (
        <section>
          <h2>Sign in</h2>
          <p><input type="email" placeholder="you@example.com" value={email} onChange={(e) => setEmail(e.target.value)} /> <button onClick={signIn}>Email me a link</button></p>
          {sentTo && <p>Link sent to <code>{sentTo}</code>. In dev it is printed by the backend — open it from the log.</p>}
          <pre>{`curl -X POST /api/auth/sign-in/magic-link -H "content-type: application/json" -d '{"email":"you@example.com"}'`}</pre>
        </section>
      ) : (
        <section>
          <p>Signed in as <code>{me.user.email}</code> · role <strong>{me.user.role}</strong> <button onClick={signOut}>Sign out</button></p>
          <h2>API tokens</h2>
          <p><input value={name} onChange={(e) => setName(e.target.value)} /> <button onClick={mint}>Create token</button></p>
          {minted && <p>Copy it now — it is shown once: <code>{minted}</code></p>}
          <pre>{`curl /api/me -H "authorization: Bearer pat_..."`}</pre>
          <table><thead><tr><th>name</th><th>created</th><th>last used</th><th></th></tr></thead>
            <tbody>{tokens.map((t) => <tr key={t.id}><td>{t.name}</td><td>{t.created_at}</td><td>{t.last_used_at ?? "never"}</td><td><button onClick={() => revoke(t.id)}>Revoke</button></td></tr>)}</tbody></table>
          {me.user.role !== "member" && <p><a href="/api/admin/users">users</a> · <a href="/api/admin/audit">audit log</a></p>}
        </section>
      )}
    </main>
  );
}
"##;
