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
import go from "highlight.js/lib/languages/go";
import yaml from "highlight.js/lib/languages/yaml";
import ini from "highlight.js/lib/languages/ini";
import sql from "highlight.js/lib/languages/sql";
import markdown from "highlight.js/lib/languages/markdown";
import dockerfile from "highlight.js/lib/languages/dockerfile";
import cpp from "highlight.js/lib/languages/cpp";
import java from "highlight.js/lib/languages/java";
import csharp from "highlight.js/lib/languages/csharp";
import ruby from "highlight.js/lib/languages/ruby";
import php from "highlight.js/lib/languages/php";
for (const [name, grammar] of Object.entries({ javascript, typescript, rust, python, bash, json, css, xml, swift, go, yaml, ini, sql, markdown, dockerfile, cpp, java, csharp, ruby, php })) hljs.registerLanguage(name, grammar);

const EXTENSIONS: Record<string, string> = {
  js: "javascript", jsx: "javascript", mjs: "javascript", cjs: "javascript",
  ts: "typescript", tsx: "typescript", mts: "typescript", cts: "typescript",
  rs: "rust", py: "python", pyi: "python", sh: "bash", bash: "bash", zsh: "bash",
  json: "json", jsonc: "json", css: "css", html: "xml", htm: "xml", xml: "xml", svg: "xml", vue: "xml", svelte: "xml",
  swift: "swift", go: "go", yaml: "yaml", yml: "yaml", toml: "ini", ini: "ini", conf: "ini",
  sql: "sql", md: "markdown", mdx: "markdown", c: "cpp", h: "cpp", cc: "cpp", cpp: "cpp", hpp: "cpp",
  java: "java", cs: "csharp", rb: "ruby", php: "php",
};
export function languageForPath(path: string): string {
  const name = path.replace(/\\/g, "/").split("/").at(-1)?.toLowerCase() ?? "";
  if (name === "dockerfile" || name.startsWith("dockerfile.")) return "dockerfile";
  if ([".bashrc", ".zshrc", ".bash_profile", ".profile"].includes(name)) return "bash";
  return EXTENSIONS[name.split(".").at(-1) ?? ""] ?? "";
}
export function highlight(text: string, language: string): string {
  return hljs.getLanguage(language) ? hljs.highlight(text, { language, ignoreIllegals: true }).value : text.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}
