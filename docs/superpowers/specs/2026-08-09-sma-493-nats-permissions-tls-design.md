# SMA-493 — NATS account isolation, subject permissions, TLS, and credential rotation

**Status:** design (revised after adversarial review)
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
is the platform's authorization change graph, reconstructable from the envelope alone, with no
application-level access.

There is no application-level mitigation. The publisher cannot know who is subscribed. On a
shared NATS account, every co-tenant is a subscriber.

### 1.2 What the service does not currently make possible

Three of the four requirements are not merely undocumented — they are **unreachable from the
current config surface**:

| Requirement | State today |
| -- | -- |
| Dedicated account, subject permissions | Purely an ops artifact; nothing exists to deploy or review |
| TLS | `url` accepts `tls://`, but with no way to name a CA the client falls back to the **system** trust store (`async-nats` `tls.rs:61-74`) — a broker with a private-CA certificate is undialable without rebuilding the image's trust bundle |
| `.creds` over url-embedded credentials | `credentials_file` exists but nothing *requires* it. Worse: `nats://user:pass@host` is not merely discouraged, it is **silently ignored** — `ServerAddr::username()`/`password()` (`lib.rs:1682,1692`) have no caller in the connect path, so an operator who puts credentials in the url believes they are authenticated and is not |
| Credential rotation | Confirmed broken — see below |

### 1.3 The rotation question, answered

SMA-471 §7 flagged "whether `.creds` is re-read on reconnect" as an open item. It is not.

`ConnectOptions::with_credentials_file` (`options.rs:429`) reads the file once, parses it, and
caches the JWT **string** plus the `KeyPair` in `options.auth`. Every reconnect rebuilds its
`CONNECT` from that cached `auth.jwt` (`connector.rs:666-671`). A rotated file on disk is never
seen. NATS user JWTs can carry an expiry; when the cached one lapses, every reconnect fails
`AuthorizationViolation` and the process cannot recover without a restart — precisely the
"outage that outlives the credential" the issue predicted.

There is a rotation-safe path in the same crate: `auth_callback` is invoked on **every**
connection attempt (`connector.rs:681-688`), inside the same `try_connect_to` that rebuilds the
TLS config (`connector.rs:544`, invoked at `:568`/`:602`) — so a callback that re-reads the file,
and a CA bundle named by path, both pick up rotation without a restart.

## 2. Decisions

### D1 — One dedicated account, `PAIGASUS_IAM`, holding both service users

A dedicated NATS account for IAM events, not a shared one. This is the other half of SMA-471 D5,
which rejected subject prefixing for multi-deployment separation on the grounds that accounts are
the right mechanism; that argument only holds once an account actually exists.

Both service identities — the IAM publisher and the gateway consumer — live **inside** that one
account. Giving the gateway its own account requires JetStream service exports/imports for the
consumer API plus a stream deliver-subject export: substantial complexity for no isolation gain,
because the boundary that matters is *IAM events vs. every other tenant of the broker*, not IAM
vs. gateway. Within the account, separation is by user permissions (D3, D5), which is sufficient
and reviewable.

`SYS` stays the system account and holds no IAM traffic.

Account-level JetStream limits are set explicitly, not left unbounded:

| Limit | Value | Why |
| -- | -- | -- |
| `max_memory` | `0` | The stream is `File` storage (SMA-471 D8); memory streams are refused outright |
| `max_file` | `10GB` | ~2 orders of magnitude above the 7-day `max_age` working set at any plausible IAM event rate; a bound that catches a runaway, not one that shapes normal operation |
| `max_streams` | `4` | `IAM_EVENTS` plus headroom; not an open-ended stream factory |
| `max_consumers` | `32` | The gateway durable plus headroom for operator inspection |

Hitting `max_file` is a distinct publish failure (`insufficient resources`) and gets its own
runbook line (§5).

### D2 — Operator/nsc JWTs are the production target; static **nkey** users are the CI vehicle

Production uses decentralized auth: an operator, the `PAIGASUS_IAM` account, and per-service user
JWTs distributed as `.creds` files. That is what `credentials_file` already supports, what makes
rotation meaningful at all (D8), and what carries per-account JetStream limits.

