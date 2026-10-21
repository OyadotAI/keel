import { test } from 'node:test';
import assert from 'node:assert/strict';
import { Codex, appServerArgs, historyFrames, script } from './codex.mjs';

test('streamed text is not duplicated by the completed item', async () => {
  const events = [];
  const c = new Codex((event, data) => events.push({ event, data }), () => {});
  await c.message({ method: 'item/agentMessage/delta', params: { itemId: 'm1', delta: '**Hello**' } });
  await c.message({ method: 'item/completed', params: { item: { id: 'm1', type: 'agentMessage', text: '**Hello**' } } });
  assert.equal(events.length, 1);
  assert.equal(events[0].data[0].append, '**Hello**');
});

test('a spawned member thread neither ends the turn nor speaks for it', async () => {
  const events = [];
  const c = new Codex((event, data) => events.push({ event, data }), () => {});
  c.thread = 'parent';
  await c.message({ method: 'item/agentMessage/delta', params: { threadId: 'child', itemId: 'm1', delta: 'not mine' } });
  await c.message({ method: 'turn/completed', params: { threadId: 'child', turn: { id: 't', status: 'completed' } } });
  assert.equal(events.length, 0);
  await c.message({ method: 'item/agentMessage/delta', params: { threadId: 'parent', itemId: 'm2', delta: 'mine' } });
  assert.equal(events.length, 1);
});

