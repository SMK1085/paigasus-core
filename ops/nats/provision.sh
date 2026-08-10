#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# Provisions one environment's NATS account, users, stream and durable consumer (SMA-493).
# Run ONCE per environment, by an operator, from a machine that can reach the broker.
#
# Requires THREE tools — nsc alone cannot do this job:
#   nsc   — mints the operator, accounts, users and .creds files
#   nats  — creates the stream and the filtered durable consumer (a running-server operation)
#   a nats-server that loads the generated resolver config (see step 4 below)
#
# The subject lists come from `subjects.env`; edit them there, never here.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=/dev/null
source "$here/subjects.env"

OPERATOR="${OPERATOR:-paigasus}"
OUT_DIR="${OUT_DIR:-$here/out}"
NATS_URL="${NATS_URL:?set NATS_URL, e.g. tls://nats.internal:4222}"

mkdir -p "$OUT_DIR"

# --- 1. Operator + accounts -----------------------------------------------------------------
nsc add operator --name "$OPERATOR" --sys 2>/dev/null || echo "operator $OPERATOR already exists"
nsc add account --name PAIGASUS_IAM 2>/dev/null || echo "account PAIGASUS_IAM already exists"

# Account-level JetStream limits (SMA-493 D1). --js-mem-storage 0 disables memory storage: the
# stream is File-backed (SMA-471 D8) and a memory stream loses everything on a broker restart.
nsc edit account --name PAIGASUS_IAM \
  --js-mem-storage 0 \
  --js-disk-storage 10737418240 \
  --js-streams 4 \
  --js-consumer 32

# --- 2. Users -------------------------------------------------------------------------------
# `nsc add user` takes repeated --allow-pub / --allow-sub flags; the arrays are expanded one
# subject per flag so the lists stay declarative in subjects.env.
add_user() {
  local name="$1"; shift
  local -n pub_ref="$1"; shift
  local -n sub_ref="$1"; shift

  local args=()
  for s in "${pub_ref[@]}"; do args+=(--allow-pub "$s"); done
  for s in "${sub_ref[@]}"; do args+=(--allow-sub "$s"); done

  # No --expiry: a non-expiring user JWT cannot strand a long-running process on a reconnect
  # (SMA-493 §3.1). Rotation stays available — the service re-reads its .creds on every
  # connection attempt (D8) — it is simply not forced on a schedule. An operator who DOES set an
  # expiry takes on monitoring it; nothing here alerts on approaching expiry.
  nsc add user --account PAIGASUS_IAM --name "$name" "${args[@]}"
  nsc generate creds --account PAIGASUS_IAM --name "$name" > "$OUT_DIR/$name.creds"
  chmod 600 "$OUT_DIR/$name.creds"
}

add_user iam-publisher    PUBLISHER_PUB   PUBLISHER_SUB
add_user gateway-consumer CONSUMER_PUB    CONSUMER_SUB
add_user iam-provisioner  PROVISIONER_PUB PROVISIONER_SUB

# --- 3. Resolver config + push --------------------------------------------------------------
# The broker needs this stanza to validate the account JWTs minted above; `nsc push` uploads the
# account itself. Without both, every service authenticates against a server that has never heard
# of the account.
nsc generate config --nats-resolver > "$OUT_DIR/resolver.conf"
echo "include the generated $OUT_DIR/resolver.conf in the broker's nats-server.conf, restart it, then re-run with PUSH=1"
if [ "${PUSH:-0}" = "1" ]; then
  nsc push --account PAIGASUS_IAM
fi

# --- 4. Stream + durable (nats CLI, against the running broker) -----------------------------
# The stream config MUST match `PublisherConfig`'s defaults: SMA-471 D7 fails the service's boot
# when an adopted stream is weaker than configured, and retention/storage/duplicate_window are
# NOT editable in place — fixing drift means deleting the stream, i.e. a maintenance window.
# See rs/crates/services/paigasus-iam/src/config.rs (`impl Default for PublisherConfig`).
nats --server "$NATS_URL" --creds "$OUT_DIR/iam-provisioner.creds" \
  stream add "$STREAM_NAME" \
  --subjects "iam.>" \
  --storage file \
  --retention limits \
  --dupe-window 1h \
  --max-age 7d \
  --replicas 1 \
  --discard old \
  --max-msgs=-1 --max-bytes=-1 --max-msg-size=-1 --max-msgs-per-subject=-1 \
  --no-allow-rollup --no-deny-delete --no-deny-purge --defaults

# The durable carries the subject filter, because a pull consumer's permissions cannot (D5).
filter_csv=$(IFS=,; echo "${CONSUMER_FILTER_SUBJECTS[*]}")
nats --server "$NATS_URL" --creds "$OUT_DIR/iam-provisioner.creds" \
  consumer add "$STREAM_NAME" "$DURABLE_NAME" \
  --pull \
  --filter "$filter_csv" \
  --ack explicit \
  --deliver all \
  --max-deliver 5 \
  --defaults

echo "provisioned. Deploy iam-publisher.creds with paigasus-iam and gateway-consumer.creds with the gateway."
echo "KEEP iam-provisioner.creds OUT of any deployment — it can create and delete streams."
