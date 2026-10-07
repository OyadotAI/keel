import { spawnSync } from 'node:child_process';
import { resolve } from 'node:path';
import assert from 'node:assert/strict';

// Run the shipped executable, not the build machine's Node. Signature failures
// can happen before the helper reads a single command (even --version can pass).
const app = process.argv[2];
assert.ok(app, 'Usage: node scripts/runtime-smoke.mjs /path/to/Keel.app');
const binary = resolve(app, 'Contents/MacOS/keel-node');
const helper = resolve(app, 'Contents/Resources/runtime/keel-runtime.mjs');
const result = spawnSync(binary, [helper], {
  input: JSON.stringify({ method: 'smoke-test' }) + '\n',
  encoding: 'utf8',
  timeout: 15000,
});
assert.ifError(result.error);
assert.equal(result.signal, null, `Bundled runtime crashed: ${result.signal}\n${result.stderr}`);
assert.equal(result.status, 0, `Bundled runtime exited ${result.status}\n${result.stderr}`);
const records = result.stdout.trim().split('\n').filter(Boolean).map((line) => JSON.parse(line));
assert.ok(records.some((record) => record.event === 'fatal' && record.data === 'Error: Unknown runtime command'),
  `Runtime did not process input: ${result.stdout}\n${result.stderr}`);
console.log('Bundled runtime started and processed a command.');
