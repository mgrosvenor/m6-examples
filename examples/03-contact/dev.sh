#!/bin/bash
set -e
SITE="$(cd "$(dirname "$0")" && pwd)"
trap 'kill $(jobs -p) 2>/dev/null' EXIT

m6-html "$SITE" "$SITE/configs/m6-html.conf" &
m6-file "$SITE" "$SITE/configs/m6-file.conf" &
"$SITE/target/release/render-contact" "$SITE" "$SITE/configs/render-contact.conf" &
m6-http "$SITE" &

echo "Running at https://localhost:8443"
wait
