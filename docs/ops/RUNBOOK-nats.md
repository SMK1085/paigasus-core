# NATS RUNBOOK — permissions, TLS and credentials (SMA-493)

Operator-facing reference for the `PAIGASUS_IAM` NATS account: the dedicated identities, subject
permissions, mandatory TLS, and credential rotation `paigasus-iam`'s outbox publisher
(`[outbox.publisher].backend = "nats"`, SMA-471) authenticates against. This document is
symptom-first — what you see, why, and what to do — written for someone already under pressure.
For the *design rationale* behind a specific grant (why the publisher needs a `subscribe` grant at
all, why a pull consumer can't be narrowed by permissions, what a JetStream domain changes), see
[`ops/nats/permissions.md`](../../ops/nats/permissions.md); for provisioning mechanics (`nsc`,
`provision.sh`, the two-pass run order), see [`ops/nats/README.md`](../../ops/nats/README.md).
This runbook does not repeat either — it points at them.

For the boot-time failure modes that have nothing to do with permissions or TLS (an unreachable
broker, a drifted `IAM_EVENTS` stream config), see `RUNBOOK-observability.md`'s ["NATS backend:
boot hard-fails…"](./RUNBOOK-observability.md#nats-backend-boot-hard-fails-on-an-unreachable-broker-or-a-drifted-stream-sma-471)
section instead.

**Read this before anything else below**: a denied publish does not look like a permissions error.
It looks like nothing at all — a slow, then failed, timeout. That single fact is why this document
exists, and §1/§2 are both variations of it.

The account holds exactly three identities — `iam-publisher` (deployed with the outbox relay),
`gateway-consumer` (SMA-492, deployed with the AI Gateway), and `iam-provisioner` (operator
tooling only, **never** deployed with a running service — see `permissions.md` §9). All three are
minted by `provision.sh` with **non-expiring** user JWTs by default: rotation stays available (the
service re-reads its `.creds` on every connection attempt, §5 below), it is simply not forced on a
schedule. An operator who does set `--expiry` when minting a user takes on monitoring the approach
of that expiry by hand — nothing in this stack alerts on it.

---

## 1. A denied publish looks like a timeout, not an error

**Symptom.** `event_outbox` rows fail to publish and retry across relay ticks;
`IamOutboxPublishFailures` (`RUNBOOK-observability.md` §4) eventually fires once a tick actually
errors, and before that, `iam_nats_publish_duration_seconds`'s p99 (§2.2 of the same doc) creeps up
toward `[outbox.publisher].publish_timeout_secs` (default `2`). Nothing on the HTTP/gRPC surface
looks unhealthy — `/healthz`/`/readyz` do not gate on NATS after boot.

**Cause.** NATS evaluates and reports a publish permission violation **asynchronously**: the
broker sends `-ERR 'Permissions Violation for Publish to "<subject>"'` on the connection itself,
not as a reply to the specific request that triggered it. `NatsEventPublisher`'s ack-await
therefore has no reply to receive, and after `publish_timeout_secs` it fails as an ordinary
timeout (`NatsPublisherError::Publish`) — nothing at that call site distinguishes "the broker
refused this" from "the broker is merely slow" (`nats_publisher.rs`'s module doc, D9). This is the
single most likely misconfiguration in the whole design, and it presents as latency, not failure.

**Remediation.**
1. Grep the service log for `nats server error` — `NatsEventPublisher::connect`'s `event_callback`
   logs every `Event::ServerError` at `error` level with the broker's verbatim text, which names
   the refused subject: `nats server error: -ERR 'Permissions Violation for Publish to
   "iam.something"'`.
2. Cross-reference `IamOutboxPublishFailures` and the affected rows' `event_outbox.last_error`
   (`RUNBOOK-observability.md` §4) to confirm timing and scope.
3. Add the refused subject to the right identity's `PUBLISHER_PUB` / `CONSUMER_PUB` /
   `PROVISIONER_PUB` list in `ops/nats/subjects.env`. Keep `ops/nats/test/accounts.conf.tmpl` in
   sync — `check-subjects.sh` fails CI the instant the two disagree — then re-mint the widened
   grant and redeliver it (`nsc edit user`, `nsc generate creds`; see [`ops/nats/README.md`
   §"Widening an existing user's grant"](../../ops/nats/README.md#widening-an-existing-users-grant)
   for the exact commands and why re-running `provision.sh` here is a silent no-op). A live
   connection carries the permission set from its original `CONNECT`, so the affected service needs
   to reconnect (restart it, or wait for its own reconnect) before the wider grant takes effect.

## 2. Every publish times out and nothing appears in the log — the inbox-prefix mismatch

**Symptom.** Identical to §1 from the outside — publishes time out, `IamOutboxPublishFailures`
fires, `iam_nats_publish_duration_seconds` creeps toward `publish_timeout_secs` — but grepping for
`nats server error` finds **nothing**. No permissions violation, no client error, no line on the
connection at all.

**Cause.** `[outbox.publisher].inbox_prefix` does not match the connecting identity's `subscribe`
grant. A JetStream publish is a request/reply under the hood: the client publishes, then waits for
the broker's `PubAck` on its own inbox. That inbox subscription is scoped by
`ConnectOptions::custom_inbox_prefix`, which `inbox_prefix` feeds directly (`config.rs`). Left
unset, async-nats falls back to its library default `_INBOX`, which matches **none** of
`subjects.env`'s three grants (`_INBOX_IAM_PUB`, `_INBOX_GW`, `_INBOX_PROV`). The design decision
this stems from (D4) states the consequence plainly: the mismatch "presents as publish timeouts
with no mention of permissions" — the ack genuinely has nowhere to land, and unlike an outright
denied publish (§1) this does not reach the `event_callback` at all.

**Remediation.**
1. Confirm `[outbox.publisher].inbox_prefix` is set and matches the deploying identity's prefix in
   `ops/nats/subjects.env` exactly — `_INBOX_IAM_PUB` for `iam-publisher`, `_INBOX_GW` for
   `gateway-consumer`, `_INBOX_PROV` for `iam-provisioner`. Do not include the trailing `.>` in the
   config value; async-nats appends the reply suffix itself.
2. Fix whichever side is wrong (config or account grant) and restart or let the service reconnect.
3. Do not "fix" this by widening the grant to a shared `_INBOX.>` — that reopens the exact
   cross-identity inbox read `permissions.md` §7 and its test
   (`neither_service_identity_can_subscribe_to_the_others_inbox`, `tests/nats_permissions.rs`)
   exist to close. Each identity keeps its own prefix; only the config's `inbox_prefix` changes.

## 3. Boot fails with `jetstream stream IAM_EVENTS could not be ensured`

**Symptom.** The process exits nonzero at boot, before any listener binds — no `/healthz` failure,
no metrics scrape, `CrashLoopBackOff` under an orchestrator (see `RUNBOOK-observability.md`'s "NATS
backend: boot hard-fails…" section for the general shape of this). The log's final line names the
stream: `jetstream stream IAM_EVENTS could not be ensured` (`NatsPublisherError::Ensure`).

**Cause.** `NatsEventPublisher::connect` establishes the stream by probing
`$JS.API.STREAM.INFO.IAM_EVENTS` first and falling through to `$JS.API.STREAM.CREATE.IAM_EVENTS`
only on a 404 (`get_or_create_stream`). If the connecting identity's `publish` grant is missing the
`STREAM.INFO` subject, the broker refuses the probe outright as a permissions violation instead of
answering with a 404 — `get_or_create_stream` never reaches its create path at all, so a missing
`STREAM.INFO` grant and a missing `STREAM.CREATE` grant produce the identical `Ensure` error.

**Remediation.** Confirm the connecting identity's `publish` grant carries **both**
`$JS.API.STREAM.INFO.IAM_EVENTS` and `$JS.API.STREAM.CREATE.IAM_EVENTS` — for `iam-publisher` this
is `PUBLISHER_PUB` in `ops/nats/subjects.env`, and both are required together, since the INFO probe
is what has to succeed before the CREATE fallback is ever reached. Fix `subjects.env`, keep the
test fixture in sync, re-provision, and restart. This is never transient: restarting the pod
without changing the grant reproduces the identical failure every time.

## 4. Boot fails naming the credentials file

**Symptom.** Same boot-time, no-listener failure as §3, but the log names the credentials file:
`nats credentials file <path> could not be loaded` or `nats credentials file <path> could not be
parsed`.

**Cause.** A valid credential file has two labelled, properly-terminated blocks — a
`-----BEGIN NATS USER JWT-----` … `------END NATS USER JWT------` block and a
`-----BEGIN USER NKEY SEED-----` … `------END USER NKEY SEED------` block — exactly what `nsc
generate creds` produces. The loader also accepts a bare seed-only file: just the `USER NKEY SEED`
block, no JWT block, for nkey-only authentication (`creds.rs`'s module doc). Two deliberately
distinct error variants (`nats_publisher.rs`), split because "the file is missing" and "the file is
not what you think it is" have different remediations:

- `NatsPublisherError::Credentials` — the file could not be **read**: absent, wrong permissions, an
  unmounted volume. Wraps the underlying `io::Error`.
- `NatsPublisherError::CredentialsParse` — the file **was** read but is not a valid `.creds`:
  neither `-----BEGIN USER NKEY SEED-----` was found (`CredsError::MissingSeed`) nor, if a JWT
  block was opened, its matching `END NATS USER JWT` marker (`CredsError::MissingJwt`), or the seed
  present doesn't parse as a valid nkey (`CredsError::BadSeed`). This loader is deliberately
  stricter than async-nats' own parser — it keys on the block **labels**, not "whichever two dashed
  blocks appear first" — so a reordered or mislabelled file fails loudly instead of being silently
  misread as something else (`creds.rs`'s module doc).

**Remediation.** For `Credentials`, check the mount first — path exists, permissions, the volume
actually attached — before assuming the credential itself is bad. For `CredentialsParse`, open the
file and confirm both blocks are present and each properly closed; `nsc generate creds` always
produces this exact two-block shape, so a hand-edited or partially-copied file is the usual cause.
Both are re-read fresh on every connection attempt (D8), so once the file is fixed, the next
connection attempt (or a restart) picks it up with no code change.

## 5. Reconnects fail right after a credential rotation

**Symptom.** A previously-healthy connection stops reconnecting cleanly immediately after a
`.creds` file is rotated: `iam_nats_connected` (`RUNBOOK-observability.md` §2.2) drops and stays at
`0`, and the log carries either `auth callback failed` (logged directly by async-nats, wrapping the
same `Credentials`/`CredentialsParse`-shaped text as §4 — "...could not be read: ..." or "...is
malformed: ...") or a broker-side authorization refusal.

**Cause.** `NatsEventPublisher::connect` installs an auth **callback** rather than
`ConnectOptions::with_credentials_file`, specifically so the credential is re-read from disk on
**every** connection attempt rather than cached from the first — that is what makes rotation
possible without a restart (D8). It cuts both ways: a reconnect failure right after a rotation
means the *new* file is the problem, not the old one. Two distinct causes produce the two log
shapes above: the new file itself is malformed (the callback fails before the broker ever
evaluates the credential — see §4's two variants), or the file parses fine but its user has been
removed from `PAIGASUS_IAM` (the JWT is well-formed, but the account no longer recognizes it, and
the broker refuses the authentication).

**Remediation.** Roll back to the previous, known-good `.creds` file — no restart needed either
way, since the very next connection attempt re-reads whatever is on disk. Then re-mint the intended
credential correctly (`nsc generate creds`) and confirm its user still exists in the account before
redeploying it. **How the file is delivered matters as much as its content**: replace it via
atomic replacement — a Kubernetes `Secret` volume's symlink swap, or a hand-rolled temp-file +
`rename(2)` over the target path — never a truncate-and-rewrite in place, which opens a window
where a reconnect racing the rewrite reads a partially-written, unparseable file and fails for a
reason that has nothing to do with the credential's actual validity (`permissions.md` §8).

## 6. TLS handshake fails after the broker's certificate changes

**Symptom.** Every connection attempt fails with a bare TLS/certificate-verification error — no
permissions violation, no authentication error, just a handshake failure — starting right after the
broker's certificate (or its issuing CA) changed.

**Cause.** `[outbox.publisher].root_ca_bundle` **replaces** the system trust store; it does not
extend it. Once any bundle is configured, async-nats assigns exactly the certificates named in that
file instead of also loading the OS trust store (`config.rs`'s field doc; `permissions.md` §6). A
deployment that names only its private CA and then moves the broker behind a publicly-trusted
certificate — or the reverse — loses **all** trust it wasn't already holding in that one file. This
is a total outage, not a partial one, and the bare handshake failure it presents as points nowhere
near the actual cause.

**Remediation.** If the broker now presents a certificate from a CA not already in the bundle,
concatenate the new CA into that same file — there is no way to layer multiple `root_ca_bundle`
values; one file must carry every CA the client needs to trust. If the broker moved to a
publicly-trusted certificate, unset the field entirely to fall back to the system trust store. The
bundle is re-read on every connection attempt, exactly like the credential (D7), so once the file
is fixed no restart is needed.

## 7. Publishes fail with `insufficient resources`

**Symptom.** Publishes start failing broker-side with `insufficient resources` — distinct from a
permissions violation or a timeout: the connection is healthy and the subject is permitted, but
JetStream itself refuses the write.

**Cause.** The `PAIGASUS_IAM` account's JetStream disk-storage limit — set by `provision.sh`'s
`nsc edit account --js-disk-storage` (10 GiB / `10737418240` bytes by default) — has been reached.
This is an **account-level** ceiling, independent of `IAM_EVENTS`'s own `max_age_secs`: the stream
can be well within its own configured retention window and still hit the account limit if
`max_age_secs` is generous (or `0`, unlimited) relative to what the account was provisioned with.

**Remediation.** Either raise the account's JetStream limit (`nsc edit account --js-disk-storage
<bytes>`, then `nsc push`) or shorten `[outbox.publisher].max_age_secs` so the stream ages out
messages before it can grow to fill the account limit. Do not run with `max_age_secs = 0` unless
the account limit is generous enough to absorb genuinely unbounded growth — `NatsEventPublisher::
connect` already warns at startup when `max_age_secs = 0` for exactly this reason.

## 8. A JetStream domain reshapes every `$JS.*` grant

**Symptom.** Not a live incident on its own — a planning note for the day someone introduces a
JetStream domain (for cross-account or leaf-node bridging). The moment a domain is turned on, every
existing `$JS.*` grant across all three identities silently stops matching, and every publish, ack,
and pull starts failing permission checks simultaneously.

**Cause.** Today's deployment sets no `domain` anywhere (`permissions.md` §5). Introducing one
moves the JetStream API prefix from `$JS.API.…` to **`$JS.<domain>.API.…`** — so, for example,
`$JS.API.STREAM.INFO.IAM_EVENTS` becomes `$JS.<domain>.API.STREAM.INFO.IAM_EVENTS` — and moves the
ack-publish subject from `$JS.ACK.<stream>.<consumer>.>` to **`$JS.ACK.<domain>.<account-hash>.
<stream>.<consumer>.>`**. Every `$JS.API.*` grant and `gateway-consumer`'s `$JS.ACK.*` grant in
`ops/nats/subjects.env` needs the new segments spliced in.

**Remediation.** Do not pre-emptively widen these grants "just in case" — an unused wildcard
segment loosens every grant beyond what the undomained deployment needs today. When a domain is
actually introduced, rewrite `subjects.env` and `test/accounts.conf.tmpl` together, in the same
commit that turns the domain on, and let `check-subjects.sh` confirm the two didn't drift apart.
See `ops/nats/permissions.md` §5 for the exact widened forms.
