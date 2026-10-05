// Talking to one project's daemon. Every request has a ceiling (`ORDINARY`, the same 90 s the
// Swift client kept); the streams are the one deliberate exception and live in `stream.worker.ts`.

export interface Endpoint {
  port: number;
  /// Sent on every request. The webview's origin is shared with every Tauri app on the machine;
  /// this is what makes a request this app's (`pair::app_token` in the daemon).
  token: string;
}

export const ORDINARY = 90_000;

type Params = Record<string, string | number | boolean | null | undefined>;

/// A query string. `encodeURIComponent` writes `+` as `%2B`, which matters: the daemon reads a
/// bare `+` as a space, so a prompt saying "C++" arrived as "C  ".
export function query(params: Params = {}): string {
  const parts = Object.entries(params)
    .filter(([, v]) => v !== undefined && v !== null && v !== "")
    .map(([k, v]) => `${encodeURIComponent(k)}=${encodeURIComponent(String(v))}`);
  return parts.length ? `?${parts.join("&")}` : "";
}

export function url(ep: Endpoint, path: string, params?: Params): string {
  return `http://127.0.0.1:${ep.port}${path}${query(params)}`;
}

async function request<T>(ep: Endpoint, method: string, path: string, params?: Params, body?: unknown): Promise<T> {
  const response = await fetch(url(ep, path, params), {
    method,
    headers: {
      Authorization: `Bearer ${ep.token}`,
      ...(body === undefined ? {} : { "Content-Type": "application/json" }),
    },
    body: body === undefined ? undefined : JSON.stringify(body),
    signal: AbortSignal.timeout(ORDINARY),
  });
  const text = await response.text();
  // The daemon answers a failure in plain text that says what went wrong; that is the message.
  if (!response.ok) throw new Error(text || `${method} ${path}: ${response.status}`);
  return (text ? JSON.parse(text) : undefined) as T;
}

export const get = <T>(ep: Endpoint, path: string, params?: Params) => request<T>(ep, "GET", path, params);
export const post = <T>(ep: Endpoint, path: string, body?: unknown, params?: Params) =>
  request<T>(ep, "POST", path, params, body);

/// A URL this window may frame (the CSP's `frame-src`), with `0.0.0.0` — "every interface", which
/// is what a server bound with `--host 0.0.0.0` prints — read as this machine. `null` for anything
/// else, which is shown as a link rather than as a frame the webview would silently refuse.
export function frameable(raw: string): string | null {
  try {
    const u = new URL(raw);
    if (u.hostname === "0.0.0.0") u.hostname = "127.0.0.1";
    const local = ["localhost", "127.0.0.1", "[::1]"].includes(u.hostname);
    const https = u.protocol === "https:" && u.hostname !== "[::1]";
    return local && (u.protocol === "http:" || https) ? u.toString() : null;
  } catch {
    return null;
  }
}
