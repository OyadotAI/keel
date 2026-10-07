import hljs from "highlight.js/lib/core";
import javascript from "highlight.js/lib/languages/javascript";
import typescript from "highlight.js/lib/languages/typescript";
import rust from "highlight.js/lib/languages/rust";
import python from "highlight.js/lib/languages/python";
import bash from "highlight.js/lib/languages/bash";
import json from "highlight.js/lib/languages/json";
import css from "highlight.js/lib/languages/css";
import xml from "highlight.js/lib/languages/xml";
import swift from "highlight.js/lib/languages/swift";
for (const [name, grammar] of Object.entries({ javascript, typescript, rust, python, bash, json, css, xml, swift })) hljs.registerLanguage(name, grammar);
export function highlight(text: string, language: string): string {
  return hljs.getLanguage(language) ? hljs.highlight(text, { language, ignoreIllegals: true }).value : text.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}
