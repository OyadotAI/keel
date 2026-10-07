import { useEffect, useState } from "react";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { invoke } from "@tauri-apps/api/core";
import { pasteInto } from "../typers";
import { useStore } from "../store";


/// A path as a shell word: quoted, so a space or a quote in it stays one argument.
const quote = (p: string) => (/^[\w@%+=:,./-]+$/.test(p) ? p : `'${p.replace(/'/g, `'\\''`)}'`);

/// Files dropped on the window. The webview takes a file drop itself and says so as its own
/// event, so the page's `drop` never fires — that was "drag and drop does nothing".
///
/// Onto the sidebar, or a window with no lane: the folders open as projects. Anywhere else: the
/// paths go into the lane's prompt, quoted, without pressing Enter — what dropping a file on a
/// terminal does, and how Claude Code takes an image or a file to look at.
export function Drop() {
  const [over, setOver] = useState<"lane" | "projects" | null>(null);
  useEffect(() => {
    const where = (x: number, y: number) => {
      const r = window.devicePixelRatio || 1;
      const el = document.elementFromPoint(x / r, y / r);
      const lane = useStore.getState().active;
      return !lane || el?.closest(".sidebar") ? "projects" : "lane";
    };
    const unlisten = getCurrentWebview().onDragDropEvent((e) => {
      const p = e.payload;
      if (p.type === "leave") return setOver(null);
      if (p.type === "enter" || p.type === "over") return setOver(where(p.position.x, p.position.y));
      if (p.type !== "drop") return;
      setOver(null);
      const target = where(p.position.x, p.position.y);
      const lane = useStore.getState().active;
      if (target === "projects" || !lane) {
        // A file dropped here is not a project; open the folders and leave the rest alone.
        for (const path of p.paths) void invoke<boolean>("is_dir", { path }).then((dir) => {
          if (dir) void useStore.getState().openProject(path);
        });
        return;
      }
      if (useStore.getState().lanes[lane]?.interface === "terminal") {
        pasteInto(lane, p.paths.map(quote).join(" ") + " ");
        return;
      }
      const current = useStore.getState().lanes[lane];
      useStore.setState((s) => ({ lanes: { ...s.lanes, [lane]: { ...s.lanes[lane], attachments: [...(current?.attachments ?? []), ...p.paths.map((path) => ({ path, name: path.split(/[\\/]/).pop() ?? path }))] } } }));
      useStore.getState().draft(lane, current?.draft ?? "");
    });
    return () => void unlisten.then((f) => f());
  }, []);
  if (!over) return null;
  return (
    <div className="drop" aria-hidden>
      <div className="drop-card">{over === "projects" ? "Drop a folder to open it as a project" : "Drop to add the files to the prompt"}</div>
    </div>
  );
}
