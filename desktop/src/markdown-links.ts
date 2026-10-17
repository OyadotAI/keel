/** Turn standalone web URLs in inline code into links without touching commands or examples. */
interface Node {
  type: string;
  value?: string;
  url?: string;
  children?: Node[];
}

/** Parse only a complete HTTP(S) URL; whitespace usually means this is a shell command. */
function webUrl(value?: string): boolean {
  if (!value || !/^https?:\/\/\S+$/i.test(value)) return false;
  try { return !!new URL(value).hostname; }
  catch { return false; }
}

/** Existing Markdown links own their labels, so never nest a second anchor inside them. */
function linkCode(node: Node): void {
  if (!node.children || node.type === "link" || node.type === "linkReference") return;
  node.children = node.children.map((child) => {
    if (child.type === "inlineCode" && webUrl(child.value)) return { type: "link", url: child.value, children: [child] };
    linkCode(child);
    return child;
  });
}

/** Apply the same link behavior to conversations and repository Markdown previews. */
export function remarkCodeLinks() { return linkCode; }
