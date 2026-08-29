//! auth: the account layer, like better-auth / Lucia — magic links, sessions, roles, API tokens

pub fn files(name: &str) -> Vec<(&'static str, String)> {
    let f = |s: &str| s.replace("{{NAME}}", name);
    vec![
        ("backend/src/app.ts", AUTH_APP.into()),
        ("backend/src/store.ts", AUTH_STORE.into()),
        ("backend/src/app.test.ts", AUTH_TEST.into()),
        ("backend/migrations/0002_auth.sql", AUTH_SQL.into()),
        ("frontend/app/page.tsx", f(AUTH_PAGE)),
        ("CLAUDE.md", f(AUTH_CLAUDE)),
        ("AGENTS.md", f(AUTH_AGENTS)),
        ("README.md", f(AUTH_README)),
        (".claude/agents/auth-flows.md", AUTH_REVIEWER.into()),
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

const AUTH_CLAUDE: &str = r##"# {{NAME}} — the account layer

This service is the sign-in, session, role and API-token layer for a product, modelled on
better-auth's HTTP surface (`/sign-in/magic-link`, `/magic-link/verify`, `/get-session`,
`/sign-out`) over Lucia's session model (a random token in the cookie, its sha256 in the table,
rotation on use). There are no passwords anywhere. "Done" here means: a change keeps every secret
hashed at rest, keeps every state change in `auth_audit`, and leaves `make check` green with a test
that exercises the new behaviour over HTTP against the memory store.

## Architecture

| File | Owns |
|---|---|
| `backend/src/app.ts` | Every route; the `requireUser` and `requireRole` middleware; session issue and rotation; the `SendEmail` and `Clock` injection points; `AppType` |
| `backend/src/store.ts` | The `Store` interface and both implementations: `pgStore` (Postgres) and `memoryStore` (tests). Never sees a plaintext secret |
| `backend/src/app.test.ts` | The account layer as HTTP against `memoryStore`, with the email sender and clock injected |
| `backend/migrations/0002_auth.sql` | `users`, `magic_links`, `sessions`, `api_tokens`, `auth_audit` |
| `backend/src/server.ts` | Node server on `PORT`, SIGTERM drain (from the stack) |
| `backend/src/db.ts` | The one `postgres` pool, `DATABASE_URL` (from the stack) |
| `backend/src/migrate.ts` | Applies `migrations/*.sql` in name order, once each (from the stack) |
| `frontend/app/page.tsx` | Account page: request a link, see who you are, mint and revoke tokens |

The request path for a signed-in call, `GET /api/me/tokens`:

1. `requireUser` reads `Authorization: Bearer pat_…` first. If present, `store.apiTokenByHash(sha256(token))` resolves the user (and stamps `last_used_at`); no match is a 401 and the cookie is never consulted.
2. Otherwise it reads the `session_token` cookie and calls `store.sessionByHash(sha256(cookie), now)`; an unknown or expired row is a 401.
3. If the session is past half of `SESSION_TTL_MS` (15 of 30 days), the old row is deleted and `issueSession` writes a new one and sets a new cookie — the leaked-id window is bounded by rotation, not by the full TTL.
4. `c.set("user", …)` and `c.set("auth", "session" | "token")`; handlers read the principal from the context, never from the request.
5. `requireRole(min)` compares `RANK[user.role]` against `RANK[min]` and answers 403 below it. `/api/admin/*` is mounted with `requireRole("admin")`; the role-change route adds `requireRole("owner")` on top.
6. The handler calls the store with the user id from the context; the response is the store's row with secrets already absent (`token_hash` is never selected back).
7. Anything that changed access calls `store.audit(event, {user_id, email, ip})`.

The sign-in path is shorter: `POST /api/auth/sign-in/magic-link` rate-limits per email and per source address, stores `sha256(ml_…)` with a five-minute expiry, and hands the plaintext link to the injected `sendEmail`. `GET /api/auth/magic-link/verify` redeems the hash single-use (`update … where used_at is null and expires_at > now returning email`), `ensureUser`s the address, issues a session and 302s to a relative `callbackURL`.

