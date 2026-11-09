// The reach of "Allow for this session" on a Claude runtime card.
/** Programs that run whatever they are handed, or a project's own scripts: allowing one again
 *  allows code the card never showed, so the offer is not made. */
const RUNS_ANYTHING = new Set(['sh', 'bash', 'zsh', 'fish', 'dash', 'env', 'xargs', 'find', 'exec', 'eval', 'sudo', 'doas', 'nohup', 'time', 'timeout', 'watch', 'nice',
  'node', 'deno', 'bun', 'python', 'python3', 'ruby', 'perl', 'php', 'lua', 'osascript', 'awk', 'sed', 'make', 'npm', 'pnpm', 'yarn', 'npx', 'pnpx', 'bunx', 'uv', 'uvx',
  'cargo', 'go', 'ssh', 'docker', 'kubectl']);

/** What "Allow for this session" covers: exactly the command on the card, run again — never a
 *  pattern. A program-and-subcommand rule let `git status` cover `git status -c …`-shaped tricks,
 *  and any looser rule is a rule the person did not read. No offer for a compound command, an
 *  environment prefix, a path, or a program that runs other code. A web page is allowed by its
 *  host, any other tool by name. */
export function sessionKey(tool, input) {
  if (tool === 'ExitPlanMode') return undefined;
  if (tool === 'WebFetch') {
    try { return `WebFetch:${new URL(String(input?.url ?? '')).host}`; } catch { return undefined; }
  }
  if (tool !== 'Bash') return tool;
  const command = String(input?.command ?? '').trim();
  if (!command || /[;&|`$<>()\\\n'"*?{}~=]/.test(command)) return undefined;
  const argv = command.split(/\s+/);
  if (argv[0].includes('/') || RUNS_ANYTHING.has(argv[0])) return undefined;
  return `Bash:${argv.join(' ')}`;
}
