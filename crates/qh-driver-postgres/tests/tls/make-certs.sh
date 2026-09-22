#!/usr/bin/env bash
# The TLS fixtures the driver's tests verify against, and the pair the
# `qh-pg-tls` container serves.
#
#   crates/qh-driver-postgres/tests/tls/make-certs.sh          # regenerate in place
#
# Four files matter, and they are committed rather than generated per run because
# the container's certificate has to be signed by the same CA the test puts in its
# root store — a certificate made at test time could not be the one the server is
# already serving.
#
#   ca.crt         the test CA. The "trusts it" half puts this in a root store.
#   server.crt     signed by ca.crt, for localhost/127.0.0.1. The container serves
#                  this one, so a test with ca.crt in its store verifies it.
#   other-ca.crt   a second CA that signed nothing here. A root store holding only
#                  this one is a store that does not trust the server — which is
#                  how the refusing half is tested without a second container.
#
# These keys protect nothing: they belong to a throwaway container on 127.0.0.1
# that holds generated data. Keep them out of any other program.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DAYS=3650
CURVE=prime256v1

# 1. A CA, twice: one that signs the server certificate and one that does not.
make_ca() {
  local name=$1 subject=$2
  openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:"$CURVE" -nodes \
    -keyout "$HERE/$name.key" -out "$HERE/$name.crt" -days "$DAYS" \
    -subj "$subject" \
    -addext "basicConstraints=critical,CA:TRUE" \
    -addext "keyUsage=critical,keyCertSign,cRLSign" >/dev/null 2>&1
}

make_ca ca "/CN=QueryHive test CA"
make_ca other-ca "/CN=QueryHive untrusted test CA"

# 2. The server certificate: signed by `ca`, and valid for every name the tests
#    dial. An IP address is verified as an IP SAN, not as a DNS name, so 127.0.0.1
#    has to be listed as one.
openssl req -newkey ec -pkeyopt ec_paramgen_curve:"$CURVE" -nodes \
  -keyout "$HERE/server.key" -out "$HERE/server.csr" -subj "/CN=localhost" \
  >/dev/null 2>&1

cat >"$HERE/server.ext" <<'EXT'
basicConstraints=CA:FALSE
keyUsage=critical,digitalSignature,keyEncipherment
extendedKeyUsage=serverAuth
subjectAltName=DNS:localhost,IP:127.0.0.1,IP:::1
EXT

openssl x509 -req -in "$HERE/server.csr" -CA "$HERE/ca.crt" -CAkey "$HERE/ca.key" \
  -CAcreateserial -out "$HERE/server.crt" -days "$DAYS" \
  -extfile "$HERE/server.ext" >/dev/null 2>&1

rm -f "$HERE/server.csr" "$HERE/server.ext" "$HERE/ca.srl"

# The key is unencrypted because PostgreSQL refuses a passphrase-protected key at
# startup; it is readable here only because this directory exists for these tests.
chmod 600 "$HERE"/*.key

echo "wrote ca.crt, server.crt/server.key, other-ca.crt to $HERE"
openssl x509 -in "$HERE/server.crt" -noout -subject -issuer -ext subjectAltName
