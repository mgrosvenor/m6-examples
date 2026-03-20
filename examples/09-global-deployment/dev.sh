#!/bin/bash
# dev.sh — run a 6-node global deployment topology on a single machine.
#
# Topology (all on loopback):
#   Origin H2C (9000) ← cache nodes connect here
#   Origin public (9001, TLS) ← direct access
#   Cache SF (9002), NYC (9003), Chicago (9004), London (9005), Singapore (9006)
#
# The cache nodes simply proxy all traffic to the origin H2C instance.
# This mirrors the production setup where cache nodes talk to Sydney over WireGuard.
#
# Prerequisites: m6-http, m6-html, m6-file, m6-auth, m6-auth-cli, m6-md,
#                render-cms (from 07-dev-to-production), mkcert.
set -e

SITE09="$(cd "$(dirname "$0")" && pwd)"
SITE07="$(cd "$SITE09/../07-dev-to-production" && pwd)"
M6="${M6:-$(cd "$SITE09/../../m6" && pwd)}"
EXAMPLES="${EXAMPLES:-$(cd "$SITE09/.." && pwd)}"
export PATH="$M6/target/release:$PATH"

# ── TLS certs (shared by all local TLS instances) ─────────────────────────────
mkdir -p "$SITE09/certs"
if [ ! -f "$SITE09/certs/local.pem" ]; then
    if ! command -v mkcert &>/dev/null; then
        echo "ERROR: mkcert not found. Install with: brew install mkcert" >&2
        exit 1
    fi
    mkcert -install 2>/dev/null || true
    mkcert -key-file "$SITE09/certs/local-key.pem" \
           -cert-file "$SITE09/certs/local.pem" \
           localhost 127.0.0.1
fi

# ── Auth keys ─────────────────────────────────────────────────────────────────
mkdir -p "$SITE09/keys"
if [ ! -f "$SITE09/keys/auth.pem" ]; then
    echo "Generating auth signing keys..."
    openssl genpkey -algorithm EC -pkeyopt ec_paramgen_curve:P-256 -out "$SITE09/keys/auth.pem"
    openssl pkey -in "$SITE09/keys/auth.pem" -pubout -out "$SITE09/keys/auth.pub"
    chmod 600 "$SITE09/keys/auth.pem"
fi

# ── Auth database ─────────────────────────────────────────────────────────────
mkdir -p "$SITE09/configs/data"
if [ ! -f "$SITE09/configs/data/auth.db" ]; then
    echo "Creating admin user (password: admin)..."
    AUTH_CONF="$SITE07/configs/m6-auth.conf"
    M6_AUTH_DATA_DIR="$SITE09/configs/data" \
    M6_AUTH_KEY_FILE="$SITE09/keys/auth.pem" \
        m6-auth-cli "$AUTH_CONF" user add admin --password admin
    M6_AUTH_DATA_DIR="$SITE09/configs/data" \
    M6_AUTH_KEY_FILE="$SITE09/keys/auth.pem" \
        m6-auth-cli "$AUTH_CONF" group add editors
    M6_AUTH_DATA_DIR="$SITE09/configs/data" \
    M6_AUTH_KEY_FILE="$SITE09/keys/auth.pem" \
        m6-auth-cli "$AUTH_CONF" group member add editors admin
fi

# ── Generate posts.json ────────────────────────────────────────────────────────
m6-md "$SITE07/content/posts/" --output "$SITE07/data/posts.json"

# ── Clean up stale processes ──────────────────────────────────────────────────
for port in 9000 9001 9002 9003 9004 9005 9006; do
    lsof -ti :"$port" 2>/dev/null | xargs kill -9 2>/dev/null || true
done
sleep 0.3
rm -f /tmp/m6/m6-html*.sock /tmp/m6/m6-file*.sock /tmp/m6/m6-auth*.sock \
       /tmp/m6/render-cms*.sock
mkdir -p /tmp/m6

# ── Backend services (reuse 07's content and config) ──────────────────────────
M6_SOCKET_OVERRIDE=/tmp/m6/m6-html.sock \
    m6-html "$SITE07" "$SITE07/configs/m6-html.conf" &
