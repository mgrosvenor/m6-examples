#!/usr/bin/env bash
# setup-cache-node.sh — first-time setup for a cache node (SF/NYC/Chicago/London/Singapore).
# Usage: ./setup-cache-node.sh <city> <domain> <wg-config>
# Example: ./setup-cache-node.sh sf example.com wireguard/sf-wg0.conf
set -euo pipefail

CITY="${1:?Usage: $0 <city> <domain> <wg-conf>}"
DOMAIN="${2:?Usage: $0 <city> <domain> <wg-conf>}"
WG_CONF="${3:?Usage: $0 <city> <domain> <wg-conf>}"
SITE_DIR="/var/www/my-blog"

echo "=== Installing dependencies ==="
apt-get update -q
apt-get install -y wireguard certbot ufw

echo "=== Creating m6 user ==="
id m6 &>/dev/null || useradd --system --no-create-home --shell /usr/sbin/nologin m6

echo "=== Creating site directory ==="
mkdir -p "$SITE_DIR"
chown m6:m6 "$SITE_DIR"

echo "=== Setting up /etc/m6 ==="
mkdir -p /etc/m6

# System config
CONF_SRC="configs/cache-${CITY}.toml"
[ -f "$CONF_SRC" ] || CONF_SRC="configs/cache-sf.toml"   # fallback to template
install -m 640 -o root -g m6 "$CONF_SRC" "/etc/m6/cache.toml"
sed -i "s/example.com/$DOMAIN/g" /etc/m6/cache.toml

# Node config pointer for systemd unit
echo "NODE_CONFIG=/etc/m6/cache.toml" > /etc/m6/node-config

echo "=== WireGuard ==="
install -m 600 "$WG_CONF" /etc/wireguard/wg0.conf
systemctl enable --now wg-quick@wg0

echo "=== Firewall ==="
ufw allow 22/tcp
ufw allow 80/tcp
ufw allow 443/tcp
ufw allow 443/udp
ufw --force enable

echo "=== TLS certificate ==="
# Cert is for the shared domain — SAN or wildcard needed for multi-node.
# Using standalone temporarily; switch to DNS challenge for wildcard.
certbot certonly --standalone --agree-tos --non-interactive \
    --email "admin@${DOMAIN}" -d "$DOMAIN" || \
    echo "NOTE: certbot failed — run manually after DNS is pointed at this node."

echo "=== Installing systemd unit ==="
install -m 644 systemd/m6-http-cache.service /etc/systemd/system/
systemctl daemon-reload

echo "=== Deploying site content from origin ==="
echo "Run on your dev machine:"
echo "  rsync -av --delete --exclude 'content/drafts/' --exclude 'data/auth.db' \\"
echo "    --exclude 'keys/' /var/www/my-blog/ root@<THIS_NODE_IP>:/var/www/my-blog/"

echo ""
echo "Done — $CITY cache node configured."
echo "  Start: systemctl enable --now m6-http-cache"
echo "  Verify tunnel: ping 10.0.0.1"
echo "  Verify cache: curl -sk https://$DOMAIN/ | head -5"
