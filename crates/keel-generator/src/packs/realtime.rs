//! realtime: rooms, history and presence over SSE, like Centrifugo ══════════════════════

pub fn files(name: &str) -> Vec<(&'static str, String)> {
    let f = |s: &str| s.replace("{{NAME}}", name);
    vec![
        ("backend/src/app.ts", REALTIME_APP.into()),
        ("backend/src/store.ts", REALTIME_STORE.into()),
        ("backend/src/app.test.ts", REALTIME_TEST.into()),
        ("backend/migrations/0002_realtime.sql", REALTIME_SQL.into()),
        ("frontend/app/page.tsx", f(REALTIME_PAGE)),
        ("CLAUDE.md", f(REALTIME_CLAUDE)),
        ("AGENTS.md", f(REALTIME_AGENTS)),
        ("README.md", f(REALTIME_README)),
        (
            ".claude/agents/stream-semantics.md",
            REALTIME_REVIEWER.into(),
        ),
    ]
}

const REALTIME_APP: &str = r##"import { Hono } from "hono";
import { streamSSE } from "hono/streaming";
import { getSignedCookie, setSignedCookie } from "hono/cookie";
import { createHmac, timingSafeEqual } from "node:crypto";
import { z } from "zod";
import { pgStore, type Store, type Publication } from "./store";

// Centrifugo's surface, on SSE instead of WebSocket: a channel ("room") per topic, a
// subscription token (HS256 JWT with `sub`, `channel`, `exp`) minted from the session by an HTTP
// route, publications with a gapless per-room offset, history with `since` for recovery,
// presence with TTL heartbeats, and a slow client disconnected rather than buffered forever.
// SSE because @hono/node-server has no WebSocket upgrade without another dependency, and every
// browser, curl and nginx already speaks it; sending is a POST, which is how Centrifugo's
// HTTP API works anyway. Fan-out across replicas is the store's job (Postgres LISTEN/NOTIFY).

const SECRET = () => process.env.SESSION_SECRET ?? "dev-secret-change-me";
const TOKEN_TTL_S = 15 * 60;
export const PRESENCE_TTL_MS = 30_000;
const HEARTBEAT_MS = 10_000;
const QUEUE_MAX = 256; // Centrifugo's client_queue_max_size: past this the client is too slow to keep.
const ROOM = z.string().regex(/^[\w:.-]{1,64}$/);

const b64 = (s: string | Buffer) => Buffer.from(s).toString("base64url");
const sign = (input: string, secret: string) => createHmac("sha256", secret).update(input).digest("base64url");

// A subscription token: proof that this session may join this room, valid for a short while.
// The room is in the token, so a token for "lobby" opens nothing else.
export function mintToken(sub: string, channel: string, exp: number, secret = SECRET()): string {
  const input = `${b64(JSON.stringify({ alg: "HS256", typ: "JWT" }))}.${b64(JSON.stringify({ sub, channel, exp }))}`;
  return `${input}.${sign(input, secret)}`;
}
export function verifyToken(token: string | undefined, channel: string, now: Date, secret = SECRET()): { sub: string } | null {
  const parts = (token ?? "").split(".");
  if (parts.length !== 3) return null;
  const want = sign(`${parts[0]}.${parts[1]}`, secret);
  if (want.length !== parts[2].length || !timingSafeEqual(Buffer.from(want), Buffer.from(parts[2]))) return null;
  try {
    const claims = JSON.parse(Buffer.from(parts[1], "base64url").toString());
    if (claims.channel !== channel || typeof claims.sub !== "string" || claims.exp * 1000 <= now.getTime()) return null;
    return { sub: claims.sub };
  } catch { return null; }
}

export function createApp(store: Store, clock: () => Date = () => new Date()) {
  const bearer = (c: { req: { header(n: string): string | undefined; query(n: string): string | undefined } }) =>
    c.req.header("authorization")?.replace(/^Bearer /, "") ?? c.req.query("token");

  const app = new Hono()
    .get("/api/health", (c) => c.json({ status: "ok" }))
    .get("/api/health/ready", (c) => c.json({ status: "ok", db: "ok" as const }))

    // The session. ponytail: a name in a signed cookie stands in for sign-in; put the auth pack's
    // session here and the rest of the file does not change.
    .post("/api/session", async (c) => {
      const p = z.object({ user: z.string().regex(/^[\w.-]{1,40}$/) }).safeParse(await c.req.json().catch(() => null));
      if (!p.success) return c.json({ error: { message: "user required", code: "invalid" } }, 400);
      await setSignedCookie(c, "session", p.data.user, SECRET(), { httpOnly: true, sameSite: "Lax", path: "/" });
      return c.json({ user: p.data.user });
    })

    // Mint a join token from the session. This is where an app decides who may enter which room.
    .post("/api/rooms/:room/token", async (c) => {
      const room = ROOM.safeParse(c.req.param("room"));
      const user = await getSignedCookie(c, SECRET(), "session");
      if (!user) return c.json({ error: { message: "no session", code: "unauthorized" } }, 401);
      if (!room.success) return c.json({ error: { message: "bad room name", code: "invalid" } }, 400);
      const exp = Math.floor(clock().getTime() / 1000) + TOKEN_TTL_S;
      return c.json({ token: mintToken(user, room.data, exp), channel: room.data, exp });
    })

    // Publish: append to the log, then fan out. The offset comes back so a sender can dedupe its own echo.
    .post("/api/rooms/:room/publish", async (c) => {
      const room = c.req.param("room");
      const who = verifyToken(bearer(c), room, clock());
      if (!who) return c.json({ error: { message: "bad or expired token", code: "unauthorized" } }, 401);
      const raw = await c.req.text();
      if (raw.length > 4096) return c.json({ error: { message: "message over 4 KB", code: "too_large" } }, 413);
      let body: unknown; try { body = JSON.parse(raw); } catch { body = null; }
      if (!body || typeof body !== "object" || !("data" in body)) return c.json({ error: { message: "data required", code: "invalid" } }, 400);
      const pub = await store.append(room, who.sub, (body as { data: unknown }).data);
      await store.publish(room, pub);
      return c.json({ offset: pub.offset });
    })

    .get("/api/rooms/:room/history", async (c) => {
      const room = c.req.param("room");
      if (!verifyToken(bearer(c), room, clock())) return c.json({ error: { message: "bad or expired token", code: "unauthorized" } }, 401);
      return c.json(await store.history(room, Number(c.req.query("since") ?? 0), Math.min(Number(c.req.query("limit") ?? 100), 500)));
    })
    .get("/api/rooms/:room/presence", async (c) => {
      const room = c.req.param("room");
      if (!verifyToken(bearer(c), room, clock())) return c.json({ error: { message: "bad or expired token", code: "unauthorized" } }, 401);
      return c.json({ clients: await store.presence(room, clock(), PRESENCE_TTL_MS) });
    })

    // The live stream. `since` is the last offset the client saw: everything after it is replayed
    // from history, then live publications follow, in order, with no gap and no repeat — the
    // subscription is opened before history is read, and anything already delivered is dropped.
    .get("/api/rooms/:room/events", async (c) => {
      const room = c.req.param("room");
      const who = verifyToken(bearer(c), room, clock());
      if (!who) return c.json({ error: { message: "bad or expired token", code: "unauthorized" } }, 401);
      const since = Number(c.req.query("since") ?? 0);
      const conn = crypto.randomUUID();
      return streamSSE(c, async (s) => {
        let last = since; let open = true; let wake = () => {};
        const queue: Publication[] = [];
        const unsubscribe = await store.subscribe(room, (p) => { queue.push(p); wake(); });
        s.onAbort(() => { open = false; wake(); });
        try {
          await store.touch(room, who.sub, conn, clock());
          const h = await store.history(room, since, 500);
          // A cursor older than what is kept cannot be recovered exactly; say so, like Centrifugo's `recovered: false`.
          const recovered = since === 0 || h.publications.length === 0 || h.publications[0].offset === since + 1;
          await s.writeSSE({ event: "subscribed", data: JSON.stringify({ channel: room, conn, top: h.top, recovered }) });
          for (const p of h.publications) { await s.writeSSE({ event: "publication", id: String(p.offset), data: JSON.stringify(p) }); last = p.offset; }
          let beat = clock().getTime();
          while (open) {
            if (queue.length > QUEUE_MAX) { await s.writeSSE({ event: "disconnect", data: JSON.stringify({ reason: "slow", last }) }); break; }
            const p = queue.shift();
            if (p) { if (p.offset > last) { await s.writeSSE({ event: "publication", id: String(p.offset), data: JSON.stringify(p) }); last = p.offset; } continue; }
            const now = clock().getTime();
            if (now - beat >= HEARTBEAT_MS) { beat = now; await store.touch(room, who.sub, conn, clock()); await s.writeSSE({ event: "ping", data: "" }); }
            await new Promise<void>((r) => { wake = r; setTimeout(r, HEARTBEAT_MS).unref(); });
          }
        } finally {
          await unsubscribe();
          await store.leave(room, conn);
        }
      });
    });
  return app;
}

