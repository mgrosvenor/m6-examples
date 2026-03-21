#!/usr/bin/env bash
# invalidate-cache.sh — force all cache nodes to drop their in-memory cache.
#
# m6-http flushes its cache and reloads routing whenever site.toml is touched.
# This script touches site.toml on every cache node simultaneously over SSH.
#
# Usage:
#   ./invalidate-cache.sh                     # invalidate all cache nodes
#   ./invalidate-cache.sh sf nyc              # invalidate specific nodes
#   ./invalidate-cache.sh --origin            # also invalidate origin
#
# Prerequisites:
#   SSH access to each node as the deploy user (key-based auth recommended).
#   The NODES array below should match your actual hostnames / IPs.

set -euo pipefail

# ── Node config ────────────────────────────────────────────────────────────────
# Map node name → SSH target (user@host or just host if ~/.ssh/config sets user)
declare -A NODES
NODES[sf]="syd.example.com"          # replace with actual SSH targets
NODES[nyc]="nyc.example.com"
NODES[chicago]="chi.example.com"
NODES[london]="lon.example.com"
NODES[singapore]="sin.example.com"
NODES[origin]="syd.example.com"

SITE_DIR="/var/www/my-blog"
SSH_OPTS="-o StrictHostKeyChecking=accept-new -o ConnectTimeout=10"

# ── Parse args ────────────────────────────────────────────────────────────────
TARGETS=()
INCLUDE_ORIGIN=false

if [[ $# -eq 0 ]]; then
    # Default: all cache nodes (not origin)
    TARGETS=(sf nyc chicago london singapore)
else
    for arg in "$@"; do
        case "$arg" in
            --origin) INCLUDE_ORIGIN=true ;;
            *)        TARGETS+=("$arg") ;;
        esac
    done
    [[ ${#TARGETS[@]} -eq 0 ]] && TARGETS=(sf nyc chicago london singapore)
fi

$INCLUDE_ORIGIN && TARGETS+=(origin)

# ── Invalidate ────────────────────────────────────────────────────────────────
echo "Invalidating cache on: ${TARGETS[*]}"

PIDS=()
for node in "${TARGETS[@]}"; do
    host="${NODES[$node]:?Unknown node: $node}"
    echo "  → $node ($host)"
    # shellcheck disable=SC2086
    ssh $SSH_OPTS "$host" "touch ${SITE_DIR}/site.toml" &
    PIDS+=($!)
done

# Wait for all SSH commands to complete
FAILED=0
for pid in "${PIDS[@]}"; do
    wait "$pid" || { echo "WARNING: one node failed"; FAILED=1; }
done

if [[ $FAILED -eq 0 ]]; then
    echo "Cache invalidated on all ${#TARGETS[@]} node(s)."
else
    echo "Cache invalidation completed with errors." >&2
    exit 1
fi
