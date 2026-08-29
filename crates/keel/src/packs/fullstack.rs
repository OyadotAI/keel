//! fullstack: a CRUD resource with sessions and optimistic concurrency ══════════════════

pub fn files(name: &str) -> Vec<(&'static str, String)> {
    let f = |s: &str| s.replace("{{NAME}}", name);
    vec![
        ("backend/src/app.ts", FULLSTACK_APP.into()),
        ("backend/src/store.ts", FULLSTACK_STORE.into()),
        ("backend/src/schema.ts", FULLSTACK_SCHEMA.into()),
        ("backend/src/seed.ts", FULLSTACK_SEED.into()),
        ("backend/src/app.test.ts", FULLSTACK_TEST.into()),
        (
            "backend/migrations/0002_fullstack.sql",
            FULLSTACK_SQL.into(),
        ),
        ("frontend/app/page.tsx", f(FULLSTACK_PAGE)),
    ]
}

const FULLSTACK_APP: &str = r##"import { Hono, type Context } from "hono";
import { getSignedCookie, setSignedCookie, deleteCookie } from "hono/cookie";
import { pgStore, type Store } from "./store";
import { postInput, postUpdate, signIn } from "./schema";

// The scaffold's full-stack shape, made concrete: one resource (posts) with list, read, create,
// update and delete; a server-side session behind a signed HttpOnly cookie; the request shapes
// shared with the frontend as zod schemas (schema.ts); and optimistic concurrency — an update
// names the version it edited, and a stale one gets 409 with the current row instead of
// overwriting someone else's work.

const SECRET = () => process.env.SESSION_SECRET ?? "dev-secret-change-me";
const COOKIE = "sid";

export function createApp(store: Store) {
  const app = new Hono()
    .get("/api/health", (c) => c.json({ status: "ok" }))
    .get("/api/health/ready", (c) => c.json({ status: "ok", db: "ok" as const }))

    // Sign in is a name, no password: the session machinery is real, the identity check is the
    // part a product replaces (see the auth template). The cookie holds a signed random id; the
    // name lives in the sessions table.
    .post("/api/session", async (c) => {
      const p = signIn.safeParse(await c.req.json().catch(() => null));
      if (!p.success) return c.json({ error: { message: p.error.issues[0].message, code: "invalid" } }, 400);
      const id = await store.createSession(p.data.name);
      await setSignedCookie(c, COOKIE, id, SECRET(), { httpOnly: true, sameSite: "Lax", path: "/", secure: process.env.NODE_ENV === "production" });
      return c.json({ name: p.data.name }, 201);
    })
    .get("/api/session", async (c) => {
      const id = await getSignedCookie(c, SECRET(), COOKIE);
      const s = id ? await store.session(id) : null;
      return s ? c.json({ name: s.name }) : c.json({ error: { message: "not signed in", code: "unauthorized" } }, 401);
    })
    .delete("/api/session", (c) => { deleteCookie(c, COOKIE, { path: "/" }); return c.body(null, 204); })

    .get("/api/posts", async (c) => c.json({ posts: await store.list(Math.min(Number(c.req.query("limit") ?? 50), 200)) }))
    .get("/api/posts/:id", async (c) => {
      const p = await store.get(Number(c.req.param("id")));
      return p ? c.json(p) : c.json({ error: { message: "no such post", code: "not_found" } }, 404);
    })
    .post("/api/posts", async (c) => {
      const who = await user(c); if (!who) return c.json({ error: { message: "sign in first", code: "unauthorized" } }, 401);
      const p = postInput.safeParse(await c.req.json().catch(() => null));
      if (!p.success) return c.json({ error: { message: p.error.issues[0].message, code: "invalid", issues: p.error.flatten().fieldErrors } }, 400);
      return c.json(await store.create(p.data, who), 201);
    })
    .put("/api/posts/:id", async (c) => {
      const who = await user(c); if (!who) return c.json({ error: { message: "sign in first", code: "unauthorized" } }, 401);
      const id = Number(c.req.param("id"));
      const p = postUpdate.safeParse(await c.req.json().catch(() => null));
      if (!p.success) return c.json({ error: { message: p.error.issues[0].message, code: "invalid", issues: p.error.flatten().fieldErrors } }, 400);
      const current = await store.get(id);
      if (!current) return c.json({ error: { message: "no such post", code: "not_found" } }, 404);
      if (current.author !== who) return c.json({ error: { message: "not your post", code: "forbidden" } }, 403);
      const { version, ...input } = p.data;
      const updated = await store.update(id, version, input);
      // Nothing matched and the row exists: someone saved since this edit began. Hand back what
      // is there now so the client can show the difference and retry from it.
      if (!updated) return c.json({ error: { message: "edited by someone else since you loaded it", code: "conflict" }, current: (await store.get(id))! }, 409);
      return c.json(updated);
    })
    .delete("/api/posts/:id", async (c) => {
      const who = await user(c); if (!who) return c.json({ error: { message: "sign in first", code: "unauthorized" } }, 401);
      const current = await store.get(Number(c.req.param("id")));
      if (!current) return c.json({ error: { message: "no such post", code: "not_found" } }, 404);
      if (current.author !== who) return c.json({ error: { message: "not your post", code: "forbidden" } }, 403);
      await store.remove(current.id);
      return c.body(null, 204);
    });

  async function user(c: Context): Promise<string | null> {
    const id = await getSignedCookie(c, SECRET(), COOKIE);
    return id ? (await store.session(id))?.name ?? null : null;
  }
  return app;
}