const app = createApp(pgStore());
export type AppType = typeof app;
export default app;
"##;

const REALTIME_STORE: &str = r##"import { db } from "./db";

export type Publication = { offset: number; client: string; data: unknown; created_at: string };
export type Presence = { client: string; conns: number; seen_at: string };

export interface Store {
  /// Append to a room's log. Offsets are a gapless per-room sequence — the cursor clients resume from.
  append(room: string, client: string, data: unknown): Promise<Publication>;
  /// Messages with offset > since, oldest first, at most `limit`. `top` is the room's latest offset.
  history(room: string, since: number, limit: number): Promise<{ publications: Publication[]; top: number }>;
  /// Fan-out across replicas: every subscriber on every replica sees every publication once.
  subscribe(room: string, fn: (p: Publication) => void): Promise<() => Promise<void>>;
  publish(room: string, p: Publication): Promise<void>;
  /// Presence is a heartbeat, not a membership list: `touch` on connect and every few seconds,
  /// `leave` on a clean close, and anything older than `ttl` is a replica that died.
  touch(room: string, client: string, conn: string, now: Date): Promise<void>;
  leave(room: string, conn: string): Promise<void>;
  presence(room: string, now: Date, ttlMs: number): Promise<Presence[]>;
}

const HISTORY_MAX = 1000;

export function pgStore(): Store {
  return {
    append: (room, client, data) => db.begin(async (tx) => {
      // Lock the room row; the next offset is read and written under it, so two replicas cannot
      // both take offset N.
      const [r] = await tx<{ top: string }[]>`insert into rooms (name, top) values (${room}, 1)
        on conflict (name) do update set top = rooms.top + 1 returning top`;
      const offset = Number(r.top);
      const [m] = await tx<{ created_at: string }[]>`insert into messages (room, "offset", client, data) values (${room}, ${offset}, ${client}, ${JSON.stringify(data)}) returning created_at::text`;
      await tx`delete from messages where room = ${room} and "offset" <= ${offset - HISTORY_MAX}`;
      return { offset, client, data, created_at: m.created_at };
    }),
    history: async (room, since, limit) => {
      const top = Number((await db<{ top: string }[]>`select top from rooms where name = ${room}`)[0]?.top ?? 0);
      const publications = await db<Publication[]>`select "offset"::int, client, data, created_at::text from messages
        where room = ${room} and "offset" > ${since} order by "offset" limit ${limit}`;
      return { publications, top };
    },
    // Postgres LISTEN/NOTIFY: no broker to run, and it is already the durable store every replica
    // shares. The payload cap is 8000 bytes, which is why message data is capped at 4 KB (app.ts).
    // ponytail: move to Redis pub/sub when a room fans out to thousands of connections per replica.
    subscribe: async (room, fn) => {
      const l = await db.listen(`room:${room}`, (payload) => fn(JSON.parse(payload)));
      return () => l.unlisten();
    },
    publish: async (room, p) => { await db.notify(`room:${room}`, JSON.stringify(p)); },
    touch: async (room, client, conn, now) => { await db`insert into presence (room, client, conn, seen_at) values (${room}, ${client}, ${conn}, ${now})
      on conflict (room, conn) do update set seen_at = excluded.seen_at`; },
    leave: async (room, conn) => { await db`delete from presence where room = ${room} and conn = ${conn}`; },
    presence: (room, now, ttl) => db<Presence[]>`select client, count(*)::int as conns, max(seen_at)::text as seen_at from presence
      where room = ${room} and seen_at > ${new Date(now.getTime() - ttl)} group by client order by client`,
  };
}

export function memoryStore(): Store {
  const logs = new Map<string, Publication[]>(); const subs = new Map<string, Set<(p: Publication) => void>>();
  const pres = new Map<string, { client: string; seen: number }>();
  const log = (room: string) => { let l = logs.get(room); if (!l) { l = []; logs.set(room, l); } return l; };
  return {
    append: async (room, client, data) => { const l = log(room); const p = { offset: (l.at(-1)?.offset ?? 0) + 1, client, data, created_at: new Date().toISOString() }; l.push(p); if (l.length > HISTORY_MAX) l.shift(); return p; },
    history: async (room, since, limit) => { const l = log(room); return { publications: l.filter((p) => p.offset > since).slice(0, limit), top: l.at(-1)?.offset ?? 0 }; },
    subscribe: async (room, fn) => { let s = subs.get(room); if (!s) { s = new Set(); subs.set(room, s); } s.add(fn); return async () => { s!.delete(fn); }; },
    publish: async (room, p) => { for (const fn of subs.get(room) ?? []) fn(p); },
    touch: async (room, client, conn, now) => { pres.set(`${room}\n${conn}`, { client, seen: now.getTime() }); },
    leave: async (room, conn) => { pres.delete(`${room}\n${conn}`); },
    presence: async (room, now, ttl) => {
      const by = new Map<string, Presence>();
      for (const [k, v] of pres) {
        if (!k.startsWith(room + "\n") || v.seen <= now.getTime() - ttl) continue;
        const cur = by.get(v.client) ?? { client: v.client, conns: 0, seen_at: new Date(0).toISOString() };
        cur.conns++; if (new Date(v.seen).toISOString() > cur.seen_at) cur.seen_at = new Date(v.seen).toISOString();
        by.set(v.client, cur);
      }
      return [...by.values()].sort((a, b) => a.client.localeCompare(b.client));
    },
  };
}
"##;

