// What each lane's agent terminal accepts as typed input. Keel's own prompts — "fix this
// finding", a background job's report — go in the way a person's would, into the CLI's own
// composer, so the CLI queues them, shows them and owns them like anything else typed there.
const typers = new Map<string, (text: string) => void>();

export function register(lane: string, type: (text: string) => void): () => void {
  typers.set(lane, type);
  return () => {
    if (typers.get(lane) === type) typers.delete(lane);
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
