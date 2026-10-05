// A conversation, folded from the daemon's `turn` ops (crates/keel-workspace/src/conversation.rs)
// and its `fact` events (crates/keel/src/turns.rs).
//
// Pure, and immutable in the narrow sense that matters for rendering: an op copies the turn it
// touches and the one block or call inside it, and nothing else. A view subscribed to one block
// re-renders when that block changes and never because a sibling did — which is the whole of what
// made the Swift transcript slow while a reply streamed.

export type StepKind = "say" | "think";

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
  for (const f of frames) {
    if (f.op === "open") {
      // A resumed replay sends the same turn again; it replaces, so a reconnect is idempotent.
      const prompt = (f.prompt as string | null) ?? null;
      const at = f.turn ? turns.findIndex((t) => t.key === f.turn) : -1;
      if (at >= 0) turns[at] = { ...fresh(f.turn, prompt), id: turns[at].id };
      else turns.push(fresh(f.turn, prompt));
      continue;
    }
    let i = locate(turns, f.turn);
    if (i < 0) {
      // Ops for a turn this view never saw open — the head of a cut replay.
      turns.push(fresh(f.turn, null));
      i = turns.length - 1;
    }
    const t = { ...turns[i] };
    if (f.turn && !t.key) t.key = f.turn;
    turns[i] = op(t, f);
  }
  return { turns };
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
  return { turns };
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
  return { turns };
}