const REALTIME_TEST: &str = r##"import { describe, expect, test } from "bun:test";
import { createApp, mintToken, verifyToken, PRESENCE_TTL_MS } from "./app";
import { memoryStore, type Store } from "./store";

const T0 = new Date("2026-03-10T12:00:00Z");
const json = (body: unknown, headers: Record<string, string> = {}) => ({ method: "POST", headers: { "content-type": "application/json", ...headers }, body: JSON.stringify(body) });

async function session(app: ReturnType<typeof createApp>, user: string, room = "lobby") {
  const res = await app.request("/api/session", json({ user }));
  const cookie = res.headers.get("set-cookie")!.split(";")[0];
  const tok = await app.request(`/api/rooms/${room}/token`, { method: "POST", headers: { cookie } });
  return { cookie, token: (await tok.json()).token as string };
}

// Read SSE events from a live stream. `until` decides when we have seen enough; the request is
// then aborted the way a browser closing the tab would.
async function events(app: ReturnType<typeof createApp>, path: string, token: string, until: (evs: { event: string; data: string }[]) => boolean, during?: () => Promise<void>) {
  const ctl = new AbortController();
  const res = await app.request(path, { headers: { authorization: `Bearer ${token}` }, signal: ctl.signal });
  expect(res.status).toBe(200);
  expect(res.headers.get("content-type")).toContain("text/event-stream");
  const reader = res.body!.getReader(); const dec = new TextDecoder(); let buf = ""; const out: { event: string; data: string }[] = [];
  let kicked = false;
  for (;;) {
    const { value, done } = await reader.read(); if (done) break;
    buf += dec.decode(value, { stream: true });
    let i; while ((i = buf.indexOf("\n\n")) >= 0) {
      const block = buf.slice(0, i); buf = buf.slice(i + 2);
      const ev = /event: (\w+)/.exec(block)?.[1]; const data = /data: (.*)/.exec(block)?.[1] ?? "";
      if (ev) out.push({ event: ev, data });
    }
    if (!kicked && during) { kicked = true; await during(); }
    if (until(out)) { ctl.abort(); await reader.cancel(); break; }
  }
  await new Promise((r) => setTimeout(r, 20)); // let the handler's cleanup (unsubscribe, leave) run
  return out;
}

describe("realtime", () => {
  test("a join token comes from the session, is bound to one room, and expires", async () => {
    const app = createApp(memoryStore(), () => T0);
    expect((await app.request("/api/rooms/lobby/token", { method: "POST" })).status).toBe(401);
    const { token } = await session(app, "ana");
    expect(verifyToken(token, "lobby", T0)).toEqual({ sub: "ana" });
    expect(verifyToken(token, "other", T0)).toBeNull();
    expect(verifyToken(token, "lobby", new Date(T0.getTime() + 16 * 60_000))).toBeNull();
    expect(verifyToken(token.slice(0, -2) + "xx", "lobby", T0)).toBeNull();
    expect(verifyToken(mintToken("ana", "lobby", 9e9, "other-secret"), "lobby", T0)).toBeNull();
    expect((await app.request("/api/rooms/lobby/publish", json({ data: 1 }))).status).toBe(401);
  });

  test("publications get gapless offsets, history resumes from a cursor, size is capped", async () => {
    const app = createApp(memoryStore(), () => T0);
    const { token } = await session(app, "ana");
    const auth = { authorization: `Bearer ${token}` };
    for (const i of [1, 2, 3]) expect(await (await app.request("/api/rooms/lobby/publish", json({ data: { i } }, auth))).json()).toEqual({ offset: i });
    const h = await (await app.request("/api/rooms/lobby/history?since=1", { headers: auth })).json();
    expect(h.top).toBe(3);
    expect(h.publications.map((p: { offset: number; client: string }) => [p.offset, p.client])).toEqual([[2, "ana"], [3, "ana"]]);
    expect((await app.request("/api/rooms/lobby/publish", json({ data: "x".repeat(5000) }, auth))).status).toBe(413);
    expect((await app.request("/api/rooms/lobby/publish", json({ nope: 1 }, auth))).status).toBe(400);
  });

  test("the stream replays after the cursor, then delivers live, in order and without repeats", async () => {
    const store = memoryStore(); const app = createApp(store, () => T0);
    const { token } = await session(app, "ana");
    const auth = { authorization: `Bearer ${token}` };
    for (const i of [1, 2, 3]) await app.request("/api/rooms/lobby/publish", json({ data: { i } }, auth));
    const got = await events(app, "/api/rooms/lobby/events?since=1", token, (e) => e.filter((x) => x.event === "publication").length >= 3, async () => {
      // A publication arriving while the client is connected, and a replica echoing an old one.
      await app.request("/api/rooms/lobby/publish", json({ data: { i: 4 } }, auth));
      await store.publish("lobby", { offset: 2, client: "ana", data: { i: 2 }, created_at: "" });
    });
    expect(got[0].event).toBe("subscribed");
    expect(JSON.parse(got[0].data)).toMatchObject({ channel: "lobby", top: 3, recovered: true });
    expect(got.filter((e) => e.event === "publication").map((e) => JSON.parse(e.data).offset)).toEqual([2, 3, 4]);
  });

  test("a cursor older than the retained history is reported as not recovered", async () => {
    const store = memoryStore(); const app = createApp(store, () => T0);
    const { token } = await session(app, "ana");
    for (let i = 0; i < 1005; i++) await store.append("lobby", "bot", i);
    const got = await events(app, "/api/rooms/lobby/events?since=1", token, (e) => e.length >= 1);
    expect(JSON.parse(got[0].data).recovered).toBe(false);
  });

  test("presence follows heartbeats and forgets a connection after the TTL", async () => {
    let now = T0; const store: Store = memoryStore(); const app = createApp(store, () => now);
    const { token } = await session(app, "ana");
    const auth = { authorization: `Bearer ${token}` };
    await store.touch("lobby", "bob", "c1", now); await store.touch("lobby", "bob", "c2", now);
    await events(app, "/api/rooms/lobby/events", token, (e) => e.length >= 1);
    // ana's stream closed cleanly, so she left; bob's two connections are still fresh.
    expect(await (await app.request("/api/rooms/lobby/presence", { headers: auth })).json()).toEqual({ clients: [{ client: "bob", conns: 2, seen_at: T0.toISOString() }] });
    now = new Date(T0.getTime() + PRESENCE_TTL_MS + 1);
    expect((await (await app.request("/api/rooms/lobby/presence", { headers: auth })).json()).clients).toEqual([]);
  });
});
"##;

