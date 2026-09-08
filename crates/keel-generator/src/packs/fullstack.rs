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
        ("CLAUDE.md", f(FULLSTACK_CLAUDE)),
        ("AGENTS.md", f(FULLSTACK_AGENTS)),
        ("README.md", f(FULLSTACK_README)),
        (
            ".claude/agents/resource-semantics.md",
            FULLSTACK_REVIEWER.into(),
        ),
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

const FULLSTACK_CLAUDE: &str = r##"# {{NAME}} — working agreement

{{NAME}} is a full-stack application: one resource (`posts`) with list, read, create, update and
delete; a server-side session behind a signed HttpOnly cookie; request shapes written once as zod
schemas and used by both the API and the form; and optimistic concurrency, so two people editing
the same row do not silently overwrite each other. It is modelled on the shape of a Rails
resource (a controller, a model with `lock_version`, a session) and a Next.js App Router page
with server actions. "Done" here means: every write needs a session; a stale save is refused with
409 and the current row; the form and the API disagree about nothing because they share
`schema.ts`; and the tests prove all of that without a database.

## Architecture

| File | Owns |
|---|---|
| `backend/src/app.ts` | The routes: `/api/session` (POST/GET/DELETE), `/api/posts` (GET/POST), `/api/posts/:id` (GET/PUT/DELETE). The `user(c)` helper that turns the cookie into a name. Exports `AppType`. |
| `backend/src/schema.ts` | `postInput`, `postUpdate` (= input + `version`), `signIn`, and the `Post` type. The single source of truth for shapes and limits. Imported by the frontend as `@backend/schema`. |
| `backend/src/store.ts` | `Store`: sessions and posts. `pgStore` (the optimistic lock is a `WHERE`), `memoryStore` (the tests). |
| `backend/src/seed.ts` | Two posts if the table is empty. |
| `backend/src/db.ts` | The `postgres` pool from `DATABASE_URL`. |
| `backend/src/migrate.ts` | Applies `backend/migrations/*.sql` once each. |
| `backend/migrations/0002_fullstack.sql` | `sessions`, `posts`. |
| `backend/src/app.test.ts` | Four tests on `memoryStore()`. |
| `frontend/app/page.tsx` | Server-rendered page; four server actions (`signInAction`, `createAction`, `updateAction`, `deleteAction`); forms validated with the same zod objects; errors carried in `?error=`. |
| `frontend/lib/api.ts` | `hc<AppType>` — the typed client, available to any client component you add. |

The request path for an edit, from a browser tab that loaded the page ten minutes ago:

1. The page rendered the row with `<input type="hidden" name="version" value={p.version}>` — the version the user is editing *from*.
2. Save posts the form to `updateAction` (a server action, on the Next.js server). It parses `{title, body, version}` with `postUpdate` — the same object the API will use — and redirects to `/?error=…` on a violation before any network call.
3. `api()` forwards the browser's `sid` cookie to `PUT /api/posts/:id`.
4. The API reads the signed cookie (`getSignedCookie` with `SESSION_SECRET`), looks the id up in `sessions`, and has a name or returns 401.
5. `postUpdate.safeParse` again on the API side — the boundary validates for itself, whatever the caller did.
6. `store.get(id)` → 404 if gone; `current.author !== who` → 403.
7. `store.update(id, version, input)` is one statement: `update posts set … version = version + 1 where id = $1 and version = $2 returning *`. No row matched and the post exists → 409 with `current` — the row as it is now.
8. The action redirects with the conflict message; the page re-renders with the current row and its new `version` in the hidden field, so the next save starts from the truth.

Data model:

