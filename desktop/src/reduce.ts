// A conversation, folded from the daemon's `turn` ops (crates/keel-workspace/src/conversation.rs)
// and its `fact` events (crates/keel/src/turns.rs).
//
// Pure, and immutable in the narrow sense that matters for rendering: an op copies the turn it
// touches and the one block or call inside it, and nothing else. A view subscribed to one block
// re-renders when that block changes and never because a sibling did — which is the whole of what
// made the Swift transcript slow while a reply streamed.

/// `you` is what the person typed while the turn ran, which joined it as the CLI's mid-turn input does.
export type StepKind = "say" | "think" | "you" | "notice";

export interface Block {
  kind: StepKind;
  text: string;
}

export type CallState = "running" | "ok" | "failed" | "interrupted";

export interface Call {
  id: string;
  tool: string;
  parent?: string;
  input?: Record<string, unknown>;
  subject: string;
  reason?: string;
  writes: string[];
  state: CallState;
  output?: string;
  cut?: number;
}

export interface Usage {
  context: number;
  input: number;
  output: number;
  cache_read: number;
  cache_write: number;
  cost?: number;
  ms?: number;
}

export interface Failure {
  kind: string;
  message: string;
  resets_at?: number;
  history: boolean;
}

export interface Gate {
  status: string;
  command?: string | null;
  problems?: { file?: string; line?: number; message?: string }[];
}

export interface Turn {
  /// Client id, stable for React keys.
  id: string;
  /// The opener's uuid once known; facts and ops are filed under it.
  key: string | null;
  prompt: string | null;
  /// Steps in order: `b:<step>` for prose and reasoning, `c:<call id>` for a call.
  steps: string[];
  blocks: Record<string, Block>;
  calls: Record<string, Call>;
  /// A subagent's calls, under the call that started it.
  children: Record<string, string[]>;
  usage?: Usage;
  failures: Failure[];
  files: string[];
  gate?: Gate;
  commit?: string;
  /// Why the turn was not saved, when it was not: said by the daemon as `turn.shared`.
  shared?: string;
  /// The tree before the turn ran, as a git tree id — what "rewind to before this turn" restores.
  snapshot?: string;
  /// When the turn started and how long it took, from the daemon's own facts.
  startedAt?: string;
  ms?: number;
  raw: number;
  /// Why the conversation part ended; absent while it runs.
  closed?: string;
  /// The whole turn, gate and commit included, is over.
  ended: boolean;
}

export interface Conversation {
  turns: Turn[];
  /// Subagents launched in the background and still working, by the call that launched them:
  /// that call answered at once, and the agent runs on across turns until its notification.
  background?: Record<string, string>;
}

export interface Frame {
  turn: string | null;
  op: string;
  [field: string]: unknown;
}

export interface Fact {
  turn: string | null;
  kind: string;
  [field: string]: unknown;
}

let next = 0;
const fresh = (key: string | null, prompt: string | null): Turn => ({
  id: `t${++next}`,
  key,
  prompt,
  steps: [],
  blocks: {},
  calls: {},
  children: {},
  failures: [],
  files: [],
  raw: 0,
  ended: false,
});

/// The turn a frame or fact belongs to.
///
/// By key when a turn has it. A key nobody has yet belongs to the latest turn with no key — closed
/// or not, because a chat turn's ops are keyless and its gate, commit and files are keyed facts
/// that arrive *after* its `close`. A keyless frame belongs to the turn still open.
function locate(turns: Turn[], key: string | null): number {
  if (key) {
    for (let i = turns.length - 1; i >= 0; i--) if (turns[i].key === key) return i;
    for (let i = turns.length - 1; i >= 0; i--) if (turns[i].key === null) return i;
    return -1;
  }
  const last = turns.length - 1;
  return last >= 0 && !turns[last].closed ? last : -1;
}

