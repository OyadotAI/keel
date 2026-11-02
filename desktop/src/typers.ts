// What each lane's agent terminal accepts as typed input. Keel's own prompts — "fix this
// finding", a background job's report — go in the way a person's would, into the CLI's own
// composer, so the CLI queues them, shows them and owns them like anything else typed there.
interface Connection { write(text: string): boolean | void; focus?(): void; visible: boolean; visibility: symbol }
const typers = new Map<string, Connection>();

/** A paste cannot migrate to a reconnected or newly visible terminal after validation. */
export interface TerminalLease { current(): boolean; focus(): void }

function deliver(connection: Connection | undefined, text: string): boolean {
  if (!connection) return false;
  try { return connection.write(text) !== false; } catch { return false; }
}

/** Registration belongs to this socket; older cleanup cannot remove its replacement. */
export function register(lane: string, type: (text: string) => boolean | void, options: { visible?: boolean; focus?(): void } = {}) {
  const connection: Connection = { write: type, focus: options.focus, visible: options.visible ?? true, visibility: Symbol() };
  typers.set(lane, connection);
  const unregister = () => {
    if (typers.get(lane) === connection) {
      typers.delete(lane);
      for (const closed of [...(exits.get(lane) ?? [])]) closed();
    }
  };
  return Object.assign(unregister, { visible(value: boolean) {
    if (connection.visible !== value) { connection.visible = value; connection.visibility = Symbol(); }
  } });
}

/** Capture current ownership and visibility, including a switch away and back. */
export function terminalLease(lane: string): TerminalLease | undefined {
  const connection = typers.get(lane);
  if (!connection?.visible) return;
  const visibility = connection.visibility;
  const current = () => typers.get(lane) === connection && connection.visible && connection.visibility === visibility;
  return { current, focus: () => { try { if (current()) connection.focus?.(); } catch { /* Input was accepted; a lost focus must not report failed delivery. */ } } };
}

/// Type `text` into the lane's agent and press Enter. `false` when the lane has no terminal open.
export function typeInto(lane: string, text: string, lease?: TerminalLease): boolean {
  if (lease && !lease.current()) return false;
  // Bracketed paste, so a multi-line prompt is one message and not one per line — with every
  // control character but newline and tab removed first. What gets typed includes a background
  // job's own output, and an ESC in it could close the paste early and type the rest as keys.
  return deliver(typers.get(lane), `\x1b[200~${clean(text)}\x1b[201~\r`);
}

/// Paste `text` into the lane's agent without pressing Enter — dropped paths, for the person to
/// finish the sentence around. `false` when the lane has no terminal open.
export function pasteInto(lane: string, text: string, lease?: TerminalLease): boolean {
  if (lease && !lease.current()) return false;
  return deliver(typers.get(lane), `\x1b[200~${clean(text)}\x1b[201~`);
}

/// Text with no terminal control characters left in it: C0 except tab and newline, DEL, and C1.
export function clean(text: string): string {
  // eslint-disable-next-line no-control-regex -- matching control characters is the point.
  return text.replace(/[\x00-\x08\x0b-\x1f\x7f-\x9f]/g, "");
}

export function interruptTerminal(lane: string): boolean {
  return deliver(typers.get(lane), "\x03");
}

export function terminalConnected(lane: string): boolean { return typers.has(lane); }

const exits = new Map<string, Set<() => void>>();

/** Ask the CLI to exit normally (its double Ctrl+C gesture), never kill the socket.
 * A timeout keeps the terminal mounted and its history intact. */
export function exitTerminal(lane: string): Promise<void> {
  const type = typers.get(lane);
  if (!type) return Promise.resolve();
  return new Promise((resolve, reject) => {
    let second: ReturnType<typeof setTimeout> | undefined;
    const finish = (error?: Error) => {
      clearTimeout(timeout); clearTimeout(second);
      exits.get(lane)?.delete(closed);
      if (!exits.get(lane)?.size) exits.delete(lane);
      if (error) reject(error); else resolve();
    };
    const closed = () => finish();
    const timeout = setTimeout(() => finish(new Error("The terminal did not exit. Exit the agent in the terminal, then choose Chat again.")), 10_000);
    const waiting = exits.get(lane) ?? new Set<() => void>();
    waiting.add(closed); exits.set(lane, waiting);
    try {
      if (!deliver(type, "\x03")) throw new Error("The terminal is disconnected.");
      second = setTimeout(() => {
        if (typers.get(lane) !== type) return;
        if (!deliver(type, "\x03")) finish(new Error("The terminal is disconnected."));
      }, 200);
    } catch (error) { finish(error instanceof Error ? error : new Error(String(error))); }
  });
}
