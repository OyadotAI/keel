import { expect, it } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { MarkdownPreview } from "./MarkdownPreview";
import { MAX_MARKDOWN_PREVIEW } from "../markdown";

const render = (text: string) => renderToStaticMarkup(<MarkdownPreview text={text} path="README.md" lane="test" ep={{ port: 1, token: "private" }} revision={0} />);

it("renders document headings, tables, task lists, code fences and duplicate anchors", () => {
  const html = render('# Setup\n\n## Setup\n\n- [x] Ready\n\n| Name | Value |\n| --- | --- |\n| mode | preview |\n\n```ts\nconst ready = true;\n```');
  expect(html).toContain('id="setup"');
  expect(html).toContain('id="setup-1"');
  expect(html).toContain("<table>");
  expect(html).toContain('type="checkbox"');
  expect(html).toContain('disabled=""');
  expect(html).toContain('class="chat-code"');
  expect(html).toContain("const ready = true;");
});

it("keeps repository HTML and unsafe link schemes inert", () => {
  const html = render('<script>globalThis.executed = true</script>\n\n[unsafe](javascript:alert%281%29)');
  expect(html).not.toContain("<script>");
  expect(html).not.toContain('href="javascript:');
  expect(html).not.toContain("private");
});

it("bounds document rendering while leaving the full file available in Code", () => {
  const html = render("x".repeat(MAX_MARKDOWN_PREVIEW) + "HIDDEN_AFTER_LIMIT");
  expect(html).toContain("Preview limited");
  expect(html).not.toContain("HIDDEN_AFTER_LIMIT");
});
