#!/bin/sh
# Verify transient stapling failures retry and persistent failures never report success.
set -eu

scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT HUP INT TERM
mkdir "$scratch/bin"
cat > "$scratch/bin/xcrun" <<'FAKE'
#!/bin/sh
count=0
if [ -f "$STAPLE_TEST_COUNT" ]; then count=$(cat "$STAPLE_TEST_COUNT"); fi
count=$((count + 1))
printf '%s' "$count" > "$STAPLE_TEST_COUNT"
[ "$count" -ge "$STAPLE_TEST_SUCCEED_AT" ]
FAKE
cat > "$scratch/bin/sleep" <<'FAKE'
#!/bin/sh
exit 0
FAKE
chmod +x "$scratch/bin/xcrun" "$scratch/bin/sleep"
export PATH="$scratch/bin:$PATH" STAPLE_TEST_COUNT="$scratch/count"
STAPLE_TEST_SUCCEED_AT=3; export STAPLE_TEST_SUCCEED_AT
sh packaging/staple-image.sh "$scratch/image.dmg"
[ "$(cat "$STAPLE_TEST_COUNT")" = 3 ]
rm "$STAPLE_TEST_COUNT"
STAPLE_TEST_SUCCEED_AT=6; export STAPLE_TEST_SUCCEED_AT
if sh packaging/staple-image.sh "$scratch/image.dmg"; then
  echo "A persistent stapling failure was reported as success." >&2
  exit 1
fi
[ "$(cat "$STAPLE_TEST_COUNT")" = 5 ]
