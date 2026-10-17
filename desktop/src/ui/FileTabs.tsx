/** Compact editor tabs keep the selected file visible and every open file reachable. */
import { useLayoutEffect, useRef, useState, type KeyboardEvent, type MouseEvent } from "react";
import { useIde, type IdeLane, type TabClose } from "../ide";
import { Floating } from "./Menu";
import { Icon } from "./icons";
import { useStore } from "../store";
import { editorFileKey, useEditorFiles } from "../editor-files";

export function FileTabs({ lane, l, showCode }: { lane: string; l: IdeLane; showCode?: () => void }) {
  const project = useStore((s) => s.lanes[lane]?.project ?? lane);
  const wt = useStore((s) => s.lanes[lane]?.wt);
  const dirty = useEditorFiles((s) => l.tabs.filter((path) => { const file = s.files[editorFileKey(project, wt, path)]; return file && file.base !== file.text; }).join("\n"));
  const dirtyPaths = new Set(dirty.split("\n"));
  const strip = useRef<HTMLDivElement>(null);
  const [menu, setMenu] = useState<DOMRect | null>(null);
  const [context, setContext] = useState<{ path: string | null; at: DOMRect } | null>(null);
  const selected = l.commit ? `commit:${l.commit.sha}` : l.selected;
  const count = l.tabs.length + (l.commit ? 1 : 0);
  useLayoutEffect(() => {
    strip.current?.querySelector('[aria-pressed="true"]')?.parentElement?.scrollIntoView({ block: "nearest", inline: "nearest" });
  }, [selected, lane, l.tabs]);
  function open(path: string) {
    useIde.getState().open(lane, path, "code");
    setMenu(null);
    showCode?.();
  }
  function move(e: KeyboardEvent<HTMLDivElement>) {
    if (e.metaKey || e.ctrlKey || e.altKey || !(e.target as HTMLElement).matches("[data-file-tab]")) return;
    const tabs = [...e.currentTarget.querySelectorAll<HTMLButtonElement>("[data-file-tab]")];
    const at = tabs.indexOf(e.target as HTMLButtonElement);
    const to = { ArrowLeft: (at - 1 + tabs.length) % tabs.length, ArrowRight: (at + 1) % tabs.length, Home: 0, End: tabs.length - 1 }[e.key];
    if (to === undefined) return;
    e.preventDefault(); e.stopPropagation();
    tabs[to]?.click(); tabs[to]?.focus();
  }
  function contextMenu(e: MouseEvent, path: string | null) {
    e.preventDefault(); e.stopPropagation();
    setMenu(null);
    setContext({ path, at: new DOMRect(e.clientX, e.clientY, 0, 0) });
  }
  function scroll(direction: number) { strip.current?.scrollBy({ left: direction * strip.current.clientWidth * 0.75, behavior: "smooth" }); }
  return <div className="ide-tab-bar">
    <div ref={strip} className="ide-file-tabs" role="group" aria-label="Open file tabs" onKeyDown={move} onWheel={(e) => {
      if (Math.abs(e.deltaY) > Math.abs(e.deltaX) && !e.shiftKey) e.currentTarget.scrollLeft += e.deltaY;
    }}>
      {l.tabs.map((path) => <div className={`ide-file-tab${!l.commit && path === l.selected ? " active" : ""}`} key={path} onContextMenu={(e) => contextMenu(e, path)}>
        <button data-file-tab title={path} aria-pressed={!l.commit && path === l.selected} onClick={() => open(path)}><Icon name="file" size={13} /><span>{path.split("/").at(-1)}</span>{dirtyPaths.has(path) && <small className="ide-tab-dirty" title="Unsaved changes">●</small>}</button>
        <button className="ghost" aria-label={`Close ${path}`} title={`Close ${path}`} onClick={() => useIde.getState().close(lane, path)}><Icon name="x" size={12} /></button>
      </div>)}
      {l.commit && <div className="ide-file-tab active" onContextMenu={(e) => contextMenu(e, null)}><button data-file-tab title={l.commit.subject} aria-pressed="true"><Icon name="branch" size={13} /><span>{l.commit.sha.slice(0, 7)} · {l.commit.subject}</span></button><button className="ghost" aria-label="Close commit details" onClick={() => useIde.getState().patch(lane, { commit: undefined })}><Icon name="x" size={12} /></button></div>}
      {!count && <span className="ide-tabs-placeholder">Code workspace</span>}
    </div>
    {count > 0 && <div className="ide-tab-actions" role="group" aria-label="Tab navigation">
      <button className="ghost" aria-label="Scroll tabs left" title="Scroll tabs left" onClick={() => scroll(-1)}><Icon name="chevron-right" className="ide-tab-back" size={13} /></button>
      <button className="ghost" aria-label="Scroll tabs right" title="Scroll tabs right" onClick={() => scroll(1)}><Icon name="chevron-right" size={13} /></button>
      <button className="ghost" aria-label={`Show all open files (${count})`} title="Show all open files" aria-haspopup="menu" aria-expanded={!!menu} onClick={(e) => setMenu(menu ? null : e.currentTarget.getBoundingClientRect())}><Icon name="chevron-down" size={13} /><small>{count}</small></button>
    </div>}
    {context && <TabContextMenu lane={lane} l={l} context={context} onClose={() => setContext(null)} />}
    {menu && <Floating anchor={menu} onClose={() => setMenu(null)} width={340}>
      {l.tabs.map((path) => <button key={path} role="menuitem" className="ide-open-file" title={path} onClick={() => open(path)}><Icon name={!l.commit && path === l.selected ? "check-circle" : "file"} size={13} /><span>{path}</span></button>)}
      {l.commit && <button role="menuitem" className="ide-open-file" onClick={() => { setMenu(null); strip.current?.querySelector('[aria-pressed="true"]')?.parentElement?.scrollIntoView({ block: "nearest", inline: "nearest" }); showCode?.(); }}><Icon name="branch" size={13} /><span>{l.commit.subject}</span></button>}
    </Floating>}
  </div>;
}

/** Closing tabs never changes files on disk; the clicked tab anchors each range. */
function TabContextMenu({ lane, l, context, onClose }: { lane: string; l: IdeLane; context: { path: string | null; at: DOMRect }; onClose: () => void }) {
  const at = context.path === null ? l.tabs.length : l.tabs.indexOf(context.path);
  const count = l.tabs.length + (l.commit ? 1 : 0);
  const actions: { side: TabClose; label: string; disabled: boolean }[] = [
    { side: "current", label: "Close", disabled: false },
    { side: "left", label: "Close tabs to the left", disabled: at <= 0 },
    { side: "right", label: "Close tabs to the right", disabled: at >= count - 1 },
    { side: "others", label: "Close other tabs", disabled: count <= 1 },
  ];
  return <Floating anchor={context.at} onClose={onClose}>
    {actions.map(({ side, label, disabled }) => <button key={side} role="menuitem" disabled={disabled} onClick={() => { useIde.getState().close(lane, context.path, side); onClose(); }}>{label}</button>)}
  </Floating>;
}