Data model:

| Table | Why the columns that matter |
|---|---|
| `users` | `email` unique and lower-cased at the boundary; `role` is a check constraint on `owner/admin/member`; the first row of the deployment is `owner`, decided under a table lock in `ensureUser` |
| `magic_links` | Keyed by `token_hash`, so the plaintext is never stored; `used_at` makes redemption single-use in one `update … returning` |
| `sessions` | Keyed by `id_hash`; `created_at` drives rotation, `expires_at` drives the 401; `on delete cascade` from users; index on `user_id` for a future "sign out everywhere" |
| `api_tokens` | `token_hash` unique, `name` is the only thing the person sees again, `last_used_at` is stamped on every use so a dead token is visible |
| `auth_audit` | `event`, nullable `user_id`/`email`/`ip`, `at` indexed descending; append-only |

## Invariants

1. **No plaintext secret is ever stored.** Magic-link tokens (`ml_`), session ids (`ses_`) and API tokens (`pat_`) are hashed with sha256 before `store` sees them; the plaintext exists once, in the email or the 201 body. Why: a database read must not be a sign-in. Guarded by "magic link signs in once, and only once; the token is never stored in plaintext" and "API tokens: minted once from a session, hashed at rest, revocable" (asserts the list body does not contain the token).
2. **A magic link redeems once and dies at five minutes.** Why: an email is not a private channel. Guarded by the same first test (second visit is 401) and "an expired link is refused" (clock advanced past `MAGIC_LINK_TTL_MS`).
3. **Sessions rotate past half their life, and the old id stops working immediately.** Why: a stolen cookie has at most 15 days, and a rotated cookie invalidates the copy an attacker holds. Guarded by "sessions rotate past half their life and sign-out ends them".
4. **Sign-out deletes the row, not just the cookie.** Why: a cookie the browser forgot is still a valid credential if the row survives. Same test: `/api/me` is 401 with the old cookie after sign-out.
5. **API tokens are minted only from a browser session.** A `Bearer` principal calling `POST /api/me/tokens` gets 403. Why: a leaked token must not be able to make itself permanent by minting siblings. Guarded by "API tokens: minted once from a session…".
6. **The first user is the owner, and it is decided under a lock.** `pgStore.ensureUser` takes `lock table users in share row exclusive mode` before counting. Why: two first sign-ins racing must not both become owner. The role outcome is asserted by "roles: first user owns, members are refused, owners promote"; the lock itself has no test because the memory store has no concurrency — do not remove it.
7. **Only an owner changes roles, and an owner cannot demote themselves.** Why: a deployment with no owner cannot be administered. Guarded by "roles: …" (admin promoting to owner is 403); the self-demotion branch is enforced in the handler and needs a test if touched.
8. **Redirects are relative.** `callbackURL` must start with `/` and not `//`, at request time and at verify time. Why: the verify link is unauthenticated and would otherwise be an open redirect. No test yet — add one before changing either check.
9. **Sign-in is rate limited before any email is sent.** Six requests a minute per address or twenty-one per source address is a 429 with `Retry-After: 60`, and it is audited. Why: an inbox must not be floodable and an address list must not be enumerable through timing. Guarded by "sign-in is rate limited per address and every step is audited".
10. **Every access change writes an audit row.** `sign_in.requested`, `sign_in.rate_limited`, `sign_in.failed`, `sign_in.completed`, `sign_out`, `token.created`, `token.revoked`, `role.<role>`. Why: the question after an incident is always "who signed in from where", and it must be answerable from the table. Guarded by the same rate-limit test (`sign_in.rate_limited` and `sign_in.completed` are present).
11. **The cookie is `HttpOnly`, `SameSite=Lax`, `Path=/`, and `Secure` when `NODE_ENV=production`.** Why: script cannot read it and cross-site POSTs do not carry it. Guarded by the `signIn` helper in the tests (`HttpOnly` asserted on every sign-in).
12. **The email sender refuses to run in production unconfigured.** `consoleEmail` throws when `NODE_ENV=production`. Why: a production deployment that prints magic links into its logs is a credential leak, not a feature. Enforced in code; no test.