test('a child thread approval request is still answered', async () => {
  const sent = [];
  const c = new Codex(() => {}, async () => ({ decision: 'decline' }));
  c.thread = 'parent';
  c.write = (value) => sent.push(value);
  await c.message({ id: 5, method: 'item/commandExecution/requestApproval', params: { threadId: 'child', command: 'ls' } });
  assert.deepEqual(sent, [{ id: 5, result: { decision: 'decline' } }]);
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

test('empty reasoning does not create a disclosure in live output or history', async () => {
  const events = [];
  const c = new Codex((event, data) => events.push({ event, data }), () => {});
  const item = { id: 'r1', type: 'reasoning', summary: [], content: [], encryptedContent: 'opaque' };
  await c.message({ method: 'item/reasoning/summaryTextDelta', params: { itemId: 'r1', delta: '' } });
  await c.message({ method: 'item/completed', params: { item } });
  assert.deepEqual(events, []);
  assert.deepEqual(historyFrames([{ id: 't1', status: 'completed', items: [item] }]).map((f) => f.op), ['open', 'close']);
});

test('reasoning falls back from an empty summary to available text', () => {
  const frames = historyFrames([{ id: 't1', status: 'completed', items: [
    { id: 'r1', type: 'reasoning', summary: [], content: ['Checking the project configuration.'] },
    { id: 'r2', type: 'reasoning', summary: [{ text: 'Reviewing the result.' }], content: [] },
  ] }]);
  assert.deepEqual(frames.filter((f) => f.op === 'text').map((f) => f.append), ['Checking the project configuration.', 'Reviewing the result.']);
});

test('an empty delta does not suppress the completed reasoning summary', async () => {
  const events = [];
  const c = new Codex((event, data) => events.push({ event, data }), () => {});
  await c.message({ method: 'item/reasoning/summaryTextDelta', params: { itemId: 'r1', delta: '' } });
  await c.message({ method: 'item/completed', params: { item: { id: 'r1', type: 'reasoning', summary: ['Checking the files.'] } } });
  assert.equal(events.length, 1);
  assert.equal(events[0].data[0].append, 'Checking the files.');
});

test('streamed reasoning stays ordered without duplicating the completed summary', async () => {
  const events = [];
  const c = new Codex((event, data) => events.push({ event, data }), () => {});
  for (const delta of ['Checking ', 'the files.']) await c.message({ method: 'item/reasoning/summaryTextDelta', params: { itemId: 'r1', delta } });
  await c.message({ method: 'item/completed', params: { item: { id: 'r1', type: 'reasoning', summary: ['Checking the files.'] } } });
  assert.equal(events.map((e) => e.data[0].append).join(''), 'Checking the files.');
  assert.ok(events.every((e) => e.data[0].kind === 'think'));
});

test('a command is asked about without the shell Codex wraps it in', () => {
  assert.equal(script(['/bin/zsh', '-lc', 'git status --short']), 'git status --short');
  assert.equal(script("/bin/zsh -lc 'git status'"), 'git status');
  assert.equal(script('/bin/bash -c "npm test"'), 'npm test');
  assert.equal(script('/bin/zsh -lc git status'), undefined);
  assert.equal(script('/bin/zsh -lc ls'), 'ls');
  assert.equal(script('git status'), 'git status');
  assert.equal(script(['git', 'status']), 'git status');
  // Quoting it cannot read is Codex's own card, never a guess.
  assert.equal(script(`/bin/zsh -lc 'a'"$(rm x)"'b'`), undefined);
  assert.equal(script('/bin/zsh -lc "echo $(rm x)"'), undefined);
  assert.equal(script(['ls', '$(rm x)']), undefined);
  assert.equal(script(''), undefined);
  // Only the system's shells are stepped over; one from the repository is Codex's card.
  assert.equal(script(['./tools/zsh', '-lc', 'git status']), undefined);
  assert.equal(script("/tmp/x/zsh -lc 'git status'"), undefined);
  assert.equal(script("zsh -lc 'git status'"), undefined);
});

test('a sandboxed command is asked of Keel first, unwrapped, and its answer is final', async () => {
  const sent = []; const asked = [];
  const c = new Codex(() => {}, async () => { throw new Error('the local card must not show'); },
    async (...args) => { asked.push(args); return 'allow'; });
  c.thread = 'th'; c.mode = 'acceptEdits';
  c.write = (value) => sent.push(value);
  await c.message({ id: 7, method: 'item/commandExecution/requestApproval', params: { threadId: 'th', itemId: 'i', command: "/bin/zsh -lc 'git status'" } });
  assert.deepEqual(asked[0].slice(0, 2), ['Bash', { command: 'git status' }]);
  assert.equal(asked[0][3], 'codex');
  assert.deepEqual(sent.at(-1), { id: 7, result: { decision: 'accept' } });
});

test('the App Server is started with Keel\'s overrides before its own arguments', () => {
  assert.deepEqual(appServerArgs({ codexArgs: ['-c', 'mcp_servers.keel.url="u"'] }), ['-c', 'mcp_servers.keel.url="u"', 'app-server', '--stdio']);
  assert.deepEqual(appServerArgs({}), ['app-server', '--stdio']);
});

test('a Codex subagent is an Agent row named for it and works until it finishes', async () => {
  const events = [];
  const c = new Codex((event, data) => events.push(...(event === 'turn' ? data : [])), () => {});
  c.thread = 'main';
  const sub = (kind, id) => ({ method: 'item/completed', params: { threadId: 'main', item: { type: 'subAgentActivity', id, kind, agentThreadId: 'A1', agentPath: '/root/pm' } } });
  await c.message({ method: 'item/started', params: { threadId: 'main', item: { type: 'subAgentActivity', id: 'call_1', kind: 'started', agentThreadId: 'A1', agentPath: '/root/pm' } } });
  await c.message(sub('started', 'call_1'));
  assert.ok(events.some((o) => o.op === 'call' && o.tool === 'Agent' && o.input.subagent_type === 'pm'));
  assert.ok(events.some((o) => o.op === 'background' && o.id === 'A1' && o.agent === 'pm' && o.running));
  assert.equal(events.filter((o) => o.op === 'call').length, 1, 'started and completed items of one start are one row');
  await c.message(sub('completed', 'subagent-completed-x'));
  assert.ok(events.some((o) => o.op === 'background' && o.id === 'A1' && o.running === false));
});

test('a subagent still working when the App Server exits stops working', () => {
  const events = [];
  const c = new Codex((event, data) => events.push(...(event === 'turn' ? data : [])), () => {});
  c.agents.add('A1');
  c.fail(new Error('gone'));
  assert.ok(events.some((o) => o.op === 'background' && o.id === 'A1' && o.running === false));
});

test('an MCP call is named the way Claude Code names one', () => {
  const events = [];
  const c = new Codex((event, data) => events.push(...(event === 'turn' ? data : [])), () => {});
  c.item({ type: 'mcpToolCall', id: 'x', server: 'keel', tool: 'browser_read', status: 'completed', result: {} }, true);
  assert.equal(events.find((o) => o.op === 'call').tool, 'mcp__keel__browser_read');
});
