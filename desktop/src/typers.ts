// What each lane's agent terminal accepts as typed input. Keel's own prompts — "fix this
// finding", a background job's report — go in the way a person's would, into the CLI's own
// composer, so the CLI queues them, shows them and owns them like anything else typed there.
const typers = new Map<string, (text: string) => void>();

export function register(lane: string, type: (text: string) => void): () => void {
  typers.set(lane, type);
  return () => {
    if (typers.get(lane) === type) {
      typers.delete(lane);
      for (const closed of [...(exits.get(lane) ?? [])]) closed();
    }
  };
}

/// Type `text` into the lane's agent and press Enter. `false` when the lane has no terminal open.
export function typeInto(lane: string, text: string): boolean {
  const type = typers.get(lane);
  if (!type) return false;
  // Bracketed paste, so a multi-line prompt is one message and not one per line — with every
  // control character but newline and tab removed first. What gets typed includes a background
  // job's own output, and an ESC in it could close the paste early and type the rest as keys.
  type(`\x1b[200~${clean(text)}\x1b[201~\r`);
  return true;
}

/// Paste `text` into the lane's agent without pressing Enter — dropped paths, for the person to
/// finish the sentence around. `false` when the lane has no terminal open.
export function pasteInto(lane: string, text: string): boolean {
  const type = typers.get(lane);
  if (!type) return false;
  type(`\x1b[200~${clean(text)}\x1b[201~`);
  return true;
}

/// Text with no terminal control characters left in it: C0 except tab and newline, DEL, and C1.
export function clean(text: string): string {
  // eslint-disable-next-line no-control-regex -- matching control characters is the point.
  return text.replace(/[\x00-\x08\x0b-\x1f\x7f-\x9f]/g, "");
}

export function interruptTerminal(lane: string): boolean {
  const type = typers.get(lane);
  if (!type) return false;
  type("\x03");
  return true;
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
    const timeout = setTimeout(() => finish(new Error("The terminal did not exit. Exit Codex in the terminal, then choose Chat again.")), 10_000);
    const waiting = exits.get(lane) ?? new Set<() => void>();
    waiting.add(closed); exits.set(lane, waiting);
    try {
      type("\x03");
      second = setTimeout(() => {
        if (typers.get(lane) !== type) return;
        try { type("\x03"); } catch (error) { finish(error instanceof Error ? error : new Error(String(error))); }
      }, 200);
    } catch (error) { finish(error instanceof Error ? error : new Error(String(error))); }
  });
}