export function applyFrames(conv: Conversation, frames: Frame[]): Conversation {
  if (frames.length === 0) return conv;
  const turns = conv.turns.slice();
  let background = conv.background;
  for (const f of frames) {
    if (f.op === "background") {
      const id = f.id as string;
      background = { ...background };
      if (f.running) background[id] = (f.agent as string | undefined) ?? "";
      else delete background[id];
      continue;
    }
    if (f.op === "open") {
      // A resumed replay sends the same turn again; it replaces, so a reconnect is idempotent.
      const prompt = (f.prompt as string | null) ?? null;
      const at = f.turn ? turns.findIndex((t) => t.key === f.turn) : -1;
      if (at >= 0) turns[at] = { ...fresh(f.turn, prompt), id: turns[at].id };
      else turns.push(fresh(f.turn, prompt));
      continue;
    }
    if (f.op === "notice") {
      // Said between replies, often before the next turn opens: it belongs where it was said,
      // after the turn on screen, never to a new headless turn of its own.
      const at = Math.max(locate(turns, f.turn), turns.length - 1);
      if (at >= 0) turns[at] = notice(turns[at], f.text as string);
      continue;
    }
    let i = locate(turns, f.turn);
    // A keyless close with nothing open closes nothing: a broken stream's backstop `close` can
    // follow one the turn already had, and folding it drew a phantom empty "interrupted" turn.
    if (i < 0 && f.op === "close" && !f.turn) continue;
    if (i < 0) {
      // Ops for a turn this view never saw open — the head of a cut replay.
      turns.push(fresh(f.turn, null));
      i = turns.length - 1;
    }
    const t = { ...turns[i] };
    if (f.turn && !t.key) t.key = f.turn;
    turns[i] = op(t, f);
  }
  return background ? { turns, background } : { turns };
}

function op(t: Turn, f: Frame): Turn {
  switch (f.op) {
    case "text": {
      const step = f.step as string;
      const id = `b:${step}`;
      const had = t.blocks[step];
      t.blocks = {
        ...t.blocks,
        [step]: { kind: f.kind as StepKind, text: (had?.text ?? "") + (f.append as string) },
      };
      if (!had) t.steps = [...t.steps, id];
      return t;
    }
    case "call": {
      const id = f.id as string;
      const had = t.calls[id];
      const call: Call = {
        id,
        tool: f.tool as string,
        parent: (f.parent as string) ?? had?.parent,
        input: (f.input as Record<string, unknown>) ?? had?.input,
        subject: (f.subject as string) || had?.subject || "",
        reason: (f.reason as string) ?? had?.reason,
        writes: (f.writes as string[]) ?? had?.writes ?? [],
        state: had?.state ?? "running",
        output: had?.output,
        cut: had?.cut,
      };
      t.calls = { ...t.calls, [id]: call };
      if (!had) {
        if (call.parent) {
          t.children = { ...t.children, [call.parent]: [...(t.children[call.parent] ?? []), id] };
        } else {
          t.steps = [...t.steps, `c:${id}`];
        }
      }
      if (call.writes.length) {
        const add = call.writes.filter((w) => !t.files.includes(w));
        if (add.length) t.files = [...t.files, ...add];
      }
      return t;
    }
    case "result": {
      const id = f.id as string;
      const had = t.calls[id];
      if (!had) return t;
      t.calls = {
        ...t.calls,
        [id]: { ...had, state: f.state as CallState, output: f.output as string, cut: f.cut as number | undefined },
      };
      return t;
    }
    case "wrote": {
      const add = (f.paths as string[]).filter((p) => !t.files.includes(p));
      if (add.length) t.files = [...t.files, ...add];
      return t;
    }
    case "usage":
      t.usage = f as unknown as Usage;
      return t;
    case "failure":
      t.failures = [...t.failures, f as unknown as Failure];
      return t;
    case "raw":
      t.raw += (f.lines as unknown[]).length;
      return t;
    case "close": {
      t.closed = f.reason as string;
      // The daemon answers every running call before it closes; this is the backstop for a
      // stream that broke before it could.
      t.calls = settle(t.calls);
      return t;
    }
    default:
      return t;
  }
}

