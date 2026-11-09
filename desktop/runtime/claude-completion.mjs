// A parent result is held while its background agents or unanswered callbacks still own the turn.
const TERMINAL = new Set(['completed', 'failed', 'stopped']);
const isAgent = (task) => !task.ambient && !task.skip_transcript && (typeof task.subagent_type === 'string' || /agent|teammate/i.test(task.task_type ?? ''));

/** Finish once from actual lifecycle evidence; accepting an answer is never completion evidence. */
/** How long a result after a steer must stand with no new reply before the turn ends. The CLI
 *  starts a queued message's reply immediately after a result, so quiet this long means the steer
 *  was folded into the reply that just ended. */
export const STEER_QUIET_MS = 2000;
/** What says a new reply is under way: anything the model says, not bookkeeping. */
const REPLYING = new Set(['assistant', 'stream_event', 'user']);
export class ClaudeCompletion {
  constructor(finish, cancel, clock = { set: setTimeout, clear: clearTimeout }) { this.finish = finish; this.cancel = cancel; this.clock = clock; this.processStart(); }
  processStart() { this.level = false; this.agents = new Map(); this.callbacks = new Map(); this.begin(); }
  begin() { this.clock.clear(this.quiet); this.quiet = undefined; this.steered = false; this.result = undefined; this.done = false; this.interrupted = false; this.questionBarrier = false; this.agents.clear(); this.callbacks.clear(); }
  /** Stop was pressed: the turn ends at its result, as Esc ends it in the CLI. Background agents
   *  keep running in the session; they no longer hold the person's turn. */
  interrupt() { this.interrupted = true; if (this.result !== undefined) this.complete(-1, 'The turn was cancelled.'); }
  callback(event, id, fields) {
    if (event === 'start') {
      this.callbacks.set(id, fields);
      if (this.result !== undefined) this.questionBarrier = true;
    } else this.callbacks.delete(id);
  }
  /** Words were pushed into this turn. A result no longer ends it on its own: the CLI may answer
   *  them as a reply of their own after that result, and a turn that ended first left that reply
   *  running with nobody listening — and no claim on the tree it could write. */
  steer() { this.steered = true; }
  message(message) {
    if (this.done) return;
    if (this.quiet !== undefined && REPLYING.has(message.type)) {
      // The steer's own reply has started: the turn is not over until it is.
      this.clock.clear(this.quiet); this.quiet = undefined; this.result = undefined;
    }
    if (message.type === 'result') {
      this.result = message.is_error ? 1 : 0;
      if (this.interrupted) { this.complete(-1, 'The turn was cancelled.'); return; }
      if (this.steered) { this.clock.clear(this.quiet); this.quiet = this.clock.set(() => { this.quiet = undefined; this.check('result'); }, STEER_QUIET_MS); return; }
      if (this.callbacks.size) this.questionBarrier = true;
      this.check('result'); return;
    }
    if (message.type !== 'system') return;
    if (message.subtype === 'session_state_changed' && message.state === 'idle' && this.result !== undefined && !this.callbacks.size) {
      this.complete(this.result, 'The provider completed this session’s work.'); return;
    }
    if (message.subtype === 'background_tasks_changed') {
      this.level = true;
      this.agents = new Map((message.tasks ?? []).filter(isAgent).map((task) => [task.task_id, task]));
      this.check('snapshot'); return;
    }
    if (this.level || !message.task_id) return;
    const previous = this.agents.get(message.task_id);
    if (previous?.run_id && message.run_id && message.run_id < previous.run_id) return;
    if (previous?.ended && previous.run_id && message.run_id === previous.run_id && message.subtype !== 'task_notification') return;
    // Keep foreground identity: later background patches contain no task type or subagent name.
    if (message.subtype === 'task_started' && isAgent(message)) this.agents.set(message.task_id, message);
    else if (message.subtype === 'task_updated' && message.patch?.is_backgrounded === true && isAgent({ ...previous, ...message.patch })) this.agents.set(message.task_id, { ...previous, ...message.patch, run_id: message.run_id ?? previous?.run_id });
    else if (message.subtype === 'task_notification' && TERMINAL.has(message.status)) {
      // Retain the last run as a tombstone so an older start/notification cannot revive it.
      this.agents.set(message.task_id, { ...previous, run_id: message.run_id ?? previous?.run_id, ended: true });
    }
    this.check('task');
  }
  check(source) {
    if (this.done || this.result === undefined || this.callbacks.size || this.quiet !== undefined) return;
    if ([...this.agents.values()].some((task) => !task.ended && task.is_backgrounded !== false)) return;
    if (this.questionBarrier && source !== 'snapshot') return;
    this.complete(this.result, 'The provider completed its background work.');
  }
  complete(code, reason) {
    if (this.done) return;
    this.clock.clear(this.quiet); this.quiet = undefined;
    this.done = true; this.cancel(reason); this.callbacks.clear(); this.agents.clear(); this.finish(code);
  }
  shutdown(reason = 'The provider session closed.') { this.complete(1, reason); }
}
