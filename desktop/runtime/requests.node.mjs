// Provider callback contracts use fake time and signals, without a model turn or real session.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { Requests } from './requests.mjs';

test('a custom answer is still delivered after the short reported timeout', async (t) => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const events = [], requests = new Requests((event, data) => events.push({ event, data }));
  const asking = requests.ask({ tool: 'AskUserQuestion', input: {}, kind: 'question' });
  t.mock.timers.tick(61_000);
  const id = events[0].data.id;
  assert.equal(requests.pending.size, 1);
  assert.deepEqual(requests.answer(id, { answers: { q: 'My own\nmultiline answer' } }), { ok: true });
  assert.equal((await asking).answers.q, 'My own\nmultiline answer');
});

test('SDK cancellation withdraws the card and refuses a later answer', async () => {
  const events = [], requests = new Requests((event, data) => events.push({ event, data }));
  const controller = new AbortController();
  const closed = assert.rejects(requests.ask({ tool: 'AskUserQuestion' }, controller.signal), /cancelled/);
  const id = events[0].data.id;
  controller.abort(); await closed;
  assert.equal(requests.pending.size, 0);
  assert.equal(events.at(-1).data.answered, false);
  assert.equal(requests.answer(id, { answers: { q: 'Too late' } }).ok, false);
});

test('retrying after a dropped acknowledgement accepts the same answer once', async () => {
  const events = [], requests = new Requests((event, data) => events.push({ event, data }));
  const asking = requests.ask({ tool: 'AskUserQuestion' });
  const id = events[0].data.id, answer = { answers: { q: 'Yes' } };
  requests.answer(id, answer);
  assert.deepEqual(requests.answer(id, answer), { ok: true });
  assert.equal(requests.answer(id, { answers: { q: 'No' } }).ok, false);
  await asking;
  assert.equal(events.filter((e) => e.event === 'resolved').length, 1);
  assert.deepEqual(events.at(-1).data.answer, answer);
});

test('a completed subagent does not cancel another thread question', async () => {
  const events = [], requests = new Requests((event, data) => events.push({ event, data }));
  const child = assert.rejects(requests.ask({ input: { threadId: 'child' } }), /ended/);
  const parent = requests.ask({ input: { threadId: 'parent' } });
  requests.cancelAll('The turn ended.', 'child'); await child;
  assert.equal(requests.pending.size, 1);
  requests.answer(events.find((e) => e.event === 'request' && e.data.input.threadId === 'parent').data.id, { answers: {} });
  await parent;
});

test('completed delivery retention is bounded for long-lived runtimes', async () => {
  let id;
  const requests = new Requests((event, data) => { if (event === 'request') id = data.id; });
  for (let i = 0; i < 150; i++) {
    const answer = requests.ask({ input: {} });
    requests.answer(id, { answers: { q: String(i) } }); await answer;
  }
  assert.ok(requests.completed.size <= 128);
});