const REALTIME_SQL: &str = r##"-- One row per room, so appending can lock it: the offset is a per-room sequence with no gaps,
-- and that is what makes "resume from offset N" exact rather than approximate.
create table if not exists rooms (
  name text primary key,
  top bigint not null default 0,
  created_at timestamptz not null default now()
);

-- The history. Bounded per room by the retention the app chooses (see Store.history); a client
-- whose cursor is older than what is kept gets a `recover: false` and reloads.
create table if not exists messages (
  room text not null references rooms(name),
  "offset" bigint not null,
  client text not null,
  data jsonb not null,
  created_at timestamptz not null default now(),
  primary key (room, "offset")
);

-- Presence: who is in a room, refreshed by each connection's heartbeat. A row older than the
-- TTL is a connection whose replica died without saying goodbye; it is simply not counted.
create table if not exists presence (
  room text not null,
  client text not null,
  conn text not null,
  seen_at timestamptz not null default now(),
  primary key (room, conn)
);
create index if not exists presence_seen on presence (room, seen_at);
"##;

const REALTIME_PAGE: &str = r##""use client";
import { useEffect, useRef, useState } from "react";

type Pub = { offset: number; client: string; data: unknown; created_at: string };

// Join a room, watch it live. The cursor is the last offset seen: on a reconnect (a laptop lid,
// a deploy) the stream resumes from there, so nothing is missed and nothing repeats.
export default function Home() {
  const [user, setUser] = useState("ana");
  const [room, setRoom] = useState("lobby");
  const [token, setToken] = useState<string | null>(null);
  const [pubs, setPubs] = useState<Pub[]>([]);
  const [present, setPresent] = useState<string[]>([]);
  const [text, setText] = useState("");
  const last = useRef(0);

  async function join() {
    await fetch("/api/session", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ user }) });
    const res = await fetch(`/api/rooms/${room}/token`, { method: "POST" });
    if (!res.ok) return alert("could not join");
    setPubs([]); last.current = 0;
    setToken((await res.json()).token);
  }

  useEffect(() => {
    if (!token) return;
    const es = new EventSource(`/api/rooms/${room}/events?token=${token}&since=${last.current}`);
    es.addEventListener("publication", (e) => { const p: Pub = JSON.parse((e as MessageEvent).data); last.current = p.offset; setPubs((ps) => [...ps.slice(-199), p]); });
    const presence = setInterval(async () => {
      const r = await fetch(`/api/rooms/${room}/presence`, { headers: { authorization: `Bearer ${token}` } });
      if (r.ok) setPresent(((await r.json()).clients as { client: string }[]).map((c) => c.client));
    }, 5000);
    return () => { es.close(); clearInterval(presence); };
  }, [token, room]);

  async function send() {
    if (!token || !text) return;
    await fetch(`/api/rooms/${room}/publish`, { method: "POST", headers: { "content-type": "application/json", authorization: `Bearer ${token}` }, body: JSON.stringify({ data: { text } }) });
    setText("");
  }

  return (
    <main>
      <h1>{{NAME}} — realtime</h1>
      <p>
        From a shell: <code>{`TOKEN=$(curl -s -c c.txt -X POST localhost:8000/api/session -H 'content-type: application/json' -d '{"user":"cli"}' >/dev/null; curl -s -b c.txt -X POST localhost:8000/api/rooms/lobby/token | sed 's/.*"token":"\\([^"]*\\)".*/\\1/')`}</code>
        {" "}then <code>{`curl -N "localhost:8000/api/rooms/lobby/events?token=$TOKEN&since=0"`}</code> and, elsewhere,{" "}
        <code>{`curl -X POST localhost:8000/api/rooms/lobby/publish -H "authorization: Bearer $TOKEN" -H 'content-type: application/json' -d '{"data":{"text":"hi"}}'`}</code>.
      </p>
      <p>
        <input value={user} onChange={(e) => setUser(e.target.value)} placeholder="user" />{" "}
        <input value={room} onChange={(e) => setRoom(e.target.value)} placeholder="room" />{" "}
        <button onClick={join}>{token ? "Rejoin" : "Join"}</button>
        {token && <span> · in the room: {present.join(", ") || "…"}</span>}
      </p>
      {token && (
        <p>
          <input value={text} onChange={(e) => setText(e.target.value)} onKeyDown={(e) => e.key === "Enter" && send()} placeholder="say something" style={{ width: "60%" }} />{" "}
          <button onClick={send}>Send</button>
        </p>
      )}
      <table>
        <thead><tr><th>#</th><th>from</th><th>message</th><th>at</th></tr></thead>
        <tbody>
          {pubs.map((p) => (
            <tr key={p.offset}><td>{p.offset}</td><td>{p.client}</td><td>{typeof p.data === "object" && p.data && "text" in p.data ? String((p.data as { text: unknown }).text) : JSON.stringify(p.data)}</td><td>{p.created_at.slice(11, 19)}</td></tr>
          ))}
        </tbody>
      </table>
    </main>
  );
}
"##;

const REALTIME_CLAUDE: &str = r##"# {{NAME}} — working agreement

{{NAME}} is a realtime messaging service: rooms, a per-room message log with gapless offsets,
history you can resume from, presence with heartbeats, and a live stream. It is modelled on
Centrifugo — its channel, subscription token, publication offset, history/recovery and presence
concepts — delivered over Server-Sent Events rather than WebSocket, with publishing as a POST.
"Done" here means: a client that reconnects with its last offset sees every message after it,
once, in order, or is told `recovered: false`; a token for one room opens no other; and a client
that stops reading is disconnected rather than buffered without bound.

## Architecture

| File | Owns |
|---|---|
| `backend/src/app.ts` | The routes, the token (`mintToken`/`verifyToken`, HS256 over `{sub, channel, exp}`), the SSE loop with its replay, dedupe, heartbeat and slow-client cut-off. The constants: `TOKEN_TTL_S` 900, `PRESENCE_TTL_MS` 30 000, `HEARTBEAT_MS` 10 000, `QUEUE_MAX` 256, message cap 4 KB, `ROOM` name regex `^[\w:.-]{1,64}$`. Exports `AppType`. |
| `backend/src/store.ts` | `Store`: `append`, `history`, `subscribe`/`publish` (fan-out), `touch`/`leave`/`presence`. `pgStore` on Postgres with LISTEN/NOTIFY; `memoryStore` for tests. `HISTORY_MAX` 1000 per room. |
| `backend/src/db.ts` | The `postgres` pool from `DATABASE_URL`. `db.listen` takes a dedicated connection. |
| `backend/src/migrate.ts` | Applies `backend/migrations/*.sql` once each. |
| `backend/migrations/0002_realtime.sql` | `rooms`, `messages`, `presence`. |
| `backend/src/seed.ts` | The stack's default seed (notes); this pack does not need one — a room exists the first time someone publishes to it. |
| `backend/src/app.test.ts` | Five tests on `memoryStore()` with a fixed clock; a small SSE reader that aborts the stream like a closing tab. |
| `frontend/app/page.tsx` | A client component: session → token → `EventSource` on `/api/rooms/:room/events`, cursor in a ref, presence polled every 5 s. |
| `frontend/lib/api.ts` | `hc<AppType>` — the typed client (the page uses `fetch`/`EventSource` directly because the stream is not a JSON route). |

