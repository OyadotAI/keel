#!/usr/bin/env bash
# Regenerate the README's stills and its reel from the app's own views.
#
# Nothing here is a screen recording. Every frame is a real SwiftUI view drawing real model state,
# rendered offscreen by `VisualCatalogTests`, so the artwork cannot quietly drift from the app the
# way a recording made once and kept forever does — re-run this after a UI change and the README is
# current again. The session in it is a fixture (a payments repository that does not exist), which
# is the other half of why it is rendered rather than filmed: no real repository, path, branch or
# prompt is ever in these pictures.
set -euo pipefail

cd "$(dirname "$0")/.."
out="docs/media"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

command -v ffmpeg >/dev/null || { echo "media: ffmpeg is not installed (brew install ffmpeg)"; exit 1; }

echo "==> rendering frames"
KEEL_README_MEDIA="$work" swift test --package-path app \
    --filter testCaptureReadmeMediaWhenRequested >/dev/null

for name in turn approve; do
    frames=$(find "$work/frames/$name" -name '*.png' | wc -l | tr -d ' ')
    [ "$frames" -gt 0 ] || { echo "media: the $name reel rendered no frames"; exit 1; }
    echo "==> encoding $name.gif ($frames frames)"
    # Two passes over the same frames: one to choose 128 colours for this reel specifically, one to
    # map onto them. A shared web palette turns the gate's greens to mud. The frames are rendered
    # at 2x, so the scale down to 900 is also the retina downsample.
    ffmpeg -y -loglevel error -framerate 10 -i "$work/frames/$name/%03d.png" \
        -filter_complex "fps=10,scale=900:-1:flags=lanczos,split[s0][s1];[s0]palettegen=max_colors=128[p];[s1][p]paletteuse=dither=bayer:bayer_scale=3" \
        -loop 0 "$out/$name.gif"
done

for still in turn approval; do
    cp "$work/$still.png" "$out/$still.png"
done

echo
ls -lh "$out"/{turn,approve}.gif "$out"/{turn,approval}.png | awk '{printf "    %-6s %s\n", $5, $9}'
