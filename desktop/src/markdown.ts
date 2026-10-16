export const MAX_MARKDOWN_PREVIEW = 128_000;
export const isMarkdown = (path = "") => /\.(md|markdown|mdown)$/i.test(path);

export function documentLink(current: string, href: string): { path: string; anchor?: string } | undefined {
  if (/^[a-z][a-z\d+.-]*:|^\/\//i.test(href)) return;
  const [rawPath, rawAnchor] = href.split("#", 2);
  try {
    const path = decodeURIComponent(rawPath.split("?", 1)[0]).replace(/\\/g, "/");
    if (/^[a-z][a-z\d+.-]*:|^\/\/|\0/i.test(path)) return;
    const parts = path.startsWith("/") ? [] : current.split("/").slice(0, -1);
    if (!path) return { path: current, anchor: decodeURIComponent(rawAnchor ?? "") };
    for (const part of path.split("/")) {
      if (part === "..") { if (!parts.length) return; parts.pop(); }
      else if (part && part !== ".") parts.push(part);
    }
    return parts.length ? { path: parts.join("/"), anchor: rawAnchor ? decodeURIComponent(rawAnchor) : undefined } : undefined;
  } catch { return; }
}

export function headingSlug(text: string) {
  return text.toLowerCase().trim().replace(/[^\p{L}\p{N}\s_-]/gu, "").replace(/\s/g, "-");
}