const app = createApp(pgStore());
export type AppType = typeof app;
export default app;
"##;

const FULLSTACK_STORE: &str = r##"import { db } from "./db";
import type { Post, PostInput } from "./schema";

export interface Store {
  createSession(name: string): Promise<string>;
  session(id: string): Promise<{ name: string } | null>;
  list(limit: number): Promise<Post[]>;
  get(id: number): Promise<Post | null>;
  create(input: PostInput, author: string): Promise<Post>;
  /// Update only if the row is still at `version`. Returns the new row, or `null` when nothing
  /// matched — the caller checks whether the row is gone or merely moved on.
  update(id: number, version: number, input: PostInput): Promise<Post | null>;
  remove(id: number): Promise<boolean>;
}

export function pgStore(): Store {
  return {
    createSession: async (name) => { const id = crypto.randomUUID(); await db`insert into sessions (id, name) values (${id}, ${name})`; return id; },
    session: async (id) => (await db<{ name: string }[]>`select name from sessions where id = ${id}`)[0] ?? null,
    list: (limit) => db<Post[]>`select id::int, title, body, author, version, created_at::text, updated_at::text from posts order by id desc limit ${limit}`,
    get: async (id) => (await db<Post[]>`select id::int, title, body, author, version, created_at::text, updated_at::text from posts where id = ${id}`)[0] ?? null,
    create: async (input, author) => (await db<Post[]>`insert into posts (title, body, author) values (${input.title}, ${input.body}, ${author})
      returning id::int, title, body, author, version, created_at::text, updated_at::text`)[0],
    // The optimistic lock is the WHERE clause: one statement, no read-then-write window.
    update: async (id, version, input) => (await db<Post[]>`update posts set title = ${input.title}, body = ${input.body}, version = version + 1, updated_at = now()
      where id = ${id} and version = ${version} returning id::int, title, body, author, version, created_at::text, updated_at::text`)[0] ?? null,
    remove: async (id) => (await db`delete from posts where id = ${id}`).count > 0,
  };
}

export function memoryStore(): Store {
  const sessions = new Map<string, string>(); const posts = new Map<number, Post>(); let next = 1;
  return {
    createSession: async (name) => { const id = crypto.randomUUID(); sessions.set(id, name); return id; },
    session: async (id) => { const name = sessions.get(id); return name ? { name } : null; },
    list: async (limit) => [...posts.values()].sort((a, b) => b.id - a.id).slice(0, limit),
    get: async (id) => posts.get(id) ?? null,
    create: async (input, author) => { const now = new Date().toISOString(); const p = { id: next++, ...input, author, version: 1, created_at: now, updated_at: now }; posts.set(p.id, p); return p; },
    update: async (id, version, input) => { const p = posts.get(id); if (!p || p.version !== version) return null; const q = { ...p, ...input, version: p.version + 1, updated_at: new Date().toISOString() }; posts.set(id, q); return q; },
    remove: async (id) => posts.delete(id),
  };
}
"##;

