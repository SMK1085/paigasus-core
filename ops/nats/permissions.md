# NATS permission model — `PAIGASUS_IAM` account

This document explains the *why* behind the subject grants in `subjects.env` (the single source
of truth — `provision.sh` mints production users from it, `ops/nats/test/accounts.conf.tmpl`
mirrors it for tests, and `check-subjects.sh` fails the build the moment the two disagree). Read
it before touching either file: the grants below are the security boundary for the entire IAM
event stream — `iam.>` carries the platform's full authorization change graph (every role grant,
revocation, policy change and API-key revocation), in real time, reconstructable from the
envelope alone, with no application-level access. Getting a grant list wrong here is not a
cosmetic misconfiguration; it is either an outage (too narrow) or a confidentiality breach (too
wide).

NATS permissions are **allow-list only**. There is no explicit "deny" syntax in the account block
below — a subject that does not appear in a user's `publish.allow` or `subscribe.allow` list is
refused by the broker, full stop. Every "✗" callout in this document is describing a *consequence*
of that allow-list (what the identity in question therefore cannot do), not a literal deny rule
written anywhere.

## 1. The three users

The `PAIGASUS_IAM` account holds exactly three identities. All three subject lists live in
`subjects.env` (`PUBLISHER_PUB`/`PUBLISHER_SUB`, `CONSUMER_PUB`/`CONSUMER_SUB`,
`PROVISIONER_PUB`/`PROVISIONER_SUB`) — this table is a rendering of that file, not a second source
of truth. If the two ever disagree, `subjects.env` is correct and this file is stale.

| identity | deployed as | publish (allow) | subscribe (allow) |
|---|---|---|---|
| `iam-publisher` | `paigasus-iam`'s outbox relay (`PublisherConfig`, SMA-471) | `iam.>`, `$JS.API.STREAM.INFO.IAM_EVENTS`, `$JS.API.STREAM.CREATE.IAM_EVENTS` | `_INBOX_IAM_PUB.>` |
| `gateway-consumer` | the AI Gateway's cache-invalidation client (SMA-492) | `$JS.API.CONSUMER.MSG.NEXT.IAM_EVENTS.gateway-cache-invalidator`, `$JS.API.CONSUMER.INFO.IAM_EVENTS.gateway-cache-invalidator`, `$JS.ACK.IAM_EVENTS.gateway-cache-invalidator.>` | `_INBOX_GW.>`, `iam.policy.>` |
| `iam-provisioner` | nobody — operator tooling only, see §9 | `$JS.API.INFO`, `$JS.API.STREAM.>`, `$JS.API.CONSUMER.>` | `_INBOX_PROV.>` |

`iam-publisher` never subscribes to `iam.>` (it must not be able to read the graph it writes) and
never gets `STREAM.UPDATE`/`DELETE`/`PURGE` (SMA-471 D7 makes stream non-reconciliation
deliberate — an existing stream is adopted as-is or boot fails, never silently reshaped). The
durable's subject filter that narrows what `gateway-consumer` actually receives is not a
permission at all — see §3. The one subject-level grant `gateway-consumer` holds is the core
subscription on `iam.policy.>` (SMA-752) — see §3.1.

## 2. Why the publisher needs a `subscribe` grant

It is tempting to read "publisher" and conclude it should be write-only. It cannot be. A JetStream
publish is a NATS *request*: the client publishes the event, then waits for the broker to reply
with a `PubAck` on the client's own reply-to subject (its inbox). If `iam-publisher` has no
`subscribe` grant covering `_INBOX_IAM_PUB.>`, the broker's ack is refused delivery exactly the
way any other unauthorized subscribe would be — the client never sees it, and every single publish
times out waiting for an acknowledgment it is not allowed to receive. The failure mode is also
silent at the call site: `NatsEventPublisher` treats it as an ordinary timeout, not a permissions
error (the async client's connection-event callback is what actually surfaces the
`Permissions Violation` — see §8's cross-reference and `nats_publisher.rs`'s `event_callback`).
So: `iam-publisher` is asymmetric, not write-only — it may publish to `iam.>` and to the two
`$JS.API.STREAM.*` control subjects it needs to ensure the stream exists, and it may subscribe to
*only* its own inbox prefix, nothing else.

## 3. Why `subscribe` permissions cannot narrow a consumer

