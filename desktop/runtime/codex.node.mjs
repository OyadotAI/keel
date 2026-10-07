import { test } from 'node:test';
import assert from 'node:assert/strict';
import { Codex, historyFrames } from './codex.mjs';

test('streamed text is not duplicated by the completed item', async () => {
  const events = [];
  const c = new Codex((event, data) => events.push({ event, data }), () => {});
  await c.message({ method: 'item/agentMessage/delta', params: { itemId: 'm1', delta: '**Hello**' } });
  await c.message({ method: 'item/completed', params: { item: { id: 'm1', type: 'agentMessage', text: '**Hello**' } } });
  assert.equal(events.length, 1);
  assert.equal(events[0].data[0].append, '**Hello**');
});

test('approval returns the exact provider decision and request id', async () => {
  const sent = [];
  const amendment = { acceptWithExecpolicyAmendment: { execpolicy_amendment: ['git', 'fetch'] } };
  let offered;
  const c = new Codex(() => {}, async (_tool, _command, _input, choices) => { offered = choices; return { decision: amendment }; });
  c.write = (value) => sent.push(value);
  await c.message({ id: 47, method: 'item/commandExecution/requestApproval', params: { command: 'git fetch', availableDecisions: [amendment, 'decline'] } });
  assert.deepEqual(offered[0].value, amendment);
  assert.deepEqual(sent, [{ id: 47, result: { decision: amendment } }]);
});

test('multi-question answers retain provider question ids', async () => {
  const sent = [];
  const c = new Codex(() => {}, async () => ({ answers: { 'Which branch?': 'develop', 'Run checks?': 'Yes' } }));
  c.write = (value) => sent.push(value);
  await c.message({ id: 9, method: 'item/tool/requestUserInput', params: { questions: [{ id: 'branch', question: 'Which branch?' }, { id: 'checks', question: 'Run checks?' }] } });
  assert.deepEqual(sent[0].result.answers, { branch: { answers: ['develop'] }, checks: { answers: ['Yes'] } });
});

test('unknown server requests fail explicitly without authorizing work', async () => {
  const sent = [], events = [];
  const c = new Codex((event) => events.push(event), () => { throw new Error('must not approve'); });
  c.write = (value) => sent.push(value);
  await c.message({ id: 99, method: 'new/approval', params: {} });
  assert.equal(sent[0].error.code, -32601);
  assert.ok(events.includes('fatal'));
});

test('identical question labels retain distinct answers by provider id', async () => {
  const sent = [];
  const c = new Codex(() => {}, async () => ({ answers: { first: 'A', second: 'B' } }));
  c.write = (value) => sent.push(value);
  await c.message({ id: 10, method: 'item/tool/requestUserInput', params: { questions: [{ id: 'first', question: 'Choose' }, { id: 'second', question: 'Choose' }] } });
  assert.deepEqual(sent[0].result.answers, { first: { answers: ['A'] }, second: { answers: ['B'] } });
});

test('interrupt and steer target the current thread and turn', async () => {
  const requests = [];
  const c = new Codex(() => {}, () => {});
  c.thread = 'thread-1'; c.turn = 'turn-2';
  c.rpc = async (method, params) => requests.push({ method, params });
  await c.steer('Use the existing component'); await c.interrupt();
  assert.equal(requests[0].params.expectedTurnId, 'turn-2');
  assert.equal(requests[1].params.threadId, 'thread-1');
});

test('resumed terminal history retains prompt, tools and final reply in order', () => {
  const frames = historyFrames([{ id: 't1', status: 'completed', items: [
    { type: 'userMessage', content: [{ type: 'text', text: 'Inspect it' }] },
    { type: 'commandExecution', id: 'c1', command: 'pwd', status: 'completed', aggregatedOutput: '/repo' },
    { type: 'agentMessage', id: 'm1', text: 'Done' },
  ] }]);
  assert.deepEqual(frames.map((f) => f.op), ['open', 'call', 'result', 'text', 'close']);
  assert.equal(frames[0].prompt, 'Inspect it');
  assert.ok(frames.every((f) => f.turn === 't1'));
});
