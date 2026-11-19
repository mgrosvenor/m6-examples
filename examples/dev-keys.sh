#!/bin/bash
# dev-keys.sh — the development certificate and auth key pair every example's
# configs name, kept in the private per-user directory dev-home.sh sets up, outside
# every site directory. m6 reads no secret from a site tree, and every key path
# in a config is absolute. Writes only what is missing.
set -euo pipefail

. "$(dirname "$0")/dev-home.sh"
DEV="$M6_DEV_HOME/dev"
m6_private_dir "$DEV"
umask 077

# Development TLS certificate for localhost.
if [ ! -f "$DEV/dev.pem" ] || [ ! -f "$DEV/dev-key.pem" ]; then
    if command -v mkcert &>/dev/null; then
        mkcert -install 2>/dev/null || true
        mkcert -key-file "$DEV/dev-key.pem" \
               -cert-file "$DEV/dev.pem" \
               localhost 127.0.0.1
    elif command -v openssl &>/dev/null; then
        # No mkcert: fall back to a self-signed certificate.
        #
        # The only thing mkcert adds is a local CA your browser already trusts,
        # which matters to a person clicking around and not to `curl -k` or to a
        # test suite. This used to be a hard `exit 1`, and m6's build host has no
        # mkcert -- the CI stage that runs this example only worked because rsync
        # happened to carry a developer's own untracked keys/ directory. A check
        # that depends on an untracked file is not a check.
        echo "mkcert not found; generating a self-signed certificate with openssl."
        echo "Your browser will warn about it. For one it trusts: brew install mkcert"
        openssl req -x509 -newkey rsa:2048 -sha256 -days 365 -nodes \
            -keyout "$DEV/dev-key.pem" -out "$DEV/dev.pem" \
            -subj "/CN=localhost" -addext "subjectAltName=DNS:localhost,IP:127.0.0.1" 2>/dev/null \
            || { echo "ERROR: openssl could not write a certificate." >&2; exit 1; }
    else
        echo "ERROR: neither mkcert nor openssl found. Install one:" >&2
        echo "  mkcert   - certificate your browser trusts (brew install mkcert)" >&2
        echo "  openssl  - self-signed, browser warns" >&2
        exit 1
    fi
    chmod 600 "$DEV/dev.pem" "$DEV/dev-key.pem"
fi

# The certificate a client trusts to reach this node: mkcert's local CA when
# mkcert made the certificate, else the self-signed certificate itself. A
# renderer that publishes with `POST /genid` names this file as `ca_file`.
if [ ! -f "$DEV/ca.pem" ]; then
    if command -v mkcert &>/dev/null && [ -f "$(mkcert -CAROOT)/rootCA.pem" ]; then
        cp "$(mkcert -CAROOT)/rootCA.pem" "$DEV/ca.pem"
    else
        cp "$DEV/dev.pem" "$DEV/ca.pem"
    fi
    chmod 600 "$DEV/ca.pem"
fi

# Auth signing key pair, shared by every example that uses m6-auth.
if [ ! -f "$DEV/auth.pem" ] || [ ! -f "$DEV/auth.pub" ]; then
    # `ec_param_enc:named_curve` is not redundant: macOS's LibreSSL writes explicit
    # curve parameters where OpenSSL writes the prime256v1 OID, and the token signer
    # only accepts the named form. Without it every login on a Mac fails with
    # "JWT encode error: InvalidEcdsaKey" even with the right password.
    openssl genpkey -algorithm EC -pkeyopt ec_paramgen_curve:P-256 -pkeyopt ec_param_enc:named_curve -out "$DEV/auth.pem"
    openssl pkey -in "$DEV/auth.pem" -pubout -out "$DEV/auth.pub"
    chmod 600 "$DEV/auth.pem" "$DEV/auth.pub"
fi
