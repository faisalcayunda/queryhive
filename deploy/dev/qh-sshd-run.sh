#!/usr/bin/env bash
# A real sshd in a container, for the qh-tunnel integration tests.
#
#   deploy/qh-sshd-run.sh          # start qh-sshd-dev and a throwaway client key
#   deploy/qh-sshd-run.sh --down   # remove only qh-sshd-dev
#
# Then:
#
#   QH_TEST_SSH=1 cargo test -p qh-tunnel
#
# Container name qh-sshd-dev and host port 52222 belong to this script. It never
# touches another container: the dev databases (qh-postgres, qh-mysql, qh-trino on
# 55432, 53306, 58080) are run by deploy/dev/up.sh and are left alone even by
# `--down`, which removes qh-sshd-dev and nothing else. No `podman system prune`,
# no `podman rmi`, no `podman container rm`.
#
# The host keys are generated inside the container by `ssh-keygen -A`, not mounted
# in: a macOS bind mount arrives with modes the container does not control, and
# sshd refuses a host key whose permissions are wrong.
#
# The key and password below are throwaway fixtures for a container that listens on
# the loopback interface of a local podman machine. They are not secrets and open
# nothing else; a real bastion's key material never reaches this file.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
STATE="$REPO/target/qh-sshd"
CONTAINER=qh-sshd-dev
HOST_PORT=52222
USER_NAME=qh
PASSWORD=qh-sshd-dev-only
KEY="$STATE/id_ed25519"
IMAGE=alpine:3.20

down() {
  podman rm -f "$CONTAINER" >/dev/null 2>&1 || true
  echo "removed $CONTAINER (the client key in $STATE is left in place)"
}

wait_for_sshd() {
  for _ in $(seq 1 60); do
    if ssh_to_sshd true >/dev/null 2>&1; then
      return 0
    fi
    sleep 1
  done
  echo "qh-sshd-dev never accepted a key-authenticated connection; its own log:" >&2
  podman logs "$CONTAINER" >&2 || true
  return 1
}

# A port probe is not enough of a readiness check: podman's forwarder accepts the TCP
# connection as soon as it is set up, which is before sshd inside has bound port 22.
# Only a real authenticated connection says the container is usable.
ssh_to_sshd() {
  ssh -i "$KEY" -p "$HOST_PORT" \
    -o BatchMode=yes -o IdentitiesOnly=yes \
    -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null \
    "$USER_NAME@127.0.0.1" "$@"
}

up() {
  mkdir -p "$STATE"
  chmod 700 "$STATE"
  if [[ ! -f "$KEY" ]]; then
    # A fresh throwaway client key every time this directory is cleared, so a test
    # that leaves the key behind never leaves a key that opens anything real.
    ssh-keygen -q -t ed25519 -N '' -C "qh-tunnel-dev" -f "$KEY"
  fi

  podman rm -f "$CONTAINER" >/dev/null 2>&1 || true
  podman run -d --name "$CONTAINER" \
    -p "$HOST_PORT:22" \
    -e QH_AUTHORIZED_KEY="$(cat "$KEY.pub")" \
    -e QH_PASSWORD="$PASSWORD" \
    "$IMAGE" sh -c '
      set -e
      apk add --no-cache openssh >/dev/null
      mkdir -p /run/sshd
      ssh-keygen -A
      # Alpine ships `AllowTcpForwarding no`, which would refuse the direct-tcpip
      # channels the tunnel is made of. The drop-in is honoured because sshd_config
      # includes sshd_config.d at its top, and the first value of a directive wins.
      # MaxStartups as well: the tests run in parallel and several of them are refused
      # during the key exchange, which leaves sshd holding unauthenticated connections
      # until it notices they closed. Its default of 10 would drop the run itself.
      printf "AllowTcpForwarding yes\nPermitOpen any\nMaxStartups 100:30:200\n" \
        > /etc/ssh/sshd_config.d/00-qh-tunnel.conf
      adduser -D -s /bin/sh '"$USER_NAME"'
      echo "'"$USER_NAME"':${QH_PASSWORD}" | chpasswd
      mkdir -p /home/'"$USER_NAME"'/.ssh
      printf "%s\n" "$QH_AUTHORIZED_KEY" > /home/'"$USER_NAME"'/.ssh/authorized_keys
      chown -R '"$USER_NAME"':'"$USER_NAME"' /home/'"$USER_NAME"'/.ssh
      chmod 700 /home/'"$USER_NAME"'/.ssh
      chmod 600 /home/'"$USER_NAME"'/.ssh/authorized_keys
      exec /usr/sbin/sshd -D -e
    ' >/dev/null

  wait_for_sshd
  # The tests authenticate the same way, so a broken authorized_keys is caught here
  # rather than as a confusing failure inside a test.
  ssh_to_sshd true

  echo "sshd ready on 127.0.0.1:$HOST_PORT (key: $KEY, user: $USER_NAME, password: $PASSWORD)"
  echo "run: QH_TEST_SSH=1 cargo test -p qh-tunnel"
}

if [[ "${1:-}" == "--down" ]]; then
  down
else
  up
fi
