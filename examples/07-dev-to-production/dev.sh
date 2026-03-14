#!/bin/bash
set -e
SITE="$(cd "$(dirname "$0")" && pwd)"
trap 'kill $(jobs -p) 2>/dev/null' EXIT

m6-html "$SITE" "$SITE/configs/m6-html.conf" &
m6-file "$SITE" "$SITE/configs/m6-file.conf" &
m6-auth "$SITE" "$SITE/configs/m6-auth.conf" &
"$SITE/target/release/render-cms" "$SITE" "$SITE/configs/render-cms.conf" &

# Second argument always required — use checked-in dev system config
m6-http "$SITE" "$SITE/configs/system-dev.toml" &

echo "Running at https://localhost:8443"
wait
