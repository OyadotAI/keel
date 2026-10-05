// Reading a dev server's own output: where it is listening, and why it stopped.

/// A page on this machine in a server's own output: `Local: http://localhost:3000/`.
const LOCAL = /\bhttps?:\/\/(?:localhost|127\.0\.0\.1|0\.0\.0\.0|\[::1?\]):\d+[^\s"']*/;
export const urlIn = (lines: string[]) => {
  for (let i = lines.length - 1; i >= 0; i--) {
    // Colour first: Vite prints the URL in cyan, and `…36mhttp` has no word boundary to match at.
    // eslint-disable-next-line no-control-regex -- stripping terminal colour codes.
    const m = lines[i].replace(/\x1b\[[0-9;]*m/g, "").match(LOCAL);
    if (m) return m[0];
  }
  return undefined;
};

/// Why a dev server that was started is not running, in the words someone would act on.
export function whyStopped(log: string[]): string | undefined {
  const text = log.slice(-40).join("\n");
  if (/command not found|Cannot find module|ERR_MODULE_NOT_FOUND|MODULE_NOT_FOUND|not recognized as an internal/.test(text)) return "Its dependencies are not installed.";
  if (/EADDRINUSE|address already in use/.test(text)) return "Its port is already taken by another process.";
  if (log.length) return "It exited.";
  return undefined;
}