const FULLSTACK_SCHEMA: &str = r##"import { z } from "zod";

// The one place a post's shape is written down. The API validates request bodies with it and
// the frontend validates its form with the same object (imported as @backend/schema), so the
// two cannot drift: change a limit here and both sides change together.

export const postInput = z.object({
  title: z.string().trim().min(1, "title is required").max(200, "title is too long"),
  body: z.string().trim().max(5000, "body is too long").default(""),
});
export type PostInput = z.infer<typeof postInput>;

// An update carries the version it was edited from. The server compares it to the row's.
export const postUpdate = postInput.extend({ version: z.coerce.number().int().positive() });
export type PostUpdate = z.infer<typeof postUpdate>;

export const signIn = z.object({ name: z.string().trim().min(1, "name is required").max(40) });

export type Post = { id: number; title: string; body: string; author: string; version: number; created_at: string; updated_at: string };
"##;

const FULLSTACK_SEED: &str = r##"import { pgStore } from "./store";
import { db } from "./db";

// Sample rows so the first screen is not empty. Idempotent: run it twice, get the same rows.
const store = pgStore();
const have = await store.list(1);
if (have.length === 0) {
  await store.create({ title: "Welcome", body: "This post came from backend/src/seed.ts. Sign in and edit it." }, "seed");
  await store.create({ title: "How editing works", body: "Every save carries the version it edited. A stale save is refused with 409 and the current row." }, "seed");
  console.log("seeded 2 posts");
} else console.log("already seeded");
await db.end();
"##;

const FULLSTACK_TEST: &str = r##"import { describe, expect, test } from "bun:test";
import { createApp } from "./app";
import { memoryStore } from "./store";
import { postInput } from "./schema";

const json = (method: string, body: unknown, cookie?: string) => ({ method, headers: { "content-type": "application/json", ...(cookie ? { cookie } : {}) }, body: JSON.stringify(body) });

async function signIn(app: ReturnType<typeof createApp>, name: string) {
  const res = await app.request("/api/session", json("POST", { name }));
  expect(res.status).toBe(201);
  const raw = res.headers.get("set-cookie")!;
  expect(raw).toContain("HttpOnly");
  return raw.split(";")[0];
}

