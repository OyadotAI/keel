// "Allow for this session" never covers more than the person was shown.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { sessionKey } from './session-allow.mjs';

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
  for (const command of ['node app.js', 'python3 x.py', 'npm test', 'pnpm install', 'env ls', 'xargs rm', 'find . -delete', './script', 'cargo run', 'bash -c ls'])
    assert.equal(sessionKey('Bash', { command }), undefined, command);
});
test('a web page is allowed by its host, a plan is always asked', () => {
  assert.equal(sessionKey('WebFetch', { url: 'https://docs.example.com/a' }), 'WebFetch:docs.example.com');
  assert.equal(sessionKey('WebFetch', { url: 'not a url' }), undefined);
  assert.equal(sessionKey('ExitPlanMode', {}), undefined);
});