HTML_PID=$!

M6_SOCKET_OVERRIDE=/tmp/m6/m6-file.sock \
    m6-file "$SITE07" "$SITE07/configs/m6-file.conf" &
FILE_PID=$!

M6_SOCKET_OVERRIDE=/tmp/m6/m6-auth.sock \
    M6_AUTH_DATA_DIR="$SITE09/configs/data" \
    M6_AUTH_KEY_FILE="$SITE09/keys/auth.pem" \
    m6-auth-server "$SITE07" "$SITE07/configs/m6-auth.conf" &
AUTH_PID=$!

M6_SOCKET_OVERRIDE=/tmp/m6/render-cms.sock \
    "$EXAMPLES/target/release/render-cms" "$SITE07" "$SITE07/configs/render-cms.conf" &
RENDER_PID=$!

ALL_PIDS="$HTML_PID $FILE_PID $AUTH_PID $RENDER_PID"

# Wait for backend sockets
echo "Waiting for backend services..."
for i in $(seq 1 50); do
    [ -S /tmp/m6/m6-html.sock ] && \
    [ -S /tmp/m6/m6-file.sock ] && \
    [ -S /tmp/m6/m6-auth.sock ] && \
    [ -S /tmp/m6/render-cms.sock ] && break
    sleep 0.2
done

# ── Origin — H2C (port 9000, no TLS) ─────────────────────────────────────────
m6-http "$SITE09/site-origin" "$SITE09/configs/local/sydney-h2c.toml" &
H2C_PID=$!
ALL_PIDS="$ALL_PIDS $H2C_PID"

# ── Origin — public TLS (port 9001) ───────────────────────────────────────────
m6-http "$SITE09/site-origin" "$SITE09/configs/local/sydney-public.toml" &
PUBLIC_PID=$!
ALL_PIDS="$ALL_PIDS $PUBLIC_PID"

# ── Cache nodes (ports 9002–9006, all proxy to origin H2C on 9000) ────────────
m6-http "$SITE09/site-cache" "$SITE09/configs/local/cache-sf.toml" &
ALL_PIDS="$ALL_PIDS $!"

m6-http "$SITE09/site-cache" "$SITE09/configs/local/cache-nyc.toml" &
ALL_PIDS="$ALL_PIDS $!"

m6-http "$SITE09/site-cache" "$SITE09/configs/local/cache-chicago.toml" &
ALL_PIDS="$ALL_PIDS $!"

m6-http "$SITE09/site-cache" "$SITE09/configs/local/cache-london.toml" &
ALL_PIDS="$ALL_PIDS $!"

m6-http "$SITE09/site-cache" "$SITE09/configs/local/cache-singapore.toml" &
ALL_PIDS="$ALL_PIDS $!"

trap "kill $ALL_PIDS 2>/dev/null; wait 2>/dev/null" EXIT

echo ""
echo "┌─────────────────────────────────────────────────────────────┐"
echo "│  6-node global deployment — local demo                      │"
echo "├─────────────────────────────────────────────────────────────┤"
echo "│  Origin (Sydney)                                            │"
echo "│    H2C backbone : http://127.0.0.1:9000  (cache nodes only) │"
echo "│    Public TLS   : https://127.0.0.1:9001                    │"
echo "├─────────────────────────────────────────────────────────────┤"
echo "│  Cache nodes                                                │"
echo "│    San Francisco : https://127.0.0.1:9002                   │"
echo "│    New York      : https://127.0.0.1:9003                   │"
echo "│    Chicago       : https://127.0.0.1:9004                   │"
echo "│    London        : https://127.0.0.1:9005                   │"
echo "│    Singapore     : https://127.0.0.1:9006                   │"
echo "├─────────────────────────────────────────────────────────────┤"
echo "│  CMS: https://127.0.0.1:9001/cms  (login: admin / admin)    │"
echo "│  Each cache node proxies CMS to origin (no-store cache)     │"
echo "└─────────────────────────────────────────────────────────────┘"
echo ""

wait