The request path for one message, from a browser that just opened the page:

1. `POST /api/session {user}` sets a signed `session` cookie (`hono/cookie`, HMAC with `SESSION_SECRET`). This is the stand-in for sign-in.
2. `POST /api/rooms/:room/token` reads that cookie, checks the room name against `ROOM`, and mints a JWT `{sub: user, channel: room, exp: now + 900 s}`. This is the one place that decides who may enter which room.
3. `GET /api/rooms/:room/events?token&since=N` verifies the token *for this room*, then inside `streamSSE`: subscribes to the store **before** reading history, `touch`es presence, reads `history(room, since, 500)`, and writes a `subscribed` event with `top` and `recovered` (`since === 0`, or nothing to replay, or the first replayed offset is exactly `since + 1`).
4. Replayed publications are written with `id: offset`; `last` tracks the highest offset sent.
5. The loop: drain the queue, writing only publications with `offset > last` (a replica echoing an old one, or a message that arrived during the history read, is dropped); every 10 s without traffic, `touch` presence and write a `ping`; if the queue exceeds 256, write `disconnect {reason: "slow", last}` and end.
6. `POST /api/rooms/:room/publish {data}` with the token as `Authorization: Bearer` verifies it for the room, refuses bodies over 4096 bytes, then `store.append` (one transaction: bump `rooms.top` under its row lock, insert the message at that offset, delete anything older than the last 1000) and `store.publish` (`NOTIFY room:<name>` with the publication as payload).
7. Every replica's `db.listen` fires, every subscriber on every replica gets the publication, and step 5 decides whether to write it.
8. On abort (tab closed, network gone, deploy), `finally` unsubscribes and `leave`s presence. A replica that dies without running `finally` leaves a presence row that ages out after 30 s.

Data model:

| Table | Why the columns that matter |
|---|---|
| `rooms` | `name text primary key`, `top bigint`. The row is the lock: `append` does `insert … on conflict do update set top = rooms.top + 1 returning top`, so two replicas cannot both take offset N. |
| `messages` | `primary key (room, "offset")` — the gapless per-room sequence is what makes `since` exact. `data jsonb`. Trimmed to the last 1000 per room on every append. `room references rooms(name)`. |
| `presence` | `primary key (room, conn)` — one row per *connection*, not per user, so one user on two tabs is `conns: 2`. `seen_at` is refreshed by the heartbeat; `presence()` counts rows newer than the TTL and groups by `client`. `presence_seen (room, seen_at)` serves that query. |

## Invariants

1. **The subscription is opened before history is read.** Reversing the two lines in the `events` handler loses any message published between the read and the subscribe — a gap the client cannot detect. Guard: `app.test.ts` "the stream replays after the cursor, then delivers live, in order and without repeats".
2. **A publication is written only if `offset > last`.** That one comparison is the dedupe for three cases: a message that arrived during the replay, a replica echoing an older publication, and a client reconnecting with a cursor. Guard: the same test — the store re-publishes offset 2 mid-stream and the client sees `[2, 3, 4]`.
3. **Offsets are gapless per room and assigned under the room's row lock.** `append` in `pgStore` is one transaction; the offset comes from `returning top`, never from `max(offset) + 1` read outside it. Guard: "publications get gapless offsets, history resumes from a cursor, size is capped" (memory store); the transactional shape is by inspection.
4. **A token is bound to one room and one expiry, and is checked for the room in the URL.** `verifyToken(token, channel, now)` compares `claims.channel` to the route parameter; a token for `lobby` returns 401 on `other`. Comparison of the signature is `timingSafeEqual` with a length check first. Guard: "a join token comes from the session, is bound to one room, and expires".
5. **`recovered` is honest.** It is `true` only when the replay starts exactly at `since + 1` (or there was nothing to recover). A cursor older than the retained 1000 gets `false` and the client must reload state from elsewhere. Guard: "a cursor older than the retained history is reported as not recovered".
6. **A slow client is cut, not buffered.** `QUEUE_MAX` is checked every loop iteration; the `disconnect` event carries `last` so the client can resume from it. Without this, one stalled tab holds every publication for the room in this replica's memory. Guard: none automated — the reviewer checklist has it.
7. **Presence is a heartbeat, not a membership list.** `touch` on connect and every `HEARTBEAT_MS`; `leave` on a clean close; anything older than `PRESENCE_TTL_MS` is not counted. A replica that dies takes its `leave`s with it, and the TTL is what forgets them. Guard: "presence follows heartbeats and forgets a connection after the TTL".
8. **Publish appends before it fans out.** `store.append` then `store.publish`. A `NOTIFY` for a row that is not yet committed would be replayed by nobody. Guard: by inspection.
9. **Message data is capped at 4 KB because `NOTIFY`'s payload is capped at 8000 bytes.** Raising one without changing the other makes publishes over the limit fail at the store after being appended. Guard: the offsets test asserts 413 for 5000 bytes.
10. **The SSE handler cleans up in `finally`.** `unsubscribe` and `leave` run on every exit path — abort, slow-client cut, thrown error. A leaked subscription keeps receiving for a client that is gone. Guard: the presence test — ana's stream closed and she is not in the room.
11. **Room names are validated once, at token minting.** `ROOM` is applied in `/token`; every other route trusts the token's `channel` claim, which was validated then. A new route that takes a room from the URL and skips the token must apply `ROOM` itself.

## Extending it

**Real sign-in.** Replace the body of `POST /api/session` (and the `getSignedCookie` read in `/token`) with the `auth` pack's session; `sub` becomes the user id. Nothing below `/token` changes. Test: a `/token` request without a session is 401 (already there).

**Authorise rooms (private rooms, membership).** Only `/token` decides. Add a `store.mayJoin(user, room)` check there; the token then carries the decision for 15 minutes. Test: a user not in the room gets 403 from `/token`, and a token minted before removal still works until `exp` — decide whether that is acceptable or shorten `TOKEN_TTL_S`.

**Server-side publish (a bot, a webhook).** Call `store.append` and `store.publish` from your route; the `client` field is whatever you pass. Do not mint a token for a server — the token exists to carry a *user's* right to a room. Test: the publication appears in the stream with your `client`.