`gateway-consumer` is only supposed to see a handful of event types — the six subjects in
`subjects.env`'s `CONSUMER_FILTER_SUBJECTS` (`iam.role.granted`, `iam.role.revoked`,
`iam.api_key.revoked`, `iam.principal.archived`, `iam.policy.put`, `iam.policy.deleted`) — out of
everything published to `iam.>`.

> **The publishable surface is 25 subjects, not 8 (SMA-606, ADR-0016 amendment 2026-09-01).**
> SMA-606 added fourteen tenancy event types —
> `iam.{organization,team,project}.{created,renamed,archived,restored}` and
> `iam.membership.{attached,detached}`. Nothing in this document changes: they all sit under
> `iam.>`, so `iam-publisher`'s publish grant and `IAM_EVENTS`'s filter already cover them, and
> the reasoning below about why a filter cannot live in a permission applies to the wider space
> unaltered. `CONSUMER_FILTER_SUBJECTS` is still those same six — the thirteen genuinely new
> tenancy subjects reach no consumer, deliberately.
>
> The one behavioural change is on a subject already in the list. **`iam.role.granted` now has a
> third emitter:** besides `RoleService::grant` and `BootstrapAdminSeeder::seed_grant`,
> organization create emits it for the `org_admin` owner grant it writes in the same transaction.
> So every organization create now delivers a message to the `gateway-cache-invalidator` durable.
> That is intended — a new `org_admin` grant does change authorization decisions — but it is
> traffic SMA-492 did not size for. The event carries `"source": "organization_create"` so a
> future consumer can tell it from a user-requested grant.
>
> SMA-775 added three more event types: `iam.policy.tool_set.published`,
> `iam.policy.tool_floor.published` and `iam.policy.tool_settings.changed`. They sit under
> `iam.>` and `iam.policy.>`. `subjects.env` does not change.

The intuitive way to enforce that would be a `subscribe.allow`
naming just those event subjects. That does not work, for a structural reason: a JetStream pull
consumer's deliveries do not arrive on the event's original subject at all — they arrive on the
requesting client's inbox, via `$JS.API.CONSUMER.MSG.NEXT.<stream>.<consumer>` triggering a reply
on `_INBOX_GW.>`. The client never subscribes to `iam.role.granted` or any other event subject
directly, so a `subscribe.allow` naming those subjects would grant nothing useful and a
`subscribe.deny` naming them would restrict nothing that was ever going to be used.

The filter therefore cannot live in a permission — it lives in the **pre-provisioned durable
consumer**, `gateway-cache-invalidator` (`provision.sh` §4, `CONSUMER_FILTER_SUBJECTS` in
`subjects.env`). That filter is binding only because of what `gateway-consumer`'s permissions
*deny*: no `$JS.API.CONSUMER.CREATE.*` in any form is granted — not for `IAM_EVENTS`, not for any
other stream. If it could create its own consumer, it could create one with no filter (or a wider
one) and pull the entire stream regardless of what the pre-provisioned durable's filter says. The
narrowing is enforced by the *absence* of a capability, not the presence of one.

### 3.1 The core subscription on `iam.policy.>` (SMA-752)

The paragraphs above are about the PULL path. Since SMA-752 `gateway-consumer` also holds one
subject-level grant: `subscribe` on `iam.policy.>`. The gateway uses it to clear its tool-policy
cache (SMA-752 D4, Q1-A, chosen by Sven). Each gateway replica must get every policy event, and
a shared durable gives each event to one replica only. A JetStream publish is also an ordinary
core message on its subject, so each core subscriber of `iam.policy.>` receives it. This was
measured against the pinned `nats:2.10.14` (SMA-752 D24,
`the_consumer_core_subscription_receives_a_jetstream_policy_publish`).

The grant is a subscribe right only:

- It is the only subscribe grant outside the inbox. The other IAM subjects stay closed
  (`iam.api_key.>`, `iam.role.granted`, `iam.>`).
- It adds no publish right, so the gateway cannot forge a policy event.
- It adds no JetStream API right. `CONSUMER.CREATE` and `CONSUMER.UPDATE` stay ungranted in every
  form, and `$JS.>` cannot be subscribed to. So the durable's filter above stays binding.

Delivery is at most once. The gateway clears its cache again on every reconnect, and its
periodic refresh is the backstop for a lost message. `nats_permissions.rs` holds the negative
tests (`the_consumer_cannot_subscribe_to_other_iam_subjects`,
`the_policy_grant_adds_no_jetstream_right`).

