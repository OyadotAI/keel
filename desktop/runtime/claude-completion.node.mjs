// Completion evidence is independent from callback delivery and from ambient process lifetimes.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { ClaudeCompletion } from './claude-completion.mjs';
const result = { type: 'result', is_error: false };
const system = (subtype, fields = {}) => ({ type: 'system', subtype, ...fields });
function fixture() {
  const ends = [], cancellations = [];
  const completion = new ClaudeCompletion((code) => ends.push(code), (reason) => cancellations.push(reason));
  return { completion, ends, cancellations };
}
test('ordinary results complete once and a new turn has no stale completion', () => {
  const f = fixture(); f.completion.message(result); f.completion.shutdown(); assert.deepEqual(f.ends, [0]);
  f.completion.begin(); f.completion.message(result); assert.deepEqual(f.ends, [0, 0]);
});
test('parent result and answer acceptance retain unknown callback ownership until session idle', () => {
  const f = fixture(); f.completion.callback('start', 'q', { agentID: 'unknown' }); f.completion.message(result);
  f.completion.callback('end', 'q'); f.completion.message(system('task_notification', { task_id: 'unrelated', status: 'completed' }));
  assert.deepEqual(f.ends, []);
  f.completion.message(system('session_state_changed', { state: 'idle' })); assert.deepEqual(f.ends, [0]);
});
test('an earlier idle cannot complete a later unanswered question', () => {
  const f = fixture(); f.completion.message(system('session_state_changed', { state: 'idle' }));
  f.completion.callback('start', 'q', {}); f.completion.message(result); f.completion.callback('end', 'q'); assert.deepEqual(f.ends, []);
});
test('idle during an unanswered question is discarded rather than completing the accepted answer', () => {
  const f = fixture(); f.completion.callback('start', 'q', {}); f.completion.message(result);
  f.completion.message(system('session_state_changed', { state: 'idle' }));
  assert.equal(f.completion.callbacks.has('q'), true); assert.deepEqual(f.ends, []); assert.deepEqual(f.cancellations, []);
  f.completion.callback('end', 'q'); assert.deepEqual(f.ends, []);
  f.completion.message(system('session_state_changed', { state: 'idle' })); assert.deepEqual(f.ends, [0]);
});
test('replacement levels dominate bookends and exclude ambient servers', () => {
  const f = fixture();
  f.completion.message(system('background_tasks_changed', { tasks: [{ task_id: 'one', task_type: 'local_agent' }, { task_id: 'server', task_type: 'local_bash' }] }));
  f.completion.message(result);
  f.completion.message(system('task_notification', { task_id: 'one', status: 'completed' })); assert.deepEqual(f.ends, []);
  f.completion.message(system('background_tasks_changed', { tasks: [{ task_id: 'server', task_type: 'local_bash' }, { task_id: 'watcher', task_type: 'local_agent', ambient: true }] }));
  assert.deepEqual(f.ends, [0]);
});
test('a snapshot before callback resolution cannot be reused as answer completion', () => {
  const f = fixture(); f.completion.callback('start', 'q', {}); f.completion.message(result);
  f.completion.message(system('background_tasks_changed', { tasks: [] })); f.completion.callback('end', 'q'); assert.deepEqual(f.ends, []);
  f.completion.message(system('background_tasks_changed', { tasks: [] })); assert.deepEqual(f.ends, [0]);
});
test('legacy agents complete only after all current runs settle, ignoring late old runs', () => {
  const f = fixture();
  for (const [task_id, run_id] of [['a', '002'], ['b', '001']]) f.completion.message(system('task_started', { task_id, run_id, task_type: 'local_agent', is_backgrounded: true }));
  f.completion.message(result);
  f.completion.message(system('task_notification', { task_id: 'a', run_id: '001', status: 'completed' }));
  f.completion.message(system('task_notification', { task_id: 'b', run_id: '001', status: 'completed' })); assert.deepEqual(f.ends, []);
  f.completion.message(system('task_notification', { task_id: 'a', run_id: '002', status: 'completed' })); assert.deepEqual(f.ends, [0]);
});
test('a foreground agent promoted by a type-free legacy patch retains background ownership', () => {
  const f = fixture();
  f.completion.message(system('task_started', { task_id: 'agent', task_type: 'local_agent', is_backgrounded: false }));
  f.completion.message(system('task_updated', { task_id: 'agent', patch: { is_backgrounded: true, status: 'running' } }));
  f.completion.message(result); assert.deepEqual(f.ends, []);
  f.completion.message(system('task_notification', { task_id: 'agent', status: 'completed' })); assert.deepEqual(f.ends, [0]);
});
test('a foreground-only task does not retain a completed parent turn', () => {
  const f = fixture();
  f.completion.message(system('task_started', { task_id: 'agent', task_type: 'local_agent', is_backgrounded: false }));
  f.completion.message(result); assert.deepEqual(f.ends, [0]);
});
test('provider failure cancels callbacks before its single completion', () => {
  const events = []; const completion = new ClaudeCompletion(() => events.push('end'), () => events.push('cancel'));
  completion.callback('start', 'q', {}); completion.message(result); completion.shutdown(); completion.shutdown();
  assert.deepEqual(events, ['cancel', 'end']);
});
test('a stopped turn ends at its result while background agents keep running', () => {
  const f = fixture();
  f.completion.message(system('task_started', { task_id: 'bg', subagent_type: 'general', is_backgrounded: true }));
  f.completion.interrupt(); assert.deepEqual(f.ends, []);
  f.completion.message(result); assert.deepEqual(f.ends, [-1]);
});
test('a stop after the result ends a turn held by background agents at once', () => {
  const f = fixture();
  f.completion.message(system('task_started', { task_id: 'bg', subagent_type: 'general', is_backgrounded: true }));
  f.completion.message(result); assert.deepEqual(f.ends, []);
  f.completion.interrupt(); assert.deepEqual(f.ends, [-1]);
  f.completion.begin(); f.completion.message(result); assert.deepEqual(f.ends, [-1, 0]);
});
function steered() {
  const ends = [], timers = [];
  const clock = { set: (fn) => { timers.push(fn); return timers.length; }, clear: () => {} };
  const completion = new ClaudeCompletion((code) => ends.push(code), () => {}, clock);
  return { completion, ends, fire: () => timers.splice(0).forEach((fn) => fn()) };
}
test('a steered turn ends only when its result stands with no new reply', () => {
  const f = steered(); f.completion.steer(); f.completion.message(result);
  assert.deepEqual(f.ends, []); f.fire(); assert.deepEqual(f.ends, [0]);
});
test('a steer answered as a reply of its own keeps the turn open until that reply ends', () => {
  const f = steered(); f.completion.steer(); f.completion.message(result);
  f.completion.message({ type: 'stream_event' }); f.fire(); assert.deepEqual(f.ends, [], 'the first result ended a turn still replying');
  f.completion.message(result); f.fire(); assert.deepEqual(f.ends, [0]);
});
test('a steer landing on a held result keeps the turn open for its reply', () => {
  const f = steered(); f.completion.callback('start', 'q', {}); f.completion.message(result);
  f.completion.steer(); f.completion.callback('end', 'q'); f.completion.message({ type: 'system', subtype: 'session_state_changed', state: 'idle' });
  assert.deepEqual(f.ends, [], 'the turn ended under a steered reply');
  f.completion.message({ type: 'stream_event' }); f.fire(); assert.deepEqual(f.ends, []);
  f.completion.message(result); f.fire(); f.completion.message({ type: 'system', subtype: 'session_state_changed', state: 'idle' });
  assert.deepEqual(f.ends, [0]);
});
