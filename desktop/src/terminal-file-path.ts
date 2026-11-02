// Terminal composers receive a quoted relative path, never terminal commands or control keys.
const SIMPLE_PATH = /^[\w@%+=:,./-]+$/;
// eslint-disable-next-line no-control-regex -- reject terminal controls instead of changing file identity.
const CONTROLS = /[\x00-\x1f\x7f-\x9f]/;

/** Preserve the file's spelling and keep whitespace/quotes within one visible path reference. */
export function terminalFilePath(path: string): string {
  if (!path || CONTROLS.test(path)) throw new Error("This file name contains unsupported control characters.");
  return `${SIMPLE_PATH.test(path) ? path : `'${path.replaceAll("'", "'\\''")}'`} `;
}