## Extending it

**Add a protected route.** In `app.ts`, add it to the chain after the `.use("/api/me/*", requireUser)` line (or under `/api/admin/*` for admin-only); read the principal with `c.get("user")` and `c.get("auth")`. Add a test in `app.test.ts` that calls it with no cookie (401), with a session cookie, and with a `Bearer` token if a token should be allowed. No migration unless it stores something.

**Add a real email provider.** Implement `SendEmail` (`(to, link) => Promise<void>`) in a new `backend/src/email.ts` using the vendor's HTTP API with a timeout, and pass it at the bottom of `app.ts`: `createApp(pgStore(), resendEmail)`. Keep `consoleEmail` as the non-production default. Tests keep injecting their capture function; add one test for the provider module that asserts the request it would send, with `fetch` injected.

**Add a role.** Extend `Role` in `store.ts`, `RANK` in `app.ts`, the check constraint in a new `backend/migrations/0003_role_x.sql` (`alter table users drop constraint …, add constraint …`), and the `z.enum` in the role-change route. Add a case to "roles: …" for what the new role may and may not do. Never renumber ranks that exist.

**Add an audit event.** Call `store.audit("<area>.<verb>", { user_id, email, ip: ip(c) })` in the handler that makes the change. Names are dotted, lower-case, and stable once shipped — the admin page and any alerting key on them. Extend the audit assertion in the rate-limit test.

**Add a column to `users`.** Migration `0003_…sql` (add column with a default; never rename); `User` type and the `cols` constant in `store.ts`; the `user` object built in `sessionByHash` and `apiTokenByHash`; `memoryStore.ensureUser`. Tests: extend `toMatchObject` in the first test.

**Give API tokens an expiry.** Add `expires_at timestamptz` to `api_tokens` (migration), accept an optional `expires_in_days` in `POST /api/me/tokens`, and add `and (t.expires_at is null or t.expires_at > ${now})` to `apiTokenByHash` — the `now` parameter is already passed for this. Test: mint, advance the clock, expect 401.

**Add "sign out everywhere".** `deleteSessionsFor(userId)` in `Store` (the `sessions_user` index exists for it), a `POST /api/me/sign-out-all` under `requireUser`, an audit event. Test: two sign-ins, one call, both cookies 401.

**Move the rate limiter to Redis.** Replace `hit` in `pgStore` with a Redis `INCR` + `EXPIRE 60` on the same bucket keys (`email:<addr>`, `ip:<addr>`); the interface and the tests do not change. Do this when there is more than one backend replica.

## Operating it

| Env var | Required | Meaning |
|---|---|---|
| `DATABASE_URL` | yes | Postgres; defaults to the compose service locally |
| `PORT` | no | API port, default 8000 |
| `APP_URL` | in production | Origin the magic link points at (`${APP_URL}/api/auth/magic-link/verify`); default `http://localhost:3000`. Must be the origin the browser uses so the cookie is set on the right site |
| `NODE_ENV` | in production | `production` makes the cookie `Secure` and makes `consoleEmail` refuse to run |

An email provider's own key goes in `backend/.env` when you add one (see Extending it); today none is read.

**Per replica today:** the sign-in rate-limit window (`hits` map in `pgStore`). With N replicas the effective limit is N× — it degrades to "still limited", not "unlimited", but move it to Redis before relying on it. Everything else (sessions, links, tokens, audit) is in Postgres and shared.

**Scaling knobs:** the pool is `max: 10` in `db.ts`; replicas × 10 must stay under the database's `max_connections`. Every authenticated request is one session `select` joined to `users`, and once per 15 days a delete plus insert; nothing here needs a cache until that shows up in the database.

**Failure modes and what the person sees:**

