#!/usr/bin/env bash
# Start (or restart) the TLS Trino coordinator the TLS integration tests run
# against.
#
#   deploy/dev/qh-trino-tls.sh          # create and start it
#   deploy/dev/qh-trino-tls.sh down     # remove it (and nothing else)
#
# One container, `qh-trino-tls`, labelled `qh-test=trino-tls` so a removal here can
# only ever touch it. The development containers (`qh-trino` 58080, `qh-postgres`
# 55432, `qh-mysql` 53306) and the ones other work brings up are never named,
# stopped or removed by this script.
#
#   qh-trino-tls   127.0.0.1:58081   HTTPS with a self-signed keystore, so the
#                                    certificate verifies against nothing. That is
#                                    the point: `Require` must fail here and
#                                    `RequireNoVerify` must succeed, and both are
#                                    facts about the same server.
#
# HTTP stays enabled *inside* the container on 8080, because the image's health
# check uses it, but it is not published: from the host the coordinator is
# reachable on 58081 over TLS or not at all, so a `Prefer` fallback has nowhere to
# land and its absence is unambiguous.
#
# The keystore and the generated configuration live under ~/.cache/qh-trino-tls,
# not in the repository: they are scratch, and `podman machine` mounts $HOME into
# its VM so a bind mount of a path under it works without a `podman cp`.
#
# The keystore password is a throwaway for a local fixture, not a secret: the
# container serves only generated data on a loopback port.
#
# Start it when the TLS tests are wanted and stop it (`down`) when they are not.
# This is the one fixture that is deliberately *not* left running: the VM is 3.6
# GiB and shared, and two single-node Trinos do not fit in it — measured, not
# assumed. Bringing this one up alongside `qh-trino` has OOM-killed `qh-trino`,
# twice, which is other work's container to lose. The integration tests therefore
# skip visibly when this coordinator is not listening, rather than requiring both
# halves of the suite to be up at once.
#
# It is capped at 768 MiB with a hand-written jvm.config (`-Xmx256m`) and
# `--oom-score-adj 1000`, so that if memory does run out it is this fixture the
# kernel takes rather than a neighbour. Trino's own baseline is around 600 MB
# whatever the heap says, which is why the cap alone was not enough.
set -uo pipefail

CERT_DIR="${QH_TRINO_TLS_CERTS:-$HOME/.cache/qh-trino-tls}"
NAME=qh-trino-tls
PORT=58081
IMAGE=trinodb/trino:latest
LABEL=qh-test=trino-tls
MEMORY="${QH_TRINO_TLS_MEMORY:-768m}"
KEYSTORE_PASSWORD=qh-trino-tls-dev-only

# Remove the container only if it exists *and* carries this script's label.
remove_owned() {
  if podman container exists "$NAME" 2>/dev/null; then
    local label
    label=$(podman inspect -f '{{index .Config.Labels "qh-test"}}' "$NAME" 2>/dev/null)
    if [[ "$label" == "trino-tls" ]]; then
      podman rm -f "$NAME" >/dev/null 2>&1 || true
    else
      echo "refusing to remove $NAME: it does not carry $LABEL" >&2
      exit 1
    fi
  fi
}

# The image carries the JDK, so its `keytool` is used rather than requiring one on
# the host -- there is none here, and `keytool -genkeypair` is the shortest path
# from nothing to a keystore Trino will load.
generate_keystore() {
  if [[ -f "$CERT_DIR/tls/keystore.p12" ]]; then
    return 0
  fi
  mkdir -p "$CERT_DIR/tls"
  # `$JAVA_HOME/bin/keytool` rather than a path spelled out: the image's JDK
  # directory carries its version, which changes with the tag.
  podman run --rm -v "$CERT_DIR/tls:/tls" --entrypoint /bin/sh "$IMAGE" -c "
    exec \"\$JAVA_HOME/bin/keytool\" -genkeypair -alias trino -keyalg RSA -keysize 2048 \
      -validity 3650 -dname 'CN=127.0.0.1' -ext 'SAN=IP:127.0.0.1,DNS:localhost' \
      -storetype PKCS12 -keystore /tls/keystore.p12 \
      -storepass $KEYSTORE_PASSWORD -keypass $KEYSTORE_PASSWORD" \
    || { echo "could not generate the keystore" >&2; return 1; }
  chmod 644 "$CERT_DIR/tls/keystore.p12"
}

