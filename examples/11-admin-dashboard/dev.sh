#!/bin/bash
# dev.sh — start all services for example 11-admin-dashboard.
set -e

SITE="$(cd "$(dirname "$0")" && pwd)"
M6="$(cd "$SITE/../../../m6" && pwd)"
export PATH="$M6/target/release:$SITE/../../../m6-examples/target/release:$PATH"

PID_DIR=/tmp/m6
mkdir -p "$PID_DIR"

stop_all() {
    echo ""
    echo "Stopping services..."
    for pidfile in "$PID_DIR/m6-auth-admin.pid" \
                   "$PID_DIR/render-admin.pid" \
                   "$PID_DIR/m6-file-admin.pid" \
                   "$PID_DIR/m6-http-admin.pid"; do
        if [ -f "$pidfile" ]; then
            kill -TERM "$(cat "$pidfile")" 2>/dev/null || true
            rm -f "$pidfile"
        fi
    done
}
trap stop_all EXIT INT TERM

echo "=== 11-admin-dashboard dev server ==="

# ── m6-auth ───────────────────────────────────────────────────────────────────
echo "Starting m6-auth..."
m6-auth-server "$SITE/configs/m6-auth.conf" &
AUTH_PID=$!
echo $AUTH_PID > "$PID_DIR/m6-auth-admin.pid"

# ── render-admin ──────────────────────────────────────────────────────────────
echo "Starting render-admin..."
render-admin "$SITE" "$SITE/configs/render-admin.conf" &
ADMIN_PID=$!
echo $ADMIN_PID > "$PID_DIR/render-admin.pid"

# ── m6-file (static dashboard) ────────────────────────────────────────────────
echo "Starting m6-file..."
M6_SOCKET_OVERRIDE=/tmp/m6/m6-file-admin.sock \
    m6-file "$SITE" "$SITE/configs/m6-file.conf" &
FILE_PID=$!
echo $FILE_PID > "$PID_DIR/m6-file-admin.pid"

# ── m6-http ───────────────────────────────────────────────────────────────────
echo "Starting m6-http..."
m6-http "$SITE/site.toml" &
HTTP_PID=$!
echo $HTTP_PID > "$PID_DIR/m6-http-admin.pid"

echo ""
echo "Admin dashboard: https://localhost:8444/"
echo "Admin API:       https://localhost:8444/api/admin/*"
echo ""
echo "  GET  /api/admin/perf"
echo "  GET  /api/admin/routes"
echo "  GET  /api/admin/system"
echo "  GET  /api/admin/bench"
echo "  POST /api/admin/bench"
echo "  GET  /api/admin/bench/{id}"
echo "  GET  /api/admin/logs"
echo "  GET  /api/admin/config"
echo "  PUT  /api/admin/config"
echo "  POST /api/admin/config/touch"
echo "  POST /api/admin/cache/flush"
echo "  POST /api/admin/services/{name}/restart"
echo ""
echo "Press Ctrl+C to stop."
wait