| Table | Why the columns that matter |
|---|---|
| `sessions` | `id text primary key` is a random UUID; the cookie carries it signed, the row carries `name`. Nothing about the user lives in the cookie, so changing what a session means is a row change. No expiry column today. |
| `posts` | `version int default 1` is the optimistic lock (Rails's `lock_version`); `author text` is compared to the session's name for update and delete; `updated_at` is set by the update statement, not a trigger. `id bigserial` — the stack's UUIDv7 rule is not applied here; see Ceilings. |

## Invariants

1. **Every write needs a session, and a tampered cookie is not one.** `POST/PUT/DELETE /api/posts*` call `user(c)` first; `getSignedCookie` rejects a bad signature. Guard: `app.test.ts` "writes need a session, and a tampered cookie is not one".
2. **The shape is written once, in `schema.ts`, and both sides use it.** The API's 400 message *is* zod's message from the same object the form ran. A limit changed in one place changes in both. Guard: "the shared schema validates on the server exactly as it does in the form" compares the API's error to `postInput.safeParse(...).error.issues[0].message`.
3. **The optimistic lock is the `WHERE` clause.** `pgStore.update` is one statement with `and version = ${version}`; a read-then-compare-then-write in two statements has a window in which the second tab wins anyway. `memoryStore.update` mirrors it (`p.version !== version → null`). Guard: "a stale version is refused with 409 and the current row; a fresh one bumps the version".
4. **A 409 carries the current row.** The client can show a diff and retry from it; without the row, the user's only option is reload-and-retype. Guard: the same test asserts `body.current.version === 2`.
5. **The API distinguishes gone from moved on.** `update` returns `null` for both; the route checks `store.get(id)` first (404) and after (409). A change that returns 409 for a deleted post sends the user to edit nothing. Guard: "only the author may update or delete" asserts 404 on a PUT after the delete.
6. **Only the author updates or deletes.** Checked in the route with `current.author !== who`, after the 404 check and before the write. Guard: the same test — bob gets 403 on ana's post.
7. **The API validates for itself.** The server action validates first, but every route runs `safeParse` again; the API is a boundary that `curl` reaches without the form. Guard: the schema test posts an invalid body straight to the API.
8. **Cookies are `HttpOnly`, `SameSite=Lax`, `Secure` in production.** Set in `POST /api/session`; the frontend re-sets the same value with the same flags. Guard: `signIn` in the test asserts `HttpOnly` on the `set-cookie` header. `Secure` depends on `NODE_ENV=production` and is not tested.
9. **Trimming happens in the schema, not the route.** `title: z.string().trim()`; `"  Hello  "` is stored as `Hello` from the form and from curl alike. Guard: the schema test asserts `title: "Hello"`.
10. **Routes stay one chained expression.** `AppType` is inferred from it; the frontend's `hc` client and the `@backend/schema` import are the whole reason both halves are TypeScript. Guard: `docs/PRODUCTION.md` and the stack's own test.
11. **List is bounded.** `GET /api/posts?limit` is capped at 200, newest first. Guard: none automated; the reviewer checklist has it.

## Extending it

**Add a field to posts (say `tags: string[]`).** `0003_posts_tags.sql`: `alter table posts add column tags text[] not null default '{}'` — additive. `schema.ts`: add `tags` to `postInput` with its bound and to `Post`. `store.ts`: add the column to every `select`/`insert`/`update` in `pgStore`; `memoryStore` picks it up from `...input`. `page.tsx`: an input and a column. Test: the schema test's `toMatchObject` grows by one field, and a bad tag gets zod's message from both sides.

**Add a resource (say `comments` on a post).** `0003_comments.sql` with `post_id references posts(id) on delete cascade` and its own `version`. `schema.ts`: `commentInput`, `commentUpdate`, `Comment`. `store.ts`: five methods, both implementations. `app.ts`: routes under `/api/posts/:id/comments`, in the chain, same `user(c)` gate, same 401/403/404/409 ladder. `page.tsx`: a form under each post. Test: copy the four posts tests for comments; the conflict test is the one that matters.

**Real sign-in (passwords, OAuth).** Replace the body of `POST /api/session` — the identity check — and keep `store.createSession`. Author comparisons use the session's `name`; switch them to a user id when names stop being unique. The `auth` pack is the shape to copy. Test: a wrong password is 401 and no session row is created.

**Sign-out that invalidates the row.** `DELETE /api/session` only clears the cookie today; the row stays valid for anyone holding the old id. Add `store.deleteSession(id)` and call it. Test: the old cookie is 401 after sign-out.

**Session expiry.** `sessions.expires_at timestamptz` in a migration, set on create, checked in `store.session`. Test: a session past its time is 401.

**Pagination.** Replace `limit` with keyset on `(created_at, id)`: `?before=<created_at>,<id>`, still capped. Test: two pages do not overlap and the last page is short.

**A client component.** `import { api } from "@/lib/api"` and `api.api.posts.$get()` — typed from `AppType`. Cookies travel automatically on the same origin. Keep server actions for writes so the page works without JavaScript.

## Operating it

| Variable | Required | Meaning |
|---|---|---|
| `SESSION_SECRET` | **yes in production** | Signs the `sid` cookie. Default `dev-secret-change-me`: with it, anyone can forge a session id (they still need a row that exists — but they can create one). |
| `DATABASE_URL` | yes | Postgres. |
| `NODE_ENV` | production | `production` turns on the `Secure` cookie flag. |
| `PORT` | no | API port, default 8000. |
| `API_URL` | frontend | Where the Next.js server reaches the API; the server actions call it directly. |
| `REDIS_URL`, `SENTRY_DSN`, `POSTHOG_*` | no | From the stack; unused by this pack's code today. |

Replicas and what is where:

- Both halves are stateless. Sessions are rows, not memory, so any replica serves any cookie; the HPA runs 2–5 of the API. Nothing moves to Redis when you add replicas — the stack's rule that "session and rate-limit state live in Redis or a signed cookie" is met by Postgres here, which is fine at this size and a `sessions` cache in Redis later if the lookup shows up.
- The optimistic lock needs no coordination between replicas; it is a row-level `WHERE`.
- Pool: `db.ts` `max: 10` per replica.

Failure modes and what the user sees:

| Failure | Effect |
|---|---|
| `SESSION_SECRET` unset in production | Session ids can be forged; a guessed UUID is 2^122, so the practical attack is a created-then-reused session, but do not rely on that. Set it. |
| Postgres down | Every route 500s, including `GET /api/session`; the page shows the sign-in form to everyone (the `me` call is not `ok`). Readiness still says `db: "ok"`. |
| Two tabs edit one post | The second save gets 409; the page shows "edited by someone else — the row below is the current one; edit it again" and the current row. The user's typed text is lost — the form is server-rendered from the API's row. |
| Sign-out | Cookie cleared; the row lives on. An attacker with the old cookie value is still signed in. |
| Same name, two people | They are the same author. Names are not identities until real sign-in. |
| A huge body | Rejected at 5000 chars by the schema, on both sides, before it reaches Postgres. |

What to watch: 409 rate on `PUT /api/posts/:id` (high means people edit the same rows — fine — or the page shows stale versions — not fine); 401 on writes; `sessions` row count (it only grows); p95 of `GET /api/posts`.

## Ceilings

- **Identity is a name.** `POST /api/session {name}` with no check. Upgrade: the `auth` pack; compare authors by user id.
- **Sessions never expire and sign-out does not delete the row.** Upgrade: `expires_at` + `deleteSession`, both recipes above; a nightly `delete from sessions where expires_at < now()`.
- **`limit` pagination.** Newest 50 by default, 200 max, no cursor. Upgrade: keyset on `(created_at, id)` — the stack's rule.
- **`bigserial` ids**, not the UUIDv7 the stack rules ask for. Fine until ids are exposed and enumerable matters. Upgrade: `id uuid default gen_uuidv7()` in a new table, or keep serial and add a public `slug`.
- **The conflict UX loses the typed text.** The page is server-rendered; on 409 it shows the current row, not a merge. Upgrade: a client component that keeps the draft and shows both.
- **No `Idempotency-Key` on `POST /api/posts`.** A retried create makes two posts. Upgrade: the stack's rule — key, request hash, stored response, 24 h — as a middleware in `app.ts` backed by Redis.
- **No rate limit on `/api/session`.** Upgrade: token bucket per IP in Redis.
- **Readiness does not check Postgres.** `/api/health/ready` returns a literal.
- **Errors travel in the query string** (`?error=`). Upgrade: `useActionState` in a client component when the page grows past one form.

The stack rules — gate, typed seam, production checklist, deploy — are in `docs/PRODUCTION.md`. They apply.
"##;

const FULLSTACK_AGENTS: &str = r##"# {{NAME}} — for agents

`CLAUDE.md` has the rules and the architecture. This is how to run and test it.

## Run

    make demo         # postgres + redis, migrate, seed 2 posts, API on :8000, app on :3000
    make dev          # just postgres + redis
    make backend      # API with reload on :8000
    make frontend     # Next.js on :3000, /api proxied to :8000
    make check        # the gate: typecheck both halves, run backend tests

No worker. `bun run seed` (from `backend/`) inserts two posts when the table is empty and is
otherwise a no-op.

## Routes, with bodies

Sign in (a name; the cookie jar keeps the signed `sid`):

    curl -s -c c.txt -X POST localhost:8000/api/session -H 'content-type: application/json' -d '{"name":"ana"}'
    # 201 {"name":"ana"}
    # {"name":" "}: 400 {"error":{"message":"name is required","code":"invalid"}}
    curl -s -b c.txt localhost:8000/api/session
    # {"name":"ana"}       — without the cookie: 401 {"error":{"message":"not signed in","code":"unauthorized"}}
    curl -s -b c.txt -X DELETE localhost:8000/api/session -i | head -1
    # HTTP/1.1 204        — clears the cookie; the session row stays (see Ceilings)

Posts:

    curl -s -b c.txt -X POST localhost:8000/api/posts -H 'content-type: application/json' -d '{"title":"  Hello  ","body":"first"}'
    # 201 {"id":3,"title":"Hello","body":"first","author":"ana","version":1,"created_at":"…","updated_at":"…"}
    # without a session: 401 {"error":{"message":"sign in first","code":"unauthorized"}}
    # {"title":"   "}: 400 {"error":{"message":"title is required","code":"invalid","issues":{"title":["title is required"]}}}

    curl -s 'localhost:8000/api/posts?limit=2'
    # {"posts":[{"id":3,…},{"id":2,…}]}          — newest first, limit capped at 200
    curl -s localhost:8000/api/posts/3
    # {"id":3,…}                                  — unknown id: 404 {"error":{"message":"no such post","code":"not_found"}}

Update with the version you edited from:

    curl -s -b c.txt -X PUT localhost:8000/api/posts/3 -H 'content-type: application/json' -d '{"title":"Hello again","body":"first","version":1}'
    # 200 {"id":3,"title":"Hello again",…,"version":2,…}
    curl -s -b c.txt -X PUT localhost:8000/api/posts/3 -H 'content-type: application/json' -d '{"title":"From the other tab","body":"","version":1}'
    # 409 {"error":{"message":"edited by someone else since you loaded it","code":"conflict"},"current":{"id":3,"title":"Hello again",…,"version":2,…}}
    # "version":"one": 400; someone else's post: 403 {"error":{"message":"not your post","code":"forbidden"}}

Delete:

    curl -s -b c.txt -X DELETE localhost:8000/api/posts/3 -i | head -1
    # HTTP/1.1 204        — then GET /api/posts/3 is 404

Health: `GET /api/health` → `{"status":"ok"}`; `GET /api/health/ready` → `{"status":"ok","db":"ok"}` (a literal today).

## How the tests are built

`backend/src/app.test.ts` runs on `memoryStore()` — no Postgres — through `app.request`. The
`signIn(app, name)` helper posts to `/api/session`, asserts `HttpOnly` on the cookie, and returns
the `sid=…` pair to send back; `json(method, body, cookie?)` builds the request. Each test makes
its own app and store, so there is no shared state and no ordering.

What is covered: the session gate and a forged signature; the shared schema (the API's 400
message is compared to `postInput.safeParse` run locally — the same object, so they cannot
diverge); the optimistic lock (two tabs at version 1, the second gets 409 with `current`, a retry
from version 2 succeeds, a non-numeric version is 400); and authorship (403 for another user,
204 then 404 for the author). `pgStore`'s SQL — the `WHERE … and version =` — is exercised by
`make demo`, not by `bun test`.

To add a test: a `test(...)` in `describe("posts")`, `createApp(memoryStore())`, `signIn` for a
cookie, and assert on status *and* body (`toMatchObject` for rows, `toEqual` for error shapes).
For a new resource, the conflict test is the one to write first; for a new field, extend the
schema test so the form-side and API-side messages are still compared.
"##;

const FULLSTACK_README: &str = r##"# {{NAME}}

A full-stack application with a real backbone: a typed API, a server-rendered page with forms
that work without JavaScript, one schema shared by both, sessions in the database, and optimistic
concurrency so two people editing the same row are told rather than overwritten.

## What you get

- `posts`: list, read, create, update, delete — `GET/POST /api/posts`, `GET/PUT/DELETE /api/posts/:id`.
- Sessions: `POST/GET/DELETE /api/session`, a signed `HttpOnly` `SameSite=Lax` cookie carrying only a random id; the row carries who it is.
- One schema (`backend/src/schema.ts`, zod) validating the form on the Next.js server *and* the request at the API, so limits and messages cannot drift.
- Optimistic concurrency: every save carries the `version` it edited; a stale save is refused with 409 and the current row.
- Authorship: only the author may update or delete (403 otherwise).
- A page built from server actions and plain forms: sign in, create, edit inline, delete.
- Tests without a database, a typed `hc<AppType>` client, Docker Compose locally, kustomize manifests for a cluster.

## Five minutes

    make demo

Open http://localhost:3000, sign in as `ana`, edit the "Welcome" post. Or from a shell:

    curl -s -c c.txt -X POST localhost:8000/api/session -H 'content-type: application/json' -d '{"name":"ana"}'
    # {"name":"ana"}

    curl -s -b c.txt -X POST localhost:8000/api/posts -H 'content-type: application/json' -d '{"title":"Hello","body":"from curl"}'
    # {"id":3,"title":"Hello","body":"from curl","author":"ana","version":1,…}

    curl -s -b c.txt -X PUT localhost:8000/api/posts/3 -H 'content-type: application/json' -d '{"title":"Hello again","body":"","version":1}'
    # {"id":3,"title":"Hello again",…,"version":2,…}

    curl -s -b c.txt -X PUT localhost:8000/api/posts/3 -H 'content-type: application/json' -d '{"title":"Stale","body":"","version":1}'
    # {"error":{"message":"edited by someone else since you loaded it","code":"conflict"},"current":{…,"version":2,…}}   ← 409

## API

| Method | Path | Auth | What |
|---|---|---|---|
| POST | `/api/session` | none | `{name}` → 201, sets `sid`. |
| GET | `/api/session` | cookie | `{name}` or 401. |
| DELETE | `/api/session` | none | Clears the cookie. 204. |
| GET | `/api/posts?limit` | none | Newest first, default 50, max 200. |
| GET | `/api/posts/:id` | none | One post or 404. |
| POST | `/api/posts` | cookie | `{title, body?}` → 201 with the row. |
| PUT | `/api/posts/:id` | cookie, author | `{title, body, version}` → 200 row; 409 + `current` if stale; 403; 404. |
| DELETE | `/api/posts/:id` | cookie, author | 204; 403; 404. |
| GET | `/api/health`, `/api/health/ready` | none | Liveness; readiness (does not check the database yet). |

Errors are always `{error: {message, code}}` with `code` one of `invalid`, `unauthorized`, `forbidden`, `not_found`, `conflict`; validation errors add `issues`.

## Compared with Rails (and a Next.js full-stack app)

Same shapes, so the habits carry over:

- A resource with the seven-ish actions, a session, and `lock_version`-style optimistic locking with a conflict response.
- Validation declared once on the model (`schema.ts` is the `validates` block) and run on every write.
- Migrations as ordered files; a seed; a `make check` gate.
- Server-rendered pages posting forms — the Rails default, and the App Router's.

Better here, for a team that owns one application:

- **The seam is a type.** The frontend imports the API's route types and the request schemas; a field the API stops returning is a compile error in the page, not a runtime `undefined`.
- **The same validation object runs in the form and at the API** — not a re-implementation in JavaScript that drifts from the model.
- **Tests run in milliseconds without a database**, including the 409 path, through the real routes.
- **The concurrency answer includes the current row**, so a client can show a diff; Rails's `StaleObjectError` is an exception you catch.
- Two containers with rolling updates, probes, an HPA and a migrate init container come with it; no Puma tuning, no asset pipeline.

Not here yet — Rails has these and this does not:

- **Authentication.** Sign-in is a name. No passwords, no OAuth, no Devise; the `auth` pack is the next step.
- **An ORM, associations, generators.** Queries are SQL in `store.ts`; a new resource is written by hand (the recipe is in `CLAUDE.md`).
- **Session expiry and revocation.** Sign-out clears the cookie; the row remains valid.
- **Pagination.** `limit` only; keyset is a recipe.
- **CSRF tokens.** `SameSite=Lax` and Next's server-action origin check are the protection; there is no per-form token.
- **Idempotency keys, rate limits, background jobs, mailers, i18n, an admin, file uploads, caching, flash messages beyond `?error=`.**
- **A conflict UI that keeps your draft.** On 409 the page shows the current row; the text you typed is gone.

## Production

- **`SESSION_SECRET` must be set** wherever the app is reachable; the default is public. `NODE_ENV=production` turns on the `Secure` cookie flag.
- **Environments.** `backend/.env` → `backend/.env.age` → `k8s/secrets.yaml` via `make k8s-secrets ENV=dev|prod`.
- **Scaling.** Both halves stateless; sessions and the lock are rows. The HPA runs 2–5 API replicas; pool `max: 10` each.
- **Probes.** `/api/health` liveness, `/api/health/ready` readiness (make it check Postgres before relying on it).
- **Migrations.** `backend/migrations/`, applied by the init container before each rollout. Add columns with defaults; new shapes are new tables.
- **Secrets.** `DATABASE_URL`, `SESSION_SECRET`.
- **What pages.** 5xx on any `/api/posts*` route; 401 on `GET /api/session` from signed-in users (a secret rotated without a plan); `sessions` growing without bound.

## Roadmap

1. Real sign-in (`auth` pack) and authorship by user id.
2. Session `expires_at`, `deleteSession` on sign-out, a nightly prune.
3. Keyset pagination on `(created_at, id)`.
4. `Idempotency-Key` on `POST /api/posts`; a rate limit on `/api/session`.
5. A client component for editing that keeps the draft on 409 and shows both versions.
6. Readiness that checks Postgres.
"##;

const FULLSTACK_REVIEWER: &str = r##"---
name: resource-semantics
description: Run on any change to app.ts, schema.ts, store.ts, page.tsx's server actions, or a migration. Reads the change as the engineer who will field "my edit disappeared" and "someone edited my post", and reports what breaks the session gate, the shared schema, the optimistic lock, or authorship.
tools: Read, Grep, Glob, Bash
---

You review a CRUD change. The bugs here are the boring ones that ship every week: a route that
forgot the session check, a field validated on one side only, a write that no longer names its
version.

Report each finding as `path:line — what breaks — the request that shows it — the fix`.

Check:
1. Every mutating route (`POST`, `PUT`, `DELETE` on anything but `/api/session`) calls `user(c)`
   first and returns 401 with `code: "unauthorized"` when it is null. Grep `.post(`, `.put(`,
   `.delete(` in `backend/src/app.ts` and read each one.
2. The 404 → 403 → 409 order on update and delete: existence, then authorship, then the
   version. A 403 before the 404 leaks which ids exist to non-authors; a 409 before the 403
   lets anyone probe a row's version.
3. `store.update` in `pgStore` is one statement with `where id = … and version = …`, and the
   memory store returns `null` on a version mismatch. Two statements (`select` then `update`)
   reopen the race the lock exists to close.
4. The 409 body carries `current` (a fresh `store.get`), and the code is `conflict`.
5. Every shape is in `backend/src/schema.ts` and used on both sides: the API's `safeParse` and
   the server action's `safeParse` reference the same export. A `z.object` declared inline in
   `app.ts` or `page.tsx` is the drift the design exists to prevent.
6. Bounds on every string in the schema (`max`), `trim()` where whitespace is not meaningful,
   `z.coerce.number().int().positive()` for anything that arrives from a form as text.
7. Cookie flags in `POST /api/session` and in `signInAction`: `httpOnly`, `sameSite: "Lax"`,
   `path: "/"`, `secure` in production; the same name (`sid`) on both. A frontend that sets
   the cookie with different flags than the API silently downgrades it.
8. `SESSION_SECRET` is read through `SECRET()` only; no second default appears.
9. Authorship compares `current.author` to the *session's* name, never to a name from the
   request body.
10. `GET /api/posts` keeps a cap (`Math.min(…, 200)`) and an `order by`. A list without a
    bound is a full-table scan on the first busy day.
11. Migrations: additive during a rollout (a new column has a default), `posts.version` stays
    `int not null default 1`, and `0002` is not edited after it has shipped.
12. Server actions redirect on every failure path (`redirect("/?error=…")`) rather than
    throwing — a thrown error in a server action is a blank 500 page.
13. The typed seam: routes are still one chained expression, and `frontend/app/page.tsx`
    imports shapes from `@backend/schema`, not from a copy.
14. The tests: a new route or field has a test on `memoryStore()` that asserts status and
    body; a conflict path for any new resource with a `version`.

End with one line: `resource-semantics: N findings`, and if 0, what you checked.
"##;
