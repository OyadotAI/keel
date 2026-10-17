/** Language packages load only for the open file, keeping the editor and launch bundles small. */
import type { Extension } from "@codemirror/state";
import { EditorState } from "@codemirror/state";
import { completeAnyWord } from "@codemirror/autocomplete";
const javascript = async () => (await import("@codemirror/lang-javascript")).javascript({ jsx: true });
const languages: Record<string, () => Promise<Extension>> = {
  js: javascript, jsx: javascript, mjs: javascript, cjs: javascript,
  ts: async () => (await import("@codemirror/lang-javascript")).javascript({ typescript: true }),
  tsx: async () => (await import("@codemirror/lang-javascript")).javascript({ typescript: true, jsx: true }),
  json: async () => (await import("@codemirror/lang-json")).json(),
  html: async () => (await import("@codemirror/lang-html")).html(),
  css: async () => (await import("@codemirror/lang-css")).css(),
  py: async () => (await import("@codemirror/lang-python")).python(),
  rs: async () => (await import("@codemirror/lang-rust")).rust(),
  md: async () => (await import("@codemirror/lang-markdown")).markdown(),
};
export async function editorLanguage(path: string): Promise<Extension> {
  const suffix = path.split(".").at(-1)?.toLowerCase() ?? "";
  return [Object.hasOwn(languages, suffix) ? await languages[suffix]() : [], EditorState.languageData.of(() => [{ autocomplete: completeAnyWord }])];
}
