# paigasus-agent-host

The agent host service. This crate holds the configuration, the database migrations, the event
inbox and the HTTP surface of the service. The surface serves `/healthz`, `/readyz` and `/metrics`.
It also serves the event ingress when `[iam]` is set. Any other path returns the 404 error envelope.

The agent host needs its own database. A separate schema in a shared database is not supported.

## Configuration

A TOML file `agent-host.toml` and environment variables set the configuration. The environment
prefix is `AGENT_HOST_`. A double underscore sets a nested key, for example
`AGENT_HOST_METRICS__ADDR`. An environment variable overrides the file.

| Key | Environment variable | Default | Meaning |
|---|---|---|---|
| `http_addr` | `AGENT_HOST_HTTP_ADDR` | `0.0.0.0:8086` | Address of the HTTP listener. |
| `database_url` | `AGENT_HOST_DATABASE_URL` | none (required) | Postgres connection string. |
| `log_level` | `AGENT_HOST_LOG_LEVEL` | `info` | Log filter. |
| `metrics.enabled` | `AGENT_HOST_METRICS__ENABLED` | `true` | Serve `/metrics`. |
| `metrics.addr` | `AGENT_HOST_METRICS__ADDR` | none | Separate metrics listener. Its port must differ from the port of `http_addr`. |
| `migration.lock_wait_secs` | `AGENT_HOST_MIGRATION__LOCK_WAIT_SECS` | `120` | Seconds to wait for the migration lock. Range 1 to 3600. |
| `worker.enabled` | `AGENT_HOST_WORKER__ENABLED` | `true` | The service reads and validates this value. It has no effect until SMA-790. |
| `lease.ttl_secs` | `AGENT_HOST_LEASE__TTL_SECS` | `30` | Time to live of an inbox lease. Range 3 to 600, and at least 3 times `lease.heartbeat_interval_secs`. |
| `lease.heartbeat_interval_secs` | `AGENT_HOST_LEASE__HEARTBEAT_INTERVAL_SECS` | `10` | Seconds between two lease heartbeats. Range 1 to 200. |
| `claim.batch_limit` | `AGENT_HOST_CLAIM__BATCH_LIMIT` | `20` | Maximum number of events in one claim. Range 1 to 500. |
| `claim.max_attempts` | `AGENT_HOST_CLAIM__MAX_ATTEMPTS` | `5` | Number of attempts after which an event is dead. Range 1 to 100. |
| `claim.thread_ttl_secs` | `AGENT_HOST_CLAIM__THREAD_TTL_SECS` | `86400` | Time to live of a thread. Range 60 to 31536000. |
| `ingress.max_body_bytes` | `AGENT_HOST_INGRESS__MAX_BODY_BYTES` | `262144` | Largest request body of the ingress. Range 1024 to 4194304. |
| `iam.grpc_addr` | `AGENT_HOST_IAM__GRPC_ADDR` | none | Address of the IAM gRPC endpoint. Required when you set `[iam]`. |
| `iam.tls` | `AGENT_HOST_IAM__TLS` | none (required in `[iam]`) | `{ mode = "tls" }` with the optional `ca_cert_path`, `client_cert_path` and `client_key_path`. Or `{ mode = "loopback_insecure" }` for a loopback `iam.grpc_addr` only. A `ca_cert_path` replaces the trust store. Set `client_cert_path` and `client_key_path` together. |
| `iam.call_timeout_ms` | `AGENT_HOST_IAM__CALL_TIMEOUT_MS` | `2000` | Timeout of one IAM call. Range 100 to 30000. |
| `mid_run.policy` | `AGENT_HOST_MID_RUN__POLICY` | `queue` | Only `queue` is valid. The service refuses `fold` and `interrupt` at boot. |
| `nats.url` | `AGENT_HOST_NATS__URL` | none | NATS server URL. Required when you set `[nats]`. |
| `nats.creds_file` | `AGENT_HOST_NATS__CREDS_FILE` | none | Credentials file of the `agent-host-consumer` identity. |
| `nats.ca_file` | `AGENT_HOST_NATS__CA_FILE` | none | CA bundle for the NATS TLS link. The bundle replaces the system root certificates. It must hold every root that the server chain needs. |
| `nats.stream` | `AGENT_HOST_NATS__STREAM` | none | Must be `IAM_EVENTS`. |
| `nats.durable` | `AGENT_HOST_NATS__DURABLE` | none | Must be `agent-host-inbox`. |
| `nats.channels` | none (TOML only) | none | List of `[[nats.channels]]` tables. At least one is required. See the NATS section. |
| `memory.backend` | `AGENT_HOST_MEMORY__BACKEND` | `postgres` | Memory store backend: `postgres` or `s3`. A backend switch does not move data. `s3` needs the `memory.s3` section. |
| `memory.caps.max_file_bytes` | `AGENT_HOST_MEMORY__CAPS__MAX_FILE_BYTES` | `65536` | Largest memory file, in bytes. Greater than 0. |
| `memory.caps.max_store_bytes` | `AGENT_HOST_MEMORY__CAPS__MAX_STORE_BYTES` | `1048576` | Largest memory total of a scope, in bytes. Greater than 0. |
| `memory.caps.max_store_files` | `AGENT_HOST_MEMORY__CAPS__MAX_STORE_FILES` | `512` | Largest number of memory files in a scope. Greater than 0. |
| `memory.caps.max_loaded_file_bytes` | `AGENT_HOST_MEMORY__CAPS__MAX_LOADED_FILE_BYTES` | `8192` | The cut of each file that the run start loads into the context. A longer file is cut with a truncation note. `view` does not use it (it cuts at 16,000 characters). Greater than 0, at most `max_file_bytes`. |
| `memory.s3.bucket` | `AGENT_HOST_MEMORY__S3__BUCKET` | none (required) | The bucket. 3 to 63 characters of `a-z`, `0-9`, `.` and `-`. It starts and ends with a letter or digit. It has no `..`. |
| `memory.s3.prefix` | `AGENT_HOST_MEMORY__S3__PREFIX` | none (required) | The key prefix of the store. At most 128 bytes and 8 segments. No empty segment, no leading or trailing `/`. Each segment has 1 to 64 bytes from `A-Z a-z 0-9 . _ -` and does not start with `.`. |
| `memory.s3.region` | `AGENT_HOST_MEMORY__S3__REGION` | `us-east-1` | The region. An `AWS_REGION` variable does not change it. |
| `memory.s3.endpoint` | `AGENT_HOST_MEMORY__S3__ENDPOINT` | not set (AWS) | The URL of an S3-compatible store. Scheme `http` or `https`. No user name, password, query or fragment. An `http` URL needs `allow_http`. |
| `memory.s3.path_style` | `AGENT_HOST_MEMORY__S3__PATH_STYLE` | `true` with an `endpoint`, else `false` | Path-style addressing. With `false` and an `endpoint`, the endpoint must already hold the bucket. |
| `memory.s3.allow_http` | `AGENT_HOST_MEMORY__S3__ALLOW_HTTP` | `false` | Allow an `http` endpoint. Use it for local tests and development only. |
| `memory.s3.extra_ca_bundle_path` | `AGENT_HOST_MEMORY__S3__EXTRA_CA_BUNDLE_PATH` | not set | A PEM file of extra trust anchors. It ADDs to the platform roots. The bundle must hold roots only. The host reads it once at boot. An empty or padded path is refused. |
| `memory.s3.apply_deadline_secs` | `AGENT_HOST_MEMORY__S3__APPLY_DEADLINE_SECS` | `10` | The time limit of one change set. At least 1 and less than `lease.ttl_secs`. |
| `timers.enabled` | `AGENT_HOST_TIMERS__ENABLED` | `true` | Run the scheduler loop. `false` stops only the loop. The tools still work. No range. |
| `timers.poll_interval_secs` | `AGENT_HOST_TIMERS__POLL_INTERVAL_SECS` | `5` | Seconds between two passes. Range: 1 to 300. |
| `timers.pass_limit` | `AGENT_HOST_TIMERS__PASS_LIMIT` | `100` | Most rows that one pass processes. Range: 1 to 1000. |
| `timers.statement_timeout_ms` | `AGENT_HOST_TIMERS__STATEMENT_TIMEOUT_MS` | `5000` | `SET LOCAL statement_timeout` of each fire transaction, in milliseconds. Range: 100 to 60000. |
| `timers.min_schedule_interval_secs` | `AGENT_HOST_TIMERS__MIN_SCHEDULE_INTERVAL_SECS` | `900` | Smallest civil gap between two slots of a schedule. Range: 60 to 86400. |
| `timers.max_active_per_inbox` | `AGENT_HOST_TIMERS__MAX_ACTIVE_PER_INBOX` | `50` | Most active `agent` timers of one inbox. Range: 1 to 1000. |
| `timers.max_horizon_days` | `AGENT_HOST_TIMERS__MAX_HORIZON_DAYS` | `366` | Latest allowed follow-up, and the window for the first slot of a schedule. Range: 1 to 366. |
| `timers.max_fire_attempts` | `AGENT_HOST_TIMERS__MAX_FIRE_ATTEMPTS` | `10` | Failed fire attempts before a timer goes to `failed`. Range: 1 to 1000. |
| `timers.min_follow_up_delay_secs` | `AGENT_HOST_TIMERS__MIN_FOLLOW_UP_DELAY_SECS` | `60` | Shortest follow-up, from the time of the tool call. Range: 1 to 86400. |
| `timers.max_agent_timers_created_per_day` | `AGENT_HOST_TIMERS__MAX_AGENT_TIMERS_CREATED_PER_DAY` | `100` | Most `agent` timers that one inbox can create in a rolling 24 hours, in any status. Range: 1 to 10000. |