## 4. The four routes to the firehose the allow-list closes

"The firehose" means the entire, undecoded `iam.>` event stream — every authorization change on
the platform, as it happens. For any identity that is not `iam-publisher` or `iam-provisioner`
(concretely: `gateway-consumer`, and any future consumer identity provisioned the same way), the
allow-list in `subjects.env` is deliberately shaped to close every route JetStream and core NATS
offer to that firehose, not just the obvious one:

1. **`sub iam.>`** — core NATS subscribe, entirely outside JetStream. A JetStream publish is
   *also* an ordinary NATS publish to the event's subject; any client holding a `subscribe` grant
   on `iam.>` sees every message the instant it is published, filter or no filter. Closed by never
   granting `subscribe` on `iam.*`/`iam.>` to anything but `iam-publisher`'s own narrow inbox.
   The one exception is `gateway-consumer`'s `iam.policy.>` (§3.1): it opens the policy events
   only, which the durable's filter already delivers to the same identity.
2. **A self-created wider consumer** — see §3. Closed by granting no `CONSUMER.CREATE` verb at
   all.
3. **`STREAM.MSG.GET`** (`$JS.API.STREAM.MSG.GET.<stream>`) — JetStream's direct-by-sequence
   message fetch. It reads an arbitrary message by sequence number, walking the whole stream one
   `GET` at a time, completely independent of any consumer or its filter. Closed for
   `gateway-consumer` by never granting `$JS.API.STREAM.>` (or the narrower `STREAM.MSG.GET`
   subject) to it — only the `CONSUMER.*` subjects it actually needs are granted.
4. **`DIRECT.GET`** — JetStream's direct-get API, an alternate bulk-read path alongside
   `STREAM.MSG.GET` that a stream can expose when it has `allow_direct` enabled (this stream does
   not enable it — `NatsEventPublisher::connect`'s stream config leaves it at the JetStream
   default). Like `STREAM.MSG.GET`, it reads messages directly rather than through a filtered
   consumer. Closed the same way as (3): its subject space is not present in `gateway-consumer`'s
   allow-list, so even if a future change enabled `allow_direct` on the stream, this identity would
   still not be able to reach it.

`iam-provisioner` is the one identity that *is* granted `$JS.API.STREAM.>` (route 3 above), and
that is intentional, not an oversight — see §9 for why that is safe.

## 5. JetStream domains re-shape every `$JS.*` grant

Nothing in this deployment currently configures a JetStream domain (there is no `domain` set on
the account or the stream). If a future change introduces one — for cross-account or leaf-node
bridging — **every `$JS.*` subject above changes shape**, and the grants in both `subjects.env`
and `accounts.conf.tmpl` must be rewritten together, or every publish/ack/pull silently starts
failing permission checks against subjects that no longer match:

- The JetStream API prefix moves from `$JS.API.…` to **`$JS.<domain>.API.…`** — e.g.
  `$JS.API.STREAM.INFO.IAM_EVENTS` becomes `$JS.<domain>.API.STREAM.INFO.IAM_EVENTS`, and likewise
  for every other `$JS.API.*` subject in this account (`STREAM.CREATE`, `CONSUMER.MSG.NEXT`,
  `CONSUMER.INFO`, `INFO`, `STREAM.>`, `CONSUMER.>`).
- The ack-publish subject moves from `$JS.ACK.<stream>.<consumer>.>` to
  **`$JS.ACK.<domain>.<account-hash>.<stream>.<consumer>.>`** — `gateway-consumer`'s
  `$JS.ACK.IAM_EVENTS.gateway-cache-invalidator.>` grant would need the domain and account-hash
  segments spliced in.

Do not pre-emptively add these forms speculatively — an ungranted domain segment is inert, but an
extra wildcard segment loosens the grant beyond what today's undomained deployment needs. Add them
only alongside the domain configuration change itself, in the same commit that turns the domain on.

## 6. `root_ca_bundle` replaces the system trust store