nsc-minted artifacts cannot be committed — a `.creds` file *is* a private key. So the executable
proof runs against a static `nats-server.conf`. The first draft of this spec had those static
users authenticate with **user/password**, which does not work: `PublisherConfig` has exactly two
credential surfaces (`url`, `credentials_file`), and url userinfo is ignored (§1.2). A
user/password fixture would force the test to hand-roll its own `async_nats` client, and the
whole claim — "this subject list is sufficient for the calls *the adapter* makes" — would rest on
a duplicated call sequence free to drift.

The fixture therefore uses static **nkey** users. `auth_callback` can return
`Auth { nkey, signature, .. }` and the connector forwards it (`connector.rs:707`), so a seed on
disk authenticates against `users: [{ nkey: "U…", permissions: {…} }]` — with the permission
blocks byte-identical to the JWT-mode ones. `NatsEventPublisher::connect` is the code under test
in every integration test here, not a stand-in for it.

Concretely, D8's loader keys off the file's contents: a two-block `.creds` (JWT + seed) yields
JWT auth; a seed-only file yields nkey auth. Both are ordinary NATS conventions (`.creds` vs
`.nk`), both are one code path, and the second is what makes the permission set testable.

The residual proxy is narrow and stated: the *credential encoding* differs between fixture and
production (nkey vs JWT), while the subject lists, the adapter, and every JetStream API call are
the real ones.

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

NATS permissions are **allow-list only**: anything not listed is denied. So the "denied" sets
throughout this spec describe consequences of the allow-list, not literal `deny` blocks. What
that buys here: no `subscribe iam.>`, so the publisher cannot read the graph it writes; no
`STREAM.UPDATE`/`DELETE`/`PURGE`, making SMA-471 D7's deliberate non-reconciliation ("this
service must never silently reshape a stream external consumers depend on") enforced rather than
merely intended; and no `STREAM.MSG.GET`/`DIRECT.GET`, which would otherwise read any message in
the stream regardless of any consumer filter. `STREAM.CREATE` is needed only on first boot but
must be granted, since that boot is where the stream comes from in an unprovisioned environment.

### D4 — Per-user inbox prefixes, because `_INBOX.>` leaks across the account

Each user gets a distinct inbox prefix (`_INBOX_IAM_PUB`, `_INBOX_GW`, `_INBOX_PROV`) via
`ConnectOptions::custom_inbox_prefix`, and its `subscribe` grant is scoped to that prefix.

This is isolation, not tidiness. Pull-consumer messages are delivered to the requester's inbox,
so inside a shared account a publisher holding `sub _INBOX.>` could subscribe to **the gateway's**
inbox and read every message the gateway pulls — recovering exactly the firehose D3 denied it.
Per-user prefixes close that, and they are the only way to, because inbox replies are the one
subject space every client must be able to read.

