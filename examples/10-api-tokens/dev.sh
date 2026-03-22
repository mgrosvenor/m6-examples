#!/bin/bash
# dev.sh — start the 10-api-tokens stack for local development.
set -e

SITE="$(cd "$(dirname "$0")" && pwd)"
M6="${M6:-$(cd "$SITE/../../../m6" && pwd)}"
export PATH="$M6/target/release:$PATH"

# ── TLS certs ─────────────────────────────────────────────────────────────────
mkdir -p "$SITE/keys"
if [ ! -f "$SITE/keys/dev.pem" ]; then
    if ! command -v mkcert &>/dev/null; then
        echo "ERROR: mkcert not found. Install with: brew install mkcert" >&2
        exit 1
    fi
    mkcert -install 2>/dev/null || true
    mkcert -key-file "$SITE/keys/dev-key.pem" \
           -cert-file "$SITE/keys/dev.pem" \
           localhost 127.0.0.1
fi

# ── Auth keys ─────────────────────────────────────────────────────────────────
if [ ! -f "$SITE/keys/auth.pem" ]; then
    echo "Generating auth signing keys..."
    openssl genpkey -algorithm EC -pkeyopt ec_paramgen_curve:P-256 -out "$SITE/keys/auth.pem"
    openssl pkey -in "$SITE/keys/auth.pem" -pubout -out "$SITE/keys/auth.pub"
    chmod 600 "$SITE/keys/auth.pem"
fi

# ── Auth database ─────────────────────────────────────────────────────────────
mkdir -p "$SITE/configs/data"
if [ ! -f "$SITE/configs/data/auth.db" ]; then
    echo "Creating 'api' user (password: secret)..."
    m6-auth-cli "$SITE/configs/m6-auth.conf" user add api --role api --role user --password secret
fi

# ── Clean up stale state ──────────────────────────────────────────────────────
mkdir -p /tmp/m6
lsof -ti :8443 2>/dev/null | xargs kill -9 2>/dev/null || true
sleep 0.3
rm -f /tmp/m6/m6-html-api*.sock /tmp/m6/m6-file-api*.sock /tmp/m6/m6-auth-api*.sock

# ── Start services ────────────────────────────────────────────────────────────
M6_SOCKET_OVERRIDE=/tmp/m6/m6-html-api.sock \
    m6-html "$SITE" "$SITE/configs/m6-html.conf" &
HTML_PID=$!

M6_SOCKET_OVERRIDE=/tmp/m6/m6-file-api.sock \
    m6-file "$SITE" "$SITE/configs/m6-file.conf" &
FILE_PID=$!

M6_SOCKET_OVERRIDE=/tmp/m6/m6-auth-api.sock \
    m6-auth-server "$SITE" "$SITE/configs/m6-auth.conf" &
AUTH_PID=$!

trap 'kill $HTML_PID $FILE_PID $AUTH_PID $HTTP_PID 2>/dev/null; wait 2>/dev/null' EXIT

# Wait for backend sockets
for i in $(seq 1 30); do
    [ -S /tmp/m6/m6-html-api.sock ] && \
    [ -S /tmp/m6/m6-file-api.sock ] && \
    [ -S /tmp/m6/m6-auth-api.sock ] && break
    sleep 0.2
done

m6-http "$SITE" "$SITE/site.toml" &
HTTP_PID=$!

echo "Running at https://localhost:8443"
echo ""
echo "Login at  https://localhost:8443/login  (api / secret)"
echo ""
echo "Create an API token:"
echo "  m6-auth-cli configs/m6-auth.conf token create api --name demo"
echo ""
echo "Call the protected endpoint:"
echo "  curl -sk https://localhost:8443/api/data -H 'Authorization: Bearer <token>'"
wait