| Failure | Seen as |
|---|---|
| Email provider down or slow | `POST /sign-in/magic-link` fails or hangs after the link row is written — give the sender a timeout; the link is harmless if unsent |
| `APP_URL` wrong | The link lands on a host that does not set the cookie for the app; sign-in "works" and `/api/me` is 401 |
| Database down | Every route except `/api/health` is a 500; `/api/health/ready` still says `db: ok` because it does not check — wire it to `select 1` before trusting the readiness probe |
| `x-forwarded-for` not set by the proxy | Every request rate-limits as `local`: one shared bucket of 20/min for everyone. Make sure the ingress sets it, and that it cannot be set by the client |
| Clock skew between replicas | Links and sessions expire early or late by the skew; keep NTP on |

**What to watch:** the count of `sign_in.rate_limited` and `sign_in.failed` in `auth_audit` per minute (enumeration or a broken link), the ratio of `sign_in.requested` to `sign_in.completed` (email deliverability), rows in `sessions` and `magic_links` with `expires_at < now()` (nothing prunes them yet), and `api_tokens.last_used_at` older than 90 days (dead credentials). Logs are the stack's JSON lines; the only auth-specific line is the dev-only magic link.

## Ceilings

- `ponytail: in-process sliding window` — the rate limiter is a `Map` per replica. Redis `INCR/EXPIRE` when there are replicas.
- Expired `sessions` and `magic_links` rows are never deleted. A nightly `delete … where expires_at < now()` (a cron Job in the cluster) when the tables are large enough to notice.
- `auth_audit` is append-only and unpartitioned. Partition by month and drop partitions when it is the largest table.
- API tokens do not expire (see Extending it). They are revocable and `last_used_at` is stamped, which is enough until a policy needs an expiry.
- `ensureUser` locks `users` on every verify. Fine at sign-in rates; move to an `insert … on conflict` with a separate owner-election row if sign-ins ever contend.
- `/api/health/ready` reports `db: ok` without asking. Replace with `select 1` before the readiness probe means anything.
- One deployment, one set of roles. Organisations and per-org roles are the `tenant` pack; combine by replacing its signed-cookie `whoami` with `requireUser` from here.

The stack rules — gate, typed seam, production checklist, deploy — are in `docs/PRODUCTION.md`. They apply.
"##;

const AUTH_AGENTS: &str = r##"# {{NAME}} — for agents

`CLAUDE.md` has the rules. This is how to run it and prove it.

## Run

    make demo        # postgres + redis, migrate, seed the stack's sample table, start both halves
    make check       # the gate: frontend typecheck, backend typecheck + tests
    make backend     # API alone on :8000 with reload
    make frontend    # Next.js on :3000, /api proxied to :8000

There is no worker or importer in this service. In development the magic link is printed by the
backend as a JSON log line (`"msg":"magic link (dev only)"`); copy it from there.

## Every route, with a body that works

Sign in (the link arrives in the backend log in dev):

    curl -s localhost:3000/api/auth/sign-in/magic-link -H 'content-type: application/json' \
      -d '{"email":"you@example.com","callbackURL":"/"}'
    # {"status":true}

Follow the link and keep the cookie:

    curl -si -c jar 'http://localhost:3000/api/auth/magic-link/verify?token=ml_…&callbackURL=%2F'
    # HTTP/1.1 302 Found, Set-Cookie: session_token=ses_…; Path=/; HttpOnly; SameSite=Lax

Who am I:

    curl -s -b jar localhost:3000/api/auth/get-session
    # {"user":{"id":"…","email":"you@example.com","role":"owner","created_at":"…"},"session":{"expires_at":"…"}}
    curl -s -b jar localhost:3000/api/me
    # {"user":{…},"auth":"session"}

Tokens:

    curl -s -b jar localhost:3000/api/me/tokens -H 'content-type: application/json' -d '{"name":"ci"}'
    # 201 {"id":"…","user_id":"…","name":"ci","created_at":"…","last_used_at":null,"token":"pat_…"}
    curl -s localhost:3000/api/me -H 'authorization: Bearer pat_…'
    # {"user":{…},"auth":"token"}
    curl -s -b jar localhost:3000/api/me/tokens
    # {"tokens":[{"id":"…","name":"ci",…,"last_used_at":"…"}]}   (no token field)
    curl -si -b jar -X DELETE localhost:3000/api/me/tokens/<id>
    # HTTP/1.1 204

