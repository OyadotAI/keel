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

# A bundle of its own, rather than debug binaries swapped into the release one.
#
# `BudgetTests.testBundleStaysSmall` measures dist/Keel.app, and a debug build of it is 86 MB
# against a 60 MB ceiling. `make check` rebuilds the release bundle first so the gate is honest
# either way — but a bare `swift test` after a `make dev` failed on a budget nobody had touched,
# which is a confusing failure to leave lying around for the sake of one `cp` path.
release="dist/Keel.app"
app="dist/Keel-dev.app"
if [ ! -d "$release" ]; then
  echo "no bundle yet — run 'make app' once to create $release, then 'make dev' from then on" >&2
  exit 1
fi
# Everything but the binaries: the plist, the icon, Sparkle. Copied once, and again whenever the
# release bundle is newer than this one — which is how a change to Info.plist reaches the loop.
if [ ! -d "$app" ] || [ "$release/Contents/Info.plist" -nt "$app/Contents/Info.plist" ]; then
  echo "==> refreshing the dev bundle from $release"
  rm -rf "$app"
  cp -R "$release" "$app"
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
