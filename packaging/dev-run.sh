#!/usr/bin/env bash
# Rebuild what changed and relaunch, in about five seconds.
#
# `make app` is the release path and it is the wrong tool for moving a button: cargo --release,
# swift --release, a rendered icon, and a Developer ID codesign of the whole bundle. A minute,
# nearly all of it work that has nothing to do with the edit.
#
# This swaps debug binaries into the bundle that is already there. Two things make that legal:
# the rpath has to match the release build's (Sparkle is a dynamic framework in
# Contents/Frameworks), and the bundle has to be re-signed after its binary changes or macOS
# refuses to launch it — ad-hoc is enough, and it takes no time.
#
# There is no hot reload here and this is not pretending to be one: AppKit has no mechanism for
# swapping a running view hierarchy, so the app restarts. What it does is make the restart cheap
# enough that it stops being the thing you notice.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

app="dist/Keel.app"
if [ ! -d "$app" ]; then
  echo "no bundle yet — run 'make app' once to create $app, then 'make dev' from then on" >&2
  exit 1
fi

echo "==> building (debug)"
cargo build --quiet
swift build --package-path app \
  -Xlinker -rpath -Xlinker @executable_path/../Frameworks

bin="$(swift build --package-path app --show-bin-path)"

# Quit the running copy before overwriting the binary it is executing. The daemon needs no
# separate kill: it polls `getppid()` and exits with the app, which is the whole reason
# `--exit-with-parent` exists.
if pgrep -x KeelApp >/dev/null; then
  echo "==> quitting the running app"
  osascript -e 'tell application "Keel" to quit' 2>/dev/null || pkill -x KeelApp || true
  for _ in $(seq 1 40); do
    pgrep -x KeelApp >/dev/null || break
    sleep 0.1
  done
fi

echo "==> swapping in the new binaries"
cp "$bin/KeelApp" "$app/Contents/MacOS/KeelApp"
cp target/debug/keel "$app/Contents/MacOS/keel"
# Resources are copied too: a `Bundle.module` lookup that misses traps rather than degrades, and
# the one that bit us was a file added to the package and not to the bundle.
if [ -d "$bin/KeelApp_KeelApp.bundle" ]; then
  cp -R "$bin/KeelApp_KeelApp.bundle/." "$app/Contents/Resources/"
fi

# Ad-hoc, and only the parts that changed — `--deep` re-signs the frameworks as well and is the
# difference between this taking a moment and taking several.
codesign --force --sign - "$app/Contents/MacOS/keel"
codesign --force --sign - "$app"

echo "==> launching"
open "$app"
