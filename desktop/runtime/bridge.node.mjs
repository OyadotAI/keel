// The real bridge is bundled with an offline SDK stub to test background callback lifetimes.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, writeFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawn } from 'node:child_process';
import { createInterface } from 'node:readline';
import { once } from 'node:events';
import { build } from 'esbuild';

for (const failure of [false, true]) test(`the actual bridge ${failure ? 'cancels a background question on provider failure' : 'retains a background question until authoritative completion'}`, { timeout: 10_000 }, async () => {
  const scratch = await mkdtemp(join(tmpdir(), 'keel-bridge-question-'));
  let child;
  try {
    const stub = join(scratch, 'sdk.mjs'), output = join(scratch, 'bridge.mjs');
    await writeFile(stub, `
      import { readFile } from 'node:fs/promises';
      async function waitFile(path) { for (;;) { try { await readFile(path); return; } catch { await new Promise((resolve)=>setTimeout(resolve,5)); } } }
      export function query({options}) {
        const controller = new AbortController();
        return {
          supportedCommands: async () => [], supportedModels: async () => [],
          getContextUsage: async () => { throw new Error('fixture'); }, close() {},
          async *[Symbol.asyncIterator]() {
            yield {type:'system', subtype:'init', session_id:'fixture'};
            yield {type:'system', subtype:'background_tasks_changed', tasks:[{task_id:'agent',task_type:'local_agent'}]};
            const answer = options.canUseTool('AskUserQuestion', {questions:[{question:'Background choice?'}]}, {signal:controller.signal, toolUseID:'fixture'});
            yield {type:'result', subtype:'success', is_error:false};
            yield {type:'system', subtype:'session_state_changed',state:'idle'};
            const result = await Promise.race([answer, waitFile(${JSON.stringify(join(scratch, 'fail'))}).then(() => { throw new Error('offline provider failure'); })]);
            yield {type:'system', subtype:'background_result', result, aborted:controller.signal.aborted};
            await waitFile(${JSON.stringify(join(scratch, 'finish'))});
            yield {type:'system', subtype:'task_notification',task_id:'agent',status:'completed'};
            yield {type:'system', subtype:'background_tasks_changed',tasks:[]};
            yield {type:'system', subtype:'session_state_changed',state:'idle'};
            await new Promise(() => {});
          }
        };
      }
    `);
    await build({ entryPoints: [fileURLToPath(new URL('./bridge.mjs', import.meta.url))], outfile: output, bundle: true, platform: 'node', format: 'esm', plugins: [{ name: 'offline-sdk', setup(builder) { builder.onResolve({ filter: /^@anthropic-ai\/claude-agent-sdk$/ }, () => ({ path: stub })); } }] });
    child = spawn(process.execPath, [output], { cwd: scratch, env: { HOME: scratch, USERPROFILE: scratch, PATH: process.env.PATH, SystemRoot: process.env.SystemRoot }, stdio: ['pipe', 'pipe', 'pipe'] });
    const events = [], watchers = [];
    const lines = createInterface({ input: child.stdout });
    lines.on('line', (line) => { const value = JSON.parse(line); events.push(value); for (const check of watchers) check(); });
    const wait = (predicate) => new Promise((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error('Bridge fixture did not reach expected state')), 5000);
      const check = () => { const value = events.find(predicate); if (value) { clearTimeout(timer); resolve(value); } };
      watchers.push(check); check();
    });
    child.stdin.write(JSON.stringify({ method: 'start', query: { lane: 'fixture', provider: 'claude', cwd: scratch, repo: scratch, prompt: 'offline fixture', mode: 'plan' } }) + '\n');
    const request = await wait((e) => e.event === 'request');
    await wait((e) => e.event === 'msg' && e.data.type === 'result');
    assert.equal(events.some((e) => e.event === 'end'), false, 'the parent result released background ownership');
    assert.equal(events.some((e) => e.event === 'resolved'), false, 'a parent result cancelled the live background callback');
    if (failure) {
      await writeFile(join(scratch, 'fail'), 'fail');
      const end = await wait((e) => e.event === 'end');
      assert.equal(end.data.code, 1);
      const cancelled = events.find((e) => e.event === 'resolved');
      assert.equal(cancelled.data.answered, false);
      assert.equal(events.filter((e) => e.event === 'end').length, 1);
      return;
    }
    child.stdin.write(JSON.stringify({ method: 'answer', id: request.data.id, delivery: 'fixture-delivery', answer: { answers: { 'Background choice?': 'Custom answer' } } }) + '\n');
    const receipt = await wait((e) => e.event === 'answer_ack');
    assert.equal(receipt.data.ok, true);
    const background = await wait((e) => e.event === 'msg' && e.data.subtype === 'background_result');
    assert.equal(background.data.aborted, false);
    assert.equal(background.data.result.updatedInput.answers['Background choice?'], 'Custom answer');
    assert.equal(events.some((e) => e.event === 'end'), false, 'answer acceptance released a still-running agent');
    await writeFile(join(scratch, 'finish'), 'complete'); await wait((e) => e.event === 'end');
    assert.equal(events.filter((e) => e.event === 'end').length, 1);
  } finally {
    if (child) { const exited = once(child, 'exit'); child.stdin.end(); await exited; }
    await rm(scratch, { recursive: true, force: true });
  }
});
