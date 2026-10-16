import { memo } from "react";
import { highlight, languageForPath } from "../syntax";

/** Highlight only rendered, bounded lines; diff markers and line numbers stay outside the code. */
export const SyntaxText = memo(function SyntaxText({ text, path }: { text: string; path: string }) {
  return <span className="syntax-code" dangerouslySetInnerHTML={{ __html: highlight(text.slice(0, 16_000), languageForPath(path)) + (text.length > 16_000 ? " … line truncated" : "") }} />;
});
