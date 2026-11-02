// A pointer-transparent preview keeps internal file dragging visible without hiding drop targets.
import { useFileDrag } from "../file-drag";
import { FILE_DRAG_PREVIEW_OFFSET } from "../chat-constants";
import { Icon } from "./icons";

/** Show the checkout-relative file being referenced without moving the file itself. */
export function FileReferenceDrag() {
  const drag = useFileDrag((s) => s.drag);
  if (!drag) return null;
  return <div className="file-reference-drag" aria-hidden style={{ left: drag.x + FILE_DRAG_PREVIEW_OFFSET, top: drag.y + FILE_DRAG_PREVIEW_OFFSET }}><Icon name="file" size={13} /><span>{drag.path}</span></div>;
}