**Typing indicators / ephemeral events.** Do not append them — they would fill the 1000-message history. Add `store.publishEphemeral(room, payload)` that only `NOTIFY`s, and a branch in the SSE loop that writes it as `event: ephemeral` without touching `last`. Test: an ephemeral event does not advance `top`.

**Per-room retention.** `HISTORY_MAX` is one constant in `store.ts`. Make it a column on `rooms` if rooms differ; the trim in `append` reads it under the same lock. Test: a room with retention 10 reports `recovered: false` for a cursor 11 back.

**Move fan-out to Redis.** Only `subscribe`/`publish` in `pgStore` change (`ponytail:` note on them). Keep `append` in Postgres — it owns the offset. Test: the existing stream tests run on `memoryStore` and do not change; add one against Redis behind an env flag if you must.

**Add a route.** Take the room from the URL, verify the token for it first (`verifyToken(bearer(c), room, clock())`), return the error shape `{error: {message, code}}`. Keep it in the one chained expression so `AppType` stays typed.

## Operating it

| Variable | Required | Meaning |
|---|---|---|
| `SESSION_SECRET` | **yes in production** | HMAC key for the session cookie and the room tokens. Default `dev-secret-change-me` — with it, anyone can mint a token for any room. |
| `DATABASE_URL` | yes | Postgres; also the fan-out bus (LISTEN/NOTIFY). |
| `PORT` | no | API port, default 8000. |
| `API_URL` | frontend | Where the Next.js server reaches the API. The page's `EventSource` goes through the same origin's `/api`. |
| `REDIS_URL`, `SENTRY_DSN`, `POSTHOG_*` | no | From the stack; unused by this pack's code today. |

Replicas and what is where:

- The API scales with the HPA (2–5). Fan-out across replicas is Postgres `NOTIFY`, so a publish on replica A reaches a subscriber on replica B. Presence is in Postgres, so a room's roster is the same from every replica. Nothing is per-replica except the in-memory queue per open stream and the `db.listen` connection (one per process, shared across rooms by `postgres.js`).
- Each open stream is one `streamSSE` promise holding one queue; memory per replica is streams × queued publications × ≤ 4 KB, bounded by `QUEUE_MAX`. Connection count is the real limit, not CPU — set the HPA to scale on a connections metric before it matters.
- Postgres `NOTIFY` is one delivery per listening connection per publish; at thousands of publishes per second across many replicas, the database becomes the bus's bottleneck. That is the Redis upgrade in Ceilings.
- Through nginx (`make up`, `nginx/nginx.conf`) the `/api/` location does not set `proxy_buffering off`; if events arrive late through :8080, that is the line to add. `proxy_read_timeout 300s` will close a quiet stream after five minutes — the `ping` every 10 s keeps it open.
- Rollouts: a stream on a pod that is terminating ends when the pod does (`terminationGracePeriodSeconds: 90` in `k8s/base/backend.yaml`); the client reconnects with `since=last` and gets exactly the gap. That is what the cursor is for.

Failure modes and what the user sees:

| Failure | Effect |
|---|---|
| `SESSION_SECRET` unset in production | Tokens verify against the default secret; the room boundary is decorative. Set it. |
| Postgres down | `/publish` 500s; open streams keep running but receive nothing; presence heartbeats throw inside the loop and end the stream. Clients reconnect and get 401/500 until it is back. |
| A slow consumer | `disconnect {reason: "slow", last}` after 256 queued; the page today does not handle that event — it is a log line until you add a reconnect. |
| A cursor older than 1000 messages | `subscribed {recovered: false}`; the page today ignores the flag and shows what it got. |
| A token expiring mid-stream | Nothing: the token is checked at connect only. The stream lives as long as the connection. Publishing needs a fresh token after 15 minutes; the page does not refresh it. |
| Replica dies | Its presence rows age out in 30 s; its clients reconnect elsewhere with their cursor. |

What to watch: open SSE connections per replica; `NOTIFY` rate; `messages` row count (bounded at 1000 × rooms); `presence` row count vs. connections (a growing gap means `leave` is not running); 401s on `/publish` (expired tokens the page did not refresh).

## Ceilings

- **The session is a name in a signed cookie** (`app.ts`: `ponytail: a name in a signed cookie stands in for sign-in`). Anyone can be `ana`. Upgrade: the `auth` pack's session in `/api/session` and `/token`.
- **Fan-out is Postgres LISTEN/NOTIFY** (`store.ts`: `ponytail: move to Redis pub/sub when a room fans out to thousands of connections per replica`). Every replica gets every notification for every room it listens to; the database is the bus. Upgrade: Redis pub/sub in `subscribe`/`publish`, offsets stay in Postgres.
- **SSE, not WebSocket.** One direction per connection; sends are POSTs. Fine for chat and feeds; wrong for high-rate client → server traffic (cursors, game input). Upgrade: `ws` on the Node server and the same `Store`.
- **Tokens are checked at connect.** A revoked user keeps an open stream until it drops. Upgrade: re-verify `exp` in the loop and send `disconnect {reason: "expired"}`.
- **History is 1000 per room, trimmed on every append.** A busy room's replay window is minutes. Upgrade: per-room retention column, or trim by age from a cron.
- **Presence is polled by the page** every 5 s. Upgrade: publish presence deltas as ephemeral events (recipe above).
- **The token travels as a query parameter** for `EventSource` (browsers cannot set headers on it), so it lands in access logs. Upgrade: a one-shot ticket exchanged for the stream, or a cookie-scoped stream route.
- **No rate limit on publish.** A token holder can fill a room's history in seconds. Upgrade: a token bucket per `sub` in Redis in `/publish`.
- **Readiness does not check Postgres.** `/api/health/ready` returns a literal.

The stack rules — gate, typed seam, production checklist, deploy — are in `docs/PRODUCTION.md`. They apply.
"##;

const REALTIME_AGENTS: &str = r##"# {{NAME}} — for agents

`CLAUDE.md` has the rules and the architecture. This is how to run and test it.

## Run

    make demo         # postgres + redis, migrate, seed, API on :8000, app on :3000
    make dev          # just postgres + redis
    make backend      # API with reload on :8000
    make frontend     # Next.js on :3000, /api proxied to :8000
    make check        # the gate: typecheck both halves, run backend tests

No worker: fan-out is Postgres `NOTIFY`, delivered inside the API process. There is nothing to
seed either — a room exists the first time a message is published to it.

## Routes, with bodies

A session, then a token for a room (the cookie jar carries the session):

    curl -s -c c.txt -X POST localhost:8000/api/session -H 'content-type: application/json' -d '{"user":"ana"}'
    # {"user":"ana"}
    curl -s -b c.txt -X POST localhost:8000/api/rooms/lobby/token
    # {"token":"eyJhbGciOi….eyJzdWIiOi….<sig>","channel":"lobby","exp":1773144900}
    # no cookie: 401 {"error":{"message":"no session","code":"unauthorized"}}
    # room "a b": 400 {"error":{"message":"bad room name","code":"invalid"}}

