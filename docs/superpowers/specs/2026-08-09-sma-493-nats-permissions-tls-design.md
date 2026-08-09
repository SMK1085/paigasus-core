# SMA-493 — NATS account isolation, subject permissions, TLS, and credential rotation

**Status:** design
**Date:** 2026-08-09
**Issue:** [SMA-493](https://linear.app/smaschek/issue/SMA-493/ops-nats-subject-permissions-and-tls-before-enabling-the-iam-nats)
**Project:** Paigasus IAM — milestone *Hardening*
**Follows:** [SMA-471](https://linear.app/smaschek/issue/SMA-471/iam-implement-a-real-broker-eventpublisher-only-the-tracing-publisher) (merged, PR #112, `dc2b351`) — this is the ops half that PR's §5 and §8 deferred
**Blocks:** [SMA-492](https://linear.app/smaschek/issue/SMA-492) (gateway consumes IAM events to invalidate caches)
**ADR:** ADR-0016 (Notion) — Consequences updated by this work

## 1. Problem

SMA-471 shipped `NatsEventPublisher`, the production `EventPublisher`. It is off everywhere:
`[outbox.publisher].backend` defaults to `tracing` (D12). Turning it on is a one-line config
flip — and today that flip lands an IAM service's authorization-change feed on a broker with no
account boundary, no subject permissions, and no transport encryption, because nothing in the
service requires any of them.

### 1.1 The exposure is the metadata, not the payload

SMA-471 §5 audited every payload and found them free of secrets and PII — principal ids and
kinds, api-key ids and *display* prefixes, grant ids and role keys, policy identifiers — with a
regression test in `cloud_event.rs` holding that line.

That audit is not the interesting half. The subject is `aggregate_prn` (D5) and the envelope
carries an `actorprn` extension attribute, so a subscriber that can read `iam.>` observes, in
real time, **who granted whom which role on which org or project, and who revoked what**. That
is the platform's authorization change graph, reconstructable without decoding a single payload.

There is no application-level mitigation. The publisher cannot know who is subscribed. On a
shared NATS account, every co-tenant is a subscriber.

### 1.2 What the service does not currently make possible

Three of the four requirements are not merely undocumented — they are **unreachable from the
current config surface**:

| Requirement | State today |
| -- | -- |
| Dedicated account, subject permissions | Purely an ops artifact; nothing exists to deploy or review |
| TLS | `url` accepts `tls://`, but with no way to name a CA the client falls back to the **system** trust store (`async-nats` `tls.rs:62`) — a broker with a private-CA cert is undialable without rebuilding the image's trust bundle |
| `.creds` over url-embedded credentials | `credentials_file` exists but nothing *requires* it; `nats://user:pass@host` remains legal and the redaction that hides it in `Debug`/`Serialize` reads as an endorsement |
| Credential rotation | Confirmed broken — see below |

### 1.3 The rotation question, answered

SMA-471 §7 flagged "whether `.creds` is re-read on reconnect" as an open item. It is not.

`ConnectOptions::with_credentials_file` (`options.rs:429`) reads the file once, parses it, and
caches the JWT **string** plus the `KeyPair` in `options.auth`. Every reconnect rebuilds its
`CONNECT` from that cached `auth.jwt` (`connector.rs:666`). A rotated file on disk is never seen.
NATS user JWTs can carry an expiry; when the cached one lapses, every reconnect fails
`AuthorizationViolation` and the process cannot recover without a restart — precisely the
"outage that outlives the credential" the issue predicted.

There is a rotation-safe path in the same crate: `auth_callback` is invoked on **every**
connection attempt (`connector.rs:681`), inside the same `try_connect_to` that rebuilds the TLS
config (`connector.rs:500,543`) — so a callback that re-reads the file, and a CA bundle named by
path, both pick up rotation without a restart.

## 2. Decisions

### D1 — One dedicated account, `PAIGASUS_IAM`, holding both users

A dedicated NATS account for IAM events, not a shared one. This is the other half of SMA-471 D5,
which rejected subject prefixing for multi-deployment separation on the grounds that accounts are
the right mechanism; that argument only holds once an account actually exists.

Both identities — the IAM publisher and the gateway consumer — live **inside** that one account.
The alternative, giving the gateway its own account, requires JetStream service exports/imports
for the consumer API and a stream deliver-subject export: substantial complexity for no isolation
gain, because the boundary that matters is *IAM events vs. every other tenant of the broker*, not
IAM vs. gateway. Within the account, separation is by user permissions (D3, D5), which is
sufficient and reviewable.

`SYS` stays the system account and holds no IAM traffic.

### D2 — Operator/nsc JWTs are the production target; a static-accounts config is the CI vehicle

Production uses decentralized auth: an operator, the `PAIGASUS_IAM` account, and per-service user
JWTs distributed as `.creds` files. That is what `credentials_file` already supports, what makes
rotation meaningful at all (D8), and what carries per-account JetStream limits.

nsc-minted artifacts cannot be committed — a `.creds` file *is* a private key. So the executable
proof runs against a **static-accounts `nats-server.conf`** carrying the identical subject lists
in NATS's classic config encoding, authenticated with user/password.

This is a proxy, and the spec states it plainly rather than implying more: what the test proves is
that **this subject list is exactly sufficient and no broader** for the JetStream API calls the
adapter makes. The list is the transferable artifact; the encoding differs between the two modes,
and `provision.sh` is reviewed, not executed, in CI (§6, residual risk).

### D3 — The publisher's permission set, including the ack inbox

```
pub    iam.>
       $JS.API.STREAM.INFO.IAM_EVENTS       # get_or_create_stream probes INFO first
       $JS.API.STREAM.CREATE.IAM_EVENTS     # ...and CREATEs on a 404 (context.rs:770-774)
sub    _INBOX_IAM_PUB.>                     # JetStream ack replies land here
```

The non-obvious grant is the last one. A "write-only" publisher is not literally write-only:
`send_publish` is a request whose persistence ack returns on the client's inbox, so without a
`subscribe` grant **every publish times out**. The service reads that as a publish failure, parks
rows, and reports nothing about permissions.

Equally deliberate is what is absent. No `subscribe iam.>` — the publisher cannot read the graph
it writes. No `STREAM.UPDATE`, `STREAM.DELETE`, or `STREAM.PURGE` — SMA-471 D7 made
non-reconciliation a deliberate property ("this service must never silently reshape a stream
external consumers depend on"); denying the verbs at the broker is that decision enforced rather
than merely intended. `STREAM.CREATE` is needed only on first boot but must be granted, since
that boot is where the stream comes from.

### D4 — Per-user inbox prefixes, because `_INBOX.>` leaks across the account

Each user gets a distinct inbox prefix (`_INBOX_IAM_PUB`, `_INBOX_GW`) via
`ConnectOptions::custom_inbox_prefix`, and its `subscribe` grant is scoped to that prefix.

This is isolation, not tidiness. Inside a shared account, a publisher holding `sub _INBOX.>`
could subscribe to **the gateway's** inbox and read every message the gateway pulls — recovering
exactly the firehose D3 denied it. Per-user prefixes close that, and they are the only way to,
because inbox replies are the one subject space every client must be able to read.

The cost is a coupling: the prefix in the config and the prefix in the account's grant must
match, and a mismatch presents as publish timeouts with no mention of permissions. It gets an
explicit callout in the docs, a matched pair in every committed artifact, and a test.

### D5 — A consumer cannot be narrowed by `subscribe` permissions; the durable does it

The instinct — "give the gateway `subscribe` on only the subjects it needs" — does not work.
JetStream pull-consumer messages are delivered to the client's **inbox**, never to `iam.*`, so
subject-level subscribe permissions are simply not in the path. Granting `sub iam.role.*` narrows
nothing; granting `sub iam.>` hands over the firehose.

The filter lives in the durable consumer's config, so least-privilege has to be built the other
way round:

```
durable gateway-cache-invalidator          # provisioned by ops, NOT self-created
  filter_subjects: iam.role.granted, iam.role.revoked, iam.api_key.revoked,
                   iam.principal.archived, iam.policy.put, iam.policy.deleted

user gateway-consumer
  pub  $JS.API.CONSUMER.MSG.NEXT.IAM_EVENTS.gateway-cache-invalidator
       $JS.API.CONSUMER.INFO.IAM_EVENTS.gateway-cache-invalidator
       $JS.ACK.IAM_EVENTS.gateway-cache-invalidator.>
  sub  _INBOX_GW.>
  ✗    $JS.API.CONSUMER.CREATE.*            # cannot widen its own filter_subjects
  ✗    subscribe iam.>                      # cannot bypass JetStream via core NATS
  ✗    publish iam.*                        # cannot forge events
```

Denying `CONSUMER.CREATE` is what makes the filter binding: a compromised gateway can pull only
what a pre-provisioned, subject-filtered durable hands it. Denying `subscribe iam.>` closes the
other route to the same data.

That denial has a corollary: **someone else must create the durable.** Neither service identity
can — the publisher has no `CONSUMER.*` verbs and the consumer is denied `CREATE` by design. So
the account also holds a third identity, `iam-provisioner` (`pub $JS.API.>`, `sub
_INBOX_PROV.>`), used by ops tooling and by the test fixture, and by nothing at runtime. Its
credentials are an operator artifact, not a deployed one — the two service identities stay
least-privilege precisely because the privileged one is not deployed anywhere.

Two subject-format notes the artifacts and runbook must carry:

- **`$JS.ACK` gains tokens under a JetStream domain.** With no domain the reply subject is
  `$JS.ACK.<stream>.<consumer>.…`; configure a domain and it becomes
  `$JS.ACK.<domain>.<account-hash>.<stream>.<consumer>.…`, so the grant must widen to
  `$JS.ACK.*.*.IAM_EVENTS.gateway-cache-invalidator.>`. The tests pin the no-domain form.
- **The exact API subject list is pinned by the test, not by this document.** If
  `get_consumer_from_stream` turns out to also require `$JS.API.STREAM.INFO.IAM_EVENTS`, or the
  context requires `$JS.API.INFO`, those are read-only additions and get added — with the test as
  the authority on what is genuinely needed.

The six filtered subjects are this design's reading of what cache invalidation requires:
revocations and grants change authz outcomes. `iam.principal.created` and `iam.api_key.issued`
are excluded — nothing is cached about a principal or key that does not exist yet. SMA-492 owns
narrowing this further, and because the filter lives in the provisioned durable, it changes
without touching either service.

### D6 — TLS and credential posture are enforced in `validate()`, behind one escape hatch

`IamConfig::validate` gains three rules, all gated on `backend = "nats"`:

1. `url` must have scheme `tls://`.
2. `url` must carry no userinfo (`nats://user:pass@host`) — **rejected unconditionally, no
   opt-out**. `credentials_file` is the credential channel; the `Debug`/`Serialize` redaction
   SMA-471 added is a mitigation for accidental logging, not a licence.
3. `credentials_file` must be present.

`allow_plaintext = true` (default `false`) relaxes (1) and (3). One flag rather than two, because
"this is a dev/CI broker" is one fact, and two independently-settable flags is a way to end up
with TLS on and authentication off.

This follows the service's existing posture — `NatsEventPublisher::connect` already hard-fails
boot on an unreachable broker or a drifted stream — and makes every insecure deployment exactly
one greppable line. Cost: `dev-setup.md` and the twelve existing integration tests each gain that
line.

### D7 — A `root_certificates` config field, because otherwise TLS is unreachable

`async-nats` builds its root store from `rustls-native-certs` when no CA is named (`tls.rs:62`),
so with today's config a broker presenting a private-CA certificate cannot be dialled at all
without rebuilding the container's trust bundle. `root_certificates` (optional path to a PEM
bundle) is wired to `ConnectOptions::add_root_certificates`; omitted, behaviour is unchanged.

Because `config_tls` runs per connection attempt (`connector.rs:500,543`), a rotated CA bundle is
picked up on reconnect with no restart — the same property D8 buys for credentials, for free.

### D8 — Rotation via `with_auth_callback`, with an eager pre-flight read

`with_credentials_file` is replaced by `with_auth_callback`, invoked on every connection attempt
(`connector.rs:681`). The callback re-reads and re-parses the `.creds`, signs the server nonce
with `nkeys::KeyPair::sign`, and returns **raw** signature bytes — `async-nats` base64url-encodes
them itself (`connector.rs:691-697`).

Swapping the call naïvely would discard something SMA-471 built on purpose.
`NatsPublisherError::Credentials { path, source }` exists because a bare `io::Error` naming
neither NATS nor the path "is not an actionable boot error"; inside a callback that error is
flattened into a generic `Authentication` `ConnectError`. So `connect()` keeps an **eager
pre-flight read and parse** — a missing, unreadable, or malformed file still fails boot with the
path named — and only then installs the callback for every attempt thereafter.

A file that goes bad mid-life fails the reconnect. Our own callback logs an error naming the
path (async-nats' generic error would not), and the failure lands in the story SMA-471 already
built: `iam_nats_connected → 0`, breaker opens, rows park, `IamOutboxPublishFailures` fires.

New direct dependency: `nkeys` 0.4.5 — already in the lock tree via `async-nats`, so no build
weight, but it needs a `deny.toml` review. The two-block `.creds` parse is hand-rolled (~20
lines); `async-nats`' own is `pub(crate)` and regex-backed, and a regex dependency is not worth
adding for two delimited blocks.

### D9 — Surface server errors through `event_callback`

A denied publish is not an error the client sees — the server replies `-ERR 'Permissions
Violation for Publish to …'` asynchronously and the request simply **times out**. Without
wiring, the single most likely misconfiguration in this whole design presents as
`publish_timeout_secs` expiry with no stated cause.

`ConnectOptions::event_callback` is wired to log `Event::ServerError` / `Event::ClientError` at
`ERROR` and connection transitions at `INFO`/`WARN`. `ServerError::Other(String)` carries the
violation text verbatim, so the offending subject appears in the log.

The `iam_nats_connected` gauge keeps its 5-second sampler (SMA-471); this adds diagnosis, and
does not churn a metric that already works.

### D10 — mTLS is out of scope; no drift gate over `provision.sh`

`add_client_certificate` exists, but `.creds` is the authentication mechanism and a second one
earns nothing here. Recorded so a reviewer sees it was weighed.

A gate diffing `provision.sh`'s nsc subject lists against the test config was considered and
deferred: it needs the script restructured into greppable arrays plus a comparison script, which
is real complexity for a file a human reviews. The residual risk is stated in §6.

## 3. The fix

### 3.1 `ops/nats/` (new)

| File | Contents |
| -- | -- |
| `README.md` | What this directory is, how to provision an environment, the artifact index |
| `permissions.md` | The canonical subject lists (D3, D5) in both encodings, carrying the ack-inbox and pull-consumer gotchas and the `$JS.ACK`-under-a-domain note |
| `provision.sh` | nsc: operator → `SYS` + `PAIGASUS_IAM` (explicit JetStream limits) → `iam-publisher`, `gateway-consumer`, `iam-provisioner` users → the `IAM_EVENTS` stream → the filtered `gateway-cache-invalidator` durable → `.creds` output paths. Run once per environment; documented as such |
| `test/accounts.conf` | The **authoritative** static-mode account/user/permission block — what §4.3 executes. `permissions.md` documents; this file is what is proven |
| `test/nats-server.conf` | Port + JetStream + `include "accounts.conf"` |
| `test/nats-server-tls.conf` | The same plus a `tls { cert_file, key_file }` block |

Splitting `accounts.conf` out and `include`-ing it from both server configs means the plaintext
and TLS fixtures cannot drift apart in their permission lists.

**`provision.sh` creates the stream, and its config must match what the service requires.**
Because the durable attaches to `IAM_EVENTS`, provisioning cannot wait for the service's own
first-boot `get_or_create_stream`. That makes the script's stream config load-bearing: SMA-471
D7 fails boot when an adopted stream's `retention`, `storage`, `duplicate_window`, `subjects`,
or `max_age` is weaker than configured, so a provisioning script that creates a *drifted* stream
produces a crash-looping service — and `retention`, `storage` and `duplicate_window` are not
editable in place, making the fix a delete-and-recreate maintenance window. The script therefore
carries the same five values as `PublisherConfig`'s defaults, with a comment pointing at both
`config.rs` and the D7 verification, and `permissions.md` states the coupling. The publisher
keeps its `STREAM.CREATE` grant for the un-provisioned case (a fresh dev environment), where it
creates the stream correctly by construction.

### 3.2 `rs/crates/services/paigasus-iam/src/config.rs`

Three fields on `PublisherConfig`, all optional, all inert under the default `tracing` backend:

```toml
[outbox.publisher]
backend           = "nats"
url               = "tls://nats.internal:4222"
credentials_file  = "/etc/paigasus/iam.creds"
root_certificates = "/etc/paigasus/nats-ca.pem"   # NEW — omit for the system trust store
inbox_prefix      = "_INBOX_IAM_PUB"              # NEW — must match the account's subscribe grant
allow_plaintext   = false                         # NEW — the single dev/CI escape hatch
```

`inbox_prefix` defaults to `None`, meaning `async-nats`' own `_INBOX` — so a deployment that has
not adopted these artifacts keeps working. `Debug`/`Serialize` stay hand-rolled; the three new
fields are paths and a bool, so none needs redaction, and the existing `url` redaction is
untouched. `validate()` gains D6's three rules.

### 3.3 `rs/crates/services/paigasus-iam/src/adapters/events/`

- `creds.rs` (new): `parse_creds(&str) -> Result<(String, KeyPair), CredsError>` for the two
  decorated blocks, and `auth_from_creds(path, nonce) -> Result<Auth, AuthError>` — the callback
  body as a named, directly testable unit.
- `nats_publisher.rs`: `connect` does the D8 pre-flight, installs `with_auth_callback`,
  `custom_inbox_prefix` (when configured), `add_root_certificates` (when configured), and D9's
  `event_callback`. New error variant for a `.creds` that reads but does not parse — today that
  is an opaque `InvalidData` `io::Error` from inside `async-nats`.

### 3.4 `rs/Cargo.toml` + `rs/crates/services/paigasus-iam/Cargo.toml`

`nkeys` 0.4.5 as a workspace dependency (a direct dep of the IAM service); `rcgen` as a
**dev**-dependency for §4.4's test certificates. Both need a `deny.toml` licence check.

### 3.5 `moon.yml`

A `repo:nats-permissions` task in the `observability-drift` shape. `ops/` has no `moon.yml` and
belongs to the root `repo` project, so an ops-only edit never marks the crate affected — without
this task the permission test would skip on exactly the PRs that break it.

```yaml
  nats-permissions:
    script: '( cd rs && cargo nextest run --no-tests=pass -p paigasus-iam --test nats_permissions )'
    toolchain: 'system'
    inputs:
      - 'ops/nats/**/*'
      - 'rs/crates/services/paigasus-iam/tests/nats_permissions.rs'
      - 'rs/crates/services/paigasus-iam/src/adapters/events/**/*'
      - 'rs/crates/services/paigasus-iam/src/config.rs'
```

## 4. Tests

### 4.1 Config — unit, in `config.rs`

Plaintext `url` rejected; plaintext + `allow_plaintext` accepted; **userinfo rejected even with
`allow_plaintext`**; missing `credentials_file` rejected unless `allow_plaintext`; every rule
inert under `backend = "tracing"`; the rendered messages name the offending field.

### 4.2 Credentials — unit, in `creds.rs`

Re-reads per call (write creds A → call → overwrite with B → call → different JWT: the property
D8 exists for); the signature verifies against the seed's public key via `KeyPair::verify`;
truncated / single-block / empty files give actionable errors naming the path. Seeds are minted
in-test with `nkeys::KeyPair::new_user()` — nothing is committed.

### 4.3 Permissions — integration, `tests/nats_permissions.rs` (new)

Boots `nats:2.10.14` with the committed `ops/nats/test/` configs copied in via
`ImageExt::with_copy_to` (testcontainers 0.27, `image_ext.rs:144`) and `with_cmd(["-c", …])`.
Drives a real `async_nats` JetStream client as each user:

| As | Must succeed | Must be denied |
| -- | -- | -- |
| `iam-publisher` | `get_or_create_stream`; `send_publish` + ack for **every** `EventType::as_wire()` | `subscribe iam.>`, `STREAM.DELETE`, `STREAM.PURGE` |
| `gateway-consumer` | pull from the provisioned durable, ack | `CONSUMER.CREATE`, `subscribe iam.>`, `publish iam.*` |

The denial half is the half that earns its keep: sufficiency is the easy property, and
over-breadth is what rots. Iterating every `EventType` means a ninth event type cannot be added
without this test having an opinion.

Same Docker gating as the existing suites: hard failure in CI, skip on a Docker-less laptop.

### 4.4 TLS — integration

`rcgen` mints a CA and a server certificate with an IP SAN for `127.0.0.1` (the tests dial a
mapped host port) into a temp dir; both are copied into the container alongside
`nats-server-tls.conf`. The adapter then connects with `url = "tls://…"` and
`root_certificates = <temp CA>` and publishes. No committed keys, no secret-scanner noise, and
D7's field is proven rather than assumed. A control case — the same broker without
`root_certificates` — must fail to verify.

### 4.5 Rotation across a real reconnect — integration

A no-auth JetStream broker plus a validly-shaped `.creds`: connect, publish, restart the
container (the pattern `dedup_survives_a_broker_restart` already uses), publish again. Proves the
callback path survives a genuine reconnect rather than only working on the first dial. The
server-enforced half — a JWT that actually expires — is a stated gap (§6).

### 4.6 Existing suites

The twelve tests in `tests/nats_publisher.rs` construct a `PublisherConfig` with a plaintext URL
and no credentials, so each gains `allow_plaintext: true`. That is not overhead: it means the
escape hatch is exercised on every run, and a change that broke it would red the whole suite.

### 4.7 Full gate

```
moon ci :build :test :lint :fmt :deny :machete :typecheck :breaking :affected-smoke \
  :parity-corpus-drift :wasm-getrandom-free :promtool :observability-drift :nats-permissions \
  :release-parity :release-parity-py :release-parity-ts \
  --base origin/main --include-relations
```

`:deny` covers the two new dependencies; `:wasm-getrandom-free` still matters — `nkeys` must not
reach the wasm binding's tree (it is an IAM-service dep, so it should not, and the gate says so).

## 5. Documentation

- **`ops/nats/README.md` + `permissions.md`** — the artifacts above, written so an operator can
  provision an environment without reading Rust.
- **`docs/ops/RUNBOOK-nats.md`** (new) — permission-violation symptoms (D9's log line; a denied
  publish looks like a timeout), expired/rotated credentials, a stream INFO denial presenting as
  an `Ensure` failure rather than a 404, and the inbox-prefix mismatch. Cross-linked from the
  "NATS backend: boot hard-fails…" section of `RUNBOOK-observability.md`, which keeps its own
  scope.
- **`docs/dev-setup.md`** — `allow_plaintext = true` in the local snippet, with one line saying
  why it is required.
- **ADR-0016** (Notion) — Consequences updated: the account model (D1), the rotation question
  resolved from open to answered (§1.3, D8), and subject permissions moved from "a deployment
  requirement" to "provisioned and tested here".
- **Rustdoc** in house style on `creds.rs` and the changed `connect`, carrying D8's pre-flight
  rationale and D4's prefix coupling.

## 6. Rollout, rollback, residual risk

Rollout is inert for every current deployment: all three fields default to absent/false, and
`backend = "tracing"` is untouched. Rollback is reverting the config.

It **is** a breaking config change for a deployment already running `backend = "nats"` — it now
needs `tls://` plus `credentials_file`, or the explicit flag. No such deployment exists; making
that flip unavailable until it is secure is the entire point of the issue.

| Risk | Mitigation |
| -- | -- |
| Static-conf test is a proxy for JWT-mode permissions (D2) | Subject lists are identical and centralized in `accounts.conf`; the encoding differs. Stated, not papered over |
| `provision.sh` is reviewed, not executed, in CI (D10) | The permission lists it writes are the ones §4.3 tests; the script's mechanics are the untested part |
| No test asserts a server-enforced JWT expiry (§4.5) | The re-read property is unit-proven and the reconnect path is integration-proven; minting operator JWTs in-test was weighed and rejected as fixture complexity |
| Inbox-prefix mismatch presents as publish timeouts (D4) | Matched pairs in every artifact, a docs callout, D9's log line names the denied subject |
| The exact `$JS.API` list may need read-only additions (D5) | The test is the authority; additions are read-only and re-reviewed |
| A JetStream domain changes `$JS.ACK` arity (D5) | Documented in `permissions.md` with the widened form |
| `provision.sh` creating a stream that drifts from `PublisherConfig` crash-loops the service on first boot (§3.1) | The script carries the same five D7-verified values with a comment pointing at `config.rs`; §4.3 exercises the fixture's equivalent by having the publisher adopt a pre-created stream |

## 7. Out of scope

- **mTLS client certificates** (D10).
- **The gateway consumer implementation** — SMA-492. This ships the account, the user, the
  durable, and the permissions it will need; it subscribes to nothing.
- **A dev-stack compose file.** SMA-471 §8 already deferred this; `ops/nats/test/` is a test
  fixture, not a dev stack.
- **Clustering / `num_replicas > 1`**, and the `sync_interval: always` durability posture ADR-0016
  notes production wants — broker-side operational choices, not this service's config.
- **`/readyz` reporting NATS health** — SMA-471 §8's reasoning is unchanged.
- **Rotating the account or operator keys** (as opposed to user credentials).

## 8. Acceptance criteria

Mapping the issue's four numbered requirements:

1. **Dedicated account.** `ops/nats/provision.sh` creates `PAIGASUS_IAM` with explicit JetStream
   limits, separate from `SYS`, and `permissions.md` documents why (D1).
2. **Subject permissions.** The publisher can create/inspect `IAM_EVENTS` and publish every
   `EventType`, and cannot subscribe to `iam.>` or delete/purge the stream. The gateway consumer
   can pull and ack from the provisioned filtered durable, and cannot create a consumer,
   subscribe to `iam.>`, or publish `iam.*`. **All of it asserted by `tests/nats_permissions.rs`
   against the committed config**, wired into CI as `repo:nats-permissions`.
3. **TLS.** `backend = "nats"` requires a `tls://` url and a `credentials_file`, and rejects
   url-embedded credentials unconditionally; `root_certificates` makes a private-CA broker
   dialable; an end-to-end TLS test proves it, with a negative control.
4. **Credential rotation.** The `.creds` file is re-read on every connection attempt; a rotated
   file is picked up without a restart; a bad path or malformed file still fails boot with the
   path named. Unit-proven for the re-read, integration-proven across a real reconnect.

Plus: a denied publish is diagnosable from the logs rather than presenting as an unexplained
timeout (D9).
