# SMA-787: the event inbox — design

Linear: [SMA-787](https://linear.app/smaschek/issue/SMA-787). Builds on ADR-0028 (Notion, Proposed;
full design text in `docs/superpowers/specs/2026-11-18-sma-785-agent-host-adr-design.md` §4) and on
the SMA-786 skeleton (`docs/superpowers/specs/2026-11-19-sma-786-agent-host-skeleton-design.md`).
Blocks SMA-788 (timers) and SMA-790 (worker run loop).

Revision 2: folds in the builder check and the adversarial challenge (see §16).

## 1. Intent

Every trigger becomes a row in a durable inbox: an HTTP event, a NATS event, and later a timer, a
user message and an approval reply. Nothing calls the agent directly. One worker at a time holds the
lease of an inbox and claims its pending events as one batch. So two runs cannot race on one inbox,
and a crash cannot lose an event.

This issue builds the write side (ingress) and the claim side. SMA-790 builds the run loop, the
Helikon session and the one commit transaction on top of it.

## 2. Decisions made in the brainstorm (Sven, 2026-11-20)

- **D1 — Boundary with SMA-790: choice A.** SMA-787 owns the full claim side: the lease, the
  heartbeat, the per-thread claim of ADR (f), the minimal `run` and `thread` tables, and a minimal
  complete and release call. With these, the crash test runs without the real worker. SMA-790 adds
  the commit transaction of ADR (b) and the Helikon session.
- **D2 — Ingress authorisation: choice B.** A caller may write to an inbox when it is the inbox
  principal, **or** when IAM allows a new action `WriteInboxEvent` on the organization of the
  inbox. This needs a change in IAM (§6).
- **D3 — The playback is accepted.** It includes the ADR statuses (no `claimed` status),
  CloudEvents structured mode only, a new `paigasus-cloudevents` crate, a NATS source through
  config-file channels, and `Queue` as the only accepted mid-run policy.
- **D4 — Lease and claim SQL: approach 1A.** The SQL is hand-written in one repository adapter,
  with bound parameters, through the SeaORM connection. SeaORM entities cover only the simple reads
  and inserts.
- **D5 — The property test: approach 2A.** It uses a real Postgres and forces expiry with a
  test-only statement. It does not sleep.
- **D6 — NATS tenancy: T1.** An org filter that fails closed (§9.4). Sven approved the spec with
  this option at Gate 1 (2026-11-20).

## 3. Goals, non-goals, anti-goals

**Goals**

- G1. `POST /v1/inboxes/{inbox}/events` accepts a CloudEvent, authorises the caller, writes one row
  and returns `202`. A duplicate also returns `202` and writes no row.
- G2. A NATS JetStream source writes to the inbox through the same insert path.
- G3. One active lease per inbox, with a fencing token, a heartbeat and the database clock.
- G4. A claim takes the claimable events of one resolved thread, up to a batch limit (ADR (f)).
- G5. A crashed worker loses nothing: after its lease expires, a second worker claims the same
  events and finishes them.
- G6. `Queue` mid-run policy: an event that arrives during a run stays pending for the next claim.

**Done when (from the issue)**

- Kill the worker mid-run: the events become claimable again, and a second worker finishes them
  (§10.3).
- Two workers on one inbox never both hold a valid claim (§10.1 and §10.2).
- Handler p99 stays under 50 ms with the agent turned off (§10.4).

**Non-goals**

- The run loop, Helikon, `PgRunSession` and the commit transaction of ADR (b). These are SMA-790.
- Raw webhooks without a CloudEvents envelope. A later channel issue adds them.
- The `fold` and `interrupt` mid-run policies. Both need the Helikon turn-boundary change (ADR (h)).
- Deferred tool calls (SMA-795). The `thread.open_deferral` column exists, and it is always false
  in this issue.
- A job that deletes old `done` rows. Without it, `done` rows stay, so the dedup window has no end.
  A later issue adds the retention job.
- IAM does not move to `paigasus-cloudevents`. The three copies of the IAM auth middleware
  (gateway, agent host, IAM itself) are not merged into one crate here. That goes to the follow-up
  sweep.
- An `InboxEvent` proto message and a Rust proto3 JSON strategy (SMA-786 §6.4 handoffs). SMA-787
  puts no `agents/v1` message into `data`: HTTP callers and IAM send their own JSON. The first
  issue that puts an `agents/v1` message into an inbox event decides both. That is SMA-795
  (approval replies), unless SMA-799 comes first.
- No Helm chart change, no image publish (SMA-786 decisions stand).
- No DPoP on the agent-host ingress. API keys and OIDC bearer tokens only.

**Anti-goals** (the design fails, even if it works, when one of these is true)

- AG1. Two workers both hold a valid claim on one inbox at the same time.
- AG2. An event that got `202`, or that NATS delivered, is lost.
- AG3. A caller writes to an inbox it is not allowed to write to, or chooses the `source` or
  `actorprn` of a row.
- AG4. A worker clock extends a lease. (A worker may stop early on its own monotonic clock, §8.6.
  Only the database clock decides that a lease is still valid.)
- AG5. One run claims events of two different threads.
- AG6. Events of one organization reach the inbox of a different organization.

## 4. The CloudEvents crate: `paigasus-cloudevents`

A new library crate `rs/crates/libs/paigasus-cloudevents` (ADR (j)). No I/O, and no HTTP or NATS
dependency.

