//! realtime: rooms, history and presence over SSE, like Centrifugo ══════════════════════

pub fn files(name: &str) -> Vec<(&'static str, String)> {
    let f = |s: &str| s.replace("{{NAME}}", name);
    vec![
        ("backend/src/app.ts", REALTIME_APP.into()),
        ("backend/src/store.ts", REALTIME_STORE.into()),
        ("backend/src/app.test.ts", REALTIME_TEST.into()),
        ("backend/migrations/0002_realtime.sql", REALTIME_SQL.into()),
        ("frontend/app/page.tsx", f(REALTIME_PAGE)),
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
