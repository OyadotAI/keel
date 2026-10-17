/** A real editor with undo, syntax, local completion, inline AI and explicit conflict-safe saves. */
import { useCallback, useEffect, useRef, useState } from "react";
import { basicSetup } from "codemirror";
import { Compartment, EditorState, Prec, Transaction, StateEffect } from "@codemirror/state";
import { EditorView, keymap } from "@codemirror/view";
import { acceptCompletion } from "@codemirror/autocomplete";
import { indentWithTab } from "@codemirror/commands";
import type { Endpoint } from "../api";
import { get } from "../api";
import { editorLanguage } from "../editor-language";
import { editorHighlight, editorTheme } from "../editor-theme";
import { acceptAi, dismissAi, inlineAi, requestAi } from "../editor-ai";
import { editorFileKey, saveEditorFile, useEditorFiles } from "../editor-files";
import { useStore } from "../store";

const histories = new Map<string, EditorState>();
const positions = new Map<string, ReturnType<EditorView["scrollSnapshot"]>>();
const MAX_HISTORY = 20;
function cacheEditor(key: string, editor: EditorView) {
  histories.delete(key); histories.set(key, editor.state);
  positions.delete(key); positions.set(key, editor.scrollSnapshot());
  if (histories.size > MAX_HISTORY) { const oldest = histories.keys().next().value!; histories.delete(oldest); positions.delete(oldest); }
}
const ai = new Compartment();
const language = new Compartment();
function aiPreference() { try { return localStorage.getItem("keel.editor-ai") !== "off"; } catch { return true; } }

export default function CodeEditor({ lane, ep, wt, path, text, targetLine }: { lane: string; ep: Endpoint; wt?: string; path: string; text: string; targetLine?: { line: number; token: number } }) {
  const project = useStore((s) => s.lanes[lane]?.project ?? lane);
  const key = editorFileKey(project, wt, path);
  const file = useEditorFiles((s) => s.files[key]);
  const storageError = useEditorFiles((s) => s.storageError);
  const host = useRef<HTMLDivElement>(null);
  const view = useRef<EditorView>(null);
  const [enabled, setEnabled] = useState(aiPreference);
  const [status, setStatus] = useState("");
  const [reloading, setReloading] = useState(false);
  const save = useCallback(async () => {
    if (await saveEditorFile(key, ep, path, wt)) {
      void useStore.getState().refreshGit(lane);
      useStore.setState((s) => s.lanes[lane] ? { lanes: { ...s.lanes, [lane]: { ...s.lanes[lane], reviewRevision: (s.lanes[lane].reviewRevision ?? 0) + 1 } } } : s);
    }
  }, [key, ep, path, wt, lane]);
  useEffect(() => { useEditorFiles.getState().receive(key, text); }, [key, text]);
  useEffect(() => {
    if (!host.current) return;
    const initial = useEditorFiles.getState().files[key]?.text ?? "";
    const extensions = [basicSetup, language.of([]), editorTheme, editorHighlight, ai.of([]),
      EditorView.contentAttributes.of({ "aria-label": `Code editor: ${path}`, spellcheck: "false" }),
      Prec.highest(keymap.of([{ key: "Mod-s", run: () => { void save(); return true; } }, { key: "Alt-\\", run: requestAi },
        { key: "Tab", run: (v) => acceptCompletion(v) || acceptAi(v) || indentWithTab.run!(v) }, { key: "Shift-Tab", run: indentWithTab.shift }, { key: "Escape", run: dismissAi }])),
      EditorView.updateListener.of((update) => { if (update.docChanged) useEditorFiles.getState().edit(key, update.state.doc.toString()); })];
    let state = histories.get(key);
    if (!state || state.doc.toString() !== initial) state = EditorState.create({ doc: initial, extensions });
    else state = state.update({ effects: StateEffect.reconfigure.of(extensions) }).state;
    const editor = new EditorView({ state, parent: host.current, ...(positions.has(key) ? { scrollTo: positions.get(key) } : {}) });
    view.current = editor;
    return () => { cacheEditor(key, editor); editor.destroy(); view.current = null; };
    // Each checkout/file owns an editor lifetime; live disk updates are handled separately.
  }, [key, path, save]);
  useEffect(() => {
    let live = true;
    void editorLanguage(path).then((support) => { if (live) view.current?.dispatch({ effects: language.reconfigure(support) }); }, (error) => { if (live) setStatus(`Language support could not load: ${String(error)}`); });
    return () => { live = false; };
  }, [key, path, save]);
  useEffect(() => {
    const editor = view.current;
    if (editor && file && editor.state.doc.toString() !== file.text) editor.dispatch({ changes: { from: 0, to: editor.state.doc.length, insert: file.text }, annotations: Transaction.addToHistory.of(false) });
  }, [file]);
  useEffect(() => {
    view.current?.dispatch({ effects: ai.reconfigure(enabled ? inlineAi({ ep, wt, path, status: setStatus }) : []) });
    setStatus("");
  }, [enabled, ep, wt, path, key]);
  useEffect(() => {
    const editor = view.current;
    if (!editor || !targetLine) return;
    const line = editor.state.doc.line(Math.min(editor.state.doc.lines, Math.max(1, targetLine.line)));
    editor.dispatch({ selection: { anchor: line.from }, effects: EditorView.scrollIntoView(line.from, { y: "center" }) }); editor.focus();
  }, [targetLine, key]);
  async function reload() {
    if (file?.text !== file?.base && !window.confirm("Discard your unsaved edits and load the current file from disk?")) return;
    setReloading(true);
    try { const latest = await get<{ text: string | null }>(ep, "/api/file", { path, wt }); if (latest.text !== null) useEditorFiles.getState().discard(key, latest.text); }
    catch (error) { setStatus(String(error)); }
    finally { setReloading(false); }
  }
  const dirty = !!file && file.base !== file.text;
  return <div className="code-editor">
    <div className="code-editor-actions"><span className={dirty ? "editor-dirty" : "muted"}>{file?.saving ? "Saving…" : dirty ? "Unsaved changes" : "Saved"}</span>
      <span className="spacer" /><button aria-pressed={enabled} onClick={() => { const next = !enabled; setEnabled(next); try { localStorage.setItem("keel.editor-ai", next ? "on" : "off"); } catch { /* Optional preference. */ } }}>AI autocomplete {enabled ? "on" : "off"}</button>
      {enabled && <button onClick={() => { if (view.current) { view.current.focus(); requestAi(view.current); } }} title="Request an AI suggestion (Alt+\\)">Suggest</button>}
      <button disabled={reloading || file?.saving} onClick={() => void reload()}>Reload file</button><button className="primary" disabled={!dirty || file?.saving} title="Save file (⌘S / Ctrl+S)" onClick={() => void save()}>Save</button>
    </div>
    {(file?.error || storageError) && <div className="panel-note error" role="alert">{file?.error || storageError}</div>}
    <div className="code-editor-host" ref={host} />
    <div className="code-editor-status" role="status">{status || (enabled ? "AI suggestions use your Claude sign-in · Tab to accept · Ctrl+Space for code completion" : "Ctrl+Space for code completion · ⌘S / Ctrl+S to save")}</div>
  </div>;
}