### S3 memory backend

Set `memory.backend = "s3"` and the `memory.s3` section to keep the agent memory in an S3 bucket.
The store works with AWS S3 and with S3-compatible stores.

- A backend switch does not move data. The old backend keeps its files, and the new backend starts empty.
- The bucket must not keep object versions. The boot probe refuses a bucket that keeps versions.
- The store must give strong read-after-write consistency for `GET`, `HEAD` and `LIST`. AWS S3 does.
- Each object starts with a 51-byte header line. So the objects are not plain Markdown. Do not read them as plain files.
- Do not write objects under the prefix. If a foreign object is there, the store handles it as follows.
- An object that the adapter did not write has no header. A `get` of that path fails with a fixed `Store` error. The store counts it as `reason=header`. It does not make the whole scope fail.
- A key that is a valid object path but not a valid memory path: `list` skips it and counts `reason=key`. `delete_scope` deletes it.
- A key that is not a valid object path (for example `a//b`) makes the whole scope unreadable. The store counts it as `reason=unparsable`. Remove it with an S3 tool.
- A folder marker is a key that ends in `/`. A marker under the prefix cannot be deleted by `delete_scope`, because `object_store` removes the trailing `/`. Then `delete_scope` fails with the "keys remain" error. Delete the marker with an S3 tool. This is the same repair step as for an `unparsable` key.
- The prefixes of two deployments on one bucket must not nest. For example, do not use `mem` and `mem/a`.
- If `memory.s3.path_style` is `false` and an `endpoint` is set, the endpoint must already hold the bucket name.

