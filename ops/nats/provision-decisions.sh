#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# Provisions the gateway tool decision log (SMA-755 § 5.10): the PAIGASUS_GATEWAY account, its three
# users, the GATEWAY_DECISIONS stream and (SMA-821) the GATEWAY_DECISIONS_GEN key-value bucket. The account
# allows 2 streams: the decision stream and the stream KV_GATEWAY_DECISIONS_GEN of the bucket use both.
# It is idempotent: run it again and it changes nothing that exists. `provision.sh` calls it for a new environment, and an operator can run it alone for an
# environment that already has the IAM account.
#
# Inputs are the same as `provision.sh`: OPERATOR, OUT_DIR, NATS_URL and PUSH. With PUSH unset, the
# script makes the account, the users and the creds files and stops. With PUSH=1 it pushes the account
# to the running broker and creates the stream and the bucket when they do not exist. It needs `nsc`, and with
# PUSH=1 also the `nats` CLI.
#
# The stream and bucket values come from `subjects.env`; edit them there, never here. The relay in
# `paigasus-gateway` makes the same stream with the same values (P-20).
#
# The creds files:
#   gateway-decision-publisher.creds   deploy with the gateway
#   gateway-decision-reader.creds      for the export CLI only; never deploy with the gateway
#   gateway-decision-provisioner.creds operator artifact; never deploy
set -euo pipefail

(( BASH_VERSINFO[0] > 4 || (BASH_VERSINFO[0] == 4 && BASH_VERSINFO[1] >= 3) )) || {
  echo "this script needs bash >= 4.3 (macOS ships 3.2 — try: brew install bash)" >&2; exit 1; }

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=/dev/null
source "$here/subjects.env"

OPERATOR="${OPERATOR:-paigasus}"
OUT_DIR="${OUT_DIR:-$here/out}"
NATS_URL="${NATS_URL:?set NATS_URL, e.g. tls://nats.internal:4222}"

# Every file below holds a secret: restrict the creation mode before the first one exists.
umask 077
mkdir -p "$OUT_DIR"

# Select the operator for every `nsc` call below. `OPERATOR` was read but never passed on, so a run used
# whichever operator was current in the nsc store. Under `set -e` an unknown operator stops the script.
nsc env --operator "$OPERATOR" >/dev/null

# --- 1. Account and limits ---------------------------------------------------------------------
nsc add account --name "$GATEWAY_ACCOUNT" 2>/dev/null || echo "account $GATEWAY_ACCOUNT already exists"

# --js-mem-storage 0: the stream is file-backed. The ordered consumer of the reader asks for memory storage of
# its state (async-nats 0.53 sets memory_storage), and the broker accepts it under a memory limit of 0.
nsc edit account --name "$GATEWAY_ACCOUNT" \
  --js-mem-storage 0 \
  --js-disk-storage "$DECISIONS_ACCOUNT_DISK_BYTES" \
  --js-streams 2 \
  --js-consumer 16

# The 10 % of the account disk that the stream leaves free must hold the generation bucket.
if (( DECISIONS_ACCOUNT_DISK_BYTES - DECISIONS_MAX_BYTES < DECISIONS_GEN_MAX_BYTES )); then
  echo "DECISIONS_ACCOUNT_DISK_BYTES=$DECISIONS_ACCOUNT_DISK_BYTES is too small: the 10 % left after the stream" \
       "($(( DECISIONS_ACCOUNT_DISK_BYTES - DECISIONS_MAX_BYTES )) bytes) cannot hold the generation bucket ($DECISIONS_GEN_MAX_BYTES bytes)" >&2
  exit 1
fi

