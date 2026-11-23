#!/usr/bin/env python3
"""Render Oya Factory's app icon from the shared Oya ring geometry.

The flat ink tile and paper/lime rings follow AgentChrome's OyaLogo. Supersampling
keeps the vector geometry smooth at Dock sizes without a new rendering dependency.
Usage: python3 packaging/icon.py <out-dir>
"""
import sys
from pathlib import Path
from PIL import Image, ImageDraw

INK = (26, 26, 26, 255)
PAPER = (247, 245, 240, 255)
LIME = (205, 240, 30, 255)
OUT = 1024
SS = 4
CANVAS = OUT * SS
INSET = 100 * SS
TILE_RADIUS = 185 * SS
MARK_VIEWBOX = 512
RING_RADIUS = 160
RING_STROKE = 80
ICON_SIZES = (16, 32, 128, 256, 512)


def ring(draw, center, color):
    """Preserve OyaLogo's centered stroke while Pillow draws ellipse strokes inward."""
    scale = (CANVAS - 2 * INSET) / MARK_VIEWBOX
    radius = (RING_RADIUS + RING_STROKE / 2) * scale
    point = INSET + center * scale
    bounds = (point - radius, point - radius, point + radius, point + radius)
    draw.ellipse(bounds, outline=color, width=round(RING_STROKE * scale))


def render() -> Image.Image:
    """Use the shared rings on an ink tile so native launch surfaces match the workspace."""
    image = Image.new("RGBA", (CANVAS, CANVAS), (0, 0, 0, 0))
    draw = ImageDraw.Draw(image)
    bounds = (INSET, INSET, CANVAS - INSET, CANVAS - INSET)
    draw.rounded_rectangle(bounds, radius=TILE_RADIUS, fill=INK)
    ring(draw, 276, LIME)
    ring(draw, 236, PAPER)
    return image.resize((OUT, OUT), Image.Resampling.LANCZOS)


def main() -> None:
    """Write one reproducible master plus native iconset sizes for installed tooling."""
    out = Path(sys.argv[1] if len(sys.argv) > 1 else ".")
    iconset = out / "OyaFactory.iconset"
    iconset.mkdir(parents=True, exist_ok=True)
    icon = render()
    for size in ICON_SIZES:
        icon.resize((size, size), Image.Resampling.LANCZOS).save(iconset / f"icon_{size}x{size}.png")
        doubled = icon.resize((size * 2, size * 2), Image.Resampling.LANCZOS)
        doubled.save(iconset / f"icon_{size}x{size}@2x.png")
    icon.save(out / "icon-1024.png")
    print(iconset)


if __name__ == "__main__":
    main()
