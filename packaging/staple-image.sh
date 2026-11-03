#!/bin/sh
# Retry Apple's ticket lookup without ever publishing an unstapled disk image.
set -eu

image=$1
attempt=1
max_attempts=5
retry_delay_seconds=15
while ! xcrun stapler staple "$image"; do
  if [ "$attempt" -ge "$max_attempts" ]; then
    echo "Could not staple the notarised image after $max_attempts attempts." >&2
    exit 1
  fi
  echo "Stapling failed; retrying Apple's ticket lookup (attempt $((attempt + 1)) of $max_attempts)." >&2
  sleep "$retry_delay_seconds"
  attempt=$((attempt + 1))
done
