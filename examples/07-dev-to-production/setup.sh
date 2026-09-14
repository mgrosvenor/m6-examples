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
# `ec_param_enc:named_curve` is not redundant: macOS's LibreSSL writes explicit
# curve parameters where OpenSSL writes the prime256v1 OID, and the token signer
# only accepts the named form. Without it every login on a Mac fails with
# "JWT encode error: InvalidEcdsaKey" even with the right password. The long
# version is in examples/05-cms/setup.sh.
openssl genpkey -algorithm EC -pkeyopt ec_paramgen_curve:P-256 -pkeyopt ec_param_enc:named_curve -out "$SITE/keys/auth.pem"
openssl pkey -in "$SITE/keys/auth.pem" -pubout -out "$SITE/keys/auth.pub"
chmod 600 "$SITE/keys/auth.pem"
echo "Keys generated."

# Create first admin/editor user
m6-auth-cli "$SITE/configs/m6-auth.conf" user add admin --role admin --password admin
m6-auth-cli "$SITE/configs/m6-auth.conf" group add editors
m6-auth-cli "$SITE/configs/m6-auth.conf" group member add editors admin

echo "Setup complete. Build the renderer, then start with ./dev.sh"
echo "  cargo build --release -p render-cms"
echo "  ./dev.sh"
