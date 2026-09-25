#!/usr/bin/env bash
# Run the MySQL integration suite against an *older* server, to find the floor
# this driver actually works against.
#
#   deploy/dev/qh-mysql-old.sh 8.0
#   deploy/dev/qh-mysql-old.sh 5.7 --keep
#
# Named `qh-mysql-old` on host port 53308: never the development container
# (`qh-mysql`, 53306) that other work is using. Only this script's own container
# is ever removed.
#
# Output is the lines docs/compatibility.md is built from. `seed` and `tests` are
# reported separately on purpose: the generated fixture (deploy/dev/seed-mysql.sql)
# uses `WITH RECURSIVE`, which is MySQL 8.0+, so on an older server a failing seed
# says something about the *fixture*, not about the driver.
set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
PORT=53308
NAME=qh-mysql-old
DB_USER=qh
DB_PASSWORD=qh-dev-only
DB_NAME=qh
ROOT_PASSWORD=qh-dev-root-only

TAG=${1:?usage: qh-mysql-old.sh <image-tag, e.g. 8.0> [--keep]}
KEEP=${2:-}
# MySQL 5.7 has no arm64 image, so measuring it needs emulation. Set
# PLATFORM=linux/amd64 for that; the pull must have used it too:
#   podman pull --platform linux/amd64 mysql:5.7
PLATFORM=${PLATFORM:-}

podman rm -f "$NAME" >/dev/null 2>&1 || true
podman run -d --name "$NAME" \
  ${PLATFORM:+--platform "$PLATFORM"} \
  -e MYSQL_ROOT_PASSWORD="$ROOT_PASSWORD" \
  -e MYSQL_DATABASE="$DB_NAME" \
  -e MYSQL_USER="$DB_USER" \
  -e MYSQL_PASSWORD="$DB_PASSWORD" \
  -p "$PORT:3306" \
  "mysql:$TAG" >/dev/null || exit 1

ready=0
for _ in $(seq 1 120); do
  if podman exec "$NAME" mysqladmin ping -h 127.0.0.1 -uroot -p"$ROOT_PASSWORD" >/dev/null 2>&1; then
    ready=1
    break
  fi
  sleep 1
done
if [[ "$ready" != 1 ]]; then
  echo "version          unknown (server never became ready)"
  echo "seed             n/a"
  echo "tests            n/a"
  podman rm -f "$NAME" >/dev/null 2>&1 || true
  exit 1
fi

echo "version          $(podman exec -e MYSQL_PWD=$ROOT_PASSWORD "$NAME" mysql -uroot -N -B -e 'SELECT VERSION()')"

seed="$HERE/seed-mysql.sql"
if ! podman exec -i -e MYSQL_PWD=$ROOT_PASSWORD "$NAME" mysql -uroot "$DB_NAME" \
  < "$seed" >/tmp/qh-mysql-old-seed.log 2>&1; then
  echo "seed             FAILED: $(grep -m1 -i 'ERROR' /tmp/qh-mysql-old-seed.log)"
  # A fixture holding `WITH RECURSIVE` cannot load on a pre-8.0 server. The
  # fallback loads the same tables without the CTE so the *driver* can still be
  # measured; it is not a substitute for the real fixture and is named as such.
  if [[ -f "$HERE/qh-mysql-old-seed-prefixed.sql" ]]; then
    if podman exec -i -e MYSQL_PWD=$ROOT_PASSWORD "$NAME" mysql -uroot "$DB_NAME" \
      < "$HERE/qh-mysql-old-seed-prefixed.sql" >/tmp/qh-mysql-old-seed2.log 2>&1; then
      echo "seed fallback    ok (no-CTE fixture; wide_500k loaded by a stored-procedure loop)"
    else
      echo "seed fallback    FAILED: $(grep -m1 -i 'ERROR' /tmp/qh-mysql-old-seed2.log)"
    fi
  fi
else
  echo "seed             ok"
fi

result=$(cd "$ROOT" && QH_TEST_MYSQL=1 QH_MYSQL_PORT=$PORT \
  cargo test -p qh-driver-mysql --test integration 2>&1)
line=$(printf '%s\n' "$result" | grep -m1 '^test result:')
if [[ -n "$line" ]]; then
  echo "tests            ${line#test result: }"
else
  echo "tests            no result line"
fi
printf '%s\n' "$result" | grep -m1 '^failures:' -A6 || true

if [[ "$KEEP" != "--keep" ]]; then
  podman rm -f "$NAME" >/dev/null 2>&1 || true
fi
