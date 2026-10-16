import { expect, it } from "vitest";
import { highlight, languageForPath } from "./syntax";

it("identifies source files and common extensionless configuration files", () => {
  for (const [path, language] of Object.entries({
    "src/main.tsx": "typescript", "C:\\repo\\main.go": "go", "config.yml": "yaml",
    "Cargo.toml": "ini", "Dockerfile": "dockerfile", "Dockerfile.dev": "dockerfile",
    ".zshrc": "bash", "data.sql": "sql", "View.cs": "csharp", "README.md": "markdown",
  })) expect(languageForPath(path)).toBe(language);
  expect(languageForPath("unknown.xyz")).toBe("");
});

it("colors keywords, strings, numbers and comments across supported code and configuration", () => {
  const html = highlight('const greeting: string = "Hello"; // note\nconst count = 42;', "typescript");
  for (const token of ["keyword", "string", "number", "comment"]) expect(html).toContain(`hljs-${token}`);
  expect(highlight('func main() { println("Hello") }', "go")).toContain("hljs-keyword");
  expect(highlight('name = "Keel"', "ini")).toContain("hljs-string");
});

it("keeps source text inert in both highlighted and unknown-language files", () => {
  const source = '<script>alert("x")</script> & <img src=x onerror=alert(1)>';
  for (const language of ["", "typescript", "xml"]) {
    const html = highlight(source, language);
    expect(html).not.toContain("<script>");
    expect(html).not.toContain("<img");
    expect(html).toContain("&lt;");
    expect(html).toContain("&amp;");
  }
});
