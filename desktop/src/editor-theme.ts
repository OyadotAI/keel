/** Editor colors follow Keel's palette; text and suggestions stay readable in both themes. */
import { EditorView } from "@codemirror/view";
import { HighlightStyle, syntaxHighlighting } from "@codemirror/language";
import { tags } from "@lezer/highlight";

export const editorTheme = EditorView.theme({
  "&": { height: "100%", color: "var(--text)", backgroundColor: "var(--bg)", fontSize: "12px" },
  ".cm-scroller": { fontFamily: "var(--mono)", lineHeight: "22px", overflow: "auto" },
  ".cm-content": { padding: "8px 0", caretColor: "var(--text)" },
  ".cm-gutters": { backgroundColor: "var(--bg)", color: "var(--faint)", border: "none", minWidth: "48px" },
  ".cm-activeLine, .cm-activeLineGutter": { backgroundColor: "var(--hover)" },
  "&.cm-focused .cm-selectionBackground, .cm-selectionBackground, ::selection": { backgroundColor: "var(--tint) !important" },
  ".cm-cursor": { borderLeftColor: "var(--text)" },
  ".cm-tooltip": { backgroundColor: "var(--raised)", color: "var(--text)", border: "1px solid var(--line)", borderRadius: "6px" },
  ".cm-tooltip-autocomplete ul li[aria-selected]": { backgroundColor: "var(--tint)", color: "var(--text)" },
  ".cm-ai-ghost": { color: "var(--faint)", opacity: "0.65", whiteSpace: "pre-wrap", pointerEvents: "none" },
  "&.cm-focused": { outline: "none" },
});
export const editorHighlight = syntaxHighlighting(HighlightStyle.define([
  { tag: [tags.keyword, tags.modifier, tags.operatorKeyword], color: "var(--syntax-keyword)" },
  { tag: [tags.string, tags.regexp], color: "var(--syntax-string)" },
  { tag: [tags.number, tags.bool, tags.null], color: "var(--syntax-number)" },
  { tag: [tags.function(tags.variableName), tags.function(tags.propertyName)], color: "var(--syntax-function)" },
  { tag: [tags.typeName, tags.className], color: "var(--syntax-type)" },
  { tag: tags.comment, color: "var(--syntax-comment)", fontStyle: "italic" },
]));