function notice(turn: Turn, text: string): Turn {
  const step = `notice:${turn.steps.length}`;
  return { ...turn, blocks: { ...turn.blocks, [step]: { kind: "notice", text } }, steps: [...turn.steps, `b:${step}`] };
}

function settle(calls: Record<string, Call>): Record<string, Call> {
  let out = calls;
  for (const [id, c] of Object.entries(calls)) {
    if (c.state === "running") {
      if (out === calls) out = { ...calls };
      out[id] = { ...c, state: "interrupted" };
    }
  }
  return out;
}

/// A stream ended without closing its turn — the connection dropped, the daemon died. The same
/// rule the daemon applies, applied once here, so no row is left spinning whatever happened.
export function interrupt(conv: Conversation): Conversation {
  const last = conv.turns[conv.turns.length - 1];
  if (!last || last.closed) return conv;
  const turns = conv.turns.slice();
  turns[turns.length - 1] = { ...last, closed: "interrupted", calls: settle(last.calls) };
  return { ...conv, turns };
}

export function applyFact(conv: Conversation, fact: Fact): Conversation {
  const turns = conv.turns.slice();
  const i = locate(turns, fact.turn);
  if (i < 0) return conv;
  const t = { ...turns[i] };
  // Kept whatever the fact is: an `approval.asked` is often the first keyed thing a chat turn
  // hears, and every fact after it finds the turn by this.
  const learned = !!fact.turn && !t.key;
  if (learned) t.key = fact.turn;
  switch (fact.kind) {
    case "turn.files":
      t.files = Array.from(new Set([...t.files, ...(fact.files as string[])]));
      break;
    case "turn.gate":
      t.gate = fact as unknown as Gate;
      break;
    case "turn.commit":
      t.commit = fact.sha as string;
      break;
    case "turn.shared":
      t.shared = fact.reason as string;
      break;
    case "turn.started":
      t.snapshot = (fact.snapshot as string | null) ?? undefined;
      t.startedAt = fact.started as string;
      break;
    case "turn.ended":
      t.ended = true;
      if (typeof fact.ms === "number") t.ms = fact.ms;
      break;
    case "turn.usage":
      if (t.usage?.cost === undefined && typeof fact.cost_usd === "number") {
        t.usage = { ...(t.usage ?? { context: 0, input: 0, output: 0, cache_read: 0, cache_write: 0 }), cost: fact.cost_usd };
      }
      break;
    default:
      if (!learned) return conv;
  }
  turns[i] = t;
  return { ...conv, turns };
}

/// What the daemon appends to a prompt that carried file references (`references::provider_prompt`):
/// the transcript holds the long form, the view the short one.
const REFERENCES = "\n\nReferenced files in this checkout (JSON paths):\n";

/// A prompt reduced to the person's words, the same from either side. The transcript joins a
/// message's blocks with a space — so an image adds a trailing one and any other file adds
/// " Attached file: <path>" — holds the referenced files the view leaves out, and both carry Keel's
/// note naming the uploads. Compared raw, every turn that carried an attachment matched nothing
/// after a redraw and came back without its files, checks, commit and rewind.
const ATTACHED = /\s+Attached file: [\s\S]*$/;
const UPLOADS = /\n*Attached files in this checkout \([^)]*\):\n[\s\S]*$/;
function words(prompt: string): string {
  const cut = prompt.replace(ATTACHED, "").replace(UPLOADS, "");
  const references = cut.indexOf(REFERENCES);
  return (references < 0 ? cut : cut.slice(0, references)).trim();
}

