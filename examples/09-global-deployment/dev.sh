#!/bin/bash
# dev.sh — run a 6-node global deployment topology on a single machine.
# M6_PORT=9001
#
# Topology (all on loopback):
#   Origin H2C (9000) ← cache nodes connect here (H2C, no TLS)
#   Origin public (9001, TLS) ← direct access
#   Cache SF (9002), NYC (9003), Chicago (9004), London (9005), Singapore (9006)
#
# The cache nodes proxy all traffic to the origin H2C instance.
# This mirrors production where cache nodes talk to Sydney over WireGuard H2C.
#
# Reuses 07-dev-to-production's site content, auth setup, and backend configs.
# Prerequisites: m6-http, m6-html, m6-file, m6-auth-server, m6-md,
#                render-cms (from 07-dev-to-production), mkcert.
set -e

SITE09="$(cd "$(dirname "$0")" && pwd)"
SITE07="$(cd "$SITE09/../07-dev-to-production" && pwd)"
M6="${M6:-$(cd "$SITE09/../../../m6" && pwd)}"
EXAMPLES="${EXAMPLES:-$(cd "$SITE09/../.." && pwd)}"
export PATH="$M6/target/release:$PATH"

# ── Require 07 to have been set up first ─────────────────────────────────────
if [ ! -f "$SITE07/keys/auth.pub" ]; then
    echo "ERROR: $SITE07/keys/auth.pub not found."
    echo "Run examples/07-dev-to-production/dev.sh first to set up auth keys." >&2
    exit 1
fi

# ── TLS certs (shared by all local TLS instances) ─────────────────────────────
mkdir -p "$SITE09/certs"
if [ ! -f "$SITE09/certs/local.pem" ]; then
    if command -v mkcert &>/dev/null; then
        mkcert -install 2>/dev/null || true
        mkcert -key-file "$SITE09/certs/local-key.pem" \
               -cert-file "$SITE09/certs/local.pem" \
               localhost 127.0.0.1
    elif command -v openssl &>/dev/null; then
        # No mkcert: fall back to a self-signed certificate.
        #
        # The only thing mkcert adds is a local CA your browser already trusts,
        # which matters to a person clicking around and not to `curl -k` or to a
        # test suite. This used to be a hard `exit 1`, and m6's build host has no
        # mkcert -- the CI stage that runs this example only worked because rsync
        # happened to carry a developer's own untracked keys/ directory. A check
        # that depends on an untracked file is not a check.
        echo "mkcert not found; generating a self-signed certificate with openssl."
        echo "Your browser will warn about it. For one it trusts: brew install mkcert"
        openssl req -x509 -newkey rsa:2048 -sha256 -days 365 -nodes \
            -keyout "$SITE09/certs/local-key.pem" -out "$SITE09/certs/local.pem" \
            -subj "/CN=localhost" -addext "subjectAltName=DNS:localhost,IP:127.0.0.1" 2>/dev/null \
            || { echo "ERROR: openssl could not write a certificate." >&2; exit 1; }
    else
        echo "ERROR: neither mkcert nor openssl found. Install one:" >&2
        echo "  mkcert   - certificate your browser trusts (brew install mkcert)" >&2
        echo "  openssl  - self-signed, browser warns" >&2
        exit 1
    fi
fi

# ── Expose 07's auth public key for site-origin/site.toml ────────────────────
# site-origin/site.toml uses public_key = "../keys/auth.pub"
# which resolves to $SITE09/keys/auth.pub
mkdir -p "$SITE09/keys"
if [ ! -f "$SITE09/keys/auth.pub" ]; then
    cp "$SITE07/keys/auth.pub" "$SITE09/keys/auth.pub"
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

# ── Backend services (07's content and config) ────────────────────────────────
M6_SOCKET_OVERRIDE=/tmp/m6/m6-html.sock \
    m6-html "$SITE07" "$SITE07/configs/m6-html.conf" &
HTML_PID=$!

M6_SOCKET_OVERRIDE=/tmp/m6/m6-file.sock \
    m6-file "$SITE07" "$SITE07/configs/m6-file.conf" &
FILE_PID=$!

M6_SOCKET_OVERRIDE=/tmp/m6/m6-auth.sock \
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

# ── Origin — TLS (port 9001) + H2C backbone (port 9000) ──────────────────────
# Single m6-http instance: bind = TLS for users, h2c_bind = cleartext for cache nodes
m6-http "$SITE09/site-origin" "$SITE09/configs/local/sydney.toml" &
ORIGIN_PID=$!
ALL_PIDS="$ALL_PIDS $ORIGIN_PID"

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
