#!/bin/bash
set -e
SITE="$(cd "$(dirname "$0")" && pwd)"

# Generate signing keys
mkdir -p "$SITE/keys"
openssl ecparam -name prime256v1 -genkey -noout -out "$SITE/keys/auth.pem"
openssl ec -in "$SITE/keys/auth.pem" -pubout -out "$SITE/keys/auth.pub"
chmod 600 "$SITE/keys/auth.pem"
echo "Keys generated."

# Create first admin/editor user
m6-auth-cli "$SITE/configs/m6-auth.conf" user add admin --role admin
m6-auth-cli "$SITE/configs/m6-auth.conf" group add editors
m6-auth-cli "$SITE/configs/m6-auth.conf" group member add editors admin

echo "Setup complete. Build the renderer, then start with ./dev.sh"
echo "  cargo build --release -p render-cms"
echo "  ./dev.sh"
