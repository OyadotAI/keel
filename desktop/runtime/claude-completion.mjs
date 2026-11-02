// A parent result is held while its background agents or unanswered callbacks still own the turn.
const TERMINAL = new Set(['completed', 'failed', 'stopped']);
const isAgent = (task) => !task.ambient && !task.skip_transcript && (typeof task.subagent_type === 'string' || /agent|teammate/i.test(task.task_type ?? ''));

/** Finish once from actual lifecycle evidence; accepting an answer is never completion evidence. */
export class ClaudeCompletion {
  constructor(finish, cancel) { this.finish = finish; this.cancel = cancel; this.processStart(); }
  processStart() { this.level = false; this.agents = new Map(); this.callbacks = new Map(); this.begin(); }
  begin() { this.result = undefined; this.done = false; this.questionBarrier = false; this.agents.clear(); this.callbacks.clear(); }
  callback(event, id, fields) {
    if (event === 'start') {
      this.callbacks.set(id, fields);
      if (this.result !== undefined) this.questionBarrier = true;
    } else this.callbacks.delete(id);
  }
  message(message) {
    if (this.done) return;
    if (message.type === 'result') {
      this.result = message.is_error ? 1 : 0;
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
    if (this.done || this.result === undefined || this.callbacks.size) return;
    if ([...this.agents.values()].some((task) => !task.ended && task.is_backgrounded !== false)) return;
    if (this.questionBarrier && source !== 'snapshot') return;
    this.complete(this.result, 'The provider completed its background work.');
  }
  complete(code, reason) {
    if (this.done) return;
    this.done = true; this.cancel(reason); this.callbacks.clear(); this.agents.clear(); this.finish(code);
  }
  shutdown(reason = 'The provider session closed.') { this.complete(1, reason); }
}
