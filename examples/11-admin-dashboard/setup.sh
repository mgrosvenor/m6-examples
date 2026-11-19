#!/bin/bash
# setup.sh — one-time setup for example 11-admin-dashboard.
set -e

SITE="$(cd "$(dirname "$0")" && pwd)"
M6="$(cd "$SITE/../../../m6" && pwd)"
export PATH="$M6/target/release:$SITE/../../../m6-examples/target/release:$PATH"

echo "=== 11-admin-dashboard setup ==="

# ── Build render-admin ────────────────────────────────────────────────────────
echo "Building render-admin..."
cargo build --release -p render-admin

# Development keys, written by examples/dev-keys.sh into /tmp/m6/dev.
"$SITE/../dev-keys.sh"

# ── Dirs ──────────────────────────────────────────────────────────────────────
mkdir -p "$SITE/data" "$SITE/logs"
mkdir -p /tmp/m6

# ── User database ─────────────────────────────────────────────────────────────
if [ ! -f /tmp/m6/11-admin-dashboard/auth.db ]; then
    echo "Creating admin user..."
    m6-auth-cli "$SITE/configs/m6-auth.conf" user add admin --role admin --password admin
fi

echo ""
echo "Setup complete."
echo "Run ./dev.sh to start the server."
