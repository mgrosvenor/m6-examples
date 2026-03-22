#!/bin/bash
# setup.sh — first-time setup for example 10-api-tokens.
set -e
SITE="$(cd "$(dirname "$0")" && pwd)"

# Generate signing keys
mkdir -p "$SITE/keys"
openssl genpkey -algorithm EC -pkeyopt ec_paramgen_curve:P-256 -out "$SITE/keys/auth.pem"
openssl pkey -in "$SITE/keys/auth.pem" -pubout -out "$SITE/keys/auth.pub"
chmod 600 "$SITE/keys/auth.pem"
echo "Signing keys generated."

# Create the 'api' user with the 'api' and 'user' roles
m6-auth-cli "$SITE/configs/m6-auth.conf" user add api --role api --role user --password secret
echo "User 'api' created (password: secret)."

echo ""
echo "Setup complete. Run ./dev.sh to start the server."
echo ""
echo "Then create an API token:"
echo "  m6-auth-cli configs/m6-auth.conf token create api --name demo"
