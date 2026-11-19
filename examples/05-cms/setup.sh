#!/bin/bash
set -e
SITE="$(cd "$(dirname "$0")" && pwd)"

# Development keys, written by examples/dev-keys.sh into /tmp/m6/dev.
"$SITE/../dev-keys.sh"

# Create first admin/editor user
m6-auth-cli "$SITE/configs/m6-auth.conf" user add admin --role admin --password admin
m6-auth-cli "$SITE/configs/m6-auth.conf" group add editors
m6-auth-cli "$SITE/configs/m6-auth.conf" group member add editors admin

echo "Setup complete. Build the renderer, then start with ./dev.sh"
echo "  cargo build --release -p render-cms"
echo "  ./dev.sh"
