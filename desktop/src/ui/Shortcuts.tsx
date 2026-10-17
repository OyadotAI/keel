import { label, MAC, type Chord } from "../keys";
import { Dialog } from "./kit";
import { useStore } from "../store";

const GROUPS: [string, [string, Chord | string][]][] = [
  ["Coding Mode navigation", [
    ["Go to file (also accepts file.ts:42)", MAC ? "⌘P" : "Ctrl+P"],
    ["Command palette", MAC ? "⇧⌘P" : "Ctrl+Shift+P"],
    ["Go to line", "Ctrl+G"],
    ["Toggle Markdown preview", MAC ? "⇧⌘V" : "Ctrl+Shift+V"],
    ["Find in file", MAC ? "⌘F" : "Ctrl+F"],
    ["Show Explorer", MAC ? "⇧⌘E" : "Ctrl+Shift+E"],
    ["Show Source Control", "Ctrl+Shift+G"],
    ["Toggle sidebar", MAC ? "⌘B" : "Ctrl+B"],
    ["Toggle integrated terminal", "Ctrl+`"],
    ["Toggle terminal panel", MAC ? "⌘J" : "Ctrl+J"],
    ["Save file", MAC ? "⌘S" : "Ctrl+S"],
    ["Code completion", "Ctrl+Space"],
    ["Accept AI suggestion", "Tab"],
    ["Request AI suggestion", "Alt+\\"],
    ["Close selected file", MAC ? "⌘W" : "Ctrl+W"],
  ]],
  ["Lanes", [
    ["Next lane", { key: "ArrowDown", alt: true }],
    ["Previous lane", { key: "ArrowUp", alt: true }],
    ["Next / previous lane (tab style)", `${label({ key: "]", shift: true })} ${label({ key: "[", shift: true })}`],
    ["Move between lanes in the sidebar", "↑ ↓"],
    ["Go to lane 1–9", { key: "1" }],
    ["New lane", { key: "n" }],
    ["New lane on its own branch", { key: "n", shift: true }],
    ["Rename lane (in the sidebar)", "F2"],
    ["Close lane", { key: "w" }],
    ["Merge lane", { key: "m", shift: true }],
    ["Discard lane", { key: "Backspace", shift: true }],
  ]],
  ["Reviewing changes (in the panel)", [
    ["Move between files", "↑ ↓"],
    ["Open the diff", "Enter"],
    ["Stage / unstage", "Space"],
    ["Next / previous file in a diff", "] ["],
    ["Back to the list, then to the composer", "Esc"],
  ]],
  ["Approvals", [
    ["Allow the oldest request once (jumps to a waiting lane first)", { key: "a", shift: true }],
    ["Allow it for this session", { key: "s", shift: true }],
    ["Deny the oldest request", { key: "d", shift: true }],
    ["Pick a question's option", "1–9"],
  ]],
  ["Window", [
    ["Command palette (⌃N / ⌃P to move)", { key: "k" }],
    ["Focus composer", { key: "l" }],
    ["Toggle the shell", { key: "j" }],
    ["Toggle sidebar", { key: "e", shift: true }],
    ["Toggle panel", { key: "i", alt: true }],
    ["Panel tab 1–6", { key: "1", alt: true }],
    ["Open project", { key: "o" }],
    ["Zoom in / out — the terminal's font when it has focus", `${label({ key: "=" })} ${label({ key: "-" })}`],
    ["Actual size", { key: "0" }],
    ["Settings", { key: "," }],
    ["This list", { key: "/" }],
  ]],
];

export function Shortcuts() {
  const open = useStore((s) => s.shortcuts);
  if (!open) return null;
  const close = () => useStore.setState({ shortcuts: false });
  return (
    <Dialog title="Keyboard shortcuts" onClose={close}>
      <div className="shortcuts">
        {GROUPS.map(([group, rows]) => (
          <section key={group}>
            <div className="eyebrow">{group}</div>
            {rows.map(([what, keys]) => (
              <div key={what} className="shortcut">
                <span>{what}</span>
                <kbd>{typeof keys === "string" ? keys : label(keys)}</kbd>
              </div>
            ))}
          </section>
        ))}
      </div>
      <div className="actions">
        <button className="primary" onClick={close}>Done</button>
      </div>
    </Dialog>
  );
}