The cost is a coupling: the prefix in the config and the prefix in the account's grant must
match, and a mismatch presents as publish timeouts with no mention of permissions. It gets an
explicit callout in the docs, a matched pair in every committed artifact, and a test. **SMA-492
inherits the same obligation** — a gateway that does not set `custom_inbox_prefix` to `_INBOX_GW`
will have every pull silently return nothing. `permissions.md` states it; the prefix strings live
alongside the subject lists in `ops/nats/subjects.env` (D10) so there is one place to read them.

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
```

Everything else follows from the allow-list. No consumer-create verb in **any** of its forms —
`$JS.API.CONSUMER.CREATE.<stream>.<name>[.<filter>]` (`context.rs:1512`), the unnamed-ephemeral
form (`:1513`), or the legacy `$JS.API.CONSUMER.DURABLE.CREATE.>` — which matters because
`filter_subject(s)` is updatable by a CREATE against an existing durable, making this the control
the entire least-privilege argument rests on. No `subscribe iam.>`, closing the core-NATS route
to the same data. No `STREAM.MSG.GET`/`DIRECT.GET`, closing the third route: direct message get
reads any message in the stream regardless of consumer filter, and would void the filter entirely
the day someone enables `allow_direct` for performance. No `publish iam.*`, so a compromised
gateway cannot forge events. §4.3 asserts each of these as a positive denial, including the
*named* create form specifically — a test that only exercised the bare form would pass against a
grant that leaves the real one open.

That denial has a corollary: **someone else must create the durable.** Neither service identity
can. So the account also holds a third identity, `iam-provisioner`, used by ops tooling and by
the test fixture, and deployed nowhere. Its grant is `pub $JS.API.STREAM.>` +
`$JS.API.CONSUMER.>`, `sub _INBOX_PROV.>` — deliberately *not* the `$JS.API.>` a first draft gave
it, because that wildcard includes `STREAM.MSG.GET` and would make the provisioning credential a
full reader of the change graph §1.1 says must not leak. It is still the most privileged identity
in the account: `permissions.md` states that its `.creds` belongs in an operator's credential
store, never in a deployment secret.

Two subject-format notes the artifacts and runbook must carry:

- **A JetStream domain re-shapes both prefixes.** With no domain, acks go to
  `$JS.ACK.<stream>.<consumer>.…` and the API lives at `$JS.API.…`. Configure a domain and acks
  become `$JS.ACK.<domain>.<account-hash>.<stream>.<consumer>.…` **and** the API moves to
  `$JS.<domain>.API.…` — so every grant in this section shifts, not only the ack one. The
  artifacts and tests pin the no-domain form and `permissions.md` gives the widened variants.
- **The exact `$JS.API` list is pinned by the test, not by this document.** If
  `get_consumer_from_stream` also requires `$JS.API.STREAM.INFO.IAM_EVENTS`, or the context
  requires `$JS.API.INFO`, those are read-only additions and get added — with the test as the
  authority on what is genuinely needed.

The six filtered subjects are this design's reading of what cache invalidation requires:
revocations and grants change authz outcomes. `iam.principal.created` and `iam.api_key.issued`
are excluded because nothing is cached about a principal or key that does not exist yet — and
IAM's own caches store only positive entries (`adapters/api_keys/cache.rs`), so there is no
negative entry for a creation event to invalidate. The gateway's cache is SMA-492's to design;
if it turns out to cache negatives, the filter widens **without touching either service**,
because it lives in the provisioned durable. That changeability is the point.

### D6 — TLS and credential posture are enforced in `validate()`, behind one honestly-named flag

`IamConfig::validate` gains three rules, all gated on `backend = "nats"`:

1. `url` must have scheme `tls://`.
2. `url` must carry no userinfo — **rejected unconditionally, no opt-out**. Not merely because
   `credentials_file` is the credential channel, but because async-nats ignores url userinfo
   outright (§1.2): accepting it would let a config that looks authenticated connect anonymously.
3. `credentials_file` must be present.

`allow_insecure_broker = true` (default `false`) relaxes (1) and (3). One flag rather than two,
because "this is a dev/CI broker" is one fact and two independently-settable flags is a way to
end up with TLS on and authentication off. The name is deliberately not `allow_plaintext`: the
flag legalises an *unauthenticated* broker as well as an unencrypted one, and the narrower name
would understate that to whoever copies the line out of `dev-setup.md`. The `validate()` error
text names both effects.

**Enforcement lives only in `validate()`** — one gate, on the production path
(`main.rs` loads → validates → connects). `NatsEventPublisher::connect` does not re-police it.
That is a real consequence for the test suite, stated in §4.6 rather than glossed: the
integration tests construct `PublisherConfig` directly and never call `validate`, so this flag is
exercised by the config unit tests, and nowhere else.

### D7 — A `root_ca_bundle` config field, because otherwise TLS is unreachable

`async-nats` builds its root store from `rustls-native-certs` when no CA is named
(`tls.rs:61-74`), so with today's config a broker presenting a private-CA certificate cannot be
dialled at all without rebuilding the container's trust bundle.

The new field is wired to `ConnectOptions::add_root_certificates`, whose name is a trap:
`options.rs:543` does `self.certificates = vec![path]`, and `config_tls` skips
`load_native_certs()` entirely once `certificates` is non-empty. **Naming a CA replaces the
system trust store; it does not extend it.** A deployment that sets this and later moves the
broker behind a public CA gets a total outage diagnosed as a bare TLS error. Hence the field is
called `root_ca_bundle`, not `root_certificates`: the operator must concatenate *every* CA the
client needs into one PEM file. Stated on the field's doc comment, in `permissions.md`, and in
`RUNBOOK-nats.md`. Omitted, behaviour is exactly as today.

Because `config_tls` runs per connection attempt (`connector.rs:544`), a rotated CA bundle is
picked up on reconnect with no restart — the same property D8 buys for credentials, for free.

### D8 — Rotation via `with_auth_callback`, with an eager pre-flight read