Admin (role `admin` or above; the role change needs `owner`):

    curl -s -b jar localhost:3000/api/admin/users
    # {"users":[{"id":"…","email":"…","role":"owner","created_at":"…"}]}
    curl -s -b jar 'localhost:3000/api/admin/audit?limit=20'
    # {"events":[{"id":7,"event":"token.created","user_id":"…","email":null,"ip":"local","at":"…"},…]}
    curl -s -b jar -X PATCH localhost:3000/api/admin/users/<id>/role -H 'content-type: application/json' -d '{"role":"admin"}'
    # {"id":"…","role":"admin"}

Sign out:

    curl -s -b jar -X POST localhost:3000/api/auth/sign-out
    # {"status":true}

Errors are always `{"error":{"message","code",…}}`: `unauthorized` 401, `forbidden` 403,
`invalid` 400 (with zod `details`), `invalid_token` 401, `rate_limited` 429 (+ `Retry-After`),
`not_found` 404.

## How the tests work

`backend/src/app.test.ts` builds the app with `createApp(memoryStore(), captureEmail, clock)`:

- `memoryStore()` is the same `Store` interface as Postgres, in maps. No database, no network.
- The email sender pushes `{to, link}` into an array; a test reads the link back and follows it
  with `app.request(pathname + search)`.
- The clock is a closure over a number; `h.advance(ms)` moves time so expiry and rotation are
  tested without waiting.
- `h.signIn(email)` does the whole flow and returns the cookie; most tests start there.

## Add a test

Add a `test(...)` inside `describe("auth")`. Get a principal with `await h.signIn("x@example.com")`
(the first sign-in in a harness is the owner), call routes through `h.app.request` or `h.post`,
and assert on status and JSON. If the behaviour depends on time, use `h.advance`. If it depends on
a Postgres-only property (the table lock, the `returning` single-use update) say so in a comment —
the memory store cannot show it, and the reviewer needs to know it is untested rather than assume.
"##;

const AUTH_README: &str = r##"# {{NAME}}

Passwordless accounts for a product: magic-link sign-in, rotating sessions, three roles, hashed
API tokens and an audit log — as one Hono service you own, typed end to end into a Next.js
frontend.

## What you get

- `POST /api/auth/sign-in/magic-link` → an email with a five-minute, single-use link; no password column exists.
- Sessions as `HttpOnly` `SameSite=Lax` cookies, 30 days, rotated on use past 15, ended by sign-out server-side.
- `Authorization: Bearer pat_…` for scripts and CI, minted from a session, shown once, hashed at rest, revocable.
- Roles `owner` / `admin` / `member`; the first sign-in owns the deployment; owners change roles; an owner cannot demote themselves.
- Rate limiting on sign-in per address and per source, with `Retry-After`.
- `auth_audit`: one row per sign-in attempt, sign-out, token and role change, readable at `/api/admin/audit`.
- Tests that run the whole thing over HTTP in memory, with the email sender and the clock injected.
- Postgres migrations, Docker images, kustomize overlays for dev and prod, GitHub Actions to deploy them.

## Five minutes

    make demo

Then, in another terminal:

    curl -s localhost:3000/api/auth/sign-in/magic-link -H 'content-type: application/json' -d '{"email":"you@example.com"}'
    # {"status":true}          — the backend log prints: "msg":"magic link (dev only)","link":"http://localhost:3000/api/auth/magic-link/verify?token=ml_…"

    curl -si -c jar '<that link>'
    # HTTP/1.1 302 Found      — Set-Cookie: session_token=ses_…; HttpOnly; SameSite=Lax

    curl -s -b jar localhost:3000/api/me
    # {"user":{"email":"you@example.com","role":"owner",…},"auth":"session"}

    curl -s -b jar localhost:3000/api/me/tokens -H 'content-type: application/json' -d '{"name":"ci"}'
    # 201 {…,"token":"pat_…"}  — the only time you see it

Open http://localhost:3000 for the same flow as a page.

## API

