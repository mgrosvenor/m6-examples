#!/usr/bin/env bash
# deploy.sh — push site content to the Sydney origin.
# Cache nodes get their content via rsync from origin or this same script.
# Run from your development machine.
set -euo pipefail

ORIGIN="${1:?Usage: $0 user@sydney-ip [--binary]}"
REMOTE_SITE="/var/www/my-blog"

echo "==> Syncing site to origin ($ORIGIN)..."
rsync -av --delete \
  --exclude 'content/drafts/' \
  --exclude 'data/auth.db' \
  --exclude 'keys/' \
  --exclude '*.pem' \
  --exclude '*.pub' \
  --exclude 'target/' \
  --exclude 'logs/' \
  ./ "$ORIGIN:$REMOTE_SITE/"

ssh "$ORIGIN" chown -R m6:m6 "$REMOTE_SITE"

if [[ "${2:-}" == "--binary" ]]; then
  echo "==> Building and pushing render-cms binary..."
  cargo build --release -p render-cms
  rsync target/release/render-cms "$ORIGIN:$REMOTE_SITE/bin/"
  ssh "$ORIGIN" systemctl restart render-cms
fi

echo "==> Done. m6-http picks up site.toml changes automatically via inotify."
echo ""
echo "To push content to cache nodes (they serve static assets from their own disk):"
for NODE in sf nyc chicago london singapore; do
  echo "  rsync -av --delete --exclude 'content/drafts/' --exclude 'data/auth.db' \\"
  echo "    --exclude 'keys/' $REMOTE_SITE/ root@<${NODE}-ip>:$REMOTE_SITE/"
done