write_jvm_config() {
  # The image's own jvm.config wants 80% of the container's memory, which on a
  # `--memory 768m` cap is still more heap than a coordinator answering `SELECT 1`
  # needs. Small and fixed instead, so this container stays a guest: other work's
  # containers share the same VM, and a second Trino asking for a default heap made
  # `qh-trino` get killed once already.
  cat > "$CERT_DIR/jvm.config" <<'JVM'
-server
-Xms128m
-Xmx256m
-XX:G1HeapRegionSize=16M
-XX:+ExplicitGCInvokesConcurrent
-XX:+HeapDumpOnOutOfMemoryError
-XX:+ExitOnOutOfMemoryError
-XX:-OmitStackTraceInFastThrow
-XX:ReservedCodeCacheSize=96M
-XX:PerMethodRecompilationCutoff=10000
-XX:PerBytecodeRecompilationCutoff=10000
-Djdk.attach.allowAttachSelf=true
-Djdk.nio.maxCachedBufferSize=2000000
JVM
}

write_config() {
  # The image's own config.properties, plus HTTPS. The defaults it carries are kept
  # as they are: dropping `node-scheduler.include-coordinator` or `discovery.uri`
  # would produce a coordinator that starts and cannot plan anything.
  cat > "$CERT_DIR/config.properties" <<'CONFIG'
#single node install config
coordinator=true
node-scheduler.include-coordinator=true
discovery.uri=http://localhost:8080
catalog.management=${ENV:CATALOG_MANAGEMENT}
http-server.http.enabled=true
http-server.https.enabled=true
http-server.https.port=8443
http-server.https.keystore.path=/etc/trino/tls/keystore.p12
CONFIG
  echo "http-server.https.keystore.key=$KEYSTORE_PASSWORD" >> "$CERT_DIR/config.properties"
}

wait_ready() {
  # The image's health check reads HTTP 8080 inside the container, which stays
  # enabled; then HTTPS is probed from the host, because *that* is what the tests
  # use and a coordinator that only answers in clear would pass the first check.
  for _ in $(seq 1 180); do
    if podman exec "$NAME" /usr/lib/trino/bin/health-check >/dev/null 2>&1 \
      && curl -sk --max-time 2 "https://127.0.0.1:$PORT/v1/info" >/dev/null 2>&1; then
      return 0
    fi
    sleep 1
  done
  echo "$NAME never became ready; last log lines:" >&2
  podman logs --tail 30 "$NAME" >&2 || true
  return 1
}

start() {
  generate_keystore || exit 1
  write_config
  write_jvm_config
  remove_owned
  # `--oom-score-adj` is the important flag on a shared VM: with several
  # containers on the same machine, a global out-of-memory event must take *this*
  # fixture rather than whichever container the kernel happens to pick. It is this
  # container's tests that are dispensible, so it volunteers.
  podman run -d --name "$NAME" --label "$LABEL" \
    --memory "$MEMORY" --oom-score-adj 1000 \
    -p "$PORT:8443" \
    -v "$CERT_DIR/config.properties:/etc/trino/config.properties:ro" \
    -v "$CERT_DIR/jvm.config:/etc/trino/jvm.config:ro" \
    -v "$CERT_DIR/tls:/etc/trino/tls:ro" \
    "$IMAGE" >/dev/null || { echo "could not start $NAME" >&2; exit 1; }
  wait_ready || exit 1
  echo "$NAME  listening on https://127.0.0.1:$PORT  keystore=$CERT_DIR/tls/keystore.p12"
  echo "run: QH_TEST_TRINO=1 cargo test -p qh-driver-trino"
}

case "${1:-up}" in
  up) start ;;
  down) remove_owned; echo "removed $NAME" ;;
  *) echo "usage: $0 [up|down]" >&2; exit 2 ;;
esac
