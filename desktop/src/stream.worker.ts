// Server-sent events, read and decoded off the UI thread.
//
// The Swift app decoded every event on its main actor, one JSON parse per token, and that is a
// third of why a streaming reply hitched. Here the bytes, the SSE framing and the JSON all stay in
// this worker, and the page gets one message per stream per 16 ms with everything that arrived in
// it — so the UI does one store update per frame however fast the agent writes.

interface Open {
  type: "open";
  id: number;
  url: string;
  token: string;
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

const streams = new Map<number, AbortController>();
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

async function run(msg: Open) {
  const abort = new AbortController();
  streams.set(msg.id, abort);
  try {
    const response = await fetch(msg.url, {
      headers: { Authorization: `Bearer ${msg.token}`, Accept: "text/event-stream" },
      signal: abort.signal,
    });
    if (!response.ok || !response.body) {
      end(msg.id, (await response.text()) || `HTTP ${response.status}`);
      return;
    }
    const reader = response.body.pipeThrough(new TextDecoderStream()).getReader();
    let buffer = "";
    let name = "message";
    let data: string[] = [];
    for (;;) {
      const { value, done } = await reader.read();
      if (done) break;
      buffer += value;
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
            push(msg.id, { event: name, data: parsed });
          }
          name = "message";
          data = [];
        } else if (line.startsWith(":")) {
          // The keep-alive. Surfaced so the page can tell a quiet agent from a dead daemon.
          push(msg.id, { event: "#heartbeat", data: null });
        } else if (line.startsWith("event:")) {
          name = line.slice(6).trim();
        } else if (line.startsWith("data:")) {
          data.push(line.slice(5).replace(/^ /, ""));
        }
      }
    }
    end(msg.id);
  } catch (e) {
    end(msg.id, abort.signal.aborted ? undefined : String(e));
  }
}

self.onmessage = (e: MessageEvent<Open | Close>) => {
  if (e.data.type === "open") void run(e.data);
  else streams.get(e.data.id)?.abort();
};
