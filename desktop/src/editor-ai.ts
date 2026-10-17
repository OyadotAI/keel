/** Debounced inline AI suggestions are decorations until the person accepts them with Tab. */
import { StateEffect, StateField, type Extension } from "@codemirror/state";
import { Decoration, EditorView, ViewPlugin, WidgetType, type DecorationSet, type ViewUpdate } from "@codemirror/view";
import { url, type Endpoint } from "./api";

export interface AiOptions { ep: Endpoint; path: string; wt?: string; status(message: string): void }
interface Suggestion { at: number; text: string }
const suggest = StateEffect.define<Suggestion | null>();
class Ghost extends WidgetType {
  constructor(readonly text: string) { super(); }
  eq(other: Ghost) { return this.text === other.text; }
  toDOM() { const span = document.createElement("span"); span.className = "cm-ai-ghost"; span.textContent = this.text; span.setAttribute("aria-hidden", "true"); return span; }
}
const ghost = StateField.define<Suggestion | null>({
  create: () => null,
  update(value, transaction) {
    if (transaction.docChanged || transaction.selection) value = null;
    for (const effect of transaction.effects) if (effect.is(suggest)) value = effect.value;
    return value;
  },
  provide: (field) => EditorView.decorations.from(field, (value): DecorationSet => value ? Decoration.set([Decoration.widget({ widget: new Ghost(value.text), side: 1 }).range(value.at)]) : Decoration.none),
});
export function acceptAi(view: EditorView): boolean {
  const value = view.state.field(ghost, false);
  if (!value) return false;
  view.dispatch({ changes: { from: value.at, insert: value.text }, selection: { anchor: value.at + value.text.length }, effects: suggest.of(null) });
  return true;
}
export function dismissAi(view: EditorView): boolean {
  if (!view.state.field(ghost, false)) return false;
  view.dispatch({ effects: suggest.of(null) }); return true;
}
const trigger = StateEffect.define<null>();
export function requestAi(view: EditorView): boolean { view.dispatch({ effects: trigger.of(null) }); return true; }

/** Responses are tied to an exact document and cursor; stale suggestions are never inserted. */
export function inlineAi(options: AiOptions): Extension {
  return [ghost, ViewPlugin.fromClass(class {
    timer?: ReturnType<typeof setTimeout>;
    controller?: AbortController;
    disposed = false;
    busy = false;
    constructor(readonly view: EditorView) {}
    update(update: ViewUpdate) {
      if (update.docChanged || update.selectionSet || update.transactions.some((t) => t.effects.some((e) => e.is(trigger)))) {
        clearTimeout(this.timer);
        this.timer = setTimeout(() => void this.complete(), 750);
      }
    }
    async complete() {
      const { state } = this.view;
      if (this.disposed || this.busy || !state.selection.main.empty) return;
      const at = state.selection.main.head;
      const prefix = state.doc.sliceString(Math.max(0, at - 3000), at);
      if (!prefix.trim()) return;
      this.busy = true;
      this.controller = new AbortController();
      options.status("AI is suggesting…");
      const deadline = setTimeout(() => this.controller?.abort(), 22_000);
      try {
        const response = await fetch(url(options.ep, "/api/editor/complete", { wt: options.wt }), { method: "POST", headers: { Authorization: `Bearer ${options.ep.token}`, "Content-Type": "application/json" }, signal: this.controller.signal,
          body: JSON.stringify({ path: options.path, prefix, suffix: state.doc.sliceString(at, at + 800) }) });
        if (!response.ok) throw Error(await response.text() || "AI autocomplete is unavailable.");
        const reply: { text: string } = await response.json();
        if (this.disposed) return;
        if (this.view.state.doc !== state.doc || this.view.state.selection.main.head !== at) { options.status(""); this.timer = setTimeout(() => void this.complete(), 750); return; }
        if (typeof reply.text === "string" && reply.text && reply.text.length <= 2048) this.view.dispatch({ effects: suggest.of({ at, text: reply.text }) });
        options.status(reply.text ? "Tab accepts AI suggestion · Esc dismisses" : "");
      } catch (error) { if (!this.disposed) options.status(error instanceof Error ? error.message : String(error)); }
      finally { clearTimeout(deadline); this.busy = false; }
    }
    destroy() { this.disposed = true; clearTimeout(this.timer); this.controller?.abort(); }
  })];
}