`with_credentials_file` is replaced by `with_auth_callback`, invoked on every connection attempt
(`connector.rs:681`). The callback re-reads and re-parses the credential file, signs the server
nonce with `nkeys::KeyPair::sign`, and returns **raw** signature bytes — `async-nats`
base64url-encodes them itself (`connector.rs:694-696`).

Three implementation constraints, all load-bearing:

- `with_auth_callback` is a **constructor**, not a builder method (`options.rs:204`), so the
  options chain must start with it. The `allow_insecure_broker`-with-no-credentials path needs a
  separate `ConnectOptions::new()` branch.
- The callback's future must be `Send + Sync + 'static` (`options.rs:207`). Nothing non-`Sync`
  may be held across an `await`: read the file first, then parse and sign with no further awaits.
- The returned `Auth` carries `jwt + signature` for a two-block `.creds` and `nkey + signature`
  for a seed-only file (D2).

`connect()` keeps an **eager pre-flight read and parse** — a missing, unreadable, or malformed
file fails boot with the path named, via the typed `NatsPublisherError::Credentials` variant,
before a client object exists. The first draft justified this as "async-nats flattens the error",
which is wrong and worth correcting: `connector.rs:685-688` uses `ConnectError::with_source`,
preserving the chain, and logs the cause at `:686`. The real justifications are that a boot-time
typed error is a better operator experience than an authentication failure on attempt one, and
that it fails before any connection machinery is constructed.

A file that goes bad mid-life fails the reconnect and lands in the story SMA-471 already built:
`iam_nats_connected → 0`, breaker opens, rows park, `IamOutboxPublishFailures` fires. A **partial
write** is the same case — the parse fails and the attempt is retried — but the deployment should
not rely on that: `permissions.md` requires the credential be delivered by atomic replacement
(a Kubernetes secret mount's symlink swap, or write-to-temp-then-`rename`), never by truncating
and rewriting in place.

New direct dependency: `nkeys` 0.4.5 — already in the lock tree via `async-nats`. The two-block
parse is hand-rolled (~20 lines). The first draft justified that as avoiding a `regex`
dependency, which is moot — `regex` is already in `rs/Cargo.lock`, pulled by `async-nats`'
`auth_utils` under the `nkeys` feature this workspace enables. The real difference is
behavioural: async-nats' regex (`auth_utils.rs:74-91`) takes the first and second `-----`
delimited blocks *regardless of label*, whereas keying on `BEGIN NATS USER JWT` /
`BEGIN USER NKEY SEED` accepts and rejects a different set of files. §4.2 pins the intended
behaviour rather than leaving it incidental.

### D9 — Surface server errors through `event_callback`

A denied publish is not an error the client sees: the server replies `-ERR 'Permissions Violation
for Publish to …'` asynchronously and the request simply **times out**. Without wiring, the most
likely misconfiguration in this whole design presents as `publish_timeout_secs` expiry with no
stated cause.

`ConnectOptions::event_callback` is wired to log `Event::ServerError` / `Event::ClientError` at
`ERROR` and connection transitions at `INFO`/`WARN`. `ServerError::Other(String)` carries the
violation text verbatim, so the offending subject appears in the log.

This is also what makes §4.3's denial assertions non-vacuous, which is why it is a decision and
not a nicety: a denied `subscribe` returns `Ok(Subscriber)` and a denied `$JS.API` request just
times out, so "assert it was denied" is otherwise indistinguishable from a broken fixture. The
tests pipe `Event`s into an `mpsc` and assert on the violation text naming the exact subject.

The `iam_nats_connected` gauge keeps its 5-second sampler (SMA-471); this adds diagnosis and does
not churn a metric that already works.

### D10 — One source for the subject lists; mTLS out of scope

The permission lists exist twice — in `provision.sh` (what deploys) and `test/accounts.conf`
(what is tested). A first draft deferred reconciling them as "real complexity"; it is not. The
lists live once in `ops/nats/subjects.env` as shell arrays, `provision.sh` sources it, and
`repo:nats-permissions` asserts `accounts.conf` grants exactly those subjects. ~15 lines, and it
removes the situation where the artifact that is *proven* is not the artifact that is *deployed*.

`add_client_certificate` exists, but `.creds` is the authentication mechanism and mTLS earns
nothing on top of it here. Recorded so a reviewer sees it was weighed, not missed.

## 3. The fix

### 3.1 `ops/nats/` (new)

