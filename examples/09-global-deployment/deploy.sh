#!/usr/bin/env bash
# deploy.sh — deploy to origin and propagate to all cache nodes.
#
# Usage:
#   ./deploy.sh                        # deploy to origin only
#   ./deploy.sh --all                  # deploy to origin + push to all cache nodes
#   ./deploy.sh --binary               # deploy + rebuild and push render-cms binary
#   ./deploy.sh --invalidate           # deploy + invalidate caches after sync
#   ./deploy.sh --all --binary --invalidate
#
# Configure ORIGIN and CACHE_NODES below to match your actual hostnames.
#
# Git workflow:
#   1. Edit content locally: content/posts/, templates/, assets/
#   2. git commit && git push
#   3. ./deploy.sh --all --invalidate   (or run from CI on push to main)
set -euo pipefail

# ── Configuration ─────────────────────────────────────────────────────────────
ORIGIN="root@syd.example.com"   # SSH target for Sydney origin
REMOTE_SITE="/var/www/my-blog"
SSH_OPTS="-o StrictHostKeyChecking=accept-new -o ConnectTimeout=15"

# Cache nodes: name → SSH target
declare -A CACHE_NODES
CACHE_NODES[sf]="root@sf.example.com"
CACHE_NODES[nyc]="root@nyc.example.com"
CACHE_NODES[chicago]="root@chi.example.com"
CACHE_NODES[london]="root@lon.example.com"
CACHE_NODES[singapore]="root@sin.example.com"

# ── Flags ─────────────────────────────────────────────────────────────────────
DEPLOY_ALL=false
PUSH_BINARY=false
INVALIDATE=false

for arg in "$@"; do
    case "$arg" in
        --all)        DEPLOY_ALL=true ;;
        --binary)     PUSH_BINARY=true ;;
        --invalidate) INVALIDATE=true ;;
        *) echo "Unknown argument: $arg" >&2; exit 1 ;;
    esac
done

# ── Helpers ───────────────────────────────────────────────────────────────────

rsync_to() {
    local target="$1" label="$2"
    echo "==> Syncing to $label ($target)..."
    # shellcheck disable=SC2086
    rsync -az --delete \
        $SSH_OPTS \
        --exclude 'content/drafts/' \
        --exclude 'data/auth.db' \
        --exclude 'data/posts.json' \
        --exclude 'keys/' \
        --exclude '*.pem' \
        --exclude '*.pub' \
        --exclude 'target/' \
        --exclude '.git/' \
        --exclude 'logs/' \
        ./ "$target:$REMOTE_SITE/"
}

touch_site_toml() {
    local target="$1" label="$2"
    echo "  → invalidating cache on $label..."
    # shellcheck disable=SC2086
    ssh $SSH_OPTS "$target" "touch $REMOTE_SITE/site.toml"
}

# ── Deploy to origin ──────────────────────────────────────────────────────────
rsync_to "$ORIGIN" "origin (Sydney)"

# Set ownership
# shellcheck disable=SC2086
ssh $SSH_OPTS "$ORIGIN" "chown -R m6:m6 $REMOTE_SITE"

# Rebuild posts.json on origin from the synced markdown
# shellcheck disable=SC2086
ssh $SSH_OPTS "$ORIGIN" "m6-md $REMOTE_SITE/content/posts/ --output $REMOTE_SITE/data/posts.json"

# Optionally push binary
if $PUSH_BINARY; then
    echo "==> Building render-cms binary..."
    cargo build --release -p render-cms
    echo "==> Pushing render-cms binary to origin..."
    # shellcheck disable=SC2086
    rsync -az $SSH_OPTS target/release/render-cms "$ORIGIN:$REMOTE_SITE/bin/"
    # shellcheck disable=SC2086
    ssh $SSH_OPTS "$ORIGIN" "systemctl restart render-cms"
fi

echo "==> Origin sync complete. m6-http picks up site.toml changes via inotify."

# ── Propagate to cache nodes ──────────────────────────────────────────────────
if $DEPLOY_ALL; then
    echo ""
    echo "==> Propagating to cache nodes..."

    PIDS=()
    for node in "${!CACHE_NODES[@]}"; do
        target="${CACHE_NODES[$node]}"
        # Cache nodes need templates + assets for error pages, but NOT content/
        # (content is always fetched live from origin).
        (
            echo "  → $node ($target): syncing..."
            # shellcheck disable=SC2086
            rsync -az --delete \
                $SSH_OPTS \
                --exclude 'content/' \
                --exclude 'data/' \
                --exclude 'keys/' \
                --exclude '*.pem' \
                --exclude '*.pub' \
                --exclude 'target/' \
                --exclude '.git/' \
                --exclude 'logs/' \
                ./ "$target:$REMOTE_SITE/"
            # shellcheck disable=SC2086
            ssh $SSH_OPTS "$target" "chown -R m6:m6 $REMOTE_SITE"
            echo "  ✓ $node done"
        ) &
        PIDS+=($!)
    done

    FAILED=0
    for pid in "${PIDS[@]}"; do
        wait "$pid" || { echo "WARNING: a cache node sync failed"; FAILED=1; }
    done

    [[ $FAILED -eq 0 ]] && echo "==> All cache nodes synced." \
                        || { echo "==> Some cache nodes failed." >&2; exit 1; }
fi

# ── Invalidate caches ─────────────────────────────────────────────────────────
if $INVALIDATE; then
    echo ""
    echo "==> Invalidating caches..."

    PIDS=()
    # Always invalidate origin too
    # shellcheck disable=SC2086
    ssh $SSH_OPTS "$ORIGIN" "touch $REMOTE_SITE/site.toml" &
    PIDS+=($!)

    if $DEPLOY_ALL; then
        for node in "${!CACHE_NODES[@]}"; do
            target="${CACHE_NODES[$node]}"
            # shellcheck disable=SC2086
            ssh $SSH_OPTS "$target" "touch $REMOTE_SITE/site.toml" &
            PIDS+=($!)
        done
    fi

    for pid in "${PIDS[@]}"; do wait "$pid" || true; done
    echo "==> Caches invalidated."
fi

echo ""
echo "Deploy complete."