describe("posts", () => {
  test("writes need a session, and a tampered cookie is not one", async () => {
    const app = createApp(memoryStore());
    expect((await app.request("/api/posts", json("POST", { title: "x" }))).status).toBe(401);
    expect((await app.request("/api/session", json("POST", { name: " " }))).status).toBe(400);
    const cookie = await signIn(app, "ana");
    expect(await (await app.request("/api/session", { headers: { cookie } })).json()).toEqual({ name: "ana" });
    const forged = cookie.replace(/\.[^.]+$/, ".AAAA");
    expect((await app.request("/api/session", { headers: { cookie: forged } })).status).toBe(401);
    expect((await app.request("/api/posts", json("POST", { title: "x" }, forged))).status).toBe(401);
  });

  test("the shared schema validates on the server exactly as it does in the form", async () => {
    const app = createApp(memoryStore()); const cookie = await signIn(app, "ana");
    const bad = await app.request("/api/posts", json("POST", { title: "   ", body: "b" }, cookie));
    expect(bad.status).toBe(400);
    expect((await bad.json()).error.message).toBe(postInput.safeParse({ title: "   " }).error!.issues[0].message);
    const ok = await app.request("/api/posts", json("POST", { title: "  Hello  " }, cookie));
    expect(ok.status).toBe(201);
    expect(await ok.json()).toMatchObject({ id: 1, title: "Hello", body: "", author: "ana", version: 1 });
    expect((await (await app.request("/api/posts")).json()).posts.map((p: { id: number }) => p.id)).toEqual([1]);
  });

  test("a stale version is refused with 409 and the current row; a fresh one bumps the version", async () => {
    const app = createApp(memoryStore()); const cookie = await signIn(app, "ana");
    await app.request("/api/posts", json("POST", { title: "v1" }, cookie));
    // Two tabs load version 1. The first saves; the second's save is stale.
    const first = await app.request("/api/posts/1", json("PUT", { title: "from tab A", body: "", version: 1 }, cookie));
    expect(first.status).toBe(200);
    expect((await first.json()).version).toBe(2);
    const second = await app.request("/api/posts/1", json("PUT", { title: "from tab B", body: "", version: 1 }, cookie));
    expect(second.status).toBe(409);
    const body = await second.json();
    expect(body.error.code).toBe("conflict");
    expect(body.current).toMatchObject({ title: "from tab A", version: 2 });
    // Retrying from the current version succeeds.
    expect((await app.request("/api/posts/1", json("PUT", { title: "from tab B", body: "", version: 2 }, cookie))).status).toBe(200);
    expect((await app.request("/api/posts/1", json("PUT", { title: "x", body: "", version: "one" }, cookie))).status).toBe(400);
  });

  test("only the author may update or delete", async () => {
    const app = createApp(memoryStore()); const ana = await signIn(app, "ana"); const bob = await signIn(app, "bob");
    await app.request("/api/posts", json("POST", { title: "ana's" }, ana));
    expect((await app.request("/api/posts/1", json("PUT", { title: "bob's now", body: "", version: 1 }, bob))).status).toBe(403);
    expect((await app.request("/api/posts/1", { method: "DELETE", headers: { cookie: bob } })).status).toBe(403);
    expect((await app.request("/api/posts/1", { method: "DELETE", headers: { cookie: ana } })).status).toBe(204);
    expect((await app.request("/api/posts/1")).status).toBe(404);
    expect((await app.request("/api/posts/1", json("PUT", { title: "x", body: "", version: 1 }, ana))).status).toBe(404);
  });
});
"##;

const FULLSTACK_SQL: &str = r##"-- Server-side sessions: the cookie carries only this id (signed), the row carries who it is.
create table if not exists sessions (
  id text primary key,
  name text not null,
  created_at timestamptz not null default now()
);

-- The resource. `version` is the optimistic lock: every update says which version it edited,
-- and the row only changes if that is still the current one. Two people editing the same post
-- do not silently overwrite each other; the second one is told.
create table if not exists posts (
  id bigserial primary key,
  title text not null,
  body text not null default '',
  author text not null,
  version int not null default 1,
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now()
);
"##;

const FULLSTACK_PAGE: &str = r##"import { cookies } from "next/headers";
import { redirect } from "next/navigation";
import { revalidatePath } from "next/cache";
import { postInput, postUpdate, signIn, type Post } from "@backend/schema";

const API = process.env.API_URL ?? "http://127.0.0.1:8000";

// Server-rendered, forms posting to server actions, the API called from the server with the
// browser's session cookie forwarded. No client JavaScript is needed for any of it, and the form
// is validated with the same zod objects the API uses — one schema, two sides.

async function api(path: string, init: RequestInit = {}) {
  const sid = (await cookies()).get("sid");
  return fetch(`${API}${path}`, { ...init, cache: "no-store", headers: { "content-type": "application/json", ...(sid ? { cookie: `sid=${sid.value}` } : {}), ...init.headers } });
}

async function signInAction(form: FormData) {
  "use server";
  const p = signIn.safeParse({ name: form.get("name") });
  if (!p.success) redirect(`/?error=${encodeURIComponent(p.error.issues[0].message)}`);
  const res = await api("/api/session", { method: "POST", body: JSON.stringify(p.data) });
  // The API set the cookie on its response; carry it over to the browser on ours.
  const sid = /sid=([^;]+)/.exec(res.headers.get("set-cookie") ?? "")?.[1];
  if (sid) (await cookies()).set("sid", sid, { httpOnly: true, sameSite: "lax", path: "/" });
  revalidatePath("/");
}