Put the token in a variable for the rest:

    TOKEN=$(curl -s -b c.txt -X POST localhost:8000/api/rooms/lobby/token | sed 's/.*"token":"\([^"]*\)".*/\1/')

Publish:

    curl -s -X POST localhost:8000/api/rooms/lobby/publish -H "authorization: Bearer $TOKEN" \
      -H 'content-type: application/json' -d '{"data":{"text":"hi"}}'
    # {"offset":1}
    # token for another room, expired, or tampered: 401 {"error":{"message":"bad or expired token","code":"unauthorized"}}
    # body over 4096 bytes: 413 {"error":{"message":"message over 4 KB","code":"too_large"}}
    # no "data" key: 400 {"error":{"message":"data required","code":"invalid"}}

History from a cursor (`since` = last offset seen; `limit` ≤ 500):

    curl -s "localhost:8000/api/rooms/lobby/history?since=0&limit=100" -H "authorization: Bearer $TOKEN"
    # {"publications":[{"offset":1,"client":"ana","data":{"text":"hi"},"created_at":"2026-03-10 12:00:00.1+00"}],"top":1}

Presence (connections newer than 30 s, grouped by client):

    curl -s localhost:8000/api/rooms/lobby/presence -H "authorization: Bearer $TOKEN"
    # {"clients":[{"client":"ana","conns":1,"seen_at":"2026-03-10 12:00:05+00"}]}

The stream — `curl -N`, token as a query parameter because `EventSource` cannot set headers:

    curl -N "localhost:8000/api/rooms/lobby/events?token=$TOKEN&since=0"
    # event: subscribed
    # data: {"channel":"lobby","conn":"…uuid…","top":1,"recovered":true}
    #
    # event: publication
    # id: 1
    # data: {"offset":1,"client":"ana","data":{"text":"hi"},"created_at":"…"}
    #
    # event: ping          ← every 10 s when quiet
    # data:
    #
    # event: disconnect    ← only if you stop reading and 256 messages queue up
    # data: {"reason":"slow","last":1}

Reconnect with `since=<last offset>` and the stream replays the gap, then continues. If `since`
is older than the room's last 1000 messages, `subscribed` says `"recovered":false`.

Health: `GET /api/health` → `{"status":"ok"}`; `GET /api/health/ready` → `{"status":"ok","db":"ok"}` (a literal today).

## How the tests are built

`backend/src/app.test.ts` runs on `memoryStore()` with the clock fixed at `T0 = 2026-03-10T12:00Z`
via `createApp(store, () => T0)`; the presence test reassigns `now` to move time. Two helpers:

- `session(app, user, room)` does the cookie → token dance and returns both.
- `events(app, path, token, until, during?)` opens the SSE route through `app.request` with an
  `AbortController`, parses `event:`/`data:` blocks off the body reader, runs `during()` once
  after the first chunk (to publish while connected, or to echo a stale publication straight
  into `store.publish`), stops when `until(events)` is true, aborts, and waits 20 ms for the
  handler's `finally` to run.

The token functions are tested directly (`verifyToken` with the wrong room, a later clock, a
mangled signature, another secret). Fan-out across replicas is modelled by calling
`store.publish` by hand with an old offset; `pgStore`'s `NOTIFY` path is exercised by `make demo`
with two `curl -N` streams, not by `bun test`.

To add a test: a `test(...)` in `describe("realtime")`, a fresh `memoryStore()`, `session(...)`
for a token, and `events(...)` for anything that touches the stream — its `until` should count
the events you expect so the test ends the moment they arrive rather than on a timer. Assert on
`recovered` and on the list of offsets, not on timestamps.
"##;

const REALTIME_README: &str = r##"# {{NAME}}

Rooms, history and presence over Server-Sent Events, backed by Postgres: a Centrifugo-shaped
realtime layer that lives inside your own API.

## What you get

- Rooms with a per-room message log and gapless offsets (`POST /api/rooms/:room/publish` → `{offset}`).
- A live stream (`GET /api/rooms/:room/events`) that replays from `since`, then delivers live, in order, once — and says `recovered: false` when the cursor is older than what is kept.
- History with a cursor (`/history?since&limit`) and presence with 30-second heartbeats (`/presence`).
- Join tokens: HS256 JWTs bound to one room and one user, valid 15 minutes, minted from the session by the route where *you* decide who may enter.
- A slow client is disconnected at 256 queued messages with its last offset, not buffered forever.
- Fan-out across replicas with no broker: Postgres `LISTEN`/`NOTIFY`. Presence in Postgres too, so every replica agrees.
- Tests without a database, a typed client for the JSON routes, Docker Compose locally, kustomize manifests for a cluster.

## Five minutes

    make demo

Then, in a second shell:

    curl -s -c c.txt -X POST localhost:8000/api/session -H 'content-type: application/json' -d '{"user":"ana"}'
    TOKEN=$(curl -s -b c.txt -X POST localhost:8000/api/rooms/lobby/token | sed 's/.*"token":"\([^"]*\)".*/\1/')
    curl -N "localhost:8000/api/rooms/lobby/events?token=$TOKEN&since=0"
    # event: subscribed
    # data: {"channel":"lobby","conn":"…","top":0,"recovered":true}

And in a third:

    curl -s -X POST localhost:8000/api/rooms/lobby/publish -H "authorization: Bearer $TOKEN" \
      -H 'content-type: application/json' -d '{"data":{"text":"hello"}}'
    # {"offset":1}                       ← and the second shell prints the publication

Stop the stream (Ctrl-C), publish twice more, then reconnect with `since=1`: offsets 2 and 3 are
replayed, nothing repeats. Open http://localhost:3000, join `lobby` as `bob`, and both names are
in the room.

## API

| Method | Path | Auth | What |
|---|---|---|---|
| POST | `/api/session` | none | `{user}` → signed `session` cookie. The stand-in for sign-in. |
| POST | `/api/rooms/:room/token` | session cookie | Mint a 15-minute join token for this room. 401 without a session, 400 for a bad room name. |
| POST | `/api/rooms/:room/publish` | Bearer token for this room | `{data}` ≤ 4 KB → `{offset}`. |
| GET | `/api/rooms/:room/history?since&limit` | Bearer | Publications after `since`, ≤ 500, plus `top`. |
| GET | `/api/rooms/:room/presence` | Bearer | Clients seen in the last 30 s, with connection counts. |
| GET | `/api/rooms/:room/events?token&since` | token (query) | SSE: `subscribed`, `publication`, `ping`, `disconnect`. |
| GET | `/api/health`, `/api/health/ready` | none | Liveness; readiness (does not check the database yet). |

## Compared with Centrifugo

Same shapes, so Centrifugo's concepts and docs carry over:

