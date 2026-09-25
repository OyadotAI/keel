import json
import os
import sys
import textwrap

W, H, M = 612, 792, 72  # US Letter, 1in margins


def fail(msg):
    print(json.dumps({"error": msg}))
    sys.exit(1)


def esc(s):
    return s.replace("\\", "\\\\").replace("(", "\\(").replace(")", "\\)")


def wrap(s, size):
    # ponytail: average Helvetica glyph width ~0.5em; use real AFM widths if wrapping looks off
    return textwrap.wrap(s.expandtabs(4), max(1, int((W - 2 * M) / (size * 0.5)))) or [""]


def build(text, title, size):
    items = []
    if title:
        items += [("F2", size * 1.5, l) for l in wrap(title, size * 1.5)] + [("F1", size, "")]
    for para in text.split("\n"):
        items += [("F1", size, l) for l in wrap(para, size)]

    pages, cur, y = [], [], H - M
    for font, s, line in items:
        if y - s * 1.2 < M and cur:
            pages.append(cur)
            cur, y = [], H - M
        y -= s * 1.2
        cur.append(f"BT /{font} {s:g} Tf {M} {y:.2f} Td ({esc(line)}) Tj ET")
    pages.append(cur)

    kids = " ".join(f"{6 + 2 * i} 0 R" for i in range(len(pages)))
    objs = [
        b"<< /Type /Catalog /Pages 2 0 R >>",
        f"<< /Type /Pages /Kids [{kids}] /Count {len(pages)} >>".encode(),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>",
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica-Bold /Encoding /WinAnsiEncoding >>",
        f"<< /Title ({esc(title or '')}) /Producer (pdf-generator) >>".encode("latin-1", "replace"),
    ]
    for i, ops in enumerate(pages):
        stream = "\n".join(ops).encode("latin-1", "replace")
        objs.append(
            f"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {W} {H}] "
            f"/Resources << /Font << /F1 3 0 R /F2 4 0 R >> >> /Contents {7 + 2 * i} 0 R >>".encode()
        )
        objs.append(f"<< /Length {len(stream)} >>\nstream\n".encode() + stream + b"\nendstream")

    out = bytearray(b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n")
    offsets = []
    for n, body in enumerate(objs, 1):
        offsets.append(len(out))
        out += f"{n} 0 obj\n".encode() + body + b"\nendobj\n"
    xref = len(out)
    out += f"xref\n0 {len(objs) + 1}\n0000000000 65535 f \n".encode()
    out += "".join(f"{o:010d} 00000 n \n" for o in offsets).encode()
    out += f"trailer\n<< /Size {len(objs) + 1} /Root 1 0 R /Info 5 0 R >>\nstartxref\n{xref}\n%%EOF\n".encode()
    return bytes(out), len(pages)


def main():
    try:
        args = json.loads(sys.argv[1] if len(sys.argv) > 1 else sys.stdin.read() or "{}")
    except json.JSONDecodeError as e:
        fail(f"invalid JSON input: {e}")
    if not isinstance(args, dict):
        fail("input must be a JSON object")

    text = args.get("text")
    if not isinstance(text, str) or not text.strip():
        fail("'text' is required and must be a non-empty string")
    title = args.get("title") or ""
    if not isinstance(title, str):
        fail("'title' must be a string")
    size = args.get("font_size", 11)
    if isinstance(size, bool) or not isinstance(size, (int, float)) or not 6 <= size <= 72:
        fail("'font_size' must be a number between 6 and 72")
    path = args.get("output_path") or "document.pdf"
    if not isinstance(path, str):
        fail("'output_path' must be a string")

    data, pages = build(text.replace("\r\n", "\n"), title, size)
    path = os.path.abspath(os.path.expanduser(path))
    try:
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "wb") as f:
            f.write(data)
    except OSError as e:
        fail(f"could not write {path}: {e}")
    print(json.dumps({"path": path, "pages": pages, "bytes": len(data)}))


if __name__ == "__main__":
    main()
