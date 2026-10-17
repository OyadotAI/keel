import { createElement, memo, useEffect, useRef, useState, type ReactNode } from "react";
import ReactMarkdown, { type Components } from "react-markdown";
import remarkGfm from "remark-gfm";
import { remarkCodeLinks } from "../markdown-links";
import { openUrl } from "@tauri-apps/plugin-opener";
import { getBlob, type Endpoint } from "../api";
import { useIde } from "../ide";
import { documentLink, headingSlug, isMarkdown, MAX_MARKDOWN_PREVIEW } from "../markdown";
import { Code } from "./Markdown";

type Node = { value?: string; children?: Node[] };
const nodeText = (node?: Node): string => node?.value ?? node?.children?.map(nodeText).join("") ?? "";
function headings(): Components {
  const seen = new Map<string, number>();
  const heading = (tag: string) => ({ node, children }: { node?: Node; children?: ReactNode }) => {
    const slug = headingSlug(nodeText(node));
    const count = seen.get(slug) ?? 0;
    seen.set(slug, count + 1);
    return createElement(tag, { id: count ? `${slug}-${count}` : slug }, children);
  };
  return { h1: heading("h1"), h2: heading("h2"), h3: heading("h3"), h4: heading("h4"), h5: heading("h5"), h6: heading("h6") };
}

export const MarkdownPreview = memo(function MarkdownPreview({ text, path, lane, ep, wt, anchor, revision }: {
  text: string; path: string; lane: string; ep: Endpoint; wt?: string; anchor?: string; revision: unknown;
}) {
  const box = useRef<HTMLElement>(null);
  const source = text.slice(0, MAX_MARKDOWN_PREVIEW);
  useEffect(() => {
    if (!anchor || !box.current) return;
    const heading = box.current.querySelector<HTMLElement>(`#${CSS.escape(anchor.replace(/^user-content-/, ""))}`);
    if (heading) box.current.scrollTop += heading.getBoundingClientRect().top - box.current.getBoundingClientRect().top - 20;
    useIde.getState().patch(lane, { previewAnchor: undefined });
  }, [anchor, text, lane]);
  function follow(href?: string) {
    if (!href) return;
    if (/^https?:\/\//i.test(href)) { void openUrl(href); return; }
    const target = documentLink(path, href);
    if (!target) return;
    if (target.path !== path) {
      useIde.getState().open(lane, target.path, "code");
      if (isMarkdown(target.path)) useIde.getState().markdownView(lane, useIde.getState().lanes[lane]?.markdown?.[path] === "split" ? "split" : "preview");
    }
    useIde.getState().patch(lane, { previewAnchor: target.anchor });
  }
  let images = 0;
  return <section className="markdown-preview" ref={box} tabIndex={0} aria-label={`Markdown preview: ${path}`}>
    {text.length > MAX_MARKDOWN_PREVIEW && <p className="panel-note muted">Preview limited to the first 128,000 characters. Open Code to read the full source.</p>}
    <div className="markdown-document chat-markdown syntax-code"><ReactMarkdown remarkPlugins={[remarkGfm, remarkCodeLinks]} components={{
      ...headings(),
      pre: ({ children }) => <>{children}</>,
      code: ({ children, className, node }) => {
        const value = String(children).replace(/\n$/, "");
        const language = /language-([^ ]+)/.exec(className ?? "")?.[1] ?? "";
        const block = !!language || String(children).endsWith("\n") || (node?.position?.end.line ?? 0) > (node?.position?.start.line ?? 0);
        return block ? <Code text={value} language={language} /> : <code>{children}</code>;
      },
      table: ({ children }) => <div className="chat-table" tabIndex={0}><table>{children}</table></div>,
      a: ({ href, children, title }) => <a href={href} title={title} onClick={(e) => { e.preventDefault(); e.stopPropagation(); follow(href); }}>{children}</a>,
      img: ({ src, alt }) => {
        const href = typeof src === "string" ? src : "";
        if (!href) return <span className="muted">{alt}</span>;
        const target = documentLink(path, href);
        if (!target || ++images > 20) return <a href={href} onClick={(e) => { e.preventDefault(); follow(href); }}>{alt || "View image"}</a>;
        return <DocumentImage ep={ep} wt={wt} path={target.path} alt={alt ?? ""} revision={revision} />;
      },
    }}>{source}</ReactMarkdown></div>
  </section>;
});

function DocumentImage({ ep, wt, path, alt, revision }: { ep: Endpoint; wt?: string; path: string; alt: string; revision: unknown }) {
  const box = useRef<HTMLSpanElement>(null);
  const [visible, setVisible] = useState(false);
  const [image, setImage] = useState<{ path: string; url: string }>();
  const [error, setError] = useState<string>();
  useEffect(() => {
    if (!box.current) return;
    const observer = new IntersectionObserver(([entry]) => { if (entry.isIntersecting) { setVisible(true); observer.disconnect(); } }, { rootMargin: "200px" });
    observer.observe(box.current);
    return () => observer.disconnect();
  }, []);
  useEffect(() => {
    if (!visible) return;
    let live = true;
    let objectURL: string | undefined;
    setImage(undefined); setError(undefined);
    getBlob(ep, "/api/raw", { path, wt }).then((blob) => {
      if (!live) return;
      if (blob.size > 4 * 1024 * 1024) { setError("Image exceeds the 4 MiB preview limit."); return; }
      // SVG stays a download at the HTTP endpoint. Render it only as an image in this CSP-bound view.
      const image = /\.svg$/i.test(path) ? new Blob([blob], { type: "image/svg+xml" }) : blob;
      if (!image.type.startsWith("image/")) { setError("This file cannot be displayed as an image."); return; }
      objectURL = URL.createObjectURL(image);
      setImage({ path, url: objectURL });
    }, (e) => { if (live) setError(String(e)); });
    return () => { live = false; if (objectURL) URL.revokeObjectURL(objectURL); };
  }, [ep, wt, path, visible, revision]);
  return <span className="markdown-image" ref={box}>{image?.path === path && !error ? <img src={image.url} alt={alt} onError={() => setError("Could not display this image.")} /> : <span className="muted">{error ?? (visible ? `Loading ${alt || path}…` : alt || path)}</span>}</span>;
}
