#!/bin/bash
# dev.sh — start the 08-logviewer stack for local browser development.
#
# Builds binaries, generates TLS certs if needed, starts all three services,
# waits for readiness, then prints the URLs to open.
set -e

SITE="$(cd "$(dirname "$0")" && pwd)"
M6="${M6:-$(cd "$SITE/../../.." && pwd)/m6}"

GREEN='\033[0;32m'; YELLOW='\033[1;33m'; RESET='\033[0m'
info() { echo -e "${YELLOW}----${RESET} $1"; }

# ── Build ──────────────────────────────────────────────────────────────────────
info "Building m6 binaries..."
(cd "$M6" && cargo build --release --quiet)
export PATH="$M6/target/release:$PATH"

# ── TLS certs ─────────────────────────────────────────────────────────────────
mkdir -p "$SITE/keys"
if [[ ! -f "$SITE/keys/dev.pem" ]]; then
    info "Generating dev TLS certs..."
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

# ── Clean up stale state ──────────────────────────────────────────────────────
mkdir -p "$SITE/logs"
mkdir -p /tmp/m6

# Kill any stale m6 processes holding port 8443 or our sockets.
# lsof -ti gives the PID holding the port silently.
lsof -ti :8443 2>/dev/null | xargs kill -9 2>/dev/null || true
# Kill only THIS example's own processes.
#
# This used to be `pkill -x m6-http; pkill -x m6-html; ...`, which matches by
# process name across the whole machine. Anyone with another m6 site running --
# their own site, a preview instance, a multi-node local fleet -- lost all of it
# the moment they started this example, with no warning and nothing in the log
# saying what had happened. It killed a running seven-node local fleet exactly
# that way.
#
# `pgrep -f "$SITE"` matches the site directory in the command line instead, so
# it can only ever reach processes started for this example. The port holder on
# the bind address is killed separately above, because a stale process from an
# earlier run of THIS example is the case the cleanup is actually for.
kill_own() {
    local pid
    for pid in $(pgrep -f "$SITE" 2>/dev/null || true); do
        [ "$pid" = "$$" ] && continue
        kill "$pid" 2>/dev/null || true
    done
}
kill_own
sleep 0.3   # give the OS time to release the port and sockets

rm -f /tmp/m6/m6-html.sock /tmp/m6/m6-file.sock

# Truncate log files so each run starts fresh (avoids serving stale megabytes
# on the first tail poll from a previous session's accumulated log).
> "$SITE/logs/m6-html.log"
> "$SITE/logs/m6-file.log"
> "$SITE/logs/m6-http.log"

# ── Start services ────────────────────────────────────────────────────────────
info "Starting m6-html..."
M6_SOCKET_OVERRIDE=/tmp/m6/m6-html.sock \
    m6-html "$SITE" "$SITE/configs/m6-html.conf" >> "$SITE/logs/m6-html.log" 2>&1 &
HTML_PID=$!

info "Starting m6-file..."
M6_SOCKET_OVERRIDE=/tmp/m6/m6-file.sock \
    m6-file "$SITE" "$SITE/configs/m6-file.conf" >> "$SITE/logs/m6-file.log" 2>&1 &
FILE_PID=$!

trap 'echo ""; info "Stopping (PIDs: $HTML_PID $FILE_PID $HTTP_PID)..."; kill $HTML_PID $FILE_PID $HTTP_PID 2>/dev/null; wait 2>/dev/null' EXIT

# Wait for backend sockets
echo -n "Waiting for backend sockets"
for i in $(seq 1 20); do
    if [[ -S /tmp/m6/m6-html.sock && -S /tmp/m6/m6-file.sock ]]; then
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
    code=$($CURL -sk --http1.1 -o /dev/null -w "%{http_code}" https://localhost:8443/ 2>/dev/null || true)
    if [[ "$code" == "200" ]]; then
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

# ── Open browser ───────────────────────────────────────────────────────────────
[[ "${M6_NO_BROWSER:-0}" = "0" ]] && open "https://localhost:8443/logs" 2>/dev/null || true

# ── Print URLs ─────────────────────────────────────────────────────────────────
echo ""
echo -e "${GREEN}Stack is running.${RESET}  Press Ctrl-C to stop."
echo ""
echo "  Home page:   https://localhost:8443/"
echo "  Log viewer:  https://localhost:8443/logs"
echo ""
echo "  Logs:  $SITE/logs/m6-http.log"
echo "         $SITE/logs/m6-html.log"
echo "         $SITE/logs/m6-file.log"
echo ""
echo "  PIDs:  m6-html=$HTML_PID  m6-file=$FILE_PID  m6-http=$HTTP_PID"
echo ""

wait