- It parses, validates and builds CloudEvents 1.0 JSON structured mode.
- Required attributes: `specversion` (`"1.0"`), `id`, `source`, `type`. Optional: `time`
  (RFC 3339), `subject`, `datacontenttype`, `dataschema`, `data`. `data_base64` is rejected in v1.
- **Extensions are typed.** A value is a string, an integer or a boolean, as CloudEvents 1.0
  allows in JSON. The crate checks the name rule (lowercase `a-z0-9`, at most 20 characters).
  Typed accessors: `schemaversion` (integer), `actorprn`, `correlationid`, `threadid` (strings).
  IAM writes `schemaversion` as a JSON number (`paigasus-iam/src/adapters/events/cloud_event.rs:40`).
- **Length limits.** `id` and `type` at most 256 characters; `source`, `subject` and `dataschema`
  at most 1024; each string extension at most 1024. A longer value is a validation error (ingress
  `400`). So no value can exceed the B-tree entry limit of the unique key.
- **Time.** The crate uses `jiff::Timestamp`. The build side always prints exactly six fractional
  digits, and truncates below one microsecond (it does not round), as IAM's chrono
  `SecondsFormat::Micros` does. The build side writes a hyphenated UUID `id`.
- **Fixtures.** Byte fixtures that IAM's encoder produced, including times with trailing zeros
  (`.100000`) and with a zero fraction. Two tests: the crate **parses** each IAM fixture, and the
  crate **builds** the same bytes from the same values.

## 5. HTTP ingress

### 5.1 Route

`POST /v1/inboxes/{inbox}/events`. `{inbox}` is the inbox UUID (the `inbox.id` column), not the
PRN. The org comes from the inbox row.

The handler uses house extractors (`repo:http-extractor-envelope`): a `UuidPath`-style extractor for
the path and an `EnvelopeBytes`-style extractor for the body. The agent host gets its own copies,
because the IAM and gateway copies live in service crates.

### 5.2 Order of checks and responses

1. **Content type.** Only `application/cloudevents+json`, compared case-insensitively; parameters
   such as `charset=utf-8` are allowed. Otherwise `415` `unsupported-content-type`.
2. **Body size.** At most `ingress.max_body_bytes`. A larger `Content-Length` gives `413`
   `request-too-large` before authentication. A chunked body that grows too large gives `413`
   when it is read, which is after authentication. The order of steps 2 and 3 is normative only
   for a body with `Content-Length`.
3. **Authentication** through IAM (§6.2). `401` `invalid-token` when there is no credential or an
   inactive one. `503` `iam-unavailable` when IAM cannot be reached.
4. **Inbox lookup** by UUID (`InboxRepository::get_by_uuid`).
5. **Authorisation** (§6.3). An unknown inbox and a refused caller both give `403` `forbidden` with
   the same body, so a caller cannot probe which inbox UUIDs exist. The two paths differ in latency
   (an unknown inbox makes no `IsAuthorized` call). This is accepted: inbox UUIDs are random.
6. **Envelope parse and validation** with `paigasus-cloudevents`. Invalid JSON gives `400`
   `invalid-request-body`; a valid JSON that is not a valid CloudEvent gives `400`
   `invalid-request-schema`. A `type` that starts with `agents.` gives `400`
   `invalid-request-schema`: that prefix is reserved for events the host makes.
7. **Host-set fields** (ADR (e)):
   - `source` is replaced by `urn:paigasus:agents:http:<principal-prn>`, where the PRN is the
     canonical form from `PrincipalId` (SMA-786 §6.3). So two callers cannot collide in the dedup
     key, and a caller cannot forge the source of another channel.
   - `actorprn` is set to the same canonical principal PRN. A caller-sent value is dropped.
   - `threadid`: kept only if it parses as a `ThreadId` and that thread belongs to this inbox.
     Otherwise it is dropped, and the event starts a fresh thread. The event is not rejected.
   - `id`, `type`, `time`, `subject`, `dataschema`, `data` and other extensions are kept.
8. **Insert** (§7.2). `Inserted` and `Duplicate` both give `202` with `{"id": "<event uuid>"}`. A
   duplicate returns the id of the stored row, so a client retry gets the same body.
9. **Database error** on the lookup or the insert: `503` with a new code `store-unavailable`
   (retryable). It needs an `error.proto` value and a `ci/error-registry/check.py` row
   (`repo:error-code-single-site`).

Every error uses the agent host's `{"error":{"code","message"}}` envelope. Each file that spells a
code gets its `emits` row in the error registry.

Trust rule for later issues: SMA-788, SMA-790 and SMA-795 make each trust decision on `source`
(set by the host), never on `type` alone.

### 5.3 `GET /v1/service-info`

SMA-786 handed this route to SMA-787. It sits behind the same authentication as the events route,
with no authorisation check, as `service_info.proto` requires. Like the gateway's discovery route
(ADR-0020 D4), it accepts a caller that IAM answers with `identity-not-provisioned`, because the
console (SMA-799) calls it first. The service slug is `agents`.

## 6. Authentication and authorisation through IAM

### 6.1 The IAM change

- A new action `WriteInboxEvent` in `paigasus-iam-core`:
  - `authz/action.rs`: the enum value, the wire form `"WriteInboxEvent"`, and every list that names
    `InvokeModel` (lines 138, 279, 371 today). `Action::ALL.len()` moves from 54 to 55 (line 393).
  - `authz/schema.rs`: the action in the Cedar schema with
    `appliesTo { principal: [Principal], resource: [Root, Organization, Team, Project] }`.
  - It is a write action, so it joins the derived `forbid-archived-writes` list. That changes the
    starter policy: `STARTER_POLICY_REVISION` from 7 to 8 and `EXPECTED_STARTER_CONTENT_HASH`
    (`roles.rs:75`, `:104`), and the `STARTER_POLICY_IDS` and "nine system roles" text where they
    change.