# --- 2. Users ----------------------------------------------------------------------------------
add_user() {
  local name="$1"; shift
  local -n pub_ref="$1"; shift
  local -n sub_ref="$1"; shift
  local edit="${1:-}"

  local args=() s
  for s in "${pub_ref[@]}"; do args+=(--allow-pub "$s"); done
  for s in "${sub_ref[@]}"; do args+=(--allow-sub "$s"); done

  # No --expiry, as in `provision.sh`: a non-expiring user JWT cannot strand a running process.
  nsc add user --account "$GATEWAY_ACCOUNT" --name "$name" "${args[@]}" 2>/dev/null || echo "user $name already exists"
  # An existing user keeps its old grants: `nsc add user` fails for it. With `edit`, add the grants now, so a
  # user that exists before SMA-821 gets the generation bucket subjects. `nsc edit user` adds and is idempotent.
  if [ "$edit" = "edit" ]; then
    nsc edit user --account "$GATEWAY_ACCOUNT" --name "$name" "${args[@]}"
  fi
  nsc generate creds --account "$GATEWAY_ACCOUNT" --name "$name" > "$OUT_DIR/$name.creds"
  chmod 600 "$OUT_DIR/$name.creds"
}

add_user gateway-decision-publisher   GW_DEC_PUBLISHER_PUB   GW_DEC_PUBLISHER_SUB edit
add_user gateway-decision-reader      GW_DEC_READER_PUB      GW_DEC_READER_SUB
add_user gateway-decision-provisioner GW_DEC_PROVISIONER_PUB GW_DEC_PROVISIONER_SUB

if [ "${PUSH:-0}" != "1" ]; then
  echo "decision-log account and creds are in $OUT_DIR. Re-run with PUSH=1 to push the account and make the stream and the bucket"
  exit 0
fi

# --- 3. Push and stream ------------------------------------------------------------------------
nsc push --account "$GATEWAY_ACCOUNT"

# The stream is made only when it is absent: `stream add` on an existing stream with other values
# fails, and an edit of a live stream is an operator decision (RUNBOOK-nats.md).
prov_creds="$OUT_DIR/gateway-decision-provisioner.creds"
if nats --server "$NATS_URL" --creds "$prov_creds" --inbox-prefix "$GW_DEC_PROVISIONER_INBOX_PREFIX" stream info "$DECISIONS_STREAM_NAME" >/dev/null 2>&1; then
  echo "stream $DECISIONS_STREAM_NAME already exists; left unchanged"
else
  # This script sets no per-stream limit on the account. The relay also caps max_bytes at the account
  # storage_max_stream_bytes. If an operator adds a per-stream limit, apply the same cap here.
  nats --server "$NATS_URL" --creds "$prov_creds" --inbox-prefix "$GW_DEC_PROVISIONER_INBOX_PREFIX" \
    stream add "$DECISIONS_STREAM_NAME" \
    --subjects "$DECISIONS_SUBJECTS" \
    --storage file \
    --retention limits \
    --max-age "${DECISIONS_MAX_AGE_SECS}s" \
    --max-bytes "$DECISIONS_MAX_BYTES" \
    --discard old \
    --dupe-window "${DECISIONS_DUPE_WINDOW_SECS}s" \
    --max-msg-size "$DECISIONS_MAX_MSG_SIZE" \
    --replicas 1 \
    --defaults
fi

# The generation bucket (SMA-821). It is the stream KV_GATEWAY_DECISIONS_GEN. `--max-bucket-size` is the flag of
# `nats kv add` (checked with `nats kv add --help` in the image `natsio/nats-box:latest`, which is not pinned,
# so the CLI version is not recorded); `kv add` has no `--max-bytes`.
if nats --server "$NATS_URL" --creds "$prov_creds" --inbox-prefix "$GW_DEC_PROVISIONER_INBOX_PREFIX" kv info "$DECISIONS_GEN_BUCKET" >/dev/null 2>&1; then
  echo "bucket $DECISIONS_GEN_BUCKET already exists; left unchanged"
else
  nats --server "$NATS_URL" --creds "$prov_creds" --inbox-prefix "$GW_DEC_PROVISIONER_INBOX_PREFIX" \
    kv add "$DECISIONS_GEN_BUCKET" \
    --history 1 \
    --storage file \
    --replicas 1 \
    --max-bucket-size "$DECISIONS_GEN_MAX_BYTES"
fi

echo "provisioned. Deploy gateway-decision-publisher.creds with the gateway."
echo "KEEP gateway-decision-reader.creds and gateway-decision-provisioner.creds OUT of any gateway deployment."
