import { useEffect, useMemo, useRef, useState } from "react";
import { get } from "../api";
import { useStore } from "../store";
import { useIde, type FileNode } from "../ide";
import { fileQuery } from "../ide-keys";
import { Dialog } from "./kit";
import { score } from "./Palette";

const titles = { files: "Go to File", line: "Go to Line", find: "Find in File" };
export function FileNavigation({ lane, showCode }: { lane: string; showCode: () => void }) {
  const mode = useIde((s) => s.navigation ?? "files");
  const selected = useIde((s) => s.lanes[lane]?.selected);
  const l = useStore((s) => s.lanes[lane]);
  const ep = useStore((s) => s.projects[s.lanes[lane]?.project]?.endpoint);
  const [files, setFiles] = useState<string[]>([]);
  const [lines, setLines] = useState<string[]>([]);
  const [query, setQuery] = useState("");
  const [index, setIndex] = useState(0);
  const [loaded, setLoaded] = useState(false);
  const [error, setError] = useState<string>();
  const [retry, setRetry] = useState(0);
  const input = useRef<HTMLInputElement>(null);
  const list = useRef<HTMLDivElement>(null);
  const parsed = fileQuery(query);
  const close = () => useIde.setState({ navigation: undefined });
  useEffect(() => { input.current?.focus(); }, []);
  useEffect(() => { setIndex(0); }, [query]);
  useEffect(() => { list.current?.querySelector('[aria-selected="true"]')?.scrollIntoView({ block: "nearest" }); }, [index]);
  useEffect(() => {
    if (!ep || (l.isolated && !l.wt)) { setError("This lane’s checkout is not available yet. Open its terminal to prepare it."); return; }
    if (mode === "line") { setLoaded(true); return; }
    let live = true;
    setError(undefined);
    setLoaded(false);
    const read = mode === "files" ? get<FileNode[]>(ep, "/api/tree", { wt: l.wt }).then((nodes) => {
      const paths: string[] = [];
      const visit = (list: FileNode[]) => { for (const node of list) { if (node.dir) visit(node.children ?? []); else paths.push(node.path); } };
      visit(nodes);
      if (live) setFiles(paths);
    }) : selected ? get<{ text: string | null; note?: string }>(ep, "/api/file", { wt: l.wt, path: selected }).then((file) => {
      if (!live) return;
      if (file.text === null) setError(file.note ?? "This file cannot be searched as text.");
      else setLines(file.text.split("\n").slice(0, 20_000));
    }) : Promise.resolve();
    void read.then(() => { if (live) setLoaded(true); }, (e) => { if (live) { setLoaded(true); setError(String(e)); } });
    return () => { live = false; };
  }, [ep, l.isolated, l.wt, mode, selected, retry]);
  const rows = useMemo(() => {
    if (mode === "find") return query ? lines.flatMap((text, i) => text.toLowerCase().includes(query.toLowerCase()) ? [{ path: selected!, line: i + 1, detail: text }] : []).slice(0, 50) : [];
    if (mode === "line") return selected && /^:?[1-9]\d*(?::[1-9]\d*)?$/.test(query) ? [{ path: selected, line: Math.min(Number(query.replace(/^:/, "").split(":")[0]), 20_000), detail: "" }] : [];
    if (parsed.line && !parsed.query && selected) return [{ path: selected, line: parsed.line, detail: "" }];
    return files.map((path) => ({ path, match: score(parsed.query, path) })).filter((r) => r.match)
      .sort((a, b) => b.match!.score - a.match!.score).slice(0, 50).map(({ path }) => ({ path, line: parsed.line, detail: "" }));
  }, [mode, files, lines, query, selected, parsed.query, parsed.line]);
  const open = (row: typeof rows[number]) => {
    useIde.getState().navigate(lane, row.path, row.line);
    close(); showCode();
    requestAnimationFrame(() => requestAnimationFrame(() => document.querySelector<HTMLElement>(".ide-code-list")?.focus()));
  };
  return <Dialog title={titles[mode]} onClose={close}>
    <div className="ide-navigation">
      <input ref={input} className="field" aria-label={titles[mode]} role="combobox" aria-autocomplete="list" aria-expanded={rows.length > 0} aria-controls="file-navigation-results" aria-activedescendant={rows[index] ? `file-result-${index}` : undefined}
        placeholder={mode === "files" ? "Search by name or path · file.ts:42" : mode === "line" ? "Line number" : "Find text in the selected file"} value={query} onChange={(e) => setQuery(e.target.value)} onKeyDown={(e) => {
          if (e.key === "ArrowDown" || e.key === "ArrowUp") { e.preventDefault(); setIndex((n) => Math.max(0, Math.min(rows.length - 1, n + (e.key === "ArrowDown" ? 1 : -1)))); }
          else if (e.key === "Enter" && rows[index]) { e.preventDefault(); open(rows[index]); }
        }} />
      {error ? <p className="error" role="alert">{error}<button onClick={() => setRetry((n) => n + 1)}>Retry</button></p> : !loaded ? <p className="muted">Reading the checkout…</p> : mode !== "files" && !selected ? <p className="muted">Open a file first.</p> : !rows.length ? <p className="muted">{query ? "No matching results." : mode === "line" ? "Enter a line number to jump to it." : "Type to find matching lines."}</p> : null}
      <div ref={list} className="ide-navigation-results" id="file-navigation-results" role="listbox" aria-label="Navigation results">
        {rows.map((row, i) => <button id={`file-result-${i}`} role="option" aria-selected={index === i} key={`${row.path}:${row.line ?? ""}`} onMouseEnter={() => setIndex(i)} onClick={() => open(row)}>
          <span>{row.path}{row.line && `:${row.line}`}</span>{row.detail && <small>{row.detail}</small>}
        </button>)}
      </div>
      <small className="muted">↑ ↓ to choose · Enter to open · Esc to close</small>
    </div>
  </Dialog>;
}
