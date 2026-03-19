#!/bin/bash
set -e
SITE="$(cd "$(dirname "$0")" && pwd)"
M6="${M6:-$(cd "$SITE/../../../m6" && pwd)}"
EXAMPLES="${EXAMPLES:-$(cd "$SITE/../.." && pwd)}"
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

trap 'kill $(jobs -p) 2>/dev/null; wait 2>/dev/null' EXIT

mkdir -p /tmp/m6
lsof -ti :8443 2>/dev/null | xargs kill -9 2>/dev/null || true
sleep 0.2

M6_SOCKET_OVERRIDE=/tmp/m6/m6-html.sock    m6-html        "$SITE" "$SITE/configs/m6-html.conf"                          &
M6_SOCKET_OVERRIDE=/tmp/m6/m6-file.sock    m6-file        "$SITE" "$SITE/configs/m6-file.conf"                          &
M6_SOCKET_OVERRIDE=/tmp/m6/m6-auth.sock    m6-auth-server "$SITE" "$SITE/configs/m6-auth.conf"                          &
M6_SOCKET_OVERRIDE=/tmp/m6/render-cms.sock "$EXAMPLES/target/release/render-cms" "$SITE" "$SITE/configs/render-cms.conf"    &
sleep 0.5
m6-http "$SITE" "$SITE/configs/system-dev.toml" &

echo ""
echo "CMS:   https://localhost:8443/cms  (login: admin / admin)"
echo "Blog:  https://localhost:8443/blog"
echo ""
wait
