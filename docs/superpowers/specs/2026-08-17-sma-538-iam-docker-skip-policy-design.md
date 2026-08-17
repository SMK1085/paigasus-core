# SMA-538 — one Docker-skip policy for `paigasus-iam`, and an end to silent passes

**Issue:** [SMA-538](https://linear.app/smaschek/issue/SMA-538/iam-consolidate-the-11-duplicated-docker-skip-policy-copies-and-stop)
**Date:** 2026-08-17
**Status:** approved (design walkthrough). Supersedes two mechanisms proposed in the issue —
see "Departures from the issue".

## Problem

`paigasus-iam` has 339 integration tests across 59 binaries; **57 of those binaries start a
container**. Each of 11 entry points carries its own copy of the same decision — "if `start()`
failed and `CI` is unset, print a note and return `None`" — and every consumer of that `None`
returns early. With Docker stopped the crate reports **PASS in under a second having executed
nothing**.

The note is already there at all 11 sites. Nobody sees it, because nextest defaults to
`success-output = "never"` and discards a *passing* test's stderr. Improving the message cannot
fix this, which is why this design does not try to.

## Evidence gathered before designing

Three findings changed the design. All are reproducible.

### F1 — the issue's classification rule does not survive contact with testcontainers 0.27.3

The issue proposes: skip iff the error is `Client(ClientError::Init(..))`. Probed against a
dead daemon two ways, from `tests/redis_jwks_cache.rs` under `CI=1`:

| Simulated daemon-down | Rendered error | Variant |
|---|---|---|
| socket file absent (`DOCKER_HOST=unix:///nonexistent/docker.sock`) | `failed to initialize a docker client: Socket not found: …` | `ClientError::Init` |
| endpoint present, nothing listening (`DOCKER_HOST=tcp://127.0.0.1:1`) | `failed to create a container: Error in the hyper legacy client: client error (Connect)` | `ClientError::CreateContainer` |

`bollard_client::init` only *builds* a client — `Docker::connect_with_unix` performs no I/O —
so a stopped daemon whose endpoint still exists never reaches `Init` at all. Row 2 is the
common shape for colima, podman, a remote `DOCKER_HOST`, and systemd socket activation.

Independently, `testcontainers-0.27.3/src/core/client.rs:259` maps a **container start
failure** to `ClientError::Init`:

```rust
pub(crate) async fn start(&self, id: &str) -> Result<(), ClientError> {
    self.bollard.start_container(id, None::<StartContainerOptions>).await
        .map_err(ClientError::Init)          // <- not an init failure
}
```

So the proposed rule is wrong in both directions: it hard-fails a genuinely Docker-less laptop
(row 2), and it silently skips a real container-start failure — the precise behaviour AC 2
forbids.

### F2 — a greppable marker is invisible in every default path

nextest discards a passing test's stderr, and `.moon/tasks.yml` sets
`outputStyle: buffer-only-failure`, so Moon discards a *passing task's* output too. A marker
emitted on the skip path is therefore unobservable under both `cargo nextest run -p
paigasus-iam` and `moon ci :test`. AC 4 taken literally is satisfiable but inert.

nextest **does** accept `success-output` inside a per-override block — verified on 0.9.136,
which surfaced the existing `skipping redis_jwks_cache: …` line. But scoping it to "the Docker
suites" means 57 of 59 binaries (~336 of 339 tests), which is the same firehose the issue
already rejected. Scoped to *one* binary it costs one line.

### F3 — container logs can impersonate a dead daemon

`TestcontainersError::WaitContainer` can carry container log output. A Postgres or NATS log
line containing `connection refused` would be read as a dead daemon by any pure text match —
a fail-*open*, i.e. a new instance of the bug being fixed.

## Design

### 1. One module

Everything lands in `tests/support/docker.rs`, the standalone module SMA-521 established. It
depends on nothing but `testcontainers`, and every item carries `#[allow(dead_code)]` — so both
constraints the issue identifies still hold:

* it does not trip `dead_code`, which `[workspace.lints.rust] warnings = "deny"` makes a hard
  compile error;
* it does not require `mod support;` in the five files that lack it.

Reached as `support::docker::*` by the ~52 files carrying `mod support;`, and via
`#[path = "support/docker.rs"] mod docker;` by the rest.

### 2. Public surface

```rust
pub async fn start_or_skip<T, I>(image: T, what: &str) -> Option<ContainerAsync<I>>
where
    T: Into<ContainerRequest<I>> + Send,
    I: Image;

/// Collapses all six near-identical Redis wrappers, URL included.
pub async fn start_redis_or_skip(what: &str) -> Option<(ContainerAsync<Redis>, String)>;

/// `PAIGASUS_SKIP_DOCKER`, honoured only outside CI. Public so the canary can read it.
pub fn skip_docker() -> bool;

fn env_flag(name: &str) -> bool;
fn is_daemon_unreachable(e: &TestcontainersError) -> bool;
```

Every call-site shape reaches `Into<ContainerRequest<I>>`, verified against the sources:
`Redis::default()`, `Postgres::default().with_tag("16-alpine")`,
`Nats::default().with_cmd(&cmd)`, and
`GenericImage::new(..).with_wait_for(..).with_copy_to(..)`.

There is **no** `start_nats_or_skip`. The two NATS sites use different images —
`nats_publisher.rs` uses the `Nats` module, `nats_permissions.rs` a `GenericImage` with three
`with_copy_to` calls — so no concrete wrapper collapses both. They, Keycloak, and both Postgres
starters use the generic.

### 3. The classifier

Two stages, both load-bearing:

```rust
fn is_daemon_unreachable(e: &TestcontainersError) -> bool {
    matches!(e, TestcontainersError::Client(_))                 // stage 1
        && CONNECT_MARKERS.iter().any(|m| chain(e).contains(m)) // stage 2
}
```

**Stage 1** answers F3: restricting to `Client(_)` excludes `WaitContainer` (which carries
container logs), `PortNotExposed`, `Exec`, `Io` and `Other` before any text is examined.

**Stage 2** answers F1's second half: the variant name is never trusted, so `client.rs:259`'s
mis-tagged `Init` cannot masquerade as a dead daemon — a real start failure's message carries
no connect marker.

Markers, each observed in a real error:

```
Socket not found
client error (Connect)
connection refused
error trying to connect
Cannot connect to the Docker daemon
```

`chain(e)` walks `std::error::Error::source()` and concatenates each level's `Display`.
`format!("{e:#}")` is **not** used: alternate-Display source chaining is an `anyhow` idiom,
not a `thiserror` one.

Two exclusions are deliberate:

* **`No such file or directory`** — `nats_permissions.rs` reads fixture files for
  `with_copy_to`; a missing cert renders that text and must be a hard failure, not a skip.
* **socket permission-denied** — a misconfiguration worth seeing. Fail-closed reds it.

The classifier fails **closed**: anything unmatched is a hard failure. Its only failure mode is
a loud red, never a silent pass.

### 4. Decision table

Stated as ordered rules, first match wins, so precedence is unambiguous:

1. `start()` returned `Ok` → `Some(node)`.
2. `CI` is present → **panic**. (`CI` overrides `SKIP`; today's CI behaviour, unchanged.)
3. `PAIGASUS_SKIP_DOCKER` is on → skip + marker, whatever the error was.
4. `PAIGASUS_REQUIRE_DOCKER` is on → **panic**.
5. `is_daemon_unreachable(&e)` → skip + marker.
6. Otherwise → **panic**. This is AC 2: a container failure with a reachable daemon is hard.

The same table, enumerated:

| `start()` result | `CI` | `SKIP` | `REQUIRE` | Outcome |
|---|---|---|---|---|
| `Ok` | — | — | — | `Some(node)` |
| `Err`, unreachable | set | either | either | **panic** (rule 2) |
| `Err`, unreachable | unset | on | either | skip + marker (rule 3) |
| `Err`, unreachable | unset | off | on | **panic** (rule 4) |
| `Err`, unreachable | unset | off | off | skip + marker (rule 5) |
| `Err`, anything else | set | either | either | **panic** (rule 2) |
| `Err`, anything else | unset | on | either | skip + marker (rule 3) |
| `Err`, anything else | unset | off | either | **panic** (rule 4 or 6) |

Marker format: `SKIP[docker-unavailable] {what}: {e}` on stderr.

Rule 3 sitting above rule 4 is intentional: `PAIGASUS_SKIP_DOCKER` is the recourse when a
Docker Hub pull limit or a daemon restart would otherwise red every suite, so it must win over
`PAIGASUS_REQUIRE_DOCKER`. Rule 2 sits above both so no env var a workflow file might carry can
green CI.

### 5. Env-var parsing

`env_flag` accepts `1`, `true`, `yes` case-insensitively; everything else — including `0`, the
empty string, and unset — is off. Deliberately **not** the presence-based form `CI` uses: `CI`
is set by a platform, whereas these are typed by a human for whom `PAIGASUS_REQUIRE_DOCKER=0`
meaning "on" would be a footgun. `CI` keeps presence semantics, unchanged.

### 6. The canary

New `tests/docker_preflight.rs`: one test, `#[path]`-including `docker.rs`, calling
`start_redis_or_skip` and asserting `Some` unless `SKIP` is set.

Docker down and nothing set gives **one** red — named for the actual problem, and visible under
both nextest and Moon because it is a *failure*, not a pass. The other 56 suites still skip
quietly. `PAIGASUS_SKIP_DOCKER=1` greens it back.

It probes by starting a real Redis container rather than pinging: testcontainers exposes no
ping, client construction alone cannot detect F1's row 2, and a `bollard` dev-dependency would
mean `rs/deny.toml` / cargo-machete / lockfile churn plus a second bollard in the tree on the
next testcontainers bump. Redis is already pulled by five other suites, so there is no new
Docker Hub pull in CI or locally, and reusing `start_redis_or_skip` means the canary
end-to-end exercises the very policy it guards.

It needs an override **above** the general `kind(test)` block in `rs/.config/nextest.toml` —
first-match-wins per setting, as that file's ordering comment already documents. Without it a
genuine Docker-down run burns ~60s on two pointless retries:

```toml
[[profile.default.overrides]]
filter = 'package(=paigasus-iam) and binary(docker_preflight)'
retries = 0
```

`test-group = 'docker-containers'` is inherited from the general block — the same per-setting
precedence `keycloak_e2e` already relies on.

### 7. The `repo:nats-permissions` gate (AC 5)

`nats_permissions.rs` gains `#[path = "support/docker.rs"] mod docker;`, so AC 5's condition
fires. `moon.yml` gains exactly one input:

```yaml
- 'rs/crates/services/paigasus-iam/tests/support/docker.rs'
```

`tests/support/**/*` stays excluded — the ~748-line shared surface is still not a dependency,
and the existing "NOT included" comment must be rewritten, since it currently asserts the file
"never references it".

`--test docker_preflight` is **not** added to that task. It is a filtered run the canary cannot
cover by construction; that gap is exactly what `PAIGASUS_REQUIRE_DOCKER` is for, and `CI`
covers it in CI.

### 8. Documentation (AC 6)

`CLAUDE.md`'s gotcha and `docs/dev-setup.md`'s bullet both currently describe the silent skip as
an unavoidable hazard and prescribe `CI=1` as the only defence. Both are rewritten around the
canary and the two env vars.

### 9. Testing

* **`tests/support_docker_policy.rs`** (new, Docker-free, mirroring `support_docker_retry.rs`):
  * `env_flag` — `1`/`true`/`yes`/`TRUE`/`Yes` on; `0`/`""`/unset/`no`/`maybe` off.
  * `is_daemon_unreachable` — positive for both strings observed in F1; negative for a
    `WaitContainer` error whose log text contains `connection refused` (F3), for a
    `with_copy_to` file-not-found, and for a plain container-start failure.
* **End-to-end**: `cargo nextest run -p paigasus-iam` with `DOCKER_HOST` pointed at a dead
  socket must yield exactly one failure (`docker_preflight`); the same run with
  `PAIGASUS_SKIP_DOCKER=1` must be fully green; a normal run with Docker up must be green with
  the canary passing.
* **Gates**: the full `moon ci` list from `CLAUDE.md`, `--base origin/main
  --include-relations`.

## Departures from the issue

| Issue says | This design | Why |
|---|---|---|
| Skip iff `Client(ClientError::Init(..))` | Two-stage classifier: `Client(_)` **and** a connect marker in the source chain | F1 — the rule both over- and under-matches |
| `PAIGASUS_SKIP_DOCKER` wins regardless of `CI` | `CI` overrides `SKIP` | A stray `PAIGASUS_SKIP_DOCKER` in a workflow must not silently green CI; the hatch exists for laptops |
| A greppable marker satisfies the visibility goal | Marker **plus** a hard-failing canary | F2 — a marker alone is discarded by both nextest and Moon |
| `start_redis_or_skip()` / `start_nats_or_skip()` | `start_redis_or_skip()` only | The two NATS sites use different images; no single wrapper collapses them |

## Out of scope

The 17 further `eprintln!("skipping …")` messages in `dead_letters_pg.rs`,
`outbox_retention_pg.rs`, `outbox_retention_concurrency_pg.rs` and
`outbox_dead_letter_columns_pg.rs` are *consumers* of a `None`, not copies of the decision.
Unifying them is separate work.

## Acceptance criteria mapping

| AC | Satisfied by |
|---|---|
| 1 — one definition, no per-file copies | §1, §2 |
| 2 — reachable-daemon failure is hard | §3 (fail-closed), §4 |
| 3 — both env vars, `0`/unset off, unit-tested | §4, §5, §9 |
| 4 — uniform greppable marker | §4, and §6 makes it observable |
| 5 — gate inputs updated iff the reference is added | §7 |
| 6 — `CLAUDE.md` + `docs/dev-setup.md` | §8 |
