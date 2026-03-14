#!/bin/bash
set -e
SITE="$(cd "$(dirname "$0")" && pwd)"
trap 'kill $(jobs -p) 2>/dev/null' EXIT

# Regenerate posts.json from Markdown source before starting
# (requires m6-md: cargo install m6-md)
if command -v m6-md &>/dev/null; then
  m6-md "$SITE/content/posts/" --output "$SITE/data/posts.json"
fi

m6-html "$SITE" "$SITE/configs/m6-html.conf" &
m6-file "$SITE" "$SITE/configs/m6-file.conf" &
m6-http "$SITE" &

echo "Running at https://localhost:8443"
wait
