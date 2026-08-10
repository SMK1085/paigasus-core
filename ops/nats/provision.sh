#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# Provisions one environment's NATS account, users, stream and durable consumer (SMA-493).
# Run ONCE per environment, by an operator, from a machine that can reach the broker.
#
# Requires THREE tools — nsc alone cannot do this job:
#   nsc   — mints the operator, accounts, users and .creds files
#   nats  — creates the stream and the filtered durable consumer (a running-server operation)
#   a nats-server that loads the generated resolver config (see step 3 below)
#
# The subject lists come from `subjects.env`; edit them there, never here.
#
# TWO PASSES, because minting identities and getting a broker to recognize them are genuinely
# separate steps with a manual action (include the resolver config, restart the broker) in
# between: pass one (default, PUSH unset) mints the operator/account/users and generates the
# resolver config, then STOPS. Pass two (PUSH=1) pushes the account to the now-restarted broker
# and only then creates the stream and durable consumer. See the "phase split" comment below for
# why the broker-facing half cannot run on pass one.
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
  #
  # Guarded the same way the operator/account calls above are: pass one must be safely
  # re-runnable (and pass two re-runs this same code path before reaching the phase split below),
  # so a user that already exists is not a failure.
  nsc add user --account PAIGASUS_IAM --name "$name" "${args[@]}" 2>/dev/null || echo "user $name already exists"
  nsc generate creds --account PAIGASUS_IAM --name "$name" > "$OUT_DIR/$name.creds"
  chmod 600 "$OUT_DIR/$name.creds"
}

add_user iam-publisher    PUBLISHER_PUB   PUBLISHER_SUB
add_user gateway-consumer CONSUMER_PUB    CONSUMER_SUB
add_user iam-provisioner  PROVISIONER_PUB PROVISIONER_SUB

# --- 3. Resolver config -----------------------------------------------------------------------
# The broker needs this stanza to validate the account JWTs minted above. Without it, every
# service authenticates against a server that has never heard of the account.
nsc generate config --nats-resolver > "$OUT_DIR/resolver.conf"

# --- Phase split -------------------------------------------------------------------------------
# Everything below this point talks to a RUNNING broker that must already have the resolver
# config above loaded — `nsc push` and every `nats` CLI call after it authenticate against
# PAIGASUS_IAM, which the broker does not recognize until an operator has included
# $OUT_DIR/resolver.conf in its nats-server.conf and restarted it. On a first (pass-one) run that
# is never true yet, so continuing past this point would just fail against a server that has
# never heard of this account. Stopping here on pass one is deliberate — not a missing step, and
# not something to "simplify" away — it is what makes it safe to hand the operator the resolver
# config, wait for them to install it and restart the broker, and only THEN continue with
# PUSH=1.
if [ "${PUSH:-0}" != "1" ]; then
  echo "include the generated $OUT_DIR/resolver.conf in the broker's nats-server.conf, restart it, then re-run with PUSH=1"
  exit 0
fi

# --- 4. Push + stream + durable (nats CLI, against the running broker) ------------------------
# `nsc push` uploads the account itself; without it the broker still would not recognize
# PAIGASUS_IAM even with the resolver config loaded and the broker restarted.
nsc push --account PAIGASUS_IAM

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
# `--filter` is REPEATABLE (natscli's StringsVar), not comma-splitting — comma-splitting only
# happens on the interactive prompt path, which `--filter` on the command line skips entirely. A
# single comma-joined value would be accepted as ONE literal filter subject containing commas,
# which starts with `iam.` (so it passes the stream's subject-overlap check with no error) and
# then matches no real event subject ever again — the consumer looks healthy and silently
# delivers nothing. Pass one `--filter` per subject, the same pattern `add_user` above uses for
# `--allow-pub`/`--allow-sub`. Do NOT collapse this back into a comma-joined string.
filter_args=()
for s in "${CONSUMER_FILTER_SUBJECTS[@]}"; do filter_args+=(--filter "$s"); done
nats --server "$NATS_URL" --creds "$OUT_DIR/iam-provisioner.creds" \
  consumer add "$STREAM_NAME" "$DURABLE_NAME" \
  --pull "${filter_args[@]}" \
  --ack explicit \
  --deliver all \
  --max-deliver 5 \
  --defaults

echo "provisioned. Deploy iam-publisher.creds with paigasus-iam and gateway-consumer.creds with the gateway."
echo "KEEP iam-provisioner.creds OUT of any deployment — it can create and delete streams."
