#!/bin/bash
set -e
SITE="$(cd "$(dirname "$0")" && pwd)"
M6="${M6:-$(cd "$SITE/../../../m6" && pwd)}"
EXAMPLES="${EXAMPLES:-$(cd "$SITE/../.." && pwd)}"
export PATH="$M6/target/release:$PATH"

GREEN='\033[0;32m'; YELLOW='\033[1;33m'; RESET='\033[0m'
info() { echo -e "${YELLOW}----${RESET} $1"; }

# ── Build ──────────────────────────────────────────────────────────────────────
info "Building m6 binaries..."
(cd "$M6" && cargo build --release --quiet)

info "Building render-cms..."
(cd "$EXAMPLES" && cargo build --release -p render-cms --quiet)

# ── TLS certs ─────────────────────────────────────────────────────────────────
mkdir -p "$SITE/keys"
if [ ! -f "$SITE/keys/dev.pem" ]; then
    info "Generating dev TLS certs..."
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
    info "Generating auth signing keys..."
    openssl genpkey -algorithm EC -pkeyopt ec_paramgen_curve:P-256 -out "$SITE/keys/auth.pem"
    openssl pkey -in "$SITE/keys/auth.pem" -pubout -out "$SITE/keys/auth.pub"
    chmod 600 "$SITE/keys/auth.pem"
fi

# ── Auth database ─────────────────────────────────────────────────────────────
mkdir -p "$SITE/data"
if [ ! -f "$SITE/data/auth.db" ]; then
    info "Creating admin user (password: admin)..."
    m6-auth-cli "$SITE/configs/m6-auth.conf" user add admin --password admin
    m6-auth-cli "$SITE/configs/m6-auth.conf" group add editors
    m6-auth-cli "$SITE/configs/m6-auth.conf" group member add editors admin
fi

# ── Generate posts.json from markdown ─────────────────────────────────────────
if command -v m6-md &>/dev/null; then
    info "Generating posts.json..."
    m6-md "$SITE/content/posts/" --output "$SITE/data/posts.json"
fi

# ── Clean up stale state ──────────────────────────────────────────────────────
mkdir -p "$SITE/logs"
mkdir -p /tmp/m6

lsof -ti :8443 2>/dev/null | xargs kill -9 2>/dev/null || true
pkill -x m6-http 2>/dev/null || true
pkill -x m6-html 2>/dev/null || true
pkill -x m6-file 2>/dev/null || true
pkill -x m6-auth-server 2>/dev/null || true
pkill -x render-cms 2>/dev/null || true
sleep 0.3

rm -f /tmp/m6/m6-html*.sock /tmp/m6/m6-file*.sock /tmp/m6/m6-auth*.sock /tmp/m6/render-cms*.sock

# ── Start services ────────────────────────────────────────────────────────────
info "Starting m6-html..."
M6_SOCKET_OVERRIDE=/tmp/m6/m6-html.sock \
    m6-html "$SITE" "$SITE/configs/m6-html.conf" >> "$SITE/logs/m6-html.log" 2>&1 &
HTML_PID=$!

info "Starting m6-file..."
M6_SOCKET_OVERRIDE=/tmp/m6/m6-file.sock \
    m6-file "$SITE" "$SITE/configs/m6-file.conf" >> "$SITE/logs/m6-file.log" 2>&1 &
FILE_PID=$!

info "Starting m6-auth-server..."
M6_SOCKET_OVERRIDE=/tmp/m6/m6-auth.sock \
    m6-auth-server "$SITE" "$SITE/configs/m6-auth.conf" >> "$SITE/logs/m6-auth.log" 2>&1 &
AUTH_PID=$!

info "Starting render-cms..."
M6_SOCKET_OVERRIDE=/tmp/m6/render-cms.sock \
    "$EXAMPLES/target/release/render-cms" "$SITE" "$SITE/configs/render-cms.conf" >> "$SITE/logs/render-cms.log" 2>&1 &
CMS_PID=$!

if command -v m6-md &>/dev/null; then
    info "Starting m6-md (watch mode)..."
    m6-md "$SITE/content/posts/" --output "$SITE/data/posts.json" \
        --watch --touch "$SITE/site.toml" >> "$SITE/logs/m6-md.log" 2>&1 &
    MD_PID=$!
else
    MD_PID=""
fi

trap 'echo ""; info "Stopping..."; kill $HTML_PID $FILE_PID $AUTH_PID $CMS_PID ${MD_PID:-} $HTTP_PID 2>/dev/null; wait 2>/dev/null' EXIT

# Wait for backend sockets
echo -n "Waiting for backend sockets"
for i in $(seq 1 30); do
    if [[ -S /tmp/m6/m6-html.sock && -S /tmp/m6/m6-file.sock && -S /tmp/m6/m6-auth.sock && -S /tmp/m6/render-cms.sock ]]; then
        echo " ready."
        break
    fi
    echo -n "."
    sleep 0.25
done

info "Starting m6-http..."
m6-http "$SITE" "$SITE/configs/system-dev.toml" >> "$SITE/logs/m6-http.log" 2>&1 &
HTTP_PID=$!

# Wait for server readiness
CURL="$(command -v /opt/homebrew/opt/curl/bin/curl 2>/dev/null || echo curl)"
echo -n "Waiting for server"
READY=false
for i in $(seq 1 40); do
    code=$($CURL -sk --http1.1 -o /dev/null -w "%{http_code}" https://127.0.0.1:8443/ 2>/dev/null || true)
    if [[ "$code" == "200" || "$code" == "302" ]]; then
        echo " ready."
        READY=true
        break
    fi
    echo -n "."
    sleep 0.5
done

if [[ "$READY" != "true" ]]; then
    echo ""
    echo "ERROR: server did not become ready. Check logs in $SITE/logs/" >&2
    exit 1
fi

open "https://localhost:8443/" 2>/dev/null || true

echo ""
echo -e "${GREEN}Stack is running.${RESET}  Press Ctrl-C to stop."
echo ""
echo "  CMS:    https://localhost:8443/cms  (login: admin / admin)"
echo "  Blog:   https://localhost:8443/blog"
echo ""
echo "  Logs:   $SITE/logs/m6-http.log"
echo "          $SITE/logs/m6-html.log"
echo "          $SITE/logs/m6-file.log"
echo "          $SITE/logs/m6-auth.log"
echo "          $SITE/logs/render-cms.log"
[ -n "${MD_PID:-}" ] && echo "          $SITE/logs/m6-md.log"
echo ""

wait
