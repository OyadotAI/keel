import { expect, it } from "vitest";
import { documentLink, headingSlug, isMarkdown } from "./markdown";

it("resolves document links relative to the current file and keeps navigation inside the checkout", () => {
  expect(documentLink("docs/guide.md", "../README.md#Setup")).toEqual({ path: "README.md", anchor: "Setup" });
  expect(documentLink("docs/guide.md", "images/a%20b.png?raw=true")).toEqual({ path: "docs/images/a b.png", anchor: undefined });
  expect(documentLink("docs/guide.md", "#features")).toEqual({ path: "docs/guide.md", anchor: "features" });
  expect(documentLink("docs/guide.md", "/assets/logo.svg")).toEqual({ path: "assets/logo.svg", anchor: undefined });
  for (const href of ["../../secret", "%2e%2e/%2e%2e/secret", "javascript:alert(1)", "javascript%3Aalert(1)", "file:///etc/passwd", "//external/file", "https://example.com", "%00", "%xx"]) expect(documentLink("docs/guide.md", href)).toBeUndefined();
});

it("recognizes Markdown files and builds readable heading anchors", () => {
  expect(isMarkdown("README.MD")).toBe(true);
  expect(isMarkdown("docs/guide.markdown")).toBe(true);
  expect(isMarkdown("example.ts")).toBe(false);
  expect(headingSlug("Hello, **World**! And `code`")).toBe("hello-world-and-code");
  expect(headingSlug("مرحبا بالعالم")).toBe("مرحبا-بالعالم");
});
