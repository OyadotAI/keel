#!/usr/bin/env python3
"""Render Keel's app icon.

The icon is generated rather than checked in as a binary, so it stays reproducible and so it
cannot drift from the mark in the application. The path below is the same one the title bar draws
in `ui/index.html`: a spine with a hull curving up from the keel.

Usage: python3 packaging/icon.py <out-dir>
"""
import sys
from pathlib import Path
from PIL import Image, ImageDraw, ImageFilter

# The dark ground and interactive accent from the UI's own palette, so the icon on the Dock and
# the window it opens are the same product.
GROUND_TOP = (26, 27, 24)
GROUND_BOTTOM = (16, 17, 15)
ACCENT = (110, 168, 254)

OUT = 1024               # macOS icon canvas
# Everything is drawn at four times that and scaled down. PIL joins a polyline with mitres and no
# antialiasing, so the flattened curves come out visibly scalloped at 1x; supersampling is the
# only fix that does not mean writing a rasteriser.
SS = 4
S = OUT * SS
INSET = 100 * SS
RADIUS = 185 * SS


def bezier(p0, p1, p2, p3, steps=64):
    """Sample a cubic. PIL has no path support, so the SVG curves are flattened here."""
    out = []
    for i in range(steps + 1):
        t = i / steps
        u = 1 - t
        out.append((
            u*u*u*p0[0] + 3*u*u*t*p1[0] + 3*u*t*t*p2[0] + t*t*t*p3[0],
            u*u*u*p0[1] + 3*u*u*t*p1[1] + 3*u*t*t*p2[1] + t*t*t*p3[1],
        ))
    return out


def render() -> Image.Image:
    img = Image.new("RGBA", (S, S), (0, 0, 0, 0))

    # A vertical gradient rather than a flat fill: at Dock size it is the only thing keeping the
    # tile from reading as a black rectangle.
    grad = Image.new("RGBA", (1, S))
    for y in range(S):
        t = y / (S - 1)
        grad.putpixel((0, y), tuple(
            int(a + (b - a) * t) for a, b in zip(GROUND_TOP, GROUND_BOTTOM)
        ) + (255,))
    grad = grad.resize((S, S))

    mask = Image.new("L", (S, S), 0)
    ImageDraw.Draw(mask).rounded_rectangle(
        (INSET, INSET - 10 * SS, S - INSET, S - INSET - 10 * SS), radius=RADIUS, fill=255
    )

    # A soft drop shadow, the way every other icon on the Dock has one.
    shadow = mask.filter(ImageFilter.GaussianBlur(26 * SS)).point(lambda v: int(v * 0.5))
    img.paste((0, 0, 0, 255), (0, 22 * SS), shadow)
    img.paste(grad, (0, 0), mask)

    # The mark, in the SVG's own 20x20 coordinates.
    scale = (S - 2 * INSET) * 0.50 / 20
    ox, oy = S / 2 - 10 * scale, S / 2 - 10 * scale - 10 * SS
    def P(x, y):
        return (ox + x * scale, oy + y * scale)

    draw = ImageDraw.Draw(img)
    width = int(2.4 * scale)

    # M10 1.5 v17 — the spine.
    draw.line([P(10, 1.5), P(10, 18.5)], fill=ACCENT + (255,), width=width, joint="curve")

    # c-3.6 0 -6.4-1.9 -7.6-4.3, and its mirror — the hull rising from the keel.
    for s in (-1, 1):
        pts = bezier(
            P(10, 18.5),
            P(10 + s * 3.6, 18.5),
            P(10 + s * 6.4, 18.5 - 1.9),
            P(10 + s * 7.6, 18.5 - 4.3),
            steps=220,
        )
        draw.line(pts, fill=ACCENT + (255,), width=width, joint="curve")

    # Round the stroke ends by hand; PIL's line has no cap style.
    r = width / 2
    for pt in (P(10, 1.5), P(10 - 7.6, 14.2), P(10 + 7.6, 14.2)):
        draw.ellipse((pt[0] - r, pt[1] - r, pt[0] + r, pt[1] + r), fill=ACCENT + (255,))

    return img.resize((OUT, OUT), Image.LANCZOS)


def main() -> None:
    out = Path(sys.argv[1] if len(sys.argv) > 1 else ".")
    out.mkdir(parents=True, exist_ok=True)
    icon = render()

    iconset = out / "Keel.iconset"
    iconset.mkdir(exist_ok=True)
    # The sizes `iconutil` requires. Downscaling from one 1024 render keeps every size identical
    # in weight and alignment.
    for size in (16, 32, 128, 256, 512):
        icon.resize((size, size), Image.LANCZOS).save(iconset / f"icon_{size}x{size}.png")
        icon.resize((size * 2, size * 2), Image.LANCZOS).save(
            iconset / f"icon_{size}x{size}@2x.png"
        )
    icon.save(out / "icon-1024.png")
    print(iconset)


if __name__ == "__main__":
    main()