- A new dedicated role `agent_event_writer` in `authz/roles.rs` with only `WriteInboxEvent`, as
  `gateway_user` holds only `InvokeModel` (SMA-446 D10).
- **`org_admin` does not get `WriteInboxEvent`** (my call, revision 2). An org-level grant lets the
  holder write into the inbox of every member of the org, and those agents act for those members
  (ADR (k)). This follows the `InvokeModel` precedent (SMA-635 D4, D10). A role test pins it, like
  `roles.rs:896` ("org_admin does not hold InvokeModel").
- No TS change is needed. `ts/packages/paigasus-console-core/tests/unit/action-names.test.ts`
  checks only that the console's names are a subset of the Rust catalog. The new role is granted
  through the existing `GrantRole` API. A console control for it is not in this issue.

### 6.2 Authentication

The agent host gets an outbound IAM gRPC adapter behind an `Iam` port. It copies the API-key and
bearer parts of `rs/crates/services/paigasus-gateway/src/adapters/iam/client.rs` and
`.../http/auth.rs`, and not their DPoP part.

- The credential is `Authorization: Bearer <secret>`. As in the gateway, the host tries
  `IntrospectApiKey` first and then `Introspect`. It does not branch on the key prefix
  (`gateway/src/adapters/http/auth.rs:345-351`).
- `Authorization: DPoP …` gives `401` `invalid-token`, with no DPoP challenge. A DPoP-bound user
  token sent as a bearer is refused by IAM's `Introspect`, which also gives `401`.
- The `paigasus-org` header is not used. The org comes from the inbox.
- IAM calls per request:

  | Credential | Owner | Non-owner |
  |---|---|---|
  | API key | 1 (`IntrospectApiKey`) | 2 (+ `IsAuthorized`) |
  | OIDC token | 2 (`IntrospectApiKey` refuses, then `Introspect`) | 3 (+ `IsAuthorized`) |

### 6.3 Authorisation (D2)

All PRN comparisons compare UUIDs: `PrincipalId::parse(<introspected PRN>).uuid()` against
`inbox.principal_id`. Never a string comparison.

1. **Key scope.** For an API key, the key's `scope_prn` must be Root or exactly the organization of
   the inbox. A key scoped to another org, a team or a project gets `403`. This follows the
   gateway rule that a key is issued under one scope (gateway `auth.rs:271-293`, D5).
2. **Owner path.** If the principal is the inbox principal:
   - an API key passes step 1, then is allowed;
   - an OIDC user is allowed only when the inbox org is in the `memberships` of the `Introspect`
     response. That costs no extra IAM call.
   Accepted gap: the owner path does not run IAM's `forbid-archived-writes` policy. An owner in an
   archived org can still write to their own inbox. The agent acts only for that owner, so the
   effect stays with the owner.
3. **Non-owner path.** The gateway's self-query: `IsAuthorized` with the caller's own credential,
   the caller's own principal PRN, action `WriteInboxEvent`, and the resource = the organization
   PRN of the inbox. Allowed: continue. Refused: `403`.

There is no cache (as in the gateway, SMA-635 §4.1).

## 7. Storage

### 7.1 Tables

New migration(s) after `m0001_create_inbox`. The file split is the builder's call.

```sql
CREATE TABLE thread (
  id             uuid PRIMARY KEY,           -- the Helikon session id (ADR (f))
  inbox_id       uuid NOT NULL REFERENCES inbox(id),
  created_at     timestamptz NOT NULL DEFAULT now(),
  last_run_at    timestamptz,                -- null: no run on this thread has completed
  closed         boolean NOT NULL DEFAULT false,
  successor_id   uuid,
  predecessor_id uuid,                       -- the expired thread this one continues, if any
  open_deferral  boolean NOT NULL DEFAULT false,  -- always false until SMA-795
  UNIQUE (id, inbox_id),
  FOREIGN KEY (successor_id, inbox_id)   REFERENCES thread (id, inbox_id),
  FOREIGN KEY (predecessor_id, inbox_id) REFERENCES thread (id, inbox_id)
);

CREATE TABLE inbox_lease (
  inbox_id    uuid PRIMARY KEY REFERENCES inbox(id),
  holder      text NOT NULL,   -- diagnostic only; never in a WHERE clause
  token       bigint NOT NULL,
  expires_at  timestamptz NOT NULL
);  -- rows are never deleted, so a token never repeats (ADR (c))

CREATE TABLE run (
  id           uuid PRIMARY KEY,
  inbox_id     uuid NOT NULL REFERENCES inbox(id),
  thread_id    uuid NOT NULL,
  lease_token  bigint NOT NULL,
  status       text NOT NULL,       -- 'running' | 'completed' in SMA-787; SMA-790 adds the rest
  started_at   timestamptz NOT NULL DEFAULT now(),
  finished_at  timestamptz,
  UNIQUE (id, inbox_id),
  FOREIGN KEY (thread_id, inbox_id) REFERENCES thread (id, inbox_id)
);

CREATE TABLE run_event (            -- ADR (b) step 3: the claimed event ids of a run
  run_id    uuid NOT NULL REFERENCES run(id),
  event_id  uuid NOT NULL,          -- references inbox_event(id)
  PRIMARY KEY (run_id, event_id)
);

CREATE TABLE inbox_event (
  id              uuid PRIMARY KEY,
  inbox_id        uuid NOT NULL REFERENCES inbox(id),
  source          text NOT NULL,
  ce_id           text NOT NULL,
  ce_type         text NOT NULL,
  thread_id       uuid,             -- see below: two states
  envelope        jsonb NOT NULL,   -- the full CloudEvent after the host-set fields
  received_at     timestamptz NOT NULL DEFAULT now(),
  status          text NOT NULL DEFAULT 'pending',  -- CHECK: 'pending' | 'done' | 'dead'
  attempts        integer NOT NULL DEFAULT 0,
  not_before      timestamptz NOT NULL DEFAULT now(),
  claimed_run_id  uuid,
  last_error      text,
  UNIQUE (inbox_id, source, ce_id),
  FOREIGN KEY (thread_id, inbox_id)      REFERENCES thread (id, inbox_id),
  FOREIGN KEY (claimed_run_id, inbox_id) REFERENCES run (id, inbox_id)
);
CREATE INDEX inbox_event_claimable ON inbox_event (inbox_id, received_at) WHERE status = 'pending';
```