**Credentials.** No config key holds a credential. The host reads only these ten environment variables:
`AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `AWS_SESSION_TOKEN`, `AWS_WEB_IDENTITY_TOKEN_FILE`,
`AWS_ROLE_ARN`, `AWS_ROLE_SESSION_NAME`, `AWS_ENDPOINT_URL_STS`,
`AWS_CONTAINER_CREDENTIALS_RELATIVE_URI`, `AWS_CONTAINER_CREDENTIALS_FULL_URI` and
`AWS_CONTAINER_AUTHORIZATION_TOKEN_FILE`. An empty variable counts as not set. Any other `AWS_*`
variable changes no setting. With none of these variables set, the client asks the instance metadata
endpoint for credentials. Off AWS, the boot probe then fails with `unauthenticated` or `timeout`.

**Other variables that change the client.** These variables are not on the credential list. The host does not turn them off.

- The HTTP client reads `HTTP_PROXY`, `HTTPS_PROXY`, `ALL_PROXY` and `NO_PROXY`, and their lower-case forms. This is the default of `reqwest`. Set `NO_PROXY` for the store endpoint if the host must not use a proxy for it.
- When `REQUEST_METHOD` is set, `reqwest` ignores `HTTP_PROXY`. This is a guard for CGI.
- On Linux, `SSL_CERT_FILE` and `SSL_CERT_DIR` change the system trust roots. The client reads them through `rustls-native-certs`.
- `AWS_ENDPOINT_URL_STS` sends the web-identity token to the host that it names.

**Races.** The store is correct for one writer at a time. When two writers write one scope at the
same moment, five races remain.

1. A writer can add a file under a guarded directory between the check and the writes. The file stays.
2. Two writers can both pass the store cap check. The store can then go over a cap. A later change that makes the store larger is refused. A change that makes the store smaller still passes.
3. A delete is a `HEAD` and a `DELETE`. A change between the two is lost. In a `rename`, the new path then holds the old content.
4. A conflict between the check and the writes gives a partial result.
5. A write that lands after the last listing of `delete_scope` stays.

A `Put` has no race: the store checks its condition, so a stale `Put` never overwrites.

The `[iam]`, `[nats]` and `[timers]` sections are optional. A nested value that has no default, for example
`iam.tls`, is easiest to set in the TOML file. `agent-host.toml.example` shows every key.

## Event ingress

SMA-787 adds the event inbox. A caller sends an event to an inbox, and the service stores it.
The ingress routes are mounted only when `[iam]` is set. The ingress needs `[iam]`, because every
request is authenticated through IAM.

| Route | Meaning |
|---|---|
| `POST /v1/inboxes/{inbox}/events` | Store one CloudEvent in the inbox. `{inbox}` is the inbox UUID. |
| `GET /v1/service-info` | Return the service descriptor. The slug is `agents`. |

The `POST` route accepts only `application/cloudevents+json`. The service checks the request in
this order:

| Status | Code | Cause |
|---|---|---|
| 415 | `unsupported-content-type` | The content type is not `application/cloudevents+json`. |
| 413 | `request-too-large` | The body is larger than `ingress.max_body_bytes`. |
| 401 | `invalid-token` | The credential is missing, inactive or a DPoP credential. |
| 503 | `iam-unavailable` | The service cannot reach IAM. This includes the self-query of step 5. |
| 403 | `forbidden` | The inbox is unknown, or the caller has no right to write to it. |
| 400 | `invalid-request-body` | The body is not valid JSON. |
| 400 | `invalid-request-schema` | The body is not a valid CloudEvent, or its `type` starts with `agents.`. |
| 503 | `store-unavailable` | A database error occurred. The caller can retry. |
| 202 | none | The event is stored, or it is a duplicate. The body is `{"id": "<event uuid>"}`. |

An unknown inbox looks the same as an inbox that the caller may not write to. The service makes
the same IAM calls for both, and it gives the same `403`. When IAM is unavailable, both get `503`
`iam-unavailable`. So an IAM outage does not show which inboxes exist to a caller that is not the
owner. An OIDC caller that owns the inbox but is not a member of the inbox org gets `403` with no
IAM call, also during an outage. That answer does not reveal other inboxes. The owner of an inbox
needs no self-query, so an outage of the self-query does not stop the owner.

The probe for an unknown inbox makes the same IAM call, and writes the same denial-audit row, as
a non-owner request to an existing inbox. So it adds no new class of load.

A duplicate returns the id of the stored row. A retry by the client gets the same body.
A `Content-Length` larger than the limit gives 413 before the authentication. A chunked body that
grows too large gives 413 after the authentication.

The service sets these fields. It does not trust the caller for them. `source` becomes
`urn:paigasus:agents:http:<principal PRN>`. `actorprn` becomes the PRN of the caller.
The service sets the relational column `inbox_event.thread_id` from `threadid` only if the thread
belongs to the inbox. Otherwise the column is null, and the event starts a new thread. The original
`threadid` extension stays in the stored `envelope` in both cases. The service sets `actorprn` in
the envelope and replaces any value from the caller.

`GET /v1/service-info` uses the same authentication and has no authorisation check.

### Authentication and authorisation

The credential is `Authorization: Bearer <secret>`. The service tries `IntrospectApiKey` first and
then `Introspect`. It does not use the `paigasus-org` header. The organization comes from the inbox.

An API key must have the scope Root or the organization of the inbox. Then two cases apply:

- The caller is the principal of the inbox. The caller is allowed. An OIDC user must also be a
  member of the organization of the inbox.
- The caller is another principal. The service asks IAM for `WriteInboxEvent` on the organization
  of the inbox. The role `agent_event_writer` grants only this action.

The service compares principals by UUID, not by string. The owner path does not run the IAM policy
`forbid-archived-writes`. An owner in an archived organization can still write to the own inbox.

## Storage

The migration `m0003_create_inbox_event_tables` adds five tables: `thread`, `inbox_lease`, `run`,
`run_event` and `inbox_event`. The unique key of `inbox_event` is (`inbox_id`, `source`, `ce_id`).
It gives the deduplication. Composite foreign keys make the schema enforce that a thread, a run and an event
belong to one inbox.

`inbox_event.thread_id` has two states. Before the first claim, it is the thread that the ingress
kept, or null. From the first claim, it is the thread of the run that claimed the event. The
`threadid` that the HTTP caller sent stays in the `envelope` column. The NATS path removes `threadid`
and `actorprn` from the envelope before the insert.

An inbox has one lease row. The service takes the lease, claims a batch of events, and renews the
lease with a heartbeat. SMA-787 starts no worker. `worker.enabled` is read and validated only.
SMA-790 adds the worker.

## NATS source

The `[nats]` section is optional. Without it, no NATS task starts. The service never creates a
stream or a consumer. `ops/nats/provision.sh` creates the durable `agent-host-inbox` on the stream
`IAM_EVENTS`. The service uses the identity `agent-host-consumer`.

Each `[[nats.channels]]` table maps one subject filter to one inbox. It has these keys:
`subject`, `inbox` (the inbox UUID) and `source`. The `source` must start with
`urn:paigasus:agents:nats:`. The service refuses a section with no channel, two channels with the
same `source`, and two channels whose filters can match the same subject.

The service delivers an event only when the CloudEvents `subject` is the organization PRN of the
inbox, or a PRN in that organization. The service acks and skips all other events. An event with no
`subject` is skipped. The service drops `actorprn` and `threadid` of a NATS event.

At boot, the service reads the durable and compares its `filter_subjects` with the channel
subjects. If they differ, boot stops with a config error. After you change a channel subject,
change the filters of the durable to match. See `ops/nats/README.md`.

### First deployment

The boot order is: migrations first, then the check of each configured inbox. If an inbox does not
exist, boot stops with a config error. So do these steps in this order:

1. Boot the service without the `[nats]` section.
2. Create the inbox.
3. Add the `[nats]` section and the channel with the inbox UUID.
4. Restart the service.

## Timers

SMA-788 adds timers. A timer wakes an agent at a set time. The wake is an event in the inbox
of the timer. The event has the type `agents.timer.fired`.

The migration `m0004_create_timer` adds the table `timer`. A timer has a kind: `follow_up` (fires
once) or `schedule` (a cron pattern in an IANA time zone). A timer has an origin: `agent` or
`system`. The status of a timer is one of these:

| Status | Meaning |
|---|---|
| `active` | The timer waits for `next_fire_at`. |
| `done` | A follow-up fired. |
| `cancelled` | The agent or the host cancelled the timer. |
| `failed` | The timer failed `timers.max_fire_attempts` times with a permanent error. |

### The scheduler loop

The scheduler loop runs when `timers.enabled` is `true`. It starts after boot, and the existing
drain stops it. Each pass reads the due rows with `FOR UPDATE OF t SKIP LOCKED`, one row for each
transaction. The database clock decides what is due. Each row has its own transaction, which inserts the event into the inbox and
updates the timer. Several replicas can run the loop. When a pass processes `timers.pass_limit`
rows, the next pass starts at once.

A duplicate fire gives no second event. The key of the event is stable for one timer and one slot.
`timers.enabled = false` stops only the loop. The tools still work, so timers wait until a replica
with the loop runs.

The metrics `agent_host_scheduler_lag_seconds` and
`agent_host_scheduler_last_pass_timestamp_seconds` show a late or a hung loop.

### The agent tools

The service has four tools: `schedule_follow_up`, `create_schedule`, `list_timers` and
`cancel_timer`. The tools exist and have tests. Nothing registers them for a run before SMA-790,
because no run exists before SMA-790. An agent can not use them yet.

The tools refuse a request in these cases. The refusal is text for the model:

- A follow-up is not in the future, is sooner than `timers.min_follow_up_delay_secs`, or is later
  than `timers.max_horizon_days`.
- A cron text is invalid, or a schedule has a civil gap below `timers.min_schedule_interval_secs`.
- A time zone is not a known IANA zone.
- A schedule has no slot in `timers.max_horizon_days`.
- A create makes more than `timers.max_active_per_inbox` active `agent` timers.
- A create is over the creation budget.
- A note is longer than 1000 characters, or holds a NUL.

The note is untrusted model text. The service never writes it to a log line or to a metric label.

### The creation budget

A fired follow-up leaves the count of active timers. So an agent could set a follow-up in each run
and wake itself in a loop. The creation budget stops this. One inbox can create at most
`timers.max_agent_timers_created_per_day` `agent` timers in a rolling 24 hours. A timer counts in
any status, so cancel and create does not reset the budget. The service never enforces the budget
when a timer fires, so a schedule that exists always fires.

### The commit contract for SMA-790

The tools collect their changes in a run buffer. `TimerWorkspace::seal()` gives the inserts and
the cancels. `PgTimerStore::apply_in` writes them inside the run transaction. SMA-790 must call
it after it takes the `inbox_lease` row lock with `FOR UPDATE`. That lock is the fence for a run
that lost its lease. SMA-790 must compare the current lease token with the token of the run under
that row lock, and roll back the run on a mismatch (ADR (b) step 1). `apply_in` checks only that
the `inbox_lease` row exists. `apply_in` uses a savepoint, so an error leaves the caller transaction usable.

| Result of `apply_in` | Recommended action for SMA-790 |
|---|---|
| `Ok(report)` | Commit the run. A cancel of a timer that is not active is reported as `NotActive`. |
| `CapExceeded` or `BudgetExceeded` | Roll back the whole run as transient. The next attempt runs again. |
| `InboxMismatch` or `InvalidInsert` | Log at `error` and roll back the whole run. These are bugs. |
| A database error | Roll back the whole run and retry it. |

Timer changes do not commit with a `failed` run. A `failed` run keeps its claimed events
`pending`, so the next attempt runs the same events again. Each attempt makes a new timer id. With
`claim.max_attempts` of 5, one intent would make five timers. Timer changes commit only with a
run outcome that finishes its claimed events: `completed`, `deferred` or `stopped`.

## Migrations

The service keeps its migration state in the table `agent_host_migrations`. It runs the migrations
under the advisory lock with the key `5_580_786`.

## Local run

Run `moon run paigasus-agent-host-rs:dev`. The command calls `scripts/dev.sh`.

The script starts a Postgres 16 container on a free port of `127.0.0.1`. Then it runs
`cargo run --locked -p paigasus-agent-host` with `AGENT_HOST_DATABASE_URL` and
`AGENT_HOST_HTTP_ADDR=127.0.0.1:<free port>`. The script prints both ports. Press Ctrl-C to stop
the service. The script then removes its container.

Use `scripts/dev.sh --check` to test the boot. The script builds the service first. Then it starts
the service, waits up to 60 s for `/readyz` to return 200, and stops. It exits 0 on success and 1
on failure.

Each container has the label `paigasus.dev=agent-host` and the label `paigasus.dev.pid=<pid of the
script>`. At start, the script removes only labelled containers whose process is no longer alive.
A second run from another worktree keeps its own container. A SIGKILL leaves a container. The next
run removes it.