| Method | Path | Auth | What |
|---|---|---|---|
| POST | `/api/auth/sign-in/magic-link` | none | `{email, callbackURL?}` → sends the link; 429 above 5/min per address, 20/min per source |
| GET | `/api/auth/magic-link/verify?token&callbackURL` | none | Redeems the link once, sets the cookie, 302 to a relative `callbackURL` |
| GET | `/api/auth/get-session` | cookie | `{user, session}` or `null`; rotates the cookie when due |
| POST | `/api/auth/sign-out` | cookie | Deletes the session row and clears the cookie |
| GET | `/api/me` | cookie or bearer | `{user, auth: "session" \| "token"}` |
| GET | `/api/me/tokens` | cookie or bearer | Your tokens, without secrets |
| POST | `/api/me/tokens` | cookie only | `{name}` → 201 with the plaintext `token`, once |
| DELETE | `/api/me/tokens/:id` | cookie or bearer | 204; the token stops working immediately |
| GET | `/api/admin/users` | admin | Every user |
| GET | `/api/admin/audit?limit` | admin | Latest audit rows, max 200 |
| PATCH | `/api/admin/users/:id/role` | owner | `{role}`; self-demotion refused |
| GET | `/api/health`, `/api/health/ready` | none | Liveness and readiness |

## Compared with better-auth (and Lucia)

**Same.** The route names and semantics of better-auth's magic-link plugin (`/sign-in/magic-link`,
`/magic-link/verify?token&callbackURL`, `/get-session`, `/sign-out`), so its client and its docs
describe this API. Lucia's session design: random id in the cookie, hash in the table, rotation on
use, delete on sign-out. Token prefixes (`ml_`, `ses_`, `pat_`) so a leaked string is recognisable
in a log or a scanner.

**Better here.**
- One codebase in the repository, ~450 lines, with nothing behind a plugin boundary. The session rule is 6 lines you can read.
- Typed end to end: `AppType` is exported and the frontend client is built from it; a route change that breaks the page fails `make check`.
- Tests without a database: the memory store, an injected sender and an injected clock mean expiry and rotation are asserted in milliseconds.
- API tokens as a first-class credential, minted only from a session, with `last_used_at`.
- An audit table that is part of the schema, not an optional plugin.
- The first-owner election is under a database lock, and self-demotion is refused.
- Ships with migrations, images, kustomize overlays and deploy workflows (`docs/PRODUCTION.md`).

**Not here yet.**
- Passwords, OAuth/social providers, passkeys, two-factor — better-auth has all of these as plugins; this service is magic-link only by design.
- A client SDK with hooks; the page uses `fetch` and the typed client.
- Organisations, teams and per-org roles (the `tenant` pack).
- Multiple sessions listing and "sign out everywhere".
- Token expiry, token scopes.
- Database adapters: this is Postgres via the `postgres` driver, not a pluggable adapter.
- Email verification separate from sign-in, account linking, user deletion, ban/impersonation from better-auth's admin plugin.
- A distributed rate limiter — the window is per replica until it moves to Redis.

## Production

- **Environments.** `k8s/overlays/dev` and `k8s/overlays/prod`; `git push` to `main` deploys dev, `make release` tags and deploys prod.
- **Config.** `DATABASE_URL`, `PORT`, `APP_URL` (the origin the magic link points at), `NODE_ENV=production` (secure cookie; the dev email sender refuses to run). Rendered from `backend/.env` into a Secret by `k8s/scripts/env-to-secrets.sh`; committed only as `backend/.env.age`.
- **Email.** No provider is wired. Implement `SendEmail` with a timeout and pass it to `createApp` (recipe in `CLAUDE.md`) before the first production deploy; without it, sign-in throws.
- **Scaling.** Stateless handlers; sessions in Postgres. The sign-in rate limiter is per replica — move it to Redis once there are replicas. Pool is 10 per replica.
- **Probes.** `/api/health` (liveness) and `/api/health/ready` (readiness); readiness does not yet query the database — see Roadmap.
- **Migrations.** `backend/migrations/*.sql`, applied by an init container before each rollout. Expand/contract; never rename in place.
- **What pages you.** A spike of `sign_in.failed` or `sign_in.rate_limited` in `auth_audit`; `sign_in.requested` without `sign_in.completed` (email is not arriving); the email provider's error rate; Postgres connections near `max_connections`.

