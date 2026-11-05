// Exercise cold, warm, failed and replacement providers through the bundled production bridge.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, writeFile, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawn } from 'node:child_process';
import { createInterface } from 'node:readline';
import { once } from 'node:events';
import { build } from 'esbuild';

const SDK_STUB = `
import {appendFileSync} from 'node:fs';
export function query({prompt, options}) {
  appendFileSync('provider-launches','claude\\n');
  let model = options.model, mode = options.permissionMode;
  return {
    supportedCommands: async()=>[], supportedModels: async()=>[],
    setModel: async(value)=>{model=value;}, setPermissionMode: async(value)=>{mode=value;},
    getContextUsage: async()=>{throw new Error('offline');}, close(){},
    async *[Symbol.asyncIterator]() {
      yield {type:'system',subtype:'init',session_id:'fixture'};
      for await (const input of prompt) {
        if(input.message.content[0].text==='fail') throw new Error('offline provider failure');
        yield {type:'assistant',fixture:{model,mode,system:options.systemPrompt.append,mcp:options.mcpServers.keel.url}};
        yield {type:'result',is_error:false};
      }
    }
  };
}`;

const APP_SERVER_STUB = `
import {createInterface} from 'node:readline';
import {appendFileSync} from 'node:fs';
appendFileSync('provider-launches','codex\\n');
const emit=(value)=>console.log(JSON.stringify(value));
let system;
createInterface({input:process.stdin}).on('line',(line)=>{
 const q=JSON.parse(line); if(!q.id) return;
 let result={};
 if(q.method==='thread/start'||q.method==='thread/resume') {system=q.params.developerInstructions;result={thread:{id:'fixture'},model:q.params.model};}
 if(q.method==='skills/list')result={data:[]};
 if(q.method==='model/list')result={data:[]};
 if(q.method==='turn/start') {
  if(q.params.input[0].text==='fail') {process.exit(1);return;}
  result={turn:{id:'turn'}};
  emit({id:q.id,result});
  emit({method:'item/completed',params:{threadId:'fixture',item:{id:'reply',type:'agentMessage',text:JSON.stringify({system,model:q.params.model,sandbox:q.params.sandboxPolicy,argv:process.argv.slice(2)})}}});
  emit({method:'turn/completed',params:{threadId:'fixture',turn:{id:'turn',status:'completed'}}});return;
 }
 emit({id:q.id,result});
});`;

for (const provider of ['claude', 'codex']) test(`${provider} prepares launch once for warm turns and refreshes after provider failure`, { timeout: 15_000 }, async () => {
  const scratch = await mkdtemp(join(tmpdir(), 'keel-launch-'));
  let child;
  try {
    const stub = join(scratch, 'sdk.mjs'), helper = join(scratch, 'bridge.mjs'), app = join(scratch, 'app.mjs');
    await writeFile(stub, SDK_STUB); await writeFile(app, APP_SERVER_STUB);
    await build({ entryPoints: [fileURLToPath(new URL('./bridge.mjs', import.meta.url))], outfile: helper, bundle: true, platform: 'node', format: 'esm', plugins: [{ name: 'offline-sdk', setup(builder) { builder.onResolve({ filter: /^@anthropic-ai\/claude-agent-sdk$/ }, () => ({ path: stub })); } }] });
    child = spawn(process.execPath, [helper], { cwd: scratch, env: { HOME: scratch, USERPROFILE: scratch, PATH: process.env.PATH, SystemRoot: process.env.SystemRoot }, stdio: ['pipe', 'pipe', 'pipe'] });
    const events = [], waiters = new Set(); let launches = 0, cancelNext = true;
    const send = (q) => child.stdin.write(JSON.stringify(q) + '\n');
    createInterface({ input: child.stdout }).on('line', (line) => {
      const event = JSON.parse(line); events.push(event);
      if (event.event === 'initialize') {
        launches++;
        const reply = { method: 'initialized', id: event.data.id, launch: { system: `context-${launches}`, codexArgs: [app, '--keel-browser-restricted'], mcp: { mcpServers: { keel: { url: `http://127.0.0.1/mcp?binding=fresh-${launches}` } } } } };
        if (cancelNext) { cancelNext = false; child.stdin.write(JSON.stringify(reply) + '\n' + JSON.stringify({ method: 'interrupt' }) + '\n'); }
        else send(reply);
      }
      for (const check of waiters) check();
    });
    const waitEnd = (from) => new Promise((resolve, reject) => {
      const timer = setTimeout(() => { waiters.delete(check); reject(new Error('Offline provider did not end')); }, 5000);
      const check = () => { const end = events.slice(from).find((e) => e.event === 'end'); if (end) { clearTimeout(timer); waiters.delete(check); resolve(end); } };
      waiters.add(check); check();
    });
    const turn = async (prompt, mode = 'plan') => {
      const from = events.length;
      send({ method: 'start', query: { provider, prompt, mode, model: `model-${prompt}`, cwd: scratch, repo: scratch, executable: process.execPath, lane: 'fixture' } });
      const end = await waitEnd(from);
      return { events: events.slice(from), code: end.data.code };
    };
    assert.equal((await turn('cancelled')).code, -1);
    await assert.rejects(readFile(join(scratch, 'provider-launches')), { code: 'ENOENT' });
    const first = launches + 1;
    assert.equal((await turn('cold')).code, 0); assert.equal(launches, first);
    const warm = await turn('warm', 'acceptEdits');
    assert.equal(warm.code, 0); assert.equal(launches, first, 'warm turn rebuilt launch context');
    if (provider === 'claude') {
      const record = warm.events.find((e) => e.data?.fixture)?.data.fixture;
      assert.deepEqual(record, { model: 'model-warm', mode: 'acceptEdits', system: `context-${first}`, mcp: `http://127.0.0.1/mcp?binding=fresh-${first}` });
    } else {
      const frame = warm.events.flatMap((e) => Array.isArray(e.data) ? e.data : []).find((f) => f.op === 'text');
      const record = JSON.parse(frame.append);
      assert.equal(record.system, `context-${first}`); assert.equal(record.model, 'model-warm');
      assert.equal(record.sandbox.type, 'workspaceWrite'); assert.ok(record.argv.includes('--keel-browser-restricted'));
    }
    assert.equal((await turn('fail')).code, 1); assert.equal(launches, first);
    const replacement = await turn('replacement');
    assert.equal(replacement.code, 0, JSON.stringify(replacement.events)); assert.equal(launches, first + 1, 'replacement did not refresh restrictions');
    if (provider === 'claude') assert.equal(replacement.events.find((e) => e.data?.fixture)?.data.fixture.mcp, `http://127.0.0.1/mcp?binding=fresh-${first + 1}`);
    assert.equal((await readFile(join(scratch, 'provider-launches'), 'utf8')).trim().split('\n').length, 2);
  } finally {
    if (child) { const exited = once(child, 'exit'); child.stdin.end(); await exited; }
    await rm(scratch, { recursive: true, force: true });
  }
});
