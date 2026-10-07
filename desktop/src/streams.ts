// The page's side of the stream worker: open a stream, get its events in batches, close it.

import type { Arrived, Batch } from "./stream.worker";

const worker = new Worker(new URL("./stream.worker.ts", import.meta.url), { type: "module" });
const handlers = new Map<number, (events: Arrived[], end?: { error?: string }) => void>();
let ids = 0;

worker.onmessage = (e: MessageEvent<Batch>) => {
  const h = handlers.get(e.data.id);
  if (!h) return;
  if (e.data.end) handlers.delete(e.data.id);
  h(e.data.events, e.data.end);
};

export interface Stream {
  close(): void;
}

export function open(
  url: string,
  token: string,
  onEvents: (events: Arrived[]) => void,
  onEnd: (error?: string) => void,
): Stream {
  const id = ++ids;
  handlers.set(id, (events, end) => {
    if (events.length) onEvents(events);
    if (end) onEnd(end.error);
  });
  worker.postMessage({ type: "open", id, url, token });
  return {
    close() {
      handlers.delete(id);
      worker.postMessage({ type: "close", id });
    },
  };
}
