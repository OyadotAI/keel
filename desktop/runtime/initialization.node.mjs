// Launch requests are bounded and correlated; missing restrictions never create a provider.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { Initialization } from './initialization.mjs';

test('only a correlated complete launch response can initialize Codex', async () => {
  const events = [];
  const init = new Initialization((event, data) => events.push({ event, data }));
  const pending = init.launch({ provider: 'codex', model: 'chosen', mode: 'plan' });
  init.complete({ id: 'stale', launch: {} });
  const launch = { system: 'preserved instructions', codexArgs: ['-c', 'browser=false'], mcp: { mcpServers: { keel: { url: 'http://127.0.0.1/mcp?binding=fresh' } } } };
  init.complete({ id: events[0].data.id, launch });
  const { assertLaunch, ...prepared } = await pending;
  assertLaunch();
  assert.deepEqual(prepared, { provider: 'codex', model: 'chosen', mode: 'plan', ...launch });
  init.cancel('interrupt after reply', true);
  assert.throws(assertLaunch, { name: 'AbortError' });
});

test('incomplete restrictions fail closed and a subsequent request can retry', async () => {
  const init = new Initialization(() => {});
  const failed = init.launch({ provider: 'codex' });
  init.complete({ id: init.pending.id, launch: { system: '' } });
  await assert.rejects(failed, /launch restrictions/);
  const retried = init.launch({ provider: 'claude' });
  init.complete({ id: init.pending.id, error: 'preparation refused' });
  await assert.rejects(retried, /preparation refused/);
});

test('interruption and the initialization deadline settle every pending wait', async (t) => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const init = new Initialization(() => {});
  const cancelled = init.launch({ provider: 'claude' });
  init.cancel('interrupted');
  await assert.rejects(cancelled, /interrupted/);
  const timed = init.launch({ provider: 'claude' });
  t.mock.timers.tick(60_000);
  await assert.rejects(timed, /timed out/);
  assert.equal(init.pending, undefined);
});
