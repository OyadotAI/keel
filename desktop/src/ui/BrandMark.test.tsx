// The shared Oya mark keeps its original rings and follows the active foreground without a tile.
import { expect, it } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { BrandMark } from "./BrandMark";

it("preserves Oya ring geometry and a theme-aware foreground at the requested size", () => {
  const html = renderToStaticMarkup(<BrandMark size={32} />);
  expect(html).toContain('width="32" height="32" viewBox="0 0 512 512"');
  expect(html).toContain('<circle cx="276" cy="276" r="160" stroke="var(--lime)" stroke-width="80"');
  expect(html).toContain('<circle cx="236" cy="236" r="160" stroke="var(--text)" stroke-width="80"');
  expect(html).toContain('aria-hidden="true"');
  expect(html).not.toContain("<path");
});