`PublisherConfig.root_ca_bundle` (`config.rs`) is a path to a PEM file of root CAs used to verify
the broker's TLS certificate. It is easy to assume this *adds* to the platform's normal CA trust
store; it does not — the async-nats client assigns the certificates named here instead of loading
the OS trust store at all. Naming only your private CA and later moving the broker behind a
publicly-trusted certificate (or vice versa) is a total outage that surfaces as a bare TLS
handshake failure, not an obviously-related config error. If a deployment needs to trust more than
one CA (a private CA today, a public one during a migration, etc.), concatenate every CA it needs
into that one file — there is no way to layer multiple `root_ca_bundle` values. Leaving the field
unset keeps the pre-SMA-493 behavior of trusting the system store.

## 7. The inbox-prefix coupling

Every user above is granted `subscribe` on exactly one inbox prefix — `_INBOX_IAM_PUB.>` for
`iam-publisher`, `_INBOX_GW.>` for `gateway-consumer`, `_INBOX_PROV.>` for `iam-provisioner` — and
no other inbox (`gateway-consumer`'s only other subscribe grant is the event subject
`iam.policy.>`, §3.1). That prefix is not decorative: it is what stops one client in the account from
reading another client's JetStream acks or pull deliveries, both of which land on the requesting
client's own inbox rather than on any event subject (§2, §3). The account-side grant only helps if
the client is actually configured to use that exact prefix. `PublisherConfig.inbox_prefix`
(`config.rs`) is wired straight into `async_nats::ConnectOptions::custom_inbox_prefix` — if it is
left unset, the client falls back to the library default `_INBOX`, which does not match any grant
in this account and will not be denied at connect time. **The failure mode is a hang, not an
error**: every publish (or pull) times out waiting for a reply the broker silently refuses to
deliver, with nothing at the call site distinguishing it from a slow or unreachable broker. When
provisioning SMA-492's `gateway-consumer` identity, its client config **must** set
`custom_inbox_prefix` to `_INBOX_GW` — matching `CONSUMER_INBOX_PREFIX` in `subjects.env` exactly,
including the trailing behavior of the `.>` wildcard in the grant (the configured prefix should
NOT itself include the trailing `.>`; async-nats appends the reply suffix).

## 8. Credential delivery must be atomic

`NatsEventPublisher::connect` re-reads the `.creds` file named by `credentials_file` on **every**
connection attempt (not just once at boot) — that is what makes credential rotation possible
without a restart: replace the file, and the next reconnect picks it up. That same behavior makes
*how* the file is replaced load-bearing. If an updater truncates the existing file and rewrites it
in place, there is a window — arbitrarily short, but real — during which a concurrent read
(triggered by a reconnect racing the rewrite) sees a partially-written, unparseable file, and the
connection attempt fails with a credentials error instead of picking up either the old or the new
value cleanly.

The fix is to make the replacement atomic at the filesystem level, never a truncate-and-rewrite:

- A Kubernetes `Secret` volume mount already does this correctly — the kubelet publishes an
  updated secret by writing the new content to a new directory and swapping a symlink, which is
  atomic from the container's point of view.
- A hand-rolled rotation script must write the new `.creds` to a temporary file in the same
  directory and `rename(2)` it over the target path — `rename` is atomic on the same filesystem,
  so a concurrent reader always observes either the complete old file or the complete new one,
  never a partial write.

## 9. `iam-provisioner.creds` is an operator artifact, not a deployment secret

`iam-provisioner` is deliberately the most powerful identity in the account — `$JS.API.STREAM.>`
and `$JS.API.CONSUMER.>` include stream/consumer creation, deletion, and (per §4) message-by-
sequence reads across the whole stream. That is acceptable *only* because this identity is never
deployed with a running service — `provision.sh` runs it once, by hand, from an operator's
machine, to set up the stream and durable consumer that `iam-publisher` and `gateway-consumer`
then use with their much narrower grants. Treat `iam-provisioner.creds` (and the operator-mode
`nsc` keys that mint it) the way you would treat a database superuser password: store it in your
credential manager, never bake it into a container image, Helm values file, or CI secret used by
an actual deployment. If it leaks, rotate the account's operator/signing keys, not just this one
user.

## 10. The stream-config coupling with `PublisherConfig`

