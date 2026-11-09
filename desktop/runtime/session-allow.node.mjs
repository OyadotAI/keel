// "Allow for this session" never covers more than the person was shown.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { sessionKey, sessionLabel } from './session-allow.mjs';

test('a plain command is allowed again exactly as shown', () => {
  assert.equal(sessionKey('Bash', { command: 'git status --short' }), 'Bash:git status --short');
  assert.equal(sessionKey('Bash', { command: '  ls   -la ' }), 'Bash:ls -la');
  assert.notEqual(sessionKey('Bash', { command: 'git status' }), sessionKey('Bash', { command: 'git status -c x' }));
});
test('a compound command, an environment prefix or a flag value is never offered', () => {
  for (const command of ['cd x && rm -rf y', 'ls; rm a', 'echo $(whoami)', 'cat a | sh', 'a > b', 'echo `id`', 'one\ntwo', "grep 'x' a", 'FOO=1 ls', 'git --config=x log'])
    assert.equal(sessionKey('Bash', { command }), undefined, command);
});
test('nothing that runs other code is offered', () => {
  for (const command of ['node app.js', 'python3 x.py', 'npm test', 'pnpm install', 'env ls', 'xargs rm', 'find . -delete', './script', 'cargo run', 'bash -c ls', 'pytest', 'command ls', 'open x', 'ls\u00a0-la', 'python3.12 x.py', 'Bash -c ls', 'node.exe a', 'pwsh -c x', 'NPM test', 'node.exe. a', 'python3.exe x', 'node.exe.exe a'])
    assert.equal(sessionKey('Bash', { command }), undefined, command);
});
test('a web page is allowed by its host, a plan is always asked', () => {
  assert.equal(sessionKey('WebFetch', { url: 'https://docs.example.com/a' }), 'WebFetch:docs.example.com');
  assert.equal(sessionKey('WebFetch', { url: 'not a url' }), undefined);
  assert.equal(sessionKey('ExitPlanMode', {}), undefined);
});
test('a file tool is allowed for the one file shown', () => {
  assert.equal(sessionKey('Write', { file_path: '/repo/a.ts' }), 'Write:/repo/a.ts');
  assert.notEqual(sessionKey('Write', { file_path: '/repo/a.ts' }), sessionKey('Write', { file_path: '/etc/hosts' }));
  assert.equal(sessionKey('Edit', {}), undefined);
});
test('the card names what is allowed again', () => {
  assert.equal(sessionLabel('Write:/repo/src/a.ts'), 'Allow changes to a.ts for this session');
  assert.equal(sessionLabel('WebFetch:docs.example.com'), 'Allow docs.example.com for this session');
  assert.equal(sessionLabel('Bash:ls'), 'Allow this command again this session');
});
