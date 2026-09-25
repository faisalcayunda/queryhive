#!/usr/bin/env bash
# Bring up the local databases the integration tests and the benchmark baseline
# run against, and load the fixtures.
#
#   deploy/dev/up.sh              # start everything, load fixtures
#   deploy/dev/up.sh postgres     # one engine
#   deploy/dev/up.sh --down       # stop and remove
#
# Podman rather than Docker: it is what is installed on this machine. The
# commands are the plain `podman run` ones, so no compose provider has to be
# present; deploy/dev/compose.yaml carries the same settings for anyone using
# `podman compose` or `docker compose`.
#
# Ports are deliberately off the defaults (55432, 53306, 58080) so a database
# already listening locally is not shadowed or accidentally used.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
POSTGRES_PORT=55432
MYSQL_PORT=53306
TRINO_PORT=58080
# Throwaway credentials for a local fixture container, not a secret: the
# databases hold only generated data and nothing listens on a public interface.
DB_USER=qh
DB_PASSWORD=qh-dev-only
DB_NAME=qh
MYSQL_ROOT_PASSWORD=qh-dev-root-only

podman_run() { podman "$@"; }

wait_for_postgres() {
  local name=$1
  for _ in $(seq 1 60); do
    if podman exec "$name" pg_isready -U "$DB_USER" -d "$DB_NAME" >/dev/null 2>&1; then
      return 0
    fi
    sleep 1
  done
  echo "postgres ($name) did not become ready" >&2
  return 1
}

wait_for_mysql() {
  local name=$1
  for _ in $(seq 1 90); do
    if podman exec "$name" mysqladmin ping -h 127.0.0.1 -uroot -p"$MYSQL_ROOT_PASSWORD" >/dev/null 2>&1; then
      return 0
    fi
    sleep 1
  done
  echo "mysql ($name) did not become ready" >&2
  return 1
}

start_postgres() {
  podman rm -f qh-postgres >/dev/null 2>&1 || true
  podman run -d --name qh-postgres \
    -e POSTGRES_USER="$DB_USER" \
    -e POSTGRES_PASSWORD="$DB_PASSWORD" \
    -e POSTGRES_DB="$DB_NAME" \
    -p "$POSTGRES_PORT:5432" \
    -v qh-postgres-data:/var/lib/postgresql/data \
    postgres:17-alpine >/dev/null
  wait_for_postgres qh-postgres
  podman exec -i qh-postgres psql -q -v ON_ERROR_STOP=1 -U "$DB_USER" -d "$DB_NAME" \
    < "$HERE/seed-postgres.sql"
  echo "postgres ready on 127.0.0.1:$POSTGRES_PORT"
}

start_mysql() {
  podman rm -f qh-mysql >/dev/null 2>&1 || true
  podman run -d --name qh-mysql \
    -e MYSQL_ROOT_PASSWORD="$MYSQL_ROOT_PASSWORD" \
    -e MYSQL_DATABASE="$DB_NAME" \
    -e MYSQL_USER="$DB_USER" \
    -e MYSQL_PASSWORD="$DB_PASSWORD" \
    -p "$MYSQL_PORT:3306" \
    -v qh-mysql-data:/var/lib/mysql \
    mysql:8.4 >/dev/null
  wait_for_mysql qh-mysql
  podman exec -i qh-mysql mysql -uroot -p"$MYSQL_ROOT_PASSWORD" "$DB_NAME" \
    < "$HERE/seed-mysql.sql"
  echo "mysql ready on 127.0.0.1:$MYSQL_PORT"
}

start_trino() {
  # Not started by default: a single-node Trino wants roughly 2 GiB of its own,
  # and starting it alongside the two databases on a small VM fails slowly rather
  # than usefully. The VM here has been raised to 4 GiB, and it now serves in about
  # ten seconds. See docs/compatibility.md for what was measured.
  podman rm -f qh-trino >/dev/null 2>&1 || true
  podman run -d --name qh-trino \
    -p "$TRINO_PORT:8080" \
    trinodb/trino:latest >/dev/null
  echo "trino starting on 127.0.0.1:$TRINO_PORT (needs ~2 GiB of its own)"
}

down() {
  for name in qh-postgres qh-mysql qh-trino; do
    podman rm -f "$name" >/dev/null 2>&1 || true
  done
  echo "removed the dev containers (named volumes left in place)"
}

main() {
  if [[ "${1:-}" == "--down" ]]; then
    down
    return
  fi
  python3 "$HERE/make_seed.py"
  local target="${1:-all}"
  case "$target" in
    postgres) start_postgres ;;
    mysql) start_mysql ;;
    trino) start_trino ;;
    all)
      start_postgres
      start_mysql
      ;;
    *) echo "usage: up.sh [postgres|mysql|trino|all|--down]" >&2; return 2 ;;
  esac
}

main "$@"