`provision.sh` §4 creates the `IAM_EVENTS` stream with specific values — `--storage file`,
`--retention limits`, `--dupe-window 1h`, `--max-age 7d`, `--subjects "iam.>"`. These are not
arbitrary operator defaults; they must match `PublisherConfig::default()`
(`rs/crates/services/paigasus-iam/src/config.rs`) field-for-field, because
`NatsEventPublisher::connect` **adopts** an existing stream rather than reconciling it
(SMA-471 D7): if the live stream
is weaker than the service's configured expectations — a shorter `duplicate_window`, a
non-`Limits` retention policy, non-`File` storage, or subjects that don't cover `iam.>` — boot
fails outright with a typed drift error rather than silently reshaping (or worse, silently
under-protecting) a stream other consumers may already depend on.

**Storage type** is never editable on a live JetStream stream — the Stream Update API rejects any
storage-type change outright — so fixing a `storage` drift always means deleting and re-creating
the stream: a maintenance window and, unless messages are drained first, data loss. **Retention
policy** is editable except to or from `workqueue`, which the Update API also rejects outright;
the drift check above can only ever flag a live retention of `interest` or `workqueue` (it wants
`Limits`), so in practice an `interest` drift is fixable with `nats stream edit`, but a `workqueue`
drift needs the same delete-and-re-create treatment as `storage`. **`duplicate_window`** IS
editable in place with `nats stream edit` — verified empirically against a real broker (SMA-493
CodeRabbit round 1) — despite an earlier revision of this document listing it alongside the two
truly immovable cases above.

