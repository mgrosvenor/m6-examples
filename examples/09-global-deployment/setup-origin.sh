#!/usr/bin/env bash
# setup-origin.sh — first-time setup for the Sydney origin node.
# Run as root on a fresh Ubuntu 22.04 / Debian 12 VPS.
set -euo pipefail

DOMAIN="${1:?Usage: $0 <domain> <admin-email>}"
EMAIL="${2:?Usage: $0 <domain> <admin-email>}"
SITE_DIR="/var/www/my-blog"
M6_BIN="/usr/local/bin"

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

# Auth signing keys
if [ ! -f /etc/m6/auth.pem ]; then
    openssl genpkey -algorithm EC -pkeyopt ec_paramgen_curve:P-256 -out /etc/m6/auth.pem
    openssl pkey -in /etc/m6/auth.pem -pubout -out /etc/m6/auth.pub
    chmod 600 /etc/m6/auth.pem
    chown m6:m6 /etc/m6/auth.pem /etc/m6/auth.pub
fi

# System configs
install -m 640 -o root -g m6 configs/sydney-public.toml /etc/m6/sydney-public.toml
install -m 640 -o root -g m6 configs/sydney-h2c.toml   /etc/m6/sydney-h2c.toml
# Patch domain into configs
sed -i "s/example.com/$DOMAIN/g" /etc/m6/sydney-public.toml

echo "=== WireGuard ==="
install -m 600 wireguard/sydney-wg0.conf /etc/wireguard/wg0.conf
systemctl enable --now wg-quick@wg0

echo "=== Firewall ==="
ufw allow 22/tcp
ufw allow 80/tcp
ufw allow 443/tcp
ufw allow 443/udp    # H3/QUIC
ufw allow 51820/udp  # WireGuard
ufw --force enable

echo "=== TLS certificate ==="
certbot certonly --standalone --agree-tos --email "$EMAIL" -d "$DOMAIN"
# Auto-renewal hook — touch site.toml so m6-http reloads the new cert
mkdir -p /etc/letsencrypt/renewal-hooks/deploy
cat > /etc/letsencrypt/renewal-hooks/deploy/m6-http-reload.sh << 'HOOK'
#!/bin/bash
touch /var/www/my-blog/site.toml
HOOK
chmod +x /etc/letsencrypt/renewal-hooks/deploy/m6-http-reload.sh

echo "=== Installing systemd units ==="
install -m 644 systemd/m6-http-origin-public.service /etc/systemd/system/
install -m 644 systemd/m6-http-origin-h2c.service    /etc/systemd/system/
# (Install m6-html, m6-file, m6-auth, render-cms units from example 06/07)
systemctl daemon-reload

echo ""
echo "Done. Next steps:"
echo "  1. Copy WireGuard keys into /etc/wireguard/wg0.conf (replace placeholders)"
echo "  2. Deploy site content: ./deploy.sh user@$DOMAIN"
echo "  3. Start services:"
echo "       systemctl enable --now m6-html m6-file m6-auth render-cms"
echo "       systemctl enable --now m6-http-origin-public m6-http-origin-h2c"
