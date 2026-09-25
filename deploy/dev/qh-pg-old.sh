#!/usr/bin/env bash
# Run the PostgreSQL integration suite against an *older* server, to find the
# floor this driver actually works against.
#
#   deploy/dev/qh-pg-old.sh 15-alpine
#   deploy/dev/qh-pg-old.sh 12-alpine --keep
#
# The container is named `qh-pg-old` and listens on host port 55434, so it never
# collides with the development container (`qh-postgres`, 55432) that other work
# is using. Only a container this script created is ever removed; `--keep` leaves
# it up for a manual look.
#
# The result goes on stdout as lines the docs table is built from:
#
#   version          15.19
#   seed             ok
#   tests            ok 13 passed 0 failed
#
# See docs/compatibility.md for the measurements this produced.
set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
PORT=55434
NAME=qh-pg-old
DB_USER=qh
DB_PASSWORD=qh-dev-only
DB_NAME=qh

TAG=${1:?usage: qh-pg-old.sh <image-tag, e.g. 15-alpine> [--keep]}
KEEP=${2:-}

podman rm -f "$NAME" >/dev/null 2>&1 || true
podman run -d --name "$NAME" \
  -e POSTGRES_USER="$DB_USER" \
  -e POSTGRES_PASSWORD="$DB_PASSWORD" \
  -e POSTGRES_DB="$DB_NAME" \
  -p "$PORT:5432" \
  "postgres:$TAG" >/dev/null || exit 1

# An actual query, not `pg_isready`: the image's entrypoint starts a temporary
# server to run initdb, and on 9.6 `pg_isready` answers `ready` against *that*
# one, before `POSTGRES_DB` has been created. Waiting on a real connection is the
# difference between measuring the driver and measuring this script's timing.
ready=0
for _ in $(seq 1 60); do
  if podman exec "$NAME" psql -U "$DB_USER" -d "$DB_NAME" -tAc 'SELECT 1' >/dev/null 2>&1; then
    ready=1
    break
  fi
  sleep 1
done
if [[ "$ready" != 1 ]]; then
  echo "version          $(podman exec "$NAME" psql -U "$DB_USER" -d "$DB_NAME" -tAc 'SELECT version()' 2>&1 | head -1)"
  echo "seed             n/a (server never became ready)"
  echo "tests            n/a"
  exit 1
fi

echo "version          $(podman exec "$NAME" psql -U "$DB_USER" -d "$DB_NAME" -tAc 'SELECT version()' | sed 's/ on .*//')"

if podman exec -i "$NAME" psql -q -v ON_ERROR_STOP=1 -U "$DB_USER" -d "$DB_NAME" \
  < "$HERE/seed-postgres.sql" >/tmp/qh-pg-old-seed.log 2>&1; then
  echo "seed             ok"
else
  echo "seed             FAILED: $(grep -m1 ERROR /tmp/qh-pg-old-seed.log)"
fi

result=$(cd "$ROOT" && QH_TEST_POSTGRES=1 QH_PG_PORT=$PORT \
  cargo test -p qh-driver-postgres --test integration 2>&1)
line=$(printf '%s\n' "$result" | grep -m1 '^test result:')
if [[ -n "$line" ]]; then
  echo "tests            ${line#test result: }"
else
  echo "tests            no result line; first failure follows"
  printf '%s\n' "$result" | grep -m1 -A3 'panicked\|error\[' || true
fi

if [[ "$KEEP" != "--keep" ]]; then
  podman rm -f "$NAME" >/dev/null 2>&1 || true
fi
