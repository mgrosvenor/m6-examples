#!/bin/bash
# dev-token.sh <m6-auth.conf> <user> <role> <token-file>
#
# Writes a token for <user>, who holds <role>, to <token-file>. m6-http's
# `/genid` endpoint is gated by roles (`[genid] write_require` and
# `read_require` in the system config), so the token is what lets a renderer
# publish (`genid-write`) or a cache node follow the site's generation
# (`genid-read`). Needs the development keys (dev-keys.sh) and an auth
# database. Writes only what is missing.
set -euo pipefail

CONF="$1"
USER_NAME="$2"
ROLE="$3"
OUT="$4"

if [ ! -s "$OUT" ]; then
    mkdir -p "$(dirname "$OUT")"
    # A second run finds the user already there, which is fine.
    m6-auth-cli "$CONF" user add "$USER_NAME" --role "$ROLE" \
        --password "$(openssl rand -hex 16)" >/dev/null 2>&1 || true
    umask 077
    m6-auth-cli "$CONF" token create "$USER_NAME" --name "$USER_NAME" > "$OUT"
fi