Keep `provision.sh`'s stream-add flags and `PublisherConfig::default()` in sync by hand; there is
no automated gate cross-checking them the way `check-subjects.sh` cross-checks the permission
lists (nothing in CI can reach a running broker to inspect a live stream's config).

## 11. The `PAIGASUS_GATEWAY` account and the decision-log identities (SMA-755)

The gateway tool decision log has its own account, `PAIGASUS_GATEWAY`. A full `GATEWAY_DECISIONS`
stream can then not use the disk of `IAM_EVENTS` (anti-goal A6). The account has its own
JetStream limits: no memory storage, a disk limit, 2 streams and 16 consumers. The lists of the
three identities are in `subjects.env` (`GW_DEC_*`). This table renders that file; if they differ,
`subjects.env` is correct.

| identity | deployed as | publish (allow) | subscribe (allow) |
|---|---|---|---|
| `gateway-decision-publisher` | the gateway decision relay | `gateway.decision.>`, `$JS.API.INFO`, `$JS.API.STREAM.INFO.GATEWAY_DECISIONS`, `$JS.API.STREAM.CREATE.GATEWAY_DECISIONS`, `$JS.API.STREAM.INFO.KV_GATEWAY_DECISIONS_GEN`, `$JS.API.DIRECT.GET.KV_GATEWAY_DECISIONS_GEN.>`, `$KV.GATEWAY_DECISIONS_GEN.>` | `_INBOX_GW_DEC.>` |
| `gateway-decision-reader` | the export CLI of an operator; never the gateway | `$JS.API.CONSUMER.CREATE.GATEWAY_DECISIONS`, `…CREATE.GATEWAY_DECISIONS.>`, `$JS.API.CONSUMER.DELETE.GATEWAY_DECISIONS.*`, `$JS.API.CONSUMER.MSG.NEXT.GATEWAY_DECISIONS.*`, `$JS.API.CONSUMER.INFO.GATEWAY_DECISIONS.*` | `_INBOX_GW_RD.>` |
| `gateway-decision-provisioner` | nobody; operator tooling only | `$JS.API.INFO`, `$JS.API.STREAM.>` | `_INBOX_GW_PROV.>` |

- The publisher subscribes only to its inbox, for the PubAck. It cannot read, delete or purge the
  stream. A relay that reads its own stream would give a leak of the decisions a second path.
- The publisher has `$JS.API.INFO`. Before the relay makes the stream, it reads the account storage
  limit and sets `max-bytes` to 90 % of it when that is below the default. Without this, a smaller
  account disk makes the broker refuse the create and the relay stalls. `$JS.API.INFO` is read-only:
  it returns the usage and the limits of the account of the caller only.
- The reader uses an ordered consumer (P-13). The first create has no name. A recreate after a gap
  has a name and a filter suffix, and it deletes the old consumer first. So the grants have both
  `CREATE` forms and `DELETE`. There is no `$JS.ACK` grant: an ordered consumer has no acks. The
  reader cannot publish to `gateway.decision.>` and cannot purge.
- The provisioner makes the stream (P-15). The `nats` CLI calls `$JS.API.INFO` at start. The
  provisioner also has `$JS.API.STREAM.>`, which the publisher does not have. It is an operator
  artifact, as `iam-provisioner` is (§9).
- A user in this account cannot see `IAM_EVENTS`: it is in another account. `tests/nats_permissions.rs`
  proves it. It also proves that an ordered consumer works under `max_mem: 0`.
- The reader also has `$JS.API.CONSUMER.INFO.GATEWAY_DECISIONS.*`. After a client reconnect, the pull
  stream of an ordered consumer in async-nats 0.52 calls `consumer.get_info()` (`consumer/pull.rs:990`).
  Without this grant, a NATS reconnect during an export makes the export fail.
- `provision-decisions.sh` passes `--inbox-prefix _INBOX_GW_PROV` to the `nats` CLI. The provisioner may
  subscribe only to that prefix, so the default `_INBOX` reply subscription would be refused.

### 11.1 The generation bucket `GATEWAY_DECISIONS_GEN` (SMA-821)

- The three KV grants of the publisher exist for the reset record of the decision log (SMA-821). They touch
  only the bucket `GATEWAY_DECISIONS_GEN`, which is the stream `KV_GATEWAY_DECISIONS_GEN`. Each grant was
  measured (leave-one-out): with any one of the three removed, `the_decision_publisher_uses_the_generation_bucket`
  fails at the call that needs it (`STREAM.INFO.KV_…` at get, the direct get at `entry`, `$KV.…` at
  `create`/`update`).
- The publisher has NO `$JS.API.STREAM.CREATE.KV_GATEWAY_DECISIONS_GEN` grant (D20). Only
  `provision-decisions.sh` makes the bucket. MEASURED on a live broker: while the bucket does not exist,
  that grant lets the publisher make the stream `KV_GATEWAY_DECISIONS_GEN` as a mirror (or with `sources`)
  of `GATEWAY_DECISIONS`, with `allow_direct`. The publisher then reads every decision with its direct get
  grant. `the_decision_publisher_cannot_reach_beyond_the_generation_bucket` refuses this create. Without the
  bucket, the relay reports the check error kind `no_bucket`, and publishes still work.
- A known limit: `$JS.API.STREAM.CREATE.GATEWAY_DECISIONS` (from SMA-755) has the same shape. While
  `GATEWAY_DECISIONS` does not exist, the publisher can make it as a mirror of the generation bucket. That
  bucket holds only generation records, so this stays. Once both streams exist, the account cap of 2 streams
  refuses any further create.
- The publisher has no right on `GATEWAY_DECISIONS` itself: no read, delete or purge. The direct get is
  limited to the bucket (`$JS.API.DIRECT.GET.KV_GATEWAY_DECISIONS_GEN.>`), and it is the only read path.
  `the_decision_publisher_cannot_reach_beyond_the_generation_bucket` proves the refusals.
- The account allows 2 streams. `GATEWAY_DECISIONS` and `KV_GATEWAY_DECISIONS_GEN` use both. A third stream
  needs a higher `--js-streams` in `provision-decisions.sh`.
- `provision-decisions.sh` makes the bucket with `nats kv add --history 1 --storage file --replicas 1
  --max-bucket-size <DECISIONS_GEN_MAX_BYTES>`. It also fails when the 10 % of the account disk that the
  stream leaves free cannot hold the bucket. The smallest disk is then about 10 MiB.
- `nsc add user` fails for a user that exists, so an existing `gateway-decision-publisher` would keep its old
  grants. The script runs `nsc edit user` for this user only. `nsc edit user` only adds grants, so a subject
  that you remove from `subjects.env` stays on an existing user. The script revokes
  `$JS.API.STREAM.CREATE.KV_GATEWAY_DECISIONS_GEN` with `nsc edit user --rm`. This flag exits 0 when the grant
  is absent, so the script stays idempotent. The new creds file holds the new grants.
- Upgrade order: first the new grants, the new creds and the bucket (run `provision-decisions.sh` with
  `PUSH=1`), then the new gateway binary with the new creds. The bucket must exist before the deploy: the
  relay does not make it.
- Deployment rule: two gateway deployments with their own Redis, or with another Redis DB index, must not
  share one NATS account AND one Redis prefix. The default prefix `paigasus:gateway` collides. Give each
  deployment its own NATS account or its own Redis prefix.
