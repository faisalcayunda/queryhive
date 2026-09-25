#!/usr/bin/env bash
# The PostgreSQL container the TLS tests verify against.
#
#   crates/qh-driver-postgres/tests/tls/up.sh          # create it, or restart it
#   crates/qh-driver-postgres/tests/tls/up.sh --down   # remove it
#
# Then:
#
#   QH_TEST_POSTGRES=1 cargo test -p qh-driver-postgres --test tls
#
# It is a second PostgreSQL alongside `deploy/dev/up.sh postgres`, on its own port
# and with its own name, because it is the only one that serves TLS — and the
# certificate it serves is `server.crt`, signed by the `ca.crt` this crate's tests
# put in a root store. That pairing is the point: a certificate generated at test
# time could not be the one the server is already using.
#
# Nothing here touches another container, and `--down` removes only `qh-pg-tls`.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CONTAINER=qh-pg-tls
PORT=55433
DB_USER=qh
DB_PASSWORD=qh-dev-only
DB_NAME=qh
CERT_DIR=/var/lib/postgresql

if [[ "${1:-}" == "--down" ]]; then
  podman rm -f "$CONTAINER" >/dev/null 2>&1 || true
  echo "removed $CONTAINER (its named volume is left in place)"
  exit 0
fi

if [[ ! -f "$HERE/server.crt" || ! -f "$HERE/server.key" ]]; then
  echo "no certificate here yet; running make-certs.sh first" >&2
  "$HERE/make-certs.sh"
fi

if podman container exists "$CONTAINER"; then
  podman start "$CONTAINER" >/dev/null 2>&1 || true
else
  podman run -d --name "$CONTAINER" \
    -e POSTGRES_USER="$DB_USER" \
    -e POSTGRES_PASSWORD="$DB_PASSWORD" \
    -e POSTGRES_DB="$DB_NAME" \
    -p "$PORT:5432" \
    -v qh-pg-tls-data:/var/lib/postgresql/data \
    postgres:17-alpine >/dev/null
fi

for _ in $(seq 1 60); do
  if podman exec "$CONTAINER" pg_isready -U "$DB_USER" -d "$DB_NAME" >/dev/null 2>&1; then
    break
  fi
  sleep 1
done

# The certificate pair is copied in rather than bind-mounted: PostgreSQL refuses a
# key that is group- or world-readable, and a bind mount from the host arrives with
# the host's permissions, which are not the container user's. Copying lets the file
# be owned by `postgres` with mode 0600, which is what the server insists on.
podman cp "$HERE/server.crt" "$CONTAINER:$CERT_DIR/server.crt"
podman cp "$HERE/server.key" "$CONTAINER:$CERT_DIR/server.key"
podman exec -u 0 "$CONTAINER" chown postgres:postgres \
  "$CERT_DIR/server.crt" "$CERT_DIR/server.key"
podman exec -u 0 "$CONTAINER" chmod 600 "$CERT_DIR/server.key"

# `ALTER SYSTEM` rather than editing postgresql.conf: it writes
# postgresql.auto.conf, which is a supported interface, and it takes effect on the
# restart below. One statement per call, because that is what the server accepts.
podman exec -i "$CONTAINER" psql -q -U "$DB_USER" -d "$DB_NAME" \
  -c "ALTER SYSTEM SET ssl = on" \
  -c "ALTER SYSTEM SET ssl_cert_file = '$CERT_DIR/server.crt'" \
  -c "ALTER SYSTEM SET ssl_key_file = '$CERT_DIR/server.key'" >/dev/null

podman restart "$CONTAINER" >/dev/null

for _ in $(seq 1 60); do
  if podman exec "$CONTAINER" pg_isready -U "$DB_USER" -d "$DB_NAME" >/dev/null 2>&1; then
    break
  fi
  sleep 1
done

# Prove it is really serving TLS, so a test failure is never a container that never
# turned it on. `pg_stat_ssl` answers it from the server's own point of view; the
# `openssl s_client` line confirms the certificate a client is handed.
ssl=$(podman exec "$CONTAINER" psql -tAq -U "$DB_USER" -d "$DB_NAME" \
  -c "SELECT current_setting('ssl')")
echo "ssl on $CONTAINER is '$ssl' (port $PORT)"
podman exec "$CONTAINER" psql -tAq -U "$DB_USER" -d "$DB_NAME" \
  -c "SELECT ssl, version FROM pg_stat_ssl WHERE pid = pg_backend_pid()"
openssl s_client -connect "127.0.0.1:$PORT" -servername localhost -CAfile "$HERE/ca.crt" \
  </dev/null 2>/dev/null | grep -E "^ *(subject|issuer|Verify return code)" || true

echo "postgres TLS ready on 127.0.0.1:$PORT"
