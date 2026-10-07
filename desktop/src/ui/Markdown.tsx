import { memo, useEffect, useState } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { openUrl } from "@tauri-apps/plugin-opener";

export function Copy({ text, label = "Copy" }: { text: string; label?: string }) {
  const [state, setState] = useState(label);
  useEffect(() => { if (state === label) return; const t = setTimeout(() => setState(label), 1800); return () => clearTimeout(t); }, [state, label]);
  return <button className="chat-copy" onClick={() => void navigator.clipboard.writeText(text).then(() => setState("Copied"), () => setState("Copy failed"))}>{state}</button>;
}

function Code({ text, language }: { text: string; language: string }) {
  const [highlighted, setHighlighted] = useState<{ text: string; html: string }>();
  useEffect(() => {
    let cancelled = false;
    // Incomplete fences remain readable plain text; highlighting waits for a pause.
    const timer = setTimeout(() => {
      if (text.length > 40_000) return;
      void import("../syntax").then(({ highlight }) => {
        if (!cancelled) setHighlighted({ text, html: highlight(text, language) });
      });
    }, 180);
    return () => { cancelled = true; clearTimeout(timer); };
  }, [text, language]);
  return <div className="chat-code"><div className="chat-code-head"><span>{language || "Code"}</span><Copy text={text} /></div>
    <pre tabIndex={0}>{highlighted?.text === text ? <code dangerouslySetInnerHTML={{ __html: highlighted.html }} /> : <code>{text}</code>}</pre></div>;
}

export const Markdown = memo(function Markdown({ text }: { text: string }) {
  return <div className="chat-markdown"><ReactMarkdown remarkPlugins={[remarkGfm]} components={{
    pre: ({ children }) => <>{children}</>,
    code: ({ children, className, node }) => {
      const value = String(children).replace(/\n$/, "");
      const language = /language-([^ ]+)/.exec(className ?? "")?.[1] ?? "";
      const block = !!language || String(children).endsWith("\n") || (node?.position?.end.line ?? 0) > (node?.position?.start.line ?? 0);
      return block ? <Code text={value} language={language} /> : <code>{children}</code>;
    },
    table: ({ children }) => <div className="chat-table" tabIndex={0}><table>{children}</table></div>,
    a: ({ href, children }) => <a href={href} onClick={(e) => {
      e.preventDefault(); e.stopPropagation();
      if (href && /^https?:\/\//i.test(href)) void openUrl(href);
    }}>{children}</a>,
    img: ({ alt }) => <span className="muted">[Image: {alt || "attachment"}]</span>,
  }}>{text}</ReactMarkdown></div>;
});