| File | Contents |
| -- | -- |
| `README.md` | What this directory is, how to provision an environment, the artifact index, the tool list |
| `subjects.env` | **The single source** for every subject list and inbox prefix (D10) |
| `permissions.md` | The lists in both encodings with rationale: the ack-inbox grant, the pull-consumer narrowing argument, the JetStream-domain variants, the `root_ca_bundle` replacement semantics, credential-delivery atomicity, and the handling rule for `iam-provisioner` creds |
| `provision.sh` | Sources `subjects.env`; drives **`nsc`** (operator → `SYS` + `PAIGASUS_IAM` with D1's limits → three users → `.creds`), emits the **nats-resolver config** (`nsc generate config --nats-resolver`) the broker needs to validate those JWTs, `nsc push`es the account, then drives the **`nats` CLI** for the `IAM_EVENTS` stream and the filtered `gateway-cache-invalidator` durable. The tool list is explicit because nsc alone cannot do the last two |
| `resolver.conf` | The generated resolver stanza the broker includes — a real deployment artifact, not a by-product |
| `test/accounts.conf` | The **authoritative** static-nkey account/user/permission block — what §4.3 executes and what the D10 gate diffs against `subjects.env` |
| `test/nats-server.conf` | Port + JetStream + `include "accounts.conf"` |
| `test/nats-server-tls.conf` | The same plus a `tls { cert_file, key_file }` block |

Splitting `accounts.conf` out and `include`-ing it from both server configs means the plaintext
and TLS fixtures cannot drift apart in their permission lists.

**`provision.sh` creates the stream, and its config must match what the service requires.**
Because the durable attaches to `IAM_EVENTS`, provisioning cannot wait for the service's own
first-boot `get_or_create_stream`. That makes the script's stream config load-bearing: SMA-471 D7
fails boot when an adopted stream's `retention`, `storage`, `duplicate_window`, `subjects`, or
`max_age` is weaker than configured, so a provisioning script that creates a *drifted* stream
produces a crash-looping service — and `retention`, `storage` and `duplicate_window` are not
editable in place, making the fix a delete-and-recreate maintenance window. The script carries the
same five values as `PublisherConfig`'s defaults with a comment pointing at `config.rs` and D7,
and `permissions.md` states the coupling.

`provision.sh` mints user JWTs **without `--expiry`**, and says so: a non-expiring credential
makes §1.3's motivating outage impossible by construction, while D8 keeps rotation available for
the case that actually needs it (compromise, or a policy that mandates expiry). An operator who
sets an expiry takes on monitoring it — see §7 for the deferred gauge.

### 3.2 `rs/crates/services/paigasus-iam/src/config.rs`

Three fields on `PublisherConfig`, all optional, all inert under the default `tracing` backend:

```toml
[outbox.publisher]
backend               = "nats"
url                   = "tls://nats.internal:4222"
credentials_file      = "/etc/paigasus/iam.creds"
root_ca_bundle        = "/etc/paigasus/nats-ca.pem"   # NEW — REPLACES the system trust store
inbox_prefix          = "_INBOX_IAM_PUB"              # NEW — must match the account's subscribe grant
allow_insecure_broker = false                         # NEW — the single dev/CI escape hatch
```

`inbox_prefix` defaults to `None`, meaning `async-nats`' own `_INBOX`, so a deployment that has
not adopted these artifacts keeps working. `Debug`/`Serialize` stay hand-rolled; the three new
fields are two paths and a bool, so none needs redaction and the existing `url` redaction is
untouched — but `serialize_struct("PublisherConfig", 8)` (`config.rs:457`) hardcodes the field
count and becomes `11`. `validate()` gains D6's three rules.

### 3.3 `rs/crates/services/paigasus-iam/src/adapters/events/`

- `creds.rs` (new): `parse_credentials(&str)` handling both shapes (two-block `.creds` → JWT +
  seed; seed-only → nkey), and `auth_from_credentials(path, nonce) -> Result<Auth, AuthError>` —
  the callback body as a named, directly testable unit, written to hold nothing non-`Sync` across
  an await (D8).
- `nats_publisher.rs`: `connect` does the D8 pre-flight, then builds options starting from
  `with_auth_callback` (or `ConnectOptions::new()` when no credential is configured), adding
  `custom_inbox_prefix`, `add_root_certificates`, and D9's `event_callback`. New error variant for
  a credential file that reads but does not parse — today that is an opaque `InvalidData`
  `io::Error` from inside `async-nats`.

### 3.4 `rs/crates/libs/paigasus-iam-core/src/domain_event.rs`

`EventType::ALL` is promoted from a `#[cfg(test)]`-private constant (`domain_event.rs:73`) to
`pub const ALL: [EventType; 8]`, guarded by the existing exhaustive-`match` round-trip test.
Without this, §4.3's "every event type is covered" claim would be a fourth hand-maintained copy
of the list that a new variant does not break — which is exactly the failure the assertion exists
to prevent.

### 3.5 `Cargo.toml` + `deny.toml`

`nkeys` 0.4.5 as a workspace dependency (direct dep of the IAM service); `rcgen` as a **dev**-
dependency for §4.4's test certificates. Both are already in `rs/Cargo.lock` with licences the
current `:deny` accepts, so no `deny.toml` change is expected. `cargo machete` is the real
constraint: per CLAUDE.md's staging gotcha, `nkeys` must land in the same commit that consumes it.

### 3.6 `moon.yml` **and** `.github/workflows/ci.yml`

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
      - 'rs/crates/services/paigasus-iam/tests/support/**/*'
      - 'rs/crates/services/paigasus-iam/src/adapters/events/**/*'
      - 'rs/crates/services/paigasus-iam/src/config.rs'
      - 'rs/crates/services/paigasus-iam/Cargo.toml'
      - 'rs/Cargo.lock'
```

**Defining the task is not enough.** CI runs a hardcoded target array
(`.github/workflows/ci.yml:184`); a Moon task absent from it runs on no PR and no push to main.
`:nats-permissions` is added to that array as an explicit deliverable — otherwise this ships a
gate that never executes, which is worse than no gate, because the spec would claim coverage.

## 4. Tests

### 4.1 Config — unit, in `config.rs`

Plaintext `url` rejected; plaintext + `allow_insecure_broker` accepted; **userinfo rejected even
with `allow_insecure_broker`**; missing `credentials_file` rejected unless
`allow_insecure_broker`; every rule inert under `backend = "tracing"`; the rendered messages name
the offending field and state both of the flag's effects.

### 4.2 Credentials — unit, in `creds.rs`

Re-reads per call (write creds A → call → overwrite with B → call → different JWT: the property
D8 exists for); the signature verifies against the seed's public key via `KeyPair::verify`;
a seed-only file yields nkey auth and a two-block file yields JWT auth (D2); truncated,
single-block, and label-mismatched files give actionable errors naming the path — pinning the
D8 divergence from async-nats' label-agnostic regex rather than leaving it incidental. Seeds are
minted in-test with `nkeys::KeyPair::new_user()`; nothing is committed.

### 4.3 Permissions — integration, `tests/nats_permissions.rs` (new)

Boots `nats:2.10.14` with the committed `ops/nats/test/` configs copied in via
`ImageExt::with_copy_to` (`testcontainers` 0.27.3, `image_ext.rs:144`) and a replaced command.
Because the module's `Nats` image hardcodes stderr readiness strings
(`testcontainers-modules-0.15.0/src/nats/mod.rs:126-131`) that a custom config may not reproduce,
the fixture uses `GenericImage::new("nats", "2.10.14")` with an explicit `WaitFor`, and pins
JetStream's `store_dir` to a writable path in the image.

Every case drives `NatsEventPublisher` (publisher side) or a real `async_nats` JetStream client
authenticated as the fixture's nkey users, with D9's `event_callback` piped into an `mpsc` so
denials are positive assertions on the violation text and its exact subject:

| As | Must succeed | Must be denied |
| -- | -- | -- |
| `iam-publisher` | `connect` (stream ensure + D7 verification); publish + ack for **every** `EventType::ALL` | `subscribe iam.>`; `STREAM.DELETE`; `STREAM.PURGE`; `STREAM.MSG.GET`; `DIRECT.GET` |
| `gateway-consumer` | pull from the provisioned durable; ack | `CONSUMER.CREATE` in the **named** `…CREATE.IAM_EVENTS.gateway-cache-invalidator` form *and* the bare form *and* legacy `DURABLE.CREATE`; `subscribe iam.>`; `publish iam.*`; `STREAM.MSG.GET` |

The denial half is the half that earns its keep: sufficiency is the easy property, and
over-breadth is what rots. Iterating `EventType::ALL` (§3.4) means a ninth event type cannot be
added without this test having an opinion.

Plus the D10 gate: `accounts.conf` grants exactly the subjects in `subjects.env`.

Same Docker gating as the existing suites: hard failure in CI, skip on a Docker-less laptop.

### 4.4 TLS — integration

`rcgen` mints a CA and a server certificate with an IP SAN for `127.0.0.1` (the tests dial a
mapped host port) into a temp dir; both are copied into the container alongside
`nats-server-tls.conf`, which `include`s the same `accounts.conf`, so the adapter authenticates
with an nkey seed exactly as in §4.3. It then connects with `url = "tls://…"` and
`root_ca_bundle = <temp CA>` and publishes. No committed keys, no secret-scanner noise, and D7's
field is proven rather than assumed. Two negative controls: no `root_ca_bundle` (the system trust
store cannot verify a private CA) and a bundle containing an unrelated CA.

### 4.5 Rotation across a real reconnect — integration

The discriminating test, and it has to be discriminating by construction: connect and publish,
then **overwrite the credential file with a truncated one**, restart the container (the pattern
`dedup_survives_a_broker_restart` already uses), and assert the reconnect-and-publish now fails
with an error naming the path. That assertion is *false* under a cached-credential implementation
and *true* under D8's — the first draft's version (restart, publish again, expect success) passed
identically before and after the change, and would have shipped the fix with a regression net
that could not detect the regression. The happy path (rotate to a *valid* new credential, publish
succeeds) is asserted alongside it.

The server-enforced half — a JWT that actually expires — remains a stated gap (§6).

### 4.6 Existing suites

`tests/nats_publisher.rs` has **14** `#[tokio::test]`s sharing one config helper, which gains the
three new fields — one edit, not fourteen. They construct `PublisherConfig` directly and call
`connect`, never `validate`, so — contrary to this spec's first draft — they do **not** exercise
`allow_insecure_broker`; §4.1 is the only place that flag is tested. Saying otherwise would have
claimed coverage that does not exist.

### 4.7 Full gate

```
moon ci :build :test :lint :fmt :deny :machete :typecheck :breaking :affected-smoke \
  :parity-corpus-drift :wasm-getrandom-free :redis-connect-single-site :promtool \
  :observability-drift :nats-permissions :release-parity :release-parity-py :release-parity-ts \
  --base origin/main --include-relations
```

This is CI's actual array (`ci.yml:184`) plus the new task — CLAUDE.md's documented list omits
`:redis-connect-single-site`, which CI does run. `:deny` covers the two new dependencies;
`:wasm-getrandom-free` still matters, since `nkeys` must not reach the wasm binding's tree.

## 5. Documentation

- **`ops/nats/README.md`, `subjects.env`, `permissions.md`** — written so an operator can
  provision an environment without reading Rust, with the tool list stated up front (`nsc` + the
  `nats` CLI + a resolver config).
- **`docs/ops/RUNBOOK-nats.md`** (new) — permission-violation symptoms (D9's log line; a denied
  publish looks like a timeout), expired or rotated credentials, a stream INFO denial presenting
  as an `Ensure` failure rather than a 404, the inbox-prefix mismatch, `max_file` exhaustion as
  `insufficient resources`, and the `root_ca_bundle` replacement trap. Cross-linked from the
  "NATS backend: boot hard-fails…" section of `RUNBOOK-observability.md`, which keeps its scope.
- **`docs/dev-setup.md`** — `allow_insecure_broker = true` in the local snippet, with one line on
  what it legalises.
- **ADR-0016** (Notion) — Consequences updated: the account model (D1), the rotation question
  moved from open to answered (§1.3, D8), subject permissions moved from "a deployment
  requirement" to "provisioned and tested here", and the residual absence of an account-JWT
  revocation and operator-key-rotation story (§7).
- **Rustdoc** in house style on `creds.rs` and the changed `connect`, carrying D8's pre-flight
  rationale, D7's replacement semantics, and D4's prefix coupling.

## 6. Rollout, rollback, residual risk

Rollout is inert for every current deployment: all three fields default to absent/false, and
`backend = "tracing"` is untouched. Rollback is reverting the config.

It **is** a breaking config change for a deployment already running `backend = "nats"` — it now
needs `tls://` plus `credentials_file`, or the explicit flag. No such deployment exists; making
that flip unavailable until it is secure is the entire point of the issue.

| Risk | Mitigation |
| -- | -- |
| The fixture authenticates by nkey while production uses JWTs (D2) | Subject lists, adapter, and every JetStream call are the real ones; only the credential encoding differs, and both flow through one loader tested in §4.2 |
| `provision.sh` is reviewed, not executed, in CI | D10's gate proves its subject lists match the tested config; the script's *mechanics* (nsc/nats CLI invocation order, resolver push) remain untested |
| No test asserts a server-enforced JWT expiry (§4.5) | Re-read is unit-proven and reconnect behaviour is integration-proven with a discriminating assertion; `provision.sh` mints non-expiring JWTs, so the motivating failure requires an operator to opt into expiry |
| Inbox-prefix mismatch presents as publish timeouts (D4) | Matched pairs sourced from `subjects.env`, a docs callout, D9's log line names the denied subject |
| The exact `$JS.API` list may need read-only additions (D5) | The test is the authority; additions are read-only and re-reviewed |
| A JetStream domain changes both `$JS.ACK` and `$JS.API` (D5) | Documented in `permissions.md` with the widened forms |
| `provision.sh` creating a stream that drifts from `PublisherConfig` crash-loops the service (§3.1) | The script carries the same five D7-verified values with a comment pointing at `config.rs`; §4.3 exercises the equivalent by having the publisher adopt a pre-created stream |
| `url` holds a single seed address (`impl ToServerAddrs for str`, `lib.rs:1720-1726`) | Peers are discovered from `INFO.connect_urls` after connecting, so a cluster survives; but boot fails if the one seed is the down node. Named here rather than dismissed as broker-side |

## 7. Out of scope

- **mTLS client certificates** (D10).
- **A credential-expiry gauge and alert** (`iam_nats_credentials_expires_at`). Nearly free —
  the credential is parsed on every connect — but only meaningful once someone provisions an
  expiring JWT, which §3.1 does not. Named as a follow-up rather than built speculatively.
- **Account-JWT revocation, and operator/account key rotation.** D8 rotates *user* credentials;
  revoking an account JWT or rolling the operator key is a broker-side story this does not tell.
  Recorded in ADR-0016's consequences so it is not lost.
- **The gateway consumer implementation** — SMA-492. This ships the account, the user, the
  durable, and the permissions it will need; it subscribes to nothing.
- **A dev-stack compose file.** SMA-471 §8 already deferred this; `ops/nats/test/` is a test
  fixture, not a dev stack.
- **Clustering / `num_replicas > 1`** and the `sync_interval: always` durability posture ADR-0016
  notes production wants — broker-side operational choices.
- **`/readyz` reporting NATS health** — SMA-471 §8's reasoning is unchanged.

## 8. Acceptance criteria

Mapping the issue's four numbered requirements:

1. **Dedicated account.** `ops/nats/provision.sh` creates `PAIGASUS_IAM` with the numeric
   JetStream limits of D1, separate from `SYS`, and `permissions.md` documents why.
2. **Subject permissions.** The publisher can ensure `IAM_EVENTS` and publish every
   `EventType::ALL`, and cannot subscribe to `iam.>`, delete/purge the stream, or read it via
   `STREAM.MSG.GET`/`DIRECT.GET`. The gateway consumer can pull and ack from the provisioned
   filtered durable, and cannot create a consumer in any form, subscribe to `iam.>`, publish
   `iam.*`, or direct-get. **Asserted by `tests/nats_permissions.rs` against the committed
   config, through `NatsEventPublisher` itself on the publisher side**, with every denial a
   positive assertion on the server's violation text — and the same test proves `accounts.conf`
   matches `subjects.env`, the file `provision.sh` deploys from.
3. **TLS.** `backend = "nats"` requires a `tls://` url and a `credentials_file`, and rejects
   url-embedded credentials unconditionally; `root_ca_bundle` makes a private-CA broker dialable;
   an end-to-end TLS test proves it, with negative controls.
4. **Credential rotation.** The credential file is re-read on every connection attempt, proven by
   a test that **fails against the previous cached-credential implementation**; a rotated file is
   picked up without a restart; a bad path or malformed file still fails boot with the path named.
5. `:nats-permissions` appears in `.github/workflows/ci.yml`'s target array, so the gate actually
   runs.
6. A denied publish is diagnosable from the logs rather than presenting as an unexplained timeout
   (D9).
