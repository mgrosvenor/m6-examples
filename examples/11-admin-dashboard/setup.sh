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

# ── Keys ──────────────────────────────────────────────────────────────────────
if [ ! -f "$SITE/keys/auth.pem" ]; then
    echo "Generating JWT keys..."
    mkdir -p "$SITE/keys"
    openssl genpkey -algorithm EC -pkeyopt ec_paramgen_curve:P-256 \
        -out "$SITE/keys/auth.pem"
    openssl pkey -pubout -in "$SITE/keys/auth.pem" -out "$SITE/keys/auth.pub"
    chmod 600 "$SITE/keys/auth.pem"
fi

# ── Dirs ──────────────────────────────────────────────────────────────────────
mkdir -p "$SITE/data" "$SITE/logs"
mkdir -p /tmp/m6

# ── User database ─────────────────────────────────────────────────────────────
if [ ! -f "$SITE/data/auth.db" ]; then
    echo "Creating admin user..."
    m6-auth-cli "$SITE/configs/m6-auth.conf" user add admin --role admin --password changeme
fi

echo ""
echo "Setup complete."
echo "Run ./dev.sh to start the server."