/// The images a prompt named in Keel's note, so the same words sent with two different screenshots
/// stay two turns. A non-image file is named only on the transcript side, so it is not compared.
const uploads = (prompt: string) => [...prompt.replace(ATTACHED, "").matchAll(/\.keel\/attachments\/[^\s"'<>]+/g)].map((m) => m[0]).sort().join("\n");

function samePrompt(shown: string | null, written: string | null): boolean {
  return shown === written || (!!shown && !!written && words(shown) === words(written) && uploads(shown) === uploads(written));
}

/// How a turn ended, kept from the live view when the transcript cannot say: a failed or stopped
/// turn's history usually ends without its end marker, and redrawn as "done" it lost its error.
const KEPT_ENDINGS = new Set(["failed", "stopped", "interrupted"]);
function ending(old: Turn, restored: Turn): string | undefined {
  if (old.closed && KEPT_ENDINGS.has(old.closed)) return old.closed;
  return restored.closed === "superseded" && old.closed ? old.closed : restored.closed;
}

/// A turn Keel drew that the provider never wrote: refused before it started, so it has no
/// steps. The transcript cannot bring it back, and dropping it lost the message and its reason.
const keelOnly = (t: Turn) => t.closed === "failed" && t.steps.length === 0;

/// A turn the transcript holds, carrying what only the live view knew: Keel's facts, the error a
/// failed turn ended on, and the cost.
function merged(old: Turn, turn: Turn): Turn {
  const cost = turn.usage?.cost ?? old.usage?.cost;
  return { ...turn, id: old.id, prompt: old.prompt, closed: ending(old, turn), files: old.files, gate: old.gate, commit: old.commit, shared: old.shared,
    snapshot: old.snapshot, startedAt: old.startedAt, ms: old.ms, ended: old.ended,
    failures: turn.failures.length ? turn.failures : old.failures,
    ...(turn.usage || old.usage ? { usage: { ...(old.usage ?? turn.usage)!, ...turn.usage, cost } } : {}) };
}

/** What the transcript gained since the journal last read it (`append`): every turn the view has
 *  stays, a gained turn the view already drew (the one that ended as the reply landed) is merged
 *  into it, and the rest go after. */
export function appendTranscript(previous: Conversation, frames: Frame[]): Conversation {
  // Background agents carry over: a notification in the gained part is what settles one.
  const gained = applyFrames({ turns: [], ...(previous.background ? { background: previous.background } : {}) }, frames);
  const turns = previous.turns.slice();
  // Only the view's last turns can be ones the gained part repeats — the turn that ended as it
  // was read. Matching from the top merged a gained "continue" into the first "continue" ever sent.
  let j = Math.max(0, previous.turns.length - gained.turns.length);
  for (const turn of gained.turns) {
    const at = turns.findIndex((old, i) => i >= j && i < previous.turns.length && !keelOnly(old) && samePrompt(old.prompt, turn.prompt));
    if (at < 0) { turns.push(turn); j = previous.turns.length; continue; }
    turns[at] = merged(turns[at], turn);
    j = at + 1;
  }
  return gained.background ? { turns, background: gained.background } : { ...previous, turns };
}

/** Refresh provider history after a terminal handoff, retaining Keel's matching turn metadata.
 *  Turns are matched in order by prompt, not by position: one Keel-only turn earlier in the view
 *  shifted every index after it, and each turn after that lost its checks, commit and id — a
 *  new id is a remount, which is the scroll jumping. */
export function restoreTranscript(previous: Conversation, frames: Frame[]): Conversation {
  const restored = applyFrames({ turns: [] }, frames);
  const turns: Turn[] = [];
  let j = 0;
  for (const turn of restored.turns) {
    // A Keel-only turn is never what the provider wrote, even when the same words were sent again.
    const at = previous.turns.findIndex((old, i) => i >= j && !keelOnly(old) && samePrompt(old.prompt, turn.prompt));
    if (at < 0) { turns.push(turn); continue; }
    // The transcript is read from its tail, so a long one starts partway through: the turns the
    // view has from before its first one are kept, not dropped at the next redraw.
    turns.push(...(turns.length === 0 ? previous.turns.slice(0, at) : previous.turns.slice(j, at).filter(keelOnly)));
    j = at + 1;
    turns.push(merged(previous.turns[at], turn));
  }
  turns.push(...previous.turns.slice(j).filter(keelOnly));
  return restored.background ? { turns, background: restored.background } : { turns };
}
