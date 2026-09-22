#!/usr/bin/env bash
# Run the Trino integration suite against an *older* release, to find the floor.
#
#   deploy/dev/qh-trino-old.sh 400
#
# Named `qh-trino-old` on host port 58082, so the development container
# (`qh-trino`, 58080) that other work is using is never touched. Only this
# script's own container is ever removed.
#
# Memory is the constraint here, not time: the podman VM has ~3.6 GiB and the
# development coordinator already holds most of it. A hard cgroup limit is put on
# this container so that if anything is killed under pressure it is this one, and
# never the running container.
#
# The image's jvm.config sizes the heap as a percentage of the container limit
# (`-XX:MaxRAMPercentage=80`), so `--memory 1000m` means roughly a 780 MiB heap.
# A single-node coordinator is documented to want 1 GiB; below that it may fail
# to start at all, and `tests` then reports that rather than a driver failure.
set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
PORT=58082
NAME=qh-trino-old
MEMORY=${MEMORY:-1000m}

TAG=${1:?usage: qh-trino-old.sh <image-tag, e.g. 400> [--keep]}
KEEP=${2:-}

podman rm -f "$NAME" >/dev/null 2>&1 || true
podman run -d --name "$NAME" \
  --memory "$MEMORY" --memory-swap "$MEMORY" \
  -p "$PORT:8080" \
  "trinodb/trino:$TAG" >/dev/null || exit 1

# `/v1/info` is the endpoint the driver itself uses, so readiness is measured the
# same way the tests measure the server.
version=""
for _ in $(seq 1 120); do
  body=$(curl -s --max-time 2 "http://127.0.0.1:$PORT/v1/info" 2>/dev/null || true)
  if [[ -n "$body" ]]; then
    version=$(printf '%s' "$body" | sed -n 's/.*"version":"\([^"]*\)".*/\1/p')
    case "$body" in *'"state":"ACTIVE"'*) break ;; esac
  fi
  sleep 2
done

if [[ -z "$version" ]]; then
  echo "version          unknown (server never answered /v1/info)"
  echo "tests            n/a"
  podman logs "$NAME" 2>&1 | tail -5
  [[ "$KEEP" != "--keep" ]] && podman rm -f "$NAME" >/dev/null 2>&1
  exit 1
fi
echo "version          $version"

result=$(cd "$ROOT" && QH_TEST_TRINO=1 QH_TRINO_PORT=$PORT \
  cargo test -p qh-driver-trino --test integration 2>&1)
line=$(printf '%s\n' "$result" | grep -m1 '^test result:')
if [[ -n "$line" ]]; then
  echo "tests            ${line#test result: }"
else
  echo "tests            no result line"
fi
printf '%s\n' "$result" | grep -m1 -A6 '^failures:' || true

if [[ "$KEEP" != "--keep" ]]; then
  podman rm -f "$NAME" >/dev/null 2>&1 || true
fi
