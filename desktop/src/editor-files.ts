/** Drafts belong to a checkout and path, so switching or closing a tab never discards edits. */
import { create } from "zustand";
import { post, type Endpoint } from "./api";

export interface EditorFile { base: string; text: string; saving?: boolean; error?: string }
const KEY = "keel.editor-drafts";
const LIMIT = 1024 * 1024;
export const editorFileKey = (project: string, wt: string | undefined, path: string) => JSON.stringify([project, wt ?? "", path]);
function remembered(): Record<string, EditorFile> {
  try {
    const saved = JSON.parse(localStorage.getItem(KEY) ?? "{}");
    return Object.fromEntries(Object.entries(saved).filter((entry): entry is [string, EditorFile] => {
      const value = entry[1] as EditorFile | null;
      return !!value && typeof value.base === "string" && typeof value.text === "string" && value.base.length <= LIMIT && value.text.length <= LIMIT;
    }).map(([key, value]) => [key, { base: value.base, text: value.text }]));
  } catch { return {}; }
}
export const useEditorFiles = create<{
  files: Record<string, EditorFile>;
  storageError?: string;
  receive(key: string, text: string): void;
  edit(key: string, text: string): void;
  discard(key: string, text: string): void;
}>()((set) => ({
  files: remembered(),
  receive(key, text) { set((state) => {
    const old = state.files[key];
    const file = !old || old.text === old.base || old.text === text ? { base: text, text } : { ...old, error: text !== old.base ? "This file changed on disk. Your unsaved edits are kept." : old.error };
    return { files: { ...state.files, [key]: file } };
  }); },
  edit(key, text) { set((s) => ({ files: { ...s.files, [key]: { ...s.files[key], text } } })); },
  discard(key, text) { set((s) => ({ files: { ...s.files, [key]: { base: text, text } } })); },
}));
function patch(key: string, change: Partial<EditorFile>) {
  useEditorFiles.setState((state) => ({ files: { ...state.files, [key]: { ...state.files[key], ...change } } }));
}
/** Save the captured buffer; edits made while saving stay dirty against the newly saved base. */
export async function saveEditorFile(key: string, ep: Endpoint, path: string, wt?: string): Promise<boolean> {
  const file = useEditorFiles.getState().files[key];
  if (!file || file.saving || file.base === file.text) return false;
  const text = file.text;
  patch(key, { saving: true, error: undefined });
  try {
    await post(ep, "/api/file", { path, text, original: file.base }, { wt });
    patch(key, { base: text, saving: false, error: undefined });
    return true;
  } catch (error) { patch(key, { saving: false, error: error instanceof Error ? error.message : String(error) }); return false; }
}
export function persistEditorDrafts() {
  try {
    const unsaved = Object.fromEntries(Object.entries(useEditorFiles.getState().files).filter(([, file]) => file.base !== file.text).map(([key, { base, text }]) => [key, { base, text }]));
    localStorage.setItem(KEY, JSON.stringify(unsaved));
    if (useEditorFiles.getState().storageError) useEditorFiles.setState({ storageError: undefined });
  } catch { if (!useEditorFiles.getState().storageError) useEditorFiles.setState({ storageError: "Unsaved drafts cannot be stored for the next launch. Save your files before quitting." }); }
}
let timer: ReturnType<typeof setTimeout> | undefined;
useEditorFiles.subscribe((state, prior) => {
  if (state.files === prior.files) return;
  clearTimeout(timer); timer = setTimeout(persistEditorDrafts, 300);
});
if (typeof window !== "undefined") {
  window.addEventListener("pagehide", persistEditorDrafts);
  window.addEventListener("beforeunload", (event) => {
    persistEditorDrafts();
    if (Object.values(useEditorFiles.getState().files).some((file) => file.base !== file.text)) event.preventDefault();
  });
}
