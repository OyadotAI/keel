/** Links stay actionable in replies without changing code samples or existing link labels. */
import { expect, it } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { Markdown } from "./Markdown";
import { MarkdownPreview } from "./MarkdownPreview";

const renderers = {
  conversation: (text: string) => renderToStaticMarkup(<Markdown text={text} />),
  document: (text: string) => renderToStaticMarkup(<MarkdownPreview text={text} path="README.md" lane="test" ep={{ port: 1, token: "fixture" }} revision={0} />),
};

for (const [name, render] of Object.entries(renderers)) {
  it(`${name}: makes bare, explicit and code-formatted local URLs clickable`, () => {
    for (const url of ["http://localhost:3000", "http://127.0.0.1:5173/path?q=test#section", "https://example.com/docs"]) {
      for (const text of [`Visit ${url}`, `Open \`${url}\``, `[Open app](${url})`]) expect(render(text)).toContain(`href="${url}"`);
    }
  });
  it(`${name}: preserves code styling inside a clickable URL`, () => {
    expect(render("`http://localhost:3000`")).toContain('<a href="http://localhost:3000"><code>http://localhost:3000</code></a>');
  });
  it(`${name}: never nests anchors inside existing links`, () => {
    const html = render("[`http://localhost:3000`](http://localhost:3000) and [`https://example.com`][app]\n\n[app]: https://example.com");
    expect(html.match(/<a /g)).toHaveLength(2);
  });
  it(`${name}: keeps commands, blocks, malformed URLs and non-web protocols inert`, () => {
    for (const text of ["`curl http://localhost:3000`", "```\nhttp://localhost:3000\n```", "`http://`", "`http://[invalid`", "`javascript:alert(1)`", "`file:///etc/passwd`", "`mailto:person@example.com`", "`http://localhost:3000 run`"])
      expect(render(text)).not.toContain("<a ");
  });
}