- Channels (rooms), subscription tokens as HS256 JWTs with `sub`, `channel`, `exp`.
- Publications with a per-channel offset; history with a cursor; `recovered` true/false on subscribe.
- Presence with a TTL; a client queue limit (`client_queue_max_size`) past which the client is disconnected.
- Publishing over an HTTP API, not over the subscriber connection.

Better here, for a team that owns one application:

- It is your code: the join decision is a function in `app.ts`, next to your session, not a JWT contract with a separate server. No Centrifugo process, config or proxy endpoints.
- Typed end to end with the frontend for the JSON routes; the stream is plain SSE that `EventSource`, `curl` and any proxy already speak.
- Tests run in milliseconds without Centrifugo or a database, including the replay/dedupe/recovery semantics.
- History and presence are rows in the Postgres you already run, queryable with SQL.
- k8s manifests with rolling updates, probes and an HPA come with it.

Not here yet — Centrifugo has these and this does not:

- **WebSocket** (and its bidirectional protocol, HTTP-stream, WebTransport, gRPC). This is SSE; client → server is a POST.
- **A Redis or Nats engine.** Fan-out is Postgres `NOTIFY`, which is a database-sized bus, not a broker-sized one.
- **Client SDKs** with automatic reconnect, backoff and token refresh. The page does a first cut; `disconnect` and `recovered: false` are not yet handled there.
- **Proxy hooks** (connect/subscribe/publish/refresh to your backend) — not needed, the backend *is* this.
- **Token refresh mid-connection, connection expiry, per-user channel limits, rate limits.**
- **The admin UI, Prometheus metrics, tracing.**
- **Delta compression, channel namespaces with per-namespace options, join/leave events, RPC.**
- **Real sign-in.** `POST /api/session` takes a name.

## Production

- **`SESSION_SECRET` must be set** in every environment that matters; the default is public. It signs both the session cookie and every room token.
- **Environments.** `backend/.env` → `backend/.env.age` → `k8s/secrets.yaml` via `make k8s-secrets ENV=dev|prod`.
- **Scaling.** Stateless replicas behind the HPA; the limit is open connections per pod, not CPU — add a custom metric before relying on the CPU target. Every replica holds one `LISTEN` connection.
- **Probes.** `/api/health` liveness, `/api/health/ready` readiness (make it check Postgres). A terminating pod ends its streams; clients resume with their cursor.
- **Proxies.** Anything between the client and the pod must not buffer SSE and must allow idle connections longer than the 10-second ping. The stack's nginx config has `proxy_read_timeout 300s`; add `proxy_buffering off` on `/api/` if events arrive late.
- **Migrations.** `backend/migrations/`, applied by the init container. `messages` is trimmed to 1000 per room on every append, so it does not need pruning.
- **What pages.** 5xx on `/publish`; open connections near the pod limit; `presence` rows growing faster than connections (a `leave` that stopped running); a rising rate of `disconnect reason=slow` in the logs.

## Roadmap

1. Real sign-in via the `auth` pack, and a `mayJoin` check in `/token`.
2. The page handling `disconnect`, `recovered: false` and token expiry (reconnect with `since=last`, refresh the token every 10 minutes).
3. Redis pub/sub in `subscribe`/`publish` when a room fans out to thousands of connections.
4. Re-checking token expiry inside the stream loop.
5. A rate limit on `/publish` per `sub`.
6. WebSocket alongside SSE, on the same `Store`, when a use case needs client → server at rate.
"##;

const REALTIME_REVIEWER: &str = r##"---
name: stream-semantics
description: Run on any change to the events route, the token functions, the store's append/publish/subscribe, presence, or a migration on rooms/messages/presence. Reads the change as the engineer whose users will report "I missed a message" or "I saw it twice", and reports what breaks ordering, recovery, isolation or cleanup.
tools: Read, Grep, Glob, Bash
---

You review a realtime change. The bugs here are timing bugs and boundary bugs; the test suite
covers the ones already found, and you are looking for the next one.

Report each finding as `path:line — what breaks — the sequence that shows it — the fix`.

Check:
1. Subscribe-before-history in the `events` handler (`backend/src/app.ts`): `store.subscribe`
   is awaited before `store.history`. Reversed, a publish between the two is lost with no signal.
2. The dedupe comparison `p.offset > last` is still the only way a publication reaches
   `writeSSE`, and `last` is updated on every write — replayed and live alike.
3. `recovered` is computed from the first replayed offset (`=== since + 1`), or `since === 0`,
   or an empty replay. A change that returns `true` when the replay starts later than
   `since + 1` hides a gap.
4. `append` in `pgStore` takes the offset from `returning top` inside `db.begin`, and inserts the
   message in the same transaction. A `select max(offset)` outside a lock gives two replicas
   the same offset; a `NOTIFY` before commit announces a row that cannot be replayed.
5. `verifyToken` checks all four: signature (`timingSafeEqual`, length first), `channel ===`
   the route's room, `sub` is a string, `exp * 1000 > now`. Every route that takes `:room`
   calls it with *that* room. A route that verifies against a room from the body instead of
   the URL lets a `lobby` token read `admin`.
6. The secret: `SESSION_SECRET` is read through `SECRET()`, and a new signing use does not
   introduce a second key or a default of its own.
7. Room name validation: `ROOM` is applied wherever a room is taken from the URL *without* a
   token check (today only `/token`). A room name that reaches `NOTIFY room:<name>` or a
   table unvalidated is an injection surface.
8. Cleanup: `unsubscribe()` and `store.leave` are in `finally`, and every new exit from the
   loop (a new `break`) still reaches it. A subscription that outlives its stream receives
   into a queue nobody drains.
9. The slow-client check runs every iteration before the shift, and the `disconnect` event
   carries `last`. Raising `QUEUE_MAX` or removing the check turns one stalled tab into
   unbounded memory on the replica.
10. Payload size: the 4096-byte cap in `/publish` stays under Postgres `NOTIFY`'s 8000-byte
    limit with the JSON envelope (`offset`, `client`, `created_at`) included. Check the
    envelope has not grown.
11. Presence rows are keyed by `(room, conn)`, `touch` runs on connect and each heartbeat,
    `presence()` filters by `seen_at > now - ttl`. A change that keys by client, or counts
    without the TTL filter, shows dead replicas' users as present forever.
12. Heartbeat and timeouts: `ping` every `HEARTBEAT_MS` (10 s) remains shorter than any proxy
    idle timeout in `nginx/nginx.conf` (`proxy_read_timeout 300s`) and the ingress.
13. The frontend: `last.current` is set from each `publication`, and a reconnect passes it as
    `since`. A reconnect with `since=0` replays up to 500 messages the user already saw.
14. Migrations: `messages.primary key (room, "offset")` and `rooms.top` stay; a change that
    makes offsets global or per-replica breaks `since` for every client at once.

End with one line: `stream-semantics: N findings`, and if 0, what you checked.
"##;
