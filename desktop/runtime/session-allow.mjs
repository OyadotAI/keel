// The reach of "Allow for this session" on a Claude runtime card.
/** Programs that run whatever they are handed, or a project's own scripts: allowing one again
 *  allows code the card never showed, so the offer is not made. */
const RUNS_ANYTHING = new Set(['sh', 'bash', 'zsh', 'fish', 'dash', 'env', 'xargs', 'find', 'exec', 'eval', 'sudo', 'doas', 'nohup', 'time', 'timeout', 'watch', 'nice',
  'node', 'deno', 'bun', 'python', 'python3', 'ruby', 'perl', 'php', 'lua', 'osascript', 'awk', 'sed', 'make', 'npm', 'pnpm', 'yarn', 'npx', 'pnpx', 'bunx', 'uv', 'uvx',
  'cargo', 'go', 'ssh', 'docker', 'kubectl', 'command', 'builtin', 'source', '.', 'setsid', 'flock', 'stdbuf',
  'strace', 'ltrace', 'caffeinate', 'chroot', 'script', 'parallel', 'java', 'pip', 'pip3', 'pytest', 'jest', 'vitest', 'tsc', 'gradle',
  'mvn', 'rake', 'bundle', 'just', 'tox', 'cmake', 'ninja', 'dotnet', 'swift', 'rscript', 'open', 'cmd', 'powershell', 'pwsh', 'wsl',
  'start', 'gradlew', 'mvnw', 'poetry', 'pdm', 'hatch', 'nox', 'invoke', 'turbo', 'nx', 'lerna', 'tsx', 'ts-node', 'vite', 'webpack', 'next']);
/** The program a name runs, as the list knows it: macOS and Windows match names without case, Windows
 *  adds an extension, and `python3.12` is python. */
const program = (name) => name.toLowerCase().replace(/\.(exe|cmd|bat|ps1|com)$/, '').replace(/[0-9.]+$/, '');

const FILE_TOOLS = new Set(['Edit', 'MultiEdit', 'Write', 'NotebookEdit']);

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
  // A file tool is allowed for the one file on the card: "Write" by name would cover every path,
  // outside the checkout included.
  if (FILE_TOOLS.has(tool)) { const path = input?.file_path ?? input?.notebook_path; return typeof path === 'string' && path ? `${tool}:${path}` : undefined; }
  if (tool !== 'Bash') return tool;
  const command = String(input?.command ?? '').trim();
  if (!command || /[;&|`$<>()\\\n'"*?{}~=]/.test(command)) return undefined;
  const argv = command.split(/[ \t]+/);
  if (!/^[a-z0-9][a-z0-9._-]*$/i.test(argv[0]) || RUNS_ANYTHING.has(program(argv[0]))) return undefined;
  return `Bash:${argv.join(' ')}`;
}

/** The card's words for a key: what will be allowed again, named the way the person reads it. */
export function sessionLabel(key) {
  if (key.startsWith('Bash:')) return 'Allow this command again this session';
  const file = /^(Edit|MultiEdit|Write|NotebookEdit):(.*)$/.exec(key);
  if (file) return `Allow changes to ${file[2].split(/[\\/]/).filter(Boolean).at(-1) ?? file[2]} for this session`;
  return `Allow ${key.replace(/^WebFetch:/, '')} for this session`;
}