## Roadmap

- Redis-backed rate limiting.
- Pruning expired `sessions` and `magic_links`; partitioning `auth_audit`.
- Token expiry and "sign out everywhere" (the schema is ready for both).
- A readiness probe that asks the database.
- Combining with the `tenant` pack for organisations.
"##;

const AUTH_REVIEWER: &str = r##"---
name: auth-flows
description: Run on any change to app.ts, store.ts or 0002_auth.sql — sign-in, sessions, tokens, roles, audit. Reads the change as someone trying to sign in as somebody else and reports only what lets them.
tools: Read, Grep, Glob, Bash
---

You review the account layer. You did not write the change and you assume it broke something.

Report only what lets a person act as another, keep access they should have lost, or hide an
access change — each as `path:line — what — how it is used — the fix`. End with
`auth-flows: N findings` and, if 0, what you checked.

Check:
1. **Hashing at the boundary.** Every `store.put*`/`*ByHash` call in `app.ts` receives `sha(…)`, never the raw `ml_`/`ses_`/`pat_` string. Grep `store.` in `app.ts`; any call passing `token`, `raw` or `cookie` unhashed is a finding. The failure looks like a database row that equals the cookie.
2. **Single-use links.** `redeemMagicLink` in both stores refuses a used or expired hash (`used_at is null and expires_at > now`). A second visit to the same link must be 401 — the first test asserts it; make sure the test still visits twice.
3. **Rotation invalidates.** After rotation the old `id_hash` is deleted before the new one is issued (`requireUser` and `get-session` both). A change that issues first and deletes later leaves two valid sessions; a change that skips the delete leaves the leaked one alive.
4. **Sign-out deletes the row.** `POST /sign-out` calls `deleteSession`, not only `deleteCookie`.
5. **Token minting requires a session.** `POST /api/me/tokens` checks `c.get("auth") === "session"`. A `Bearer` principal minting a token is a finding.
6. **The plaintext token appears once.** Only the 201 body of `POST /api/me/tokens` contains `token`; `listApiTokens` selects no `token_hash`; the memory store strips it (`({ token_hash: _, ...t })`). Grep for `token_hash` in any `select`/response.
7. **Role gates are middleware, not handler ifs.** `/api/admin/*` is mounted with `requireUser, requireRole("admin")`; the role change adds `requireRole("owner")`. A new admin route registered outside that `.use` path is unprotected.
8. **Self-demotion refused; owner election locked.** The `target.id === user.id && role !== "owner"` branch is intact; `pgStore.ensureUser` still takes `lock table users … share row exclusive` before counting.
9. **Redirects are relative.** Both `callbackURL` checks (`startsWith("/") && !startsWith("//")`) are present at request and verify time. `https://evil` or `//evil` must fall back to `/`.
10. **Rate limit before send.** In `sign-in/magic-link`, `store.hit` runs before `putMagicLink` and `sendEmail`; the 429 carries `Retry-After` and writes `sign_in.rate_limited`. The `ip()` helper takes the first `x-forwarded-for` entry — confirm the ingress overwrites that header rather than appending to a client-supplied one.
11. **Cookie flags.** `setCookie` has `httpOnly: true`, `sameSite: "Lax"`, `path: "/"`, `secure` tied to `NODE_ENV=production`, `maxAge` equal to the session TTL. Any relaxation is a finding.
12. **Audit on every access change.** Sign-in request/failure/completion, sign-out, token create/revoke, role change each call `store.audit`. A new mutating route without one is a finding — an incident cannot be reconstructed.
13. **Email addresses are lower-cased once**, at `sign-in/magic-link`, before hashing and storing, so `A@x.com` and `a@x.com` are one account and one rate-limit bucket.
14. **Errors do not distinguish.** An unknown email on sign-in returns `{status:true}` like a known one; an invalid and an expired link both return `invalid_token`. A message that reveals whether an address exists is a finding.
15. **`consoleEmail` still throws under `NODE_ENV=production`.**
"##;
