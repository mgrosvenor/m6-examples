#!/bin/bash
set -e
SITE="$(cd "$(dirname "$0")" && pwd)"

# Development keys, written by examples/dev-keys.sh into /tmp/m6/dev.
"$SITE/../dev-keys.sh"

# Create first admin user — database created automatically if absent
m6-auth-cli "$SITE/configs/m6-auth.conf" user add admin --role admin --password admin

echo "Setup complete. Start the server with ./dev.sh"