async function createAction(form: FormData) {
  "use server";
  const p = postInput.safeParse({ title: form.get("title"), body: form.get("body") });
  if (!p.success) redirect(`/?error=${encodeURIComponent(p.error.issues[0].message)}`);
  const res = await api("/api/posts", { method: "POST", body: JSON.stringify(p.data) });
  if (!res.ok) redirect(`/?error=${encodeURIComponent((await res.json()).error.message)}`);
  revalidatePath("/");
}

async function updateAction(form: FormData) {
  "use server";
  const id = Number(form.get("id"));
  const p = postUpdate.safeParse({ title: form.get("title"), body: form.get("body"), version: form.get("version") });
  if (!p.success) redirect(`/?error=${encodeURIComponent(p.error.issues[0].message)}`);
  const res = await api(`/api/posts/${id}`, { method: "PUT", body: JSON.stringify(p.data) });
  if (res.status === 409) redirect(`/?error=${encodeURIComponent(`Post ${id} was edited by someone else — the row below is the current one; edit it again.`)}`);
  if (!res.ok) redirect(`/?error=${encodeURIComponent((await res.json()).error.message)}`);
  revalidatePath("/");
}

async function deleteAction(form: FormData) {
  "use server";
  await api(`/api/posts/${Number(form.get("id"))}`, { method: "DELETE" });
  revalidatePath("/");
}

export default async function Home({ searchParams }: { searchParams: Promise<{ error?: string }> }) {
  const { error } = await searchParams;
  const me = await api("/api/session");
  const name = me.ok ? ((await me.json()) as { name: string }).name : null;
  const posts = ((await (await api("/api/posts")).json()) as { posts: Post[] }).posts;
  return (
    <main>
      <h1>{{NAME}} — posts</h1>
      <p>
        Same thing from a shell: <code>{`curl -c c.txt -X POST localhost:8000/api/session -H 'content-type: application/json' -d '{"name":"cli"}'`}</code>{" "}
        then <code>{`curl -b c.txt -X POST localhost:8000/api/posts -H 'content-type: application/json' -d '{"title":"Hello","body":"from curl"}'`}</code>{" "}
        and <code>{`curl -b c.txt -X PUT localhost:8000/api/posts/1 -H 'content-type: application/json' -d '{"title":"Hello again","body":"","version":1}'`}</code> (a stale version gets 409).
      </p>
      {error && <p style={{ color: "crimson" }}>{error}</p>}
      {name ? (
        <form action={createAction}>
          <p>Signed in as <strong>{name}</strong>.</p>
          <p><input name="title" placeholder="title" required maxLength={200} /></p>
          <p><textarea name="body" placeholder="body" rows={3} maxLength={5000} style={{ width: "100%" }} /></p>
          <p><button type="submit">Create post</button></p>
        </form>
      ) : (
        <form action={signInAction}>
          <input name="name" placeholder="your name" required maxLength={40} /> <button type="submit">Sign in</button>
        </form>
      )}
      <table>
        <thead><tr><th>id</th><th>title</th><th>body</th><th>author</th><th>v</th><th>updated</th><th></th></tr></thead>
        <tbody>
          {posts.map((p) => (
            <tr key={p.id}>
              <td>{p.id}</td>
              {name === p.author ? (
                <>
                  <td>
                    <form action={updateAction} id={`edit-${p.id}`}>
                      <input type="hidden" name="id" value={p.id} />
                      <input type="hidden" name="version" value={p.version} />
                      <input name="title" defaultValue={p.title} required maxLength={200} />
                    </form>
                  </td>
                  <td><textarea name="body" form={`edit-${p.id}`} defaultValue={p.body} rows={2} maxLength={5000} /></td>
                </>
              ) : (
                <><td>{p.title}</td><td>{p.body}</td></>
              )}
              <td>{p.author}</td><td>{p.version}</td><td>{p.updated_at.slice(0, 19).replace("T", " ")}</td>
              <td>
                {name === p.author && (
                  <>
                    <button type="submit" form={`edit-${p.id}`}>Save</button>{" "}
                    <form action={deleteAction} style={{ display: "inline" }}><input type="hidden" name="id" value={p.id} /><button type="submit">Delete</button></form>
                  </>
                )}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
      {posts.length === 0 && <p>No posts yet.</p>}
    </main>
  );
}
"##;
