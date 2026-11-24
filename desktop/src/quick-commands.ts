// Quick commands: per-project saved shell commands, run from the palette in the lane's shell.
// They go to the shell and never to the agent terminal, where typed text becomes a message — so a
// command works the same whichever agent the lane runs. Kept in this machine's storage, outside
// the repository, keyed by project path; a moved project leaves its commands under the old path.
import { clean } from "./typers";

export interface QuickCommand { name: string; command: string }

const KEY = "keel.quick-commands";
/// How long a command waits for a shell that is still starting before it says it did not run.
export const SHELL_WAIT_MS = 10_000;

function readAll(): Record<string, QuickCommand[]> {
  try {
    const parsed: unknown = JSON.parse(localStorage.getItem(KEY) ?? "{}");
    return parsed && typeof parsed === "object" ? (parsed as Record<string, QuickCommand[]>) : {};
  } catch {
    return {};
  }
}

/// The project's commands, in the order they were added.
export function quickCommands(project: string): QuickCommand[] {
  const list = readAll()[project];
  return Array.isArray(list) ? list.filter((c) => typeof c?.name === "string" && typeof c?.command === "string") : [];
}

function write(project: string, list: QuickCommand[]) {
  const all = readAll();
  if (list.length) all[project] = list;
  else delete all[project];
  localStorage.setItem(KEY, JSON.stringify(all));
}

/// Adds one; throws with the sentence to show when the name is taken or either half is blank.
export function addQuickCommand(project: string, c: QuickCommand) {
  const name = c.name.trim(), command = c.command.trim();
  if (!name || !command) throw new Error("A quick command needs a name and a command.");
  const list = quickCommands(project);
  if (list.some((x) => x.name === name)) throw new Error(`A quick command named “${name}” already exists.`);
  write(project, [...list, { name, command }]);
}

export function removeQuickCommand(project: string, name: string) {
  write(project, quickCommands(project).filter((c) => c.name !== name));
}

// --- Delivery to the lane's shell ----------------------------------------------------------------

type Write = (text: string) => boolean | void;
const shells = new Map<string, Write>();
const pending = new Map<string, { c: QuickCommand; timer: ReturnType<typeof setTimeout> }>();
const failures = new Map<string, string>();
const listeners = new Set<() => void>();
const changed = () => listeners.forEach((l) => l());

/// Why the lane's last quick command did not run, for the shell's status line; cleared on success.
export function quickFailure(lane: string): string | undefined { return failures.get(lane); }
export function onQuickFailure(listener: () => void) { listeners.add(listener); return () => void listeners.delete(listener); }

function fail(lane: string, c: QuickCommand, why: string) {
  failures.set(lane, `${c.name} didn't run: ${why}`);
  changed();
}

function deliver(lane: string, write: Write, c: QuickCommand) {
  let ok = false;
  try { ok = write(`${clean(c.command)}\r`) !== false; } catch { ok = false; }
  if (ok) { if (failures.delete(lane)) changed(); }
  else fail(lane, c, "the shell is disconnected.");
}

/// The shell's socket, while it is open. A command waiting for it is typed now.
export function registerShell(lane: string, write: Write) {
  shells.set(lane, write);
  const waiting = pending.get(lane);
  if (waiting) {
    clearTimeout(waiting.timer);
    pending.delete(lane);
    deliver(lane, write, waiting.c);
    changed();
  }
  return () => {
    if (shells.get(lane) === write) shells.delete(lane);
  };
}

/// The shell could not start: a command waiting for it ends now, saying so.
export function shellFailed(lane: string, error: string) {
  const waiting = pending.get(lane);
  if (!waiting) return;
  clearTimeout(waiting.timer);
  pending.delete(lane);
  fail(lane, waiting.c, `the shell couldn't start. ${error}`);
}

/// Types the command and Enter into the lane's shell, opening it first when it is closed. One
/// command waits per lane; every wait ends — on delivery, on a start failure, or at the ceiling.
export function runQuickCommand(lane: string, c: QuickCommand, openShell: () => void) {
  // Always on screen: a shell kept alive behind a closed dock would run it where nobody can see.
  openShell();
  const write = shells.get(lane);
  if (write) return deliver(lane, write, c);
  const before = pending.get(lane);
  if (before) clearTimeout(before.timer);
  const timer = setTimeout(() => {
    if (pending.get(lane)?.timer !== timer) return;
    pending.delete(lane);
    fail(lane, c, "the shell did not open in time.");
  }, SHELL_WAIT_MS);
  pending.set(lane, { c, timer });
  changed();
}

/// Whether a command is waiting for this lane's shell — an exited shell starts again for it.
export function quickWaiting(lane: string): boolean { return pending.has(lane); }
