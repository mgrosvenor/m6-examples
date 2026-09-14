#!/bin/bash
set -e
SITE="$(cd "$(dirname "$0")" && pwd)"

# Generate signing keys
mkdir -p "$SITE/keys"
# PKCS#8 (`genpkey`), NOT SEC1 (`ecparam -genkey`). m6-auth-server refuses a
# SEC1 key with "invalid private key (tried EC and RSA): InvalidKeyFormat" and
# exits, so this script used to produce a stack whose auth server could not
# start. Each dev.sh already used genpkey, so the two disagreed about the same
# file and only the setup.sh path was broken.
openssl genpkey -algorithm EC -pkeyopt ec_paramgen_curve:P-256 -out "$SITE/keys/auth.pem"
openssl pkey -in "$SITE/keys/auth.pem" -pubout -out "$SITE/keys/auth.pub"
chmod 600 "$SITE/keys/auth.pem"
echo "Keys generated."

# Create first admin user — database created automatically if absent
m6-auth-cli "$SITE/configs/m6-auth.conf" user add admin --role admin --password admin

echo "Setup complete. Start the server with ./dev.sh"
