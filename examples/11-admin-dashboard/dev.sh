#!/bin/bash
# dev.sh — start all services for example 11-admin-dashboard.
set -e

SITE="$(cd "$(dirname "$0")" && pwd)"
M6="${M6:-$(cd "$SITE/../../../m6" && pwd)}"
export PATH="$M6/target/release:$SITE/../../../target/release:$PATH"

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
m6-http "$SITE" "$SITE/site.toml" &
HTTP_PID=$!

echo ""
echo "Admin dashboard: https://localhost:8444/"
echo "Sign in:  admin / changeme"
echo ""
echo "Press Ctrl+C to stop."
wait