- The composite foreign keys make the schema enforce that a thread, a run and an event belong to
  one inbox (AG5, AG6).
- **`inbox_event.thread_id` has two states.** Before the first claim, it is the thread that
  ingress kept (or null). From the first claim on, it is the thread of the run that claimed the
  event (§8.4 step 8). The raw `threadid` that the caller sent stays in `envelope`. The console
  shows `thread_id` as "thread" and the envelope value as "sent thread".
- `status` has a `CHECK` constraint. The domain `EventStatus` stays `Pending | Done | Dead`. There
  is no `Claimed` and no `Failed` status: a claim is the pair (`claimed_run_id`, a `running` run
  under the current live token), and `failed` is a run outcome (ADR (b)). This replaces the issue's
  status list on purpose (D3).
- `last_error` is written by SMA-790. SMA-787 writes it only when it moves an event to `dead`.
- Column types and index details beyond this sketch are the builder's call. The unique key, the
  composite keys and the partial index are not.

### 7.2 Insert with dedup

`INSERT … ON CONFLICT (inbox_id, source, ce_id) DO NOTHING RETURNING id`. No row returned means a
duplicate. The adapter then reads the stored row's `id`, `ce_type` and `envelope->'data'`, and
compares type and data with the new event **in Rust** (`serde_json::Value` equality, so two absent
`data` values are equal). A difference is logged at `warn` and counted. The port returns
`Inserted { id } | Duplicate { id, payload_matches }`.

## 8. The claim side

All of §8 is hand-written SQL in one adapter (D4). Every time that decides that a lease is valid is
the database clock (AG4): `now()` in a short transaction, `clock_timestamp()` where a statement can
wait on a lock (ADR (a), (c)).

### 8.1 Ports (in `paigasus-agent-host-core`)

The builder may refine names. The responsibilities and the error shapes are fixed:

- `InboxRepository` (from SMA-786, revised): `get_by_uuid(Uuid) -> Option<Inbox>` (the route has
  only the UUID); `insert_event(NewInboxEvent) -> InsertOutcome`. `NewInboxEvent` has no
  host-owned fields (`status`, `attempts`, `claimed_run`, `received_at`, `not_before`), so ingress
  cannot set them. Every method returns `RepositoryError` on a store error.
- `LeaseRepository`:
  - `acquire_next(holder, ttl) -> Result<Option<LeaseHandle>, StoreError>` (§8.3).
  - `acquire(inbox, holder, ttl) -> Result<Option<LeaseHandle>, StoreError>`: the same for one named
    inbox (tests and the console).
  - `heartbeat(&LeaseHandle, ttl) -> Result<(), HeartbeatError { LeaseLost, Store }>`. A database
    error is `Store`, never `LeaseLost` (SMA-786 §6.2).
  - `release(LeaseHandle)`: consumes the handle and closes any open claim.
- `ClaimRepository`:
  - `claim(&mut LeaseHandle, &ClaimConfig) -> Result<Option<Claim>, ClaimError { LeaseLost,
    ClaimOpen, Store }>` (§8.4). `Ok(None)` means no claimable event; the lease is still held.
  - `complete(&mut LeaseHandle, Claim) -> Result<Vec<EventId>, CompleteError { LeaseLost, Store }>`
    (§8.5). It returns the ids it marked `done`. SMA-790 replaces it with `RunCommitter`. The two may
    stay side by side until then.
- `LeaseHandle { inbox: InboxId, token: LeaseToken, holder: String, open_claim: Option<RunId>,
  poisoned: bool }`. It is **not** `Clone`. The `ClaimOpen` rule is a method on the handle, so
  each adapter does not implement it again. A poisoned handle (§8.4) refuses every call.
- `Claim { run: RunId, thread: ThreadId, lease_token: LeaseToken, fresh_thread: bool, events:
  Vec<InboxEvent> }`.

### 8.2 One owner for the lease row of a handle

The `LeaseKeeper` (§8.6) owns the `LeaseHandle` inside a `tokio::sync::Mutex`. Every write to the
lease row of that handle takes the mutex for the whole transaction: the heartbeat, the token
rotation in `claim`, `complete` and `release`. So a heartbeat never runs while a claim rotates the
token, and a heartbeat always sends the current token. The run code reaches `claim` and `complete`
only through the keeper.

