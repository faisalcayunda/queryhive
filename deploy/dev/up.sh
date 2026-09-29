#!/usr/bin/env bash
# Bring up the local databases the integration tests and the benchmark baseline
# run against, and load the fixtures.
#
#   deploy/dev/up.sh              # start everything, load fixtures
#   deploy/dev/up.sh postgres     # one engine
#   deploy/dev/up.sh toxiproxy    # PostgreSQL behind 15 ms latency each way (30 ms RTT)
#   deploy/dev/up.sh --down       # stop and remove
#
# Podman rather than Docker: it is what is installed on this machine. The
# commands are the plain `podman run` ones, so no compose provider has to be
# present; deploy/dev/compose.yaml carries the same settings for anyone using
# `podman compose` or `docker compose`.
#
# Ports are deliberately off the defaults (55432, 53306, 58080) so a database
# already listening locally is not shadowed or accidentally used. Every publish
# binds 127.0.0.1 only. Nothing listens on a public interface.
#
# toxiproxy (qh-toxiproxy, MIT): 127.0.0.1:55435 forwards to the PostgreSQL
# container's published port with 15 ms latency in each direction, so a query
# round trip costs about 30 ms more than on 55432. Its HTTP API (unauthenticated,
# so also loopback-only) is on 8474.
# Start postgres first; `toxiproxy` is not part of `all`.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
POSTGRES_PORT=55432
MYSQL_PORT=53306
TRINO_PORT=58080
TOXIPROXY_PORT=55435
TOXIPROXY_API_PORT=8474
TOXIPROXY_LATENCY_MS=15
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
    -p "127.0.0.1:$POSTGRES_PORT:5432" \
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
    -p "127.0.0.1:$MYSQL_PORT:3306" \
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
  # than usefully. The VM here has 6 GiB; the container is capped at 3 GiB so a
  # runaway query cannot take the databases down with it (the JVM heap follows
  # the limit). See docs/compatibility.md for what was measured.
  podman rm -f qh-trino >/dev/null 2>&1 || true
  podman run -d --name qh-trino \
    --memory 3g \
    -p "127.0.0.1:$TRINO_PORT:8080" \
    trinodb/trino:latest >/dev/null
  echo "trino starting on 127.0.0.1:$TRINO_PORT (needs ~2 GiB of its own)"
}

start_toxiproxy() {
  podman rm -f qh-toxiproxy >/dev/null 2>&1 || true
  podman run -d --name qh-toxiproxy \
    -p "127.0.0.1:$TOXIPROXY_API_PORT:8474" \
    -p "127.0.0.1:$TOXIPROXY_PORT:$TOXIPROXY_PORT" \
    ghcr.io/shopify/toxiproxy:2.12.0 >/dev/null
  local api="http://127.0.0.1:$TOXIPROXY_API_PORT"
  local up=""
  for _ in $(seq 1 30); do
    if curl -fsS "$api/version" >/dev/null 2>&1; then up=1; break; fi
    sleep 1
  done
  if [[ -z "$up" ]]; then
    echo "toxiproxy API on $api did not answer within 30 s" >&2
    return 1
  fi
  # The upstream is the host-published PostgreSQL port: the default podman
  # network has no name resolution between containers.
  curl -fsS -X POST "$api/proxies" -d "{\"name\":\"postgres\",\"listen\":\"0.0.0.0:$TOXIPROXY_PORT\",\"upstream\":\"host.containers.internal:$POSTGRES_PORT\"}" >/dev/null
  for stream in downstream upstream; do
    curl -fsS -X POST "$api/proxies/postgres/toxics" \
      -d "{\"name\":\"latency_$stream\",\"type\":\"latency\",\"stream\":\"$stream\",\"attributes\":{\"latency\":$TOXIPROXY_LATENCY_MS}}" >/dev/null
  done
  echo "toxiproxy ready: 127.0.0.1:$TOXIPROXY_PORT -> postgres, ${TOXIPROXY_LATENCY_MS} ms each way (API on $TOXIPROXY_API_PORT)"
}

down() {
  for name in qh-postgres qh-mysql qh-trino qh-toxiproxy; do
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
    toxiproxy) start_toxiproxy ;;
    all)
      start_postgres
      start_mysql
      ;;
    *) echo "usage: up.sh [postgres|mysql|trino|toxiproxy|all|--down]" >&2; return 2 ;;
  esac
}

main "$@"
