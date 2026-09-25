#!/usr/bin/env bash
# Start (or restart) the MySQL servers the TLS tests run against.
#
#   crates/qh-driver-mysql/tests/mysql-tls-container.sh            # both
#   crates/qh-driver-mysql/tests/mysql-tls-container.sh tls-off    # only the plaintext one
#
# Two containers, both created by this script and both labelled `qh-test=mysql-tls`
# so a removal here can only ever touch them. The development containers
# (`qh-mysql` 53306, `qh-postgres` 55432, `qh-trino` 58080) and the ones other work
# brings up are never named, stopped or removed here.
#
#   qh-mysql-tls    127.0.0.1:53307   TLS on, with a CA this script generates.
#                                     The certificate is signed by that CA and
#                                     carries `IP:127.0.0.1` / `DNS:localhost`, so
#                                     a client that trusts the CA and connects to
#                                  `127.0.0.1` verifies the name as well as the
#                                     chain — a failure to verify is the root,
#                                     never a hostname mismatch.
#   qh-mysql-plain  127.0.0.1:53310   a server that cannot do TLS at all
#                                     (`--tls-version=''`), which is the only kind
#                                     of server `TlsMode::Prefer` may fall back on.
#
# The certificates live under ~/.cache/qh-mysql-tls, not in the repository: they are
# scratch, and `podman machine` mounts $HOME into its VM so a bind mount of a path
# under it works without a `podman cp` round trip.
#
# Both containers are left running: the integration tests are read from them
# repeatedly, and `mysql:8.4` takes ~20 s to initialise. Remove them by hand when
# they are no longer wanted:
#
#   podman rm -f qh-mysql-tls qh-mysql-plain
set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CERT_DIR="${QH_MYSQL_TLS_CERTS:-$HOME/.cache/qh-mysql-tls}"
TLS_NAME=qh-mysql-tls
PLAIN_NAME=qh-mysql-plain
TLS_PORT=53307
PLAIN_PORT=53310
DB_USER=qh
DB_PASSWORD=qh-dev-only
DB_NAME=qh
ROOT_PASSWORD=qh-dev-root-only
LABEL=qh-test=mysql-tls

WHICH=${1:-all}

# Remove a container only if it exists *and* carries this script's label.
remove_owned() {
  local name=$1
  if podman container exists "$name" 2>/dev/null; then
    local label
    label=$(podman inspect -f '{{index .Config.Labels "qh-test"}}' "$name" 2>/dev/null)
    if [[ "$label" == "mysql-tls" ]]; then
      podman rm -f "$name" >/dev/null 2>&1 || true
    else
      echo "refusing to remove $name: it does not carry $LABEL" >&2
      exit 1
    fi
  fi
}

wait_ready() {
  local name=$1
  for _ in $(seq 1 120); do
    if podman exec -e MYSQL_PWD="$ROOT_PASSWORD" "$name" \
      mysqladmin ping -h 127.0.0.1 -uroot >/dev/null 2>&1; then
      return 0
    fi
    sleep 1
  done
  echo "$name never became ready; last log lines:" >&2
  podman logs --tail 20 "$name" >&2 || true
  return 1
}

run_container() { # name port extra-args...
  local name=$1 port=$2
  shift 2
  podman run -d --name "$name" --label "$LABEL" \
    -e MYSQL_ROOT_PASSWORD="$ROOT_PASSWORD" \
    -e MYSQL_DATABASE="$DB_NAME" \
    -e MYSQL_USER="$DB_USER" \
    -e MYSQL_PASSWORD="$DB_PASSWORD" \
    -p "$port:3306" \
    mysql:8.4 "$@" >/dev/null || return 1
}

start_tls() {
  mkdir -p "$CERT_DIR"
  if [[ ! -f "$CERT_DIR/server-cert.pem" ]]; then
    # One CA, one server certificate signed by it. `-addext` is not used because the
    # extension has to be present on the *signed* certificate, which `-extfile` on
    # the signing step is the way to do.
    openssl req -x509 -newkey rsa:2048 -nodes -keyout "$CERT_DIR/ca-key.pem" \
      -out "$CERT_DIR/ca.pem" -days 3650 -subj '/CN=qh-mysql-tls-test-ca' >/dev/null 2>&1
    openssl req -newkey rsa:2048 -nodes -keyout "$CERT_DIR/server-key.pem" \
      -out "$CERT_DIR/server-req.pem" -subj '/CN=127.0.0.1' >/dev/null 2>&1
    printf 'subjectAltName=IP:127.0.0.1,DNS:localhost\nbasicConstraints=CA:FALSE\n' \
      > "$CERT_DIR/san.cnf"
    openssl x509 -req -in "$CERT_DIR/server-req.pem" -CA "$CERT_DIR/ca.pem" \
      -CAkey "$CERT_DIR/ca-key.pem" -CAcreateserial -out "$CERT_DIR/server-cert.pem" \
      -days 3650 -extfile "$CERT_DIR/san.cnf" >/dev/null 2>&1
  fi
  # Readable by the `mysql` user the image drops to.
  chmod 644 "$CERT_DIR"/*.pem

  remove_owned "$TLS_NAME"
  run_container "$TLS_NAME" "$TLS_PORT" \
    --ssl-ca=/etc/mysql/tls/ca.pem \
    --ssl-cert=/etc/mysql/tls/server-cert.pem \
    --ssl-key=/etc/mysql/tls/server-key.pem \
    || { echo "could not start $TLS_NAME" >&2; exit 1; }
  # The mount is named separately from the flags above so a failure to see the
  # certificates says so here rather than as a mysqld that exits at startup.
  podman cp "$CERT_DIR/." "$TLS_NAME":/etc/mysql/tls >/dev/null 2>&1 || true

  wait_ready "$TLS_NAME" || exit 1
  echo "$TLS_NAME  listening on 127.0.0.1:$TLS_PORT  ca=$CERT_DIR/ca.pem"
}

start_plain() {
  remove_owned "$PLAIN_NAME"
  # `--tls-version=''` is how MySQL 8.4 says "no TLS at all": `--ssl=0` was removed
  # in 8.4, and the server otherwise generates a self-signed certificate on first
  # start and offers TLS whether or not anyone asked for it.
  run_container "$PLAIN_NAME" "$PLAIN_PORT" --tls-version= \
    || { echo "could not start $PLAIN_NAME" >&2; exit 1; }
  wait_ready "$PLAIN_NAME" || exit 1
  echo "$PLAIN_NAME  listening on 127.0.0.1:$PLAIN_PORT  tls=off"
}

case "$WHICH" in
  all) start_tls; start_plain ;;
  tls) start_tls ;;
  tls-off|plain) start_plain ;;
  *) echo "usage: $0 [all|tls|tls-off]" >&2; exit 2 ;;
esac
