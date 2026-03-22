#!/bin/bash
# setup.sh — one-time setup for example 11-admin-dashboard.
set -e

SITE="$(cd "$(dirname "$0")" && pwd)"

echo "=== 11-admin-dashboard setup ==="

# ── Build render-admin ────────────────────────────────────────────────────────
echo "Building render-admin..."
cargo build --release -p render-admin

# ── Keys ─────────────────────────────────────────────────────────────────────
if [ ! -f "$SITE/keys/auth.pem" ]; then
    echo "Generating JWT keys..."
    mkdir -p "$SITE/keys"
    openssl genpkey -algorithm EC -pkeyopt ec_paramgen_curve:P-256 \
        -out "$SITE/keys/auth.pem"
    openssl pkey -pubout -in "$SITE/keys/auth.pem" -out "$SITE/keys/auth.pub"
fi

# ── Dirs ──────────────────────────────────────────────────────────────────────
mkdir -p "$SITE/data" "$SITE/logs"
mkdir -p /tmp/m6

# ── User database ─────────────────────────────────────────────────────────────
M6="$(cd "$SITE/../../../m6" && pwd)"
export PATH="$M6/target/release:$PATH"

if [ ! -f "$SITE/data/users.db" ]; then
    echo "Creating admin user..."
    m6-auth-cli "$SITE/configs/m6-auth.conf" user create admin changeme
    m6-auth-cli "$SITE/configs/m6-auth.conf" user roles admin admin
fi

echo ""
echo "Setup complete."
echo "Run ./dev.sh to start the server."
