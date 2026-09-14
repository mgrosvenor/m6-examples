#!/bin/bash
# dev.sh — start all services for example 11-admin-dashboard.
set -e

SITE="$(cd "$(dirname "$0")" && pwd)"
EXAMPLES="$(cd "$SITE/../.." && pwd)"
M6="${M6:-$(cd "$SITE/../../../m6" && pwd)}"
export PATH="$M6/target/release:$EXAMPLES/target/release:$PATH"

# ── TLS certs ─────────────────────────────────────────────────────────────────
mkdir -p "$SITE/keys"
if [ ! -f "$SITE/keys/dev.pem" ]; then
    if command -v mkcert &>/dev/null; then
        mkcert -install 2>/dev/null || true
        mkcert -key-file "$SITE/keys/dev-key.pem" \
               -cert-file "$SITE/keys/dev.pem" \
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
            -keyout "$SITE/keys/dev-key.pem" -out "$SITE/keys/dev.pem" \
            -subj "/CN=localhost" -addext "subjectAltName=DNS:localhost,IP:127.0.0.1" 2>/dev/null \
            || { echo "ERROR: openssl could not write a certificate." >&2; exit 1; }
    else
        echo "ERROR: neither mkcert nor openssl found. Install one:" >&2
        echo "  mkcert   - certificate your browser trusts (brew install mkcert)" >&2
        echo "  openssl  - self-signed, browser warns" >&2
        exit 1
    fi
fi

# ── Setup check ───────────────────────────────────────────────────────────────
if [ ! -f "$SITE/keys/auth.pem" ] || [ ! -f "$SITE/data/auth.db" ]; then
    echo "First-time setup required. Running setup.sh..."
    "$SITE/setup.sh"
fi

# ── Clean up stale state ──────────────────────────────────────────────────────
mkdir -p /tmp/m6
lsof -ti :8444 2>/dev/null | xargs kill -9 2>/dev/null || true
sleep 0.3
rm -f /tmp/m6/m6-auth-admin.sock \
      /tmp/m6/render-admin.sock \
      /tmp/m6/m6-file-admin.sock

echo "=== 11-admin-dashboard ==="

# ── m6-auth ───────────────────────────────────────────────────────────────────
M6_SOCKET_OVERRIDE=/tmp/m6/m6-auth-admin.sock \
    m6-auth-server "$SITE" "$SITE/configs/m6-auth.conf" &
AUTH_PID=$!

# ── render-admin ──────────────────────────────────────────────────────────────
M6_SOCKET_OVERRIDE=/tmp/m6/render-admin.sock \
    render-admin "$SITE" "$SITE/configs/render-admin.conf" &
ADMIN_PID=$!

# ── m6-file (static dashboard) ────────────────────────────────────────────────
M6_SOCKET_OVERRIDE=/tmp/m6/m6-file-admin.sock \
    m6-file "$SITE" "$SITE/configs/m6-file.conf" &
FILE_PID=$!

trap 'kill $AUTH_PID $ADMIN_PID $FILE_PID $HTTP_PID 2>/dev/null; wait 2>/dev/null' EXIT

# ── Wait for backend sockets ──────────────────────────────────────────────────
printf "waiting for backends"
for i in $(seq 1 50); do
    [ -S /tmp/m6/m6-auth-admin.sock ] && \
    [ -S /tmp/m6/render-admin.sock  ] && \
    [ -S /tmp/m6/m6-file-admin.sock ] && { echo " ready."; break; }
    printf "."; sleep 0.2
done

# ── m6-http ───────────────────────────────────────────────────────────────────
# Truncate the log file so render-admin always reads current-session data.
: > /tmp/m6/m6-http-admin.log
m6-http "$SITE" "$SITE/site.toml" >> /tmp/m6/m6-http-admin.log 2>&1 &
HTTP_PID=$!

echo ""
echo "Admin dashboard: https://localhost:8444/"
echo "Sign in:  admin / admin"
echo ""
echo "Press Ctrl+C to stop."
wait
