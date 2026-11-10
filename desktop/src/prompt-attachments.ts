// Images a prompt carried: Keel names each upload in the prompt (`agent::with_attachments`), so
// the live turn and its replay both say where the image is, whichever agent ran it.

/** The trailer Keel appends after the person's words; never shown, it is Keel's own note. */
const TRAILER = /\n*Attached files in this checkout \(read them\):\n[\s\S]*$/;
const UPLOAD = /@?(\.keel\/attachments\/[^\s"'<>]+)/g;
const IMAGE = /\.(png|jpe?g|gif|webp|avif)$/i;

/** The words to show, and the uploaded images the prompt named, in order and once each. */
export function promptAttachments(prompt: string): { text: string; images: string[] } {
  const images = [...new Set([...prompt.matchAll(UPLOAD)].map((m) => m[1]).filter((p) => IMAGE.test(p) && !p.includes("..")))];
  return { text: prompt.replace(TRAILER, "").trim(), images };
}
