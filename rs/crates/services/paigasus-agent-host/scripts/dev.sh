#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# Start the agent host against a throwaway Postgres container (spec SMA-786 section 6.11).
#   scripts/dev.sh           run the service until Ctrl-C
#   scripts/dev.sh --check   build, boot, wait for /readyz 200, stop, exit 0 or 1
# The script must run under bash 3.2 and bash 5. No here-string, no mapfile, no declare -A,
# no local -n, and no pipe into a reader that can exit early.
set -euo pipefail

# Digest source: POSTGRES_16_ALPINE_DIGEST in ci/images/run.sh. Do not source that file.
POSTGRES_IMAGE="postgres:16-alpine@sha256:cf78e76683b9ca8c5733cbbdce6c9262b45b6767934dd0a95e671f9a0fc20685"
LABEL_KEY="paigasus.dev"
LABEL_VALUE="agent-host"
PID_KEY="paigasus.dev.pid"

mode="run"
case "${1:-}" in
  "") ;;
  --check) mode="check" ;;
  *) echo "usage: dev.sh [--check]" >&2; exit 2 ;;
esac

here="$(cd "$(dirname "$0")" && pwd)"
rs_dir="$(cd "$here/../../../.." && pwd)"

container=""
app_pid=""

cleanup() {
  if [ -n "$app_pid" ] && kill -0 "$app_pid" 2>/dev/null; then
    pkill -P "$app_pid" 2>/dev/null || true
    kill "$app_pid" 2>/dev/null || true
    wait "$app_pid" 2>/dev/null || true
  fi
  if [ -n "$container" ]; then
    docker rm -f "$container" >/dev/null 2>&1 || true
  fi
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

# 1. Remove only labelled containers whose owner process is gone.
stale="$(docker ps -aq --filter "label=${LABEL_KEY}=${LABEL_VALUE}")"
for id in $stale; do
  owner="$(docker inspect -f "{{ index .Config.Labels \"${PID_KEY}\" }}" "$id" 2>/dev/null || true)"
  if [ -n "$owner" ] && kill -0 "$owner" 2>/dev/null; then
    continue
  fi
  echo "dev.sh: removing stale container $id (owner pid ${owner:-unknown})"
  docker rm -f "$id" >/dev/null 2>&1 || true
done

# 2. Start Postgres on a free loopback port.
container="agent-host-dev-$$-$(date +%s)"
docker run -d --name "$container" \
  --label "${LABEL_KEY}=${LABEL_VALUE}" --label "${PID_KEY}=$$" \
  -e POSTGRES_PASSWORD=postgres -p 127.0.0.1::5432 "$POSTGRES_IMAGE" >/dev/null
port_line="$(docker port "$container" 5432/tcp | sed -n 1p)"
pg_port="${port_line##*:}"

ready=0
i=0
while [ "$i" -lt 60 ]; do
  if docker exec "$container" pg_isready -U postgres -h 127.0.0.1 >/dev/null 2>&1; then
    ready=1
    break
  fi
  i=$((i + 1))
  sleep 1
done
if [ "$ready" -ne 1 ]; then
  echo "dev.sh: Postgres was not ready after 60 s" >&2
  exit 1
fi
# The image restarts the server once after init. Wait for a real query.
i=0
while [ "$i" -lt 30 ]; do
  if docker exec "$container" psql -U postgres -h 127.0.0.1 -tc 'select 1' >/dev/null 2>&1; then
    break
  fi
  i=$((i + 1))
  sleep 1
done

http_port="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
export AGENT_HOST_DATABASE_URL="postgres://postgres:postgres@127.0.0.1:${pg_port}/postgres"
export AGENT_HOST_HTTP_ADDR="127.0.0.1:${http_port}"
echo "dev.sh: container=$container postgres=127.0.0.1:${pg_port} http=${AGENT_HOST_HTTP_ADDR}"

cd "$rs_dir"
if [ "$mode" = "run" ]; then
  # Run in the background and wait, so bash runs the signal traps at once. A foreground child
  # would delay the trap until the child exits.
  cargo run --locked -p paigasus-agent-host &
  app_pid=$!
  rc=0
  wait "$app_pid" || rc=$?
  app_pid=""
  exit "$rc"
fi

# --check: build first, so compile time does not count against the 60 s limit.
cargo build --locked -p paigasus-agent-host
cargo run --locked -p paigasus-agent-host &
app_pid=$!
code=""
i=0
while [ "$i" -lt 60 ]; do
  if ! kill -0 "$app_pid" 2>/dev/null; then
    echo "dev.sh: the service exited before /readyz answered" >&2
    app_pid=""
    exit 1
  fi
  code="$(curl -s -o /dev/null -w '%{http_code}' "http://${AGENT_HOST_HTTP_ADDR}/readyz" 2>/dev/null || true)"
  if [ "$code" = "200" ]; then
    echo "dev.sh: /readyz returned 200"
    exit 0
  fi
  i=$((i + 1))
  sleep 1
done
echo "dev.sh: /readyz did not return 200 within 60 s (last code: ${code:-none})" >&2
exit 1