### 8.3 Take the lease (ADR (c))

`acquire_next` selects candidate inboxes and takes the first one it can:

1. Candidates: inboxes with at least one claimable event (§8.4 step 4) and no live lease, in order
   of their oldest claimable `received_at`, so inbox lag stays bounded. The query locks the
   candidate `inbox` rows with `FOR NO KEY UPDATE OF inbox SKIP LOCKED`, `LIMIT` a small number
   (builder's call, for example 8). `FOR NO KEY UPDATE` does not conflict with the `FOR KEY SHARE`
   lock that an `inbox_event` foreign-key insert takes, so ingress never waits for an acquirer. An
   inbox with no lease row yet has an `inbox` row to lock, so a new inbox works.
2. For each candidate, in order, run the take. It returns a token or no row. No row: try the next
   candidate. A token: commit and return the handle.

```sql
-- take: works also for an inbox with no lease row
INSERT INTO inbox_lease (inbox_id, holder, token, expires_at)
VALUES ($inbox, $holder, 1, now() + $ttl)
ON CONFLICT (inbox_id) DO UPDATE
  SET token = inbox_lease.token + 1, holder = EXCLUDED.holder, expires_at = now() + $ttl
  WHERE inbox_lease.expires_at < now()
RETURNING token;

-- heartbeat: zero rows = lease lost
UPDATE inbox_lease SET expires_at = clock_timestamp() + $ttl
WHERE inbox_id = $inbox AND token = $t AND expires_at > clock_timestamp();
```

### 8.4 Claim (ADR (b) claim transaction, ADR (f))

`claim` first checks the handle: a poisoned handle or an open claim returns `ClaimOpen` (or a
poisoned error) and touches no row. Then one short transaction:

1. **Rotate the token** if this is not the first claim under the lease (ADR (c), "Interrupt"):
   `UPDATE inbox_lease SET token = token + 1 WHERE inbox_id = $i AND token = $t AND expires_at >
   clock_timestamp() RETURNING token`. Zero rows: roll back, return `LeaseLost`. The first claim
   after a take uses the token of the take.
2. **Lock the lease row and check it** (`FOR UPDATE`; token and `expires_at > clock_timestamp()`).
   Fail: roll back, `LeaseLost`.
3. **Move exhausted events to `dead`** (§8.4.1).
4. **Select the claimable events.** An event is claimable when all are true:
   - `status = 'pending'` and `not_before <= now()`;
   - `claimed_run_id` is null, or its run is not `running`, or its run's `lease_token` is not the
     current live token of the inbox lease (so a run of a crashed worker frees its events, with no
     reaper);
   - `attempts < max_attempts`;
   - its resolved thread has no open deferral (always true in SMA-787).
5. **Resolve threads.** An event's `thread_id` resolves through `successor_id` to the last thread
   of the chain, with a recursive CTE that has a cycle guard (`CYCLE id SET is_cycle USING path`).
   A cycle is a data error: log it and treat the event as having no thread.
6. **Group.** Take the oldest claimable event by `received_at`, then:
   - resolved thread live (exists, not `closed`, and `last_run_at` null or within `thread_ttl`):
     claim the claimable events with the same resolved thread, in `received_at` order, up to
     `batch_limit`. The run uses that thread.
   - resolved thread not live: claim the claimable events with the same resolved thread, up to
     `batch_limit`. Insert a new thread with `predecessor_id` = the resolved thread.
   - no thread: claim the claimable events with no `thread_id`, up to `batch_limit`. Insert a new
     thread with no predecessor.
   - `fresh_thread` is `true` exactly when the run's thread has `last_run_at IS NULL`. This is the
     only definition. A retry after a crash therefore reports `fresh_thread = true`.
7. **Insert the `run` row** (`status = 'running'`, the current token, the thread) and one
   `run_event` row per claimed event.
8. **Set `claimed_run_id` and `thread_id`** on the claimed events, and increment `attempts`. The
   events stay `pending`. `thread_id` becomes the run's thread, so a retry after a crash resolves
   the same events to the same thread, and a crash leaves no orphan thread row.
9. **Commit.** The transaction commits also when no event is claimable, so the rotation and the
   dead moves persist.

After COMMIT returns Ok, the handle takes the new token, and `open_claim` is set when a claim was
made. If COMMIT returns an error, the outcome is unknown: the handle becomes **poisoned**, the keeper
stops heartbeats, and the lease expires. Counters (dead events, claims) are incremented only after
a successful commit.

#### 8.4.1 Dead events

A claim moves an event to `dead` when it would be claimable by §8.4 step 4 except that
`attempts >= max_attempts`. Such an event was claimed `max_attempts` times and never finished. The
claim writes `last_error = 'max attempts reached without a commit'`. A dead event is never claimed
again. (The console replay is SMA-799.)

Accepted limit (ADR (b): a crash counts as an attempt): one event that crashes the worker every
time also uses up the attempts of the other events in its batch. They can go `dead` together with
it. SMA-790 may refine this with its crash classification.

### 8.5 Complete (the minimal commit, until SMA-790)

One transaction, under the keeper's mutex:

1. Lock the lease row; check `token = claim.lease_token`, that this is the handle's current token,
   and `expires_at > clock_timestamp()`. Fail: roll back, `LeaseLost`.
2. `UPDATE run SET status = 'completed', finished_at = now() WHERE id = $run AND status =
   'running' AND lease_token = $claim_token`. Zero rows: roll back, `LeaseLost`.
3. `UPDATE inbox_event SET status = 'done' WHERE id = ANY($claimed) AND claimed_run_id = $run AND
   status = 'pending' RETURNING id`. Only the claimed ids, never other pending events (G6).
4. Update the thread: `last_run_at = now()`. If it has a `predecessor_id`, set that thread's
   `successor_id` to this thread where it is empty.

After COMMIT returns Ok, the handle clears `open_claim`. A COMMIT error poisons the handle, as in
§8.4.

### 8.6 `LeaseKeeper`

A helper in the service crate. It owns the handle (§8.2) and runs the heartbeat in a task:

- **Deadline.** Before each heartbeat it records `sent = Instant::now()`. On success the local
  deadline becomes `sent + lease_ttl - margin` (margin: one `heartbeat_interval`). The keeper waits
  with `tokio::time::sleep_until` on absolute instants, not a relative `sleep` inside `select!`.
- **Lost.** It cancels its `CancellationToken` when a heartbeat returns `LeaseLost`, or when the
  local deadline passes with no successful heartbeat (a database timeout or a `Store` error
  counts). This local stop is allowed by AG4: it can only stop a worker early.
- **Statement timeout.** Each heartbeat runs with a statement timeout shorter than
  `heartbeat_interval`.
- **Life.** The keeper is tied to a guard. Dropping the guard (also during a panic unwind) aborts
  the heartbeat task. So a killed or panicked run stops renewing its lease, and the lease expires.
- **Token type.** The builder uses the `CancellationToken` type that Helikon 0.6.12 accepts in its
  run context (check `paigasus-helikon`; add `tokio-util` to the workspace if that is the type).

### 8.7 Config (agent-host config file)

| Key | Default | Range |
|---|---|---|
| `worker.enabled` | `true` | — (the §10.4 test sets `false`) |
| `lease.ttl_secs` | 30 | 3 to 600; at least 3 × `heartbeat_interval_secs` |
| `lease.heartbeat_interval_secs` | 10 | 1 to 200 |
| `claim.batch_limit` | 20 | 1 to 500 |
| `claim.max_attempts` | 5 | 1 to 100 |
| `claim.thread_ttl_secs` | 86400 | 60 to 31536000 |
| `ingress.max_body_bytes` | 262144 | 1024 to 4194304 |
| `iam.grpc_addr` | required when ingress is on | — |
| `iam.tls` | copy the gateway's `IamTlsConfig` as is, including its CA rule (add to or replace the trust store, per `rs/CLAUDE.md`) | — |
| `iam.call_timeout_ms` | 2000 | 100 to 30000 |
| `mid_run.policy` | `queue` | only `queue` (§11) |

## 9. NATS source

### 9.1 Config

```toml
[nats]
url = "nats://…"
creds_file = "/…/agent-host-consumer.creds"   # through paigasus-nats-creds
stream = "IAM_EVENTS"
durable = "agent-host-inbox"

[[nats.channels]]
subject = "iam.organization.>"   # one filter subject of the durable
inbox = "<inbox uuid>"
source = "urn:paigasus:agents:nats:iam-org"
```

- Each channel maps one subject filter to exactly one inbox (ADR (d)). Config validation rejects:
  two channels whose filters can match the same subject; a `source` that does not start with
  `urn:paigasus:agents:nats:`; two channels with the same `source`.
- The NATS section is optional. Without it, no NATS task starts.
- The channel binding is the authorisation for NATS (ADR (d)), together with the tenancy rule
  (§9.4).
- **Boot order.** Migrations first. Then the NATS task checks that each configured inbox exists and
  stops boot with a config error if one does not. First deployment: boot without channels, create
  the inbox, then add the channel and restart.

### 9.2 The durable

The agent host never creates a stream or a consumer. `ops/nats/provision.sh` provisions the durable
`agent-host-inbox` with:

- `--pull`, `--ack explicit`;
- `--deliver new` (a first start does not replay seven days of IAM events into an inbox);
- `--max-deliver -1` (no limit, so a long database outage cannot drop an event; AG2);
- `--wait` (ack wait) 30 s, longer than the worst insert time;
- one `--filter` per channel subject (the repeated-flag rule in `provision.sh:139-147`).

At boot, the host reads `CONSUMER.INFO` of the durable and compares its `filter_subjects` with the
configured channel subjects. A difference stops boot with a config error.

### 9.3 Behaviour

- IAM publishes each event as a structured-mode CloudEvents JSON body on the subject that equals
  its event type, for example `iam.principal.created`
  (`rs/crates/services/paigasus-iam/src/adapters/events/nats_publisher.rs:452-457`). There are no
  CloudEvents attributes in NATS headers, only `Nats-Msg-Id`.
- A message is parsed with `paigasus-cloudevents`. The host sets `source` from the channel, and
  drops any incoming `actorprn` and `threadid`. A NATS event uses no caller thread in v1. IAM's
  actor is not kept, because no reader needs it and ADR (m) does not show it to the model. The
  `id`, `type`, `time`, `subject` and `data` are kept, so IAM's event id gives the dedup.
- Then the tenancy rule (§9.4). Then the insert, through the same repository call as HTTP.
- Ack rules:
  - `Inserted` or `Duplicate`: ack.
  - Tenancy rule refuses: ack, count, log at `debug`.
  - Database error: nak with a back-off (1 s, then doubling, at most 60 s).
  - A body that does not parse: `term`, count, log at `warn` with the stream sequence. It can never
    parse later.
  - A subject that no channel maps (only possible after a boot-check gap): nak with the maximum
    delay, count, log at `error`, so the metric alerts.
- At-least-once delivery with no delivery limit, plus dedup, gives one row per event (AG2).

### 9.4 Tenancy (D6: T1)

`IAM_EVENTS` holds the events of every organization, and most IAM subjects have no org token. One
channel would move the events of every tenant into one inbox (AG6).

**The rule: an org filter that fails closed.** The host delivers a
NATS event to an inbox only when the event names the inbox org:

- the CloudEvents `subject` is the organization PRN of the inbox org; or
- the `subject` is a PRN whose org segment is the inbox org.

Every other event is acked and skipped (§9.3). An event with no `subject` is skipped. Today this
lets through IAM's organization events and nothing that has no org, for example principal events.
A later issue can add per-type rules (for example, a grant scope inside `data`).

The alternatives are in §13.

### 9.5 NATS identity

A new identity `agent-host-consumer` in `ops/nats/subjects.env`, `provision.sh`, the
`check-subjects.sh` fixture and `permissions.md`. It has its own inbox prefix and only the verbs a
pull consumer on a pre-provisioned durable needs, including `CONSUMER.INFO` for the boot check. It
follows `gateway-consumer`. `repo:nats-permissions` stays green. The gateway has no JetStream
pull-consumer code, only the provisioning, so the consume loop is new code; use the `async-nats`
pull-consumer API.

## 10. Tests

Every Docker test uses `paigasus_test_docker::start_or_skip` and is in the `docker-containers` group
of `rs/.config/nextest.toml`.

### 10.1 Property test: the sequential model (D5)

- Real Postgres, `proptest`. One connection per simulated worker, operations run one after
  another.
- Generated operations by two to four workers on one inbox: `acquire`, `heartbeat`, `claim`,
  `complete` (also with a claim of an older handle), `release`, `insert_event`, and `expire` (a
  statement **in the test** that sets `expires_at` into the past).
- Invariants after every step:
  - at most one worker has a handle whose token is the current token and whose lease is live;
  - at most one `running` run per inbox has the current live token;
  - tokens are unique and strictly increase per inbox;
  - the `done` ids that the successful `complete` calls return are disjoint (each event is done
    at most once);
  - every event that a run claimed has `thread_id = run.thread_id` (AG5);
  - `attempts <= max_attempts`;
  - a `complete` with a stale token or an old claim returns `LeaseLost` and changes no row;
  - a `claim` on a handle with an open claim returns `ClaimOpen`.
- After the sequence: a drain phase (one worker claims and completes until no event is
  claimable). Then every inserted event is `done` or `dead` (AG2).
- Coverage: the test asserts that, over all cases, at least one stale complete, one take after
  `expire` and one rotation happened.
- The binary has `retries = 0` in `nextest.toml`, so a failure is never hidden by a new seed. The
  proptest failure-persistence file is committed. The case count is fixed (builder's call, about
  64) so the test fits the `heavy-integration` mutex budget.

### 10.2 Concurrent stress test

N tasks (for example 8), each with its own connection and its own `LeaseKeeper`, run acquire,
claim, complete, release and a random `expire` on one inbox while a writer inserts events. Each
task records its successful claims and completes with the token. The test then checks the history:
no two successful completes share an event, no complete succeeded with a token that was not the
current live token at its commit (checked through the `run` rows), and after a drain every event is
`done` or `dead`.

### 10.3 Crash test

- Worker A takes the lease with a real `LeaseKeeper` and a short real `lease_ttl` (for example 2 s),
  and claims. This test proves the real database-clock path.
- Negative control: worker B's `acquire` returns `None` while A's lease is live.
- A's task is aborted (one case) or panics (one case). The heartbeat stops. B takes the lease after
  the TTL, claims the same events (`attempts = 2`, the same thread), completes, and every event is
  `done`.
- A late `complete` with A's claim returns `LeaseLost` and changes no row.

### 10.4 Handler latency

The binary runs against a real Postgres and an in-process fake IAM (no delay), with
`worker.enabled = false`. The test sends 1000 requests at a concurrency of 16, and asserts p99 <
50 ms. It runs in the `heavy-integration` mutex. If it is noisy on CI, the builder reports it; it is
not weakened silently.

### 10.5 Other tests

- `paigasus-cloudevents`: unit tests, the IAM parse and build fixtures (§4), length limits.
- HTTP: each row of §5.2; `403` with the same body for an unknown and a refused inbox; host-set
  fields; dropped `threadid`; reserved `agents.` type; duplicate `202` with the stored id.
- Authorisation: the call counts of the §6.2 table per credential kind, through the fake IAM; a key
  scoped to another org, a team and a project; an OIDC owner without membership of the inbox org.
- Keeper across a rotation: a keeper with a short interval, then claim, complete, claim, complete.
  The cancellation token is never cancelled.
- Concurrency of `acquire_next` on many inboxes: each inbox has at most one holder; an insert into
  an inbox is not blocked by an acquirer.
- Claim: per-thread grouping, `batch_limit`, a fresh thread for an expired thread with the
  successor link at complete, no-thread events, `not_before`, dead events, the cycle guard.
- Mid-run `Queue` (G6): claim, insert a new event on the same thread, complete. The new event is
  still `pending`, and the next claim takes it.
- NATS, with a real NATS: ack on insert; ack on duplicate; ack and skip on a tenancy refusal;
  `term` on a bad body; redelivery after a database error; boot fails on a filter mismatch. The
  test publishes bytes made by **IAM's encoder**, not bytes made by `paigasus-cloudevents`.
- IAM: the new action, the new role, the `org_admin` exclusion, the starter-policy revision.

## 11. Mid-run policy (ADR (h))

- A type `MidRunPolicy { Queue, Fold, Interrupt }` in the core crate.
- The config accepts `queue` only. `fold` and `interrupt` are refused at boot with a message that
  names the Helikon turn-boundary change as the reason.
- With `Queue`, there is no boundary query. A new event stays pending because `complete` marks only
  the claimed ids. The worker claims it after its commit, under the same lease, with a token
  rotation (§8.4 step 1).

## 12. Gates and repository work

- `repo:affected-smoke`: add `paigasus-cloudevents` to the expected sets in
  `ci/affected-graph/run.sh`, with its Moon edges and the `rs/.cargo/config.toml` input.
- `rs/Cargo.toml`: a literal `members` entry (A11).
- `repo:http-extractor-envelope`: house extractors only (§5.1).
- `repo:error-code-single-site`: the new `store-unavailable` code and an `emits` row per file that
  spells a code.
- `repo:observability-drift`: every new metric name in `paigasus-observability/src/names.rs`.
  Minimum: inserted, duplicate, duplicate with a different payload, lease lost, dead events, NATS
  term, NATS tenancy skip, NATS unmapped subject. SMA-798 may rename them.
- `repo:nats-permissions`: §9.5.
- Dependencies: `async-nats`, `tonic` and possibly `tokio-util` through workspace pins; check
  `deny.toml` (no `aws-lc-rs`, `openssl-sys`, `native-tls`) and `cargo-machete`.
- `rs/.config/nextest.toml`: every new Docker binary in `docker-containers`; the property binary
  with `retries = 0`.

## 13. Approaches considered

**Boundary with SMA-790.** (A, chosen) the full claim side in SMA-787. (B) a lease-only claim of
all inbox events, which matches the issue text but makes SMA-790 change the claim again. (C) a
split into stacked PRs. A was chosen so that the claim is designed once, against ADR (f).

**Ingress authorisation.** (A) owner only, with no IAM change; a service account cannot post for a
user. (B, chosen) owner, or a new `WriteInboxEvent` action on the inbox org. (C) a full
`Pgs::Agents::Inbox` entity in IAM, the largest IAM change. B follows the ADR and lets other
systems post, at the cost of an org-level grant, not a per-inbox one.

**Lease and claim SQL.** (1A, chosen) hand-written SQL in one adapter. (1B) the SeaORM builder,
which cannot express `SKIP LOCKED` with a lock target or `ON CONFLICT … WHERE` cleanly. (1C)
PL/pgSQL functions, which hide the logic in migrations.

**Property test time.** (2A, chosen) real Postgres with forced expiry. (2B) real sleeps, slow and
flaky. (2C) a pure model only, which does not test the SQL.

**NATS tenancy (D6).** (T1, chosen) an org filter on the CloudEvents `subject` that
fails closed; few IAM events pass today. (T2) NATS channels bind only to an inbox marked as an
operator inbox, and the operator accepts cross-tenant content. (T3) remove the NATS source from
SMA-787 and give it to a later issue with its own tenancy design.

## 14. Left to the builder

- The migration file split, column types beyond §7.1, index details beyond the partial index.
- Port and type names in §8.1, within the stated responsibilities and error shapes.
- The metric names (§12).
- The candidate limit of `acquire_next`, the property case count.

## 15. Open questions resolved (revision 2)

- `holder` is diagnostic only and never in a `WHERE` clause (§7.1).
- A duplicate with a different payload gets `202` and no special header. The mismatch is logged
  and counted (§7.2).
- A crash counts as an attempt for every event of the batch (§8.4.1, accepted limit).

## 16. Changelog

**Revision 2 (builder check):** the claim writes the run's thread onto its events, with
`predecessor_id` (no orphan threads); the `ClaimOpen` precondition; DPoP gives `401`; real IAM
subjects and the IAM body format; no TS change for the new action.

**Revision 2 (adversarial challenge), folded in:** one owner for the lease row (§8.2) to fix the
heartbeat-during-rotation defect; typed extensions and IAM parse fixtures (§4); `claim` and
`complete` result types, the commit rule and the poisoned handle (§8.4, §8.5); the run token and
`claimed_run_id` guards in `complete`; `FOR NO KEY UPDATE OF inbox SKIP LOCKED` and the candidate
order (§8.3); the monotonic keeper deadline and its guard (§8.6); a concurrent stress test, new
invariants, coverage and `retries = 0` (§10.1, §10.2); the crash test negative control and keeper
(§10.3); the NATS durable settings and the boot filter check (§9.2); the tenancy question (§9.4);
`org_admin` without `WriteInboxEvent` and the starter-policy changes (§6.1); UUID comparison, the
owner membership check and the key-scope rule (§6.3); length limits and `store-unavailable`
(§4, §5.2); single `fresh_thread` rule; two documented states of `thread_id`; composite foreign
keys; `run_event`; the port signatures; payload comparison in Rust; the existing error codes; the
IAM call counts per credential; the `413` order; the config keys and ranges; six-digit time;
`dataschema` and the reserved `agents.` type prefix; the SMA-786 proto handoffs; the NATS boot
order; the accepted latency side channel; the service-info rule; the CTE cycle guard; the
duplicate body.

**Not acted on:** a shared IAM-client crate for the three auth middlewares. It is out of scope for
this issue, and it goes to the follow-up sweep.
