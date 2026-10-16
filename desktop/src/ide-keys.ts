// VS Code's common navigation bindings. These apply only to the IDE workspace.
export type IdeCommand = "files" | "commands" | "line" | "find" | "explorer" | "git" | "sidebar" | "terminal" | "closeFile";
export function ideCommand(e: Pick<KeyboardEvent, "code" | "metaKey" | "ctrlKey" | "shiftKey" | "altKey">, mac: boolean): IdeCommand | undefined {
  if (e.altKey) return;
  const primary = mac ? e.metaKey && !e.ctrlKey : e.ctrlKey && !e.metaKey;
  const control = e.ctrlKey && !e.metaKey;
  if (control && !e.shiftKey && e.code === "Backquote") return "terminal";
  if (control && !e.shiftKey && e.code === "KeyG") return "line";
  if (control && e.shiftKey && e.code === "KeyG") return "git";
  if (!primary) return;
  if (e.code === "KeyP") return e.shiftKey ? "commands" : "files";
  if (e.shiftKey && e.code === "KeyE") return "explorer";
  if (e.shiftKey) return;
  const commands: Partial<Record<string, IdeCommand>> = { KeyB: "sidebar", KeyJ: "terminal", KeyF: "find", KeyW: "closeFile" };
  return commands[e.code];
}

export function fileQuery(value: string): { query: string; line?: number } {
  const match = value.match(/^(.*?):([1-9]\d*)(?::[1-9]\d*)?$/);
  return match ? { query: match[1], line: Math.min(Number(match[2]), 20_000) } : { query: value };
}
