// Server-sent events, read and decoded off the UI thread.
//
// The Swift app decoded every event on its main actor, one JSON parse per token, and that is a
// third of why a streaming reply hitched. Here the bytes, the SSE framing and the JSON all stay in
// this worker, and the page gets one message per stream per 16 ms with everything that arrived in
// it — so the UI does one store update per frame however fast the agent writes.

import { tunnelled } from "./api";

interface Open {
  type: "open";
  id: number;
  url: string;
  token: string;
  /// POST for a stream that does something (an install), GET otherwise.
  method?: "GET" | "POST";
}
interface Close {
  type: "close";
  id: number;
}

export interface Arrived {
  event: string;
  data: unknown;
}

export interface Batch {
  id: number;
  events: Arrived[];
  /// The stream ended. `error` when it did not end cleanly.
  end?: { error?: string };
}

/// What closes a stream: an aborted fetch, or a closed socket.
const streams = new Map<number, () => void>();
const queued = new Map<number, Arrived[]>();
let timer: ReturnType<typeof setTimeout> | null = null;

function flush() {
  timer = null;
  for (const [id, events] of queued) {
    if (events.length) (self as unknown as Worker).postMessage({ id, events } satisfies Batch);
  }
  queued.clear();
}

function push(id: number, event: Arrived) {
  let q = queued.get(id);
  if (!q) queued.set(id, (q = []));
  q.push(event);
  timer ??= setTimeout(flush, 16);
}

function end(id: number, error?: string) {
  flush();
  streams.delete(id);
  (self as unknown as Worker).postMessage({ id, events: [], end: { error } } satisfies Batch);
}

/// The SSE framing, fed text as it arrives. `name` and `data` carry across chunks.
function parser(id: number): (text: string) => void {
  let buffer = "";
  let name = "message";
  let data: string[] = [];
  return (text) => {
    buffer += text;
    let at: number;
    while ((at = buffer.indexOf("\n")) >= 0) {
      const line = buffer.slice(0, at).replace(/\r$/, "");
      buffer = buffer.slice(at + 1);
      if (line === "") {
        // An `event:` with no `data:` is still an event — the daemon sends `caught-up` that way.
        if (data.length || name !== "message") {
          const raw = data.join("\n");
          let parsed: unknown = raw;
          if (raw.startsWith("{") || raw.startsWith("[")) {
            try {
              parsed = JSON.parse(raw);
            } catch {
              parsed = raw;
            }
          }
          push(id, { event: name, data: parsed });
        }
        name = "message";
        data = [];
      } else if (line.startsWith(":")) {
        // The keep-alive. Surfaced so the page can tell a quiet agent from a dead daemon.
        push(id, { event: "#heartbeat", data: null });
      } else if (line.startsWith("event:")) {
        name = line.slice(6).trim();
      } else if (line.startsWith("data:")) {
        data.push(line.slice(5).replace(/^ /, ""));
      }
    }
  };
}

/// A POST stream (an install) over fetch. Rare and short-lived, so its connection is affordable.
async function post(msg: Open) {
  const abort = new AbortController();
  streams.set(msg.id, () => abort.abort());
  try {
    const response = await fetch(msg.url, {
      method: "POST",
      headers: { Authorization: `Bearer ${msg.token}`, Accept: "text/event-stream" },
      signal: abort.signal,
    });
    if (!response.ok || !response.body) {
      end(msg.id, (await response.text()) || `HTTP ${response.status}`);
      return;
    }
    const reader = response.body.pipeThrough(new TextDecoderStream()).getReader();
    const feed = parser(msg.id);
    for (;;) {
      const { value, done } = await reader.read();
      if (done) break;
      feed(value);
    }
    end(msg.id);
  } catch (e) {
    end(msg.id, abort.signal.aborted ? undefined : String(e));
  }
}

/// A GET stream through the daemon's WebSocket tunnel (`tunnel.rs`). WebKit allows six HTTP
/// connections per host and port, and each SSE stream held one for good, so three opened lanes
/// left every command to that daemon waiting in WebKit's queue behind streams that never end.
/// A WebSocket does not count against that limit. The first frame is the status; the body follows
/// as bytes.
function get(msg: Open) {
  const socket = new WebSocket(tunnelled(msg.url), ["keel", `keel.${msg.token}`]);
  socket.binaryType = "arraybuffer";
  const decoder = new TextDecoder();
  const feed = parser(msg.id);
  let status = 0;
  let refusal = "";
  let closed = false;
  const finish = (error?: string) => {
    if (closed) return;
    closed = true;
    end(msg.id, error);
  };
  streams.set(msg.id, () => {
    socket.close();
    finish();
  });
  socket.onmessage = (e: MessageEvent<string | ArrayBuffer>) => {
    if (!status && typeof e.data === "string") {
      status = Number(e.data) || 500;
      return;
    }
    const text = typeof e.data === "string" ? e.data : decoder.decode(e.data, { stream: true });
    if (status === 200) feed(text);
    else refusal += text;
  };
  // A daemon that died or never answered must end as a network error: the callers in `store.ts`
  // tell a dead daemon from a refusal by `/fetch|network|load failed/`, which fetch's own
  // "Load failed" used to satisfy. Only a clean close after a 200 is a stream that simply ended.
  socket.onclose = (e) => finish(
    status === 200 ? (e.wasClean ? undefined : "network: the stream dropped")
    : status ? refusal || `HTTP ${status}`
    : "network: could not reach the daemon",
  );
}

function run(msg: Open) {
  if ((msg.method ?? "GET") === "GET") get(msg);
  else void post(msg);
}

self.onmessage = (e: MessageEvent<Open | Close>) => {
  if (e.data.type === "open") run(e.data);
  else streams.get(e.data.id)?.();
};
