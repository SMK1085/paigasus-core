# SMA-538 — one Docker-skip policy for `paigasus-iam`, and an end to silent passes

**Issue:** [SMA-538](https://linear.app/smaschek/issue/SMA-538/iam-consolidate-the-11-duplicated-docker-skip-policy-copies-and-stop)
**Date:** 2026-08-17
**Status:** revised after adversarial challenge (round 1). Supersedes three mechanisms proposed
in the issue and one proposed in this spec's own first draft — see "Departures".

## Problem

`paigasus-iam` has 339 integration tests across 60 binaries; **57 of those binaries start a
container**. Each of 11 entry points carries its own copy of the same decision — "if `start()`
failed and `CI` is unset, print a note and return `None`" — and every consumer of that `None`
returns early. With Docker stopped the crate reports **PASS in under a second having executed
nothing**.

The note is already there at all 11 sites. Nobody sees it, because nextest defaults to
`success-output = "never"` and discards a *passing* test's stderr. Improving the message cannot
fix this, which is why this design does not try to.

## Evidence

Four findings shaped the design. All were verified against vendored sources or by running the
suite; none is taken on trust.

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
suites" means 57 of 60 binaries — measured at **336 of 339 tests**, since the only
container-free binaries are `health.rs`, `grpc_health.rs` and `support_docker_retry.rs`. That
is the firehose the issue already rejected. Scoped to *one* binary it costs one line.

### F3 — substring matching on the error text fails OPEN

This killed the first draft's classifier, and it is the single most important finding.

Ten `ClientError` variants wrap a `BollardError` verbatim, and
`bollard-0.20.2/src/errors.rs:53-59` defines

```rust
#[error("Docker responded with status code {status_code}: {message}")]
DockerResponseServerError { status_code: u16, message: String },
```

— **daemon-authored free text** interpolated straight into the Display. The reachable path is
image pull: `testcontainers-0.27.3/src/runners/async_runner.rs:343-358` calls `pull_image`
whenever `create_container` returns 404 (i.e. the image is not cached), and its failure becomes
`ClientError::PullImage`. A **healthy** daemon that cannot reach the registry — corporate
proxy, DNS, mirror down — relays text such as

```
Get "https://registry-1.docker.io/v2/": dial tcp …: connect: connection refused
```

Any classifier matching the substring `connection refused` skips there, **with Docker
running**. The canary does not catch it: the canary starts Redis, cached on any machine that
has run the suite once, so it goes green while every Postgres/NATS/Keycloak suite skips.

Three further checks against the vendored sources, each falsifying a marker the first draft
claimed was "observed in a real error":

* `Cannot connect to the Docker daemon` — **zero** occurrences anywhere in the registry. It is
  the Docker *CLI*'s message, never the API's.
* `error trying to connect` — exists only in `hyper-0.14.32` and the aws-smithy clients.
  bollard 0.20 is on hyper-util, whose Display is `client error ({kind:?})`.
* socket permission-denied does **not** fail closed as the first draft claimed: EACCES on
  connect is also `ErrorKind::Connect`, rendering identically to ECONNREFUSED.

### F4 — the error enums are exhaustively matchable, and bollard needs no new dependency

`testcontainers-0.27.3/src/lib.rs:117` is `pub use bollard;`, so bollard's types are reachable
without adding a dev-dependency. And **none** of `TestcontainersError`, `ClientError` or
`bollard::errors::Error` is `#[non_exhaustive]` — so a match with no `_` arm is legal, and an
upstream variant addition becomes a **compile error** rather than a silent reclassification.

## Design

### 1. One module

Everything lands in `tests/support/docker.rs`, the standalone module SMA-521 established. It
does not depend on `support/mod.rs`, and every item carries `#[allow(dead_code)]` — so both
constraints the issue identifies still hold:

* it does not trip `dead_code`, which `[workspace.lints.rust] warnings = "deny"` makes a hard
  compile error;
* it does not require `mod support;` in the files that lack it.

Reached as `support::docker::*` by the ~52 files carrying `mod support;`, and via
`#[path = "support/docker.rs"] mod docker;` by the rest.

Its dependency surface grows by one crate: `start_redis_or_skip` names
`testcontainers_modules::redis::Redis`. Both `testcontainers` and `testcontainers-modules` are
already dev-dependencies, so nothing new enters the tree — but the module's doc comment, which
currently claims it "deliberately depends on NOTHING else", must be restated as "nothing else
in `support/`".

### 2. Public surface

Every item is `pub` with `#[allow(dead_code)]`. This is not stylistic: a `#[path]`-included
module's private items are invisible to the *including* crate root, so the unit tests in §9
cannot reach a private `env_flag` — `tests/support_docker_retry.rs:14` already demonstrates the
`pub` requirement for `PortSource`/`mapped_port`.

```rust
pub async fn start_or_skip<T, I>(image: T, what: &str) -> Option<ContainerAsync<I>>
where
    T: Into<ContainerRequest<I>> + Send,
    I: Image;

/// Collapses all six near-identical Redis wrappers, URL included.
pub async fn start_redis_or_skip(what: &str) -> Option<(ContainerAsync<Redis>, String)>;

/// PURE — takes the raw value rather than reading the environment, so the tests need no
/// `unsafe { std::env::set_var(..) }` (unsafe under edition 2024) and no assumption about
/// process isolation.
pub fn env_flag(raw: Option<&OsStr>) -> bool;

pub fn skip_docker() -> bool;      // env_flag(PAIGASUS_SKIP_DOCKER) && CI absent
pub fn require_docker() -> bool;   // env_flag(PAIGASUS_REQUIRE_DOCKER) || CI present

pub fn is_daemon_unreachable(e: &TestcontainersError) -> bool;
```

Every call-site shape reaches `Into<ContainerRequest<I>>`, verified against the sources:
`Redis::default()`, `Postgres::default().with_tag("16-alpine")`,
`Nats::default().with_cmd(&cmd)`, and
`GenericImage::new(..).with_wait_for(..).with_copy_to(..)`.

There is **no** `start_nats_or_skip`. The two NATS sites use different images —
`nats_publisher.rs` uses the `Nats` module, `nats_permissions.rs` a `GenericImage` with three
`with_copy_to` calls — so no concrete wrapper collapses both. They, Keycloak, and both Postgres
starters use the generic.

`start_redis_or_skip` builds `redis://127.0.0.1:{port}`, identical to all six sites it
replaces. The `127.0.0.1` literal is preserved rather than fixed: a remote `DOCKER_HOST` is
already broken at those six sites today, and fixing it is not this issue's job.

### 3. The classifier — by type, never by text

Two nested exhaustive matches, no `_` arm at either level (F4 makes this legal and makes
upstream drift a compile error).

```rust
pub fn is_daemon_unreachable(e: &TestcontainersError) -> bool {
    let TestcontainersError::Client(c) = e else { return false };
    let b: &BollardError = match c {
        // The daemon never answered — these carry a raw transport error.
        ClientError::Init(b)
        | ClientError::ListContainers(b) | ClientError::CreateContainer(b)
        | ClientError::RemoveContainer(b) | ClientError::StartContainer(b)
        | ClientError::StopContainer(b)   | ClientError::PauseContainer(b)
        | ClientError::UnpauseContainer(b)| ClientError::InspectContainer(b)
        | ClientError::CreateNetwork(b)   | ClientError::InspectNetwork(b)
        | ClientError::ListNetworks(b)    | ClientError::RemoveNetwork(b)
        | ClientError::InitExec(b)        | ClientError::InspectExec(b)
        | ClientError::UploadToContainerError(b) => b,

        // The daemon ANSWERED, or we never reached it for a reason of our own making.
        // NEVER a skip — this is where F3's fail-open lived.
        ClientError::PullImage { .. } | ClientError::BuildImage { .. }
        | ClientError::Configuration(_) | ClientError::InvalidDockerHost(_)
        | ClientError::PortMapping(_)
        | ClientError::CopyToContainerError(_) | ClientError::CopyFromContainerError(_)
            => return false,
    };

    match b {
        BollardError::SocketNotFoundError(_) => true,
        BollardError::HyperLegacyError { err } => err.is_connect(),  // public, hyper-util:1642
        BollardError::IOError { err } => matches!(
            err.kind(),
            ErrorKind::NotFound | ErrorKind::ConnectionRefused
                | ErrorKind::ConnectionReset | ErrorKind::ConnectionAborted
        ),
        BollardError::RequestTimeoutError => true,
        // DockerResponseServerError, Json*, Str*, Uri*, UnsupportedURIScheme, … — the daemon
        // spoke, or we are misconfigured. Hard failure.
        _other_variants_enumerated_explicitly => false,
    }
}
```

Three properties this buys that substring matching could not:

* **F1's mis-tagging is handled structurally.** `Client::start`'s bogus `ClientError::Init`
  carries a `DockerResponseServerError` when the container genuinely failed to start — the
  daemon answered — so it falls to `false` and hard-fails. No knowledge of the mis-tag needed.
* **F3's fail-open is unreachable.** `PullImage` cannot skip regardless of the text the daemon
  relays.
* **Permission-denied really does fail closed**, via `ErrorKind::PermissionDenied` being absent
  from the `IOError` list, rather than by the assertion the first draft made and F3 falsified.

`err.is_connect()` separates a genuine connect failure from a post-connect protocol error, so
`HyperLegacyError` is not treated as unconditionally transport-level.

### 4. Decision table

Ordered rules, first match wins:

1. `start()` returned `Ok` → `Some(node)`.
2. `CI` is present → **panic**. (`CI` overrides `SKIP`; today's CI behaviour, unchanged.)
3. `PAIGASUS_SKIP_DOCKER` is on → skip + marker, whatever the error was.
4. `PAIGASUS_REQUIRE_DOCKER` is on → **panic**.
5. `is_daemon_unreachable(&e)` → skip + marker.
6. Otherwise → **panic**. This is AC 2: a container failure with a reachable daemon is hard.

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

Rule 3 above rule 4 is intentional: `PAIGASUS_SKIP_DOCKER` is the recourse when a Docker Hub
pull limit or a daemon restart would otherwise red every suite, so it must win over
`PAIGASUS_REQUIRE_DOCKER`. Rule 2 sits above both so no env var a workflow file might carry can
green CI.

**Accepted consequence — a Docker Hub 429 reds CI with no escape.** A rate limit surfaces as
`PullImage`, which rule 6 hard-fails, and rule 2 forbids `SKIP` from rescuing it. That is the
correct trade: a rate-limited CI run has genuinely not tested anything, and the fix is a
registry mirror or authentication, not a skip.

**Behaviour change on slow containers.** `keycloak_e2e.rs:79` sets a 240s startup timeout; a
timeout produces `WaitContainerError::StartupTimeout`, which is not `Client(_)`, so rule 6
panics where today it skips. Under `nextest.toml`'s `retries = 1` for that binary a loaded
laptop can therefore spend ~480s reaching a red that used to be a fast skip. This is the
intended AC 2 semantic — a reachable daemon that could not start a container is a real failure —
but it is the most user-visible consequence of the design and is recorded here deliberately.

### 5. Env-var parsing

`env_flag` accepts `1`, `true`, `yes` case-insensitively; everything else — including `0`, the
empty string, and unset — is off. Deliberately **not** the presence-based form `CI` uses: `CI`
is set by a platform, whereas these are typed by a human for whom `PAIGASUS_REQUIRE_DOCKER=0`
meaning "on" would be a footgun. `CI` keeps presence semantics, matching all 11 existing sites.

### 6. The canary

New `tests/docker_preflight.rs`: one test, `#[path]`-including `docker.rs`, calling
`start_redis_or_skip` and asserting `Some` unless `skip_docker()` is true.

Docker down and nothing set gives **one** red — named for the actual problem, and visible under
both nextest and Moon because it is a *failure*, not a pass. The other 56 suites still skip
quietly. `PAIGASUS_SKIP_DOCKER=1` greens it back. `PAIGASUS_REQUIRE_DOCKER` is a no-op for the
canary specifically (it already fails); it exists for the other 56.

It probes by starting a real Redis container rather than pinging: testcontainers exposes no
ping, client construction alone cannot detect F1's row 2, and a direct `bollard` dependency
would mean `rs/deny.toml` / cargo-machete / lockfile churn. Redis is already pulled by five
other suites, so there is no new Docker Hub pull, and reusing `start_redis_or_skip` means the
canary end-to-end exercises the very policy it guards.

It needs an override **above** the general `kind(test)` block in `rs/.config/nextest.toml` —
first-match-wins per setting, as that file's ordering comment documents:

```toml
[[profile.default.overrides]]
filter = 'package(=paigasus-iam) and binary(docker_preflight)'
retries = 1
```

`retries = 1`, not `0`: under rule 6 a transient Redis start failure with a reachable daemon —
the exact class SMA-521's retry budget exists for — now reds the build, so the backstop must
stay. One retry costs ~15s on a genuine Docker-down run, against the ~45s that two would.
`test-group = 'docker-containers'` is inherited from the general block, the same per-setting
precedence `keycloak_e2e` relies on.

**Three limitations, stated rather than papered over:**

* **Filtered runs are not covered.** `cargo nextest run -p paigasus-iam --test relay_pg` or
  `-E 'test(tenancy)'` excludes the canary and restores the full silent skip. That gap is what
  `PAIGASUS_REQUIRE_DOCKER` is for, and it is why §8 documents the variable rather than burying
  it.
* **`PAIGASUS_SKIP_DOCKER` is sticky.** Documenting it teaches a one-line permanent bypass; in
  a shell profile it returns the developer to silent green. §8 must say so in the same breath
  as introducing it.
* **Moon caches a green task.** A `moon run paigasus-iam-rs:test` that passed under
  `PAIGASUS_SKIP_DOCKER=1` leaves a cached PASS keyed on inputs, not ambient env, which replays
  after Docker returns. §8 documents that the variable is for bare `cargo nextest`, and that a
  `moon run` under it should be followed by `moon run … --force`.

One operational note for §8: a `DOCKER_HOST` that **hangs** rather than refuses hits bollard's
120s `DEFAULT_TIMEOUT` per binary; at `max-threads = 8` across 57 binaries that is roughly 14
minutes before the run reports its one red. Slow, not hung.

### 7. Gates

**`repo:nats-permissions` (AC 5).** `nats_permissions.rs` gains
`#[path = "support/docker.rs"] mod docker;`, so AC 5's condition fires. `moon.yml` gains two
inputs and one command change:

```yaml
- 'rs/crates/services/paigasus-iam/tests/support/docker.rs'
- 'rs/crates/services/paigasus-iam/tests/docker_preflight.rs'
```

```
cargo nextest run --no-tests=pass -p paigasus-iam \
  --test nats_permissions --test docker_preflight --profile iam-nats
```

`--test` is repeatable, so the canary *can* cover this gate — the first draft claimed otherwise
and was wrong. Without it, the crate's most container-heavy gate (12 containers, TLS certs)
still reports PASS having run nothing on a Docker-less laptop. Cost: one Redis container,
already pulled.

`tests/support/**/*` stays excluded — the ~748-line shared surface is still not a dependency —
and the existing "NOT included" comment must be rewritten, since it currently asserts the file
"never references it".

**New `repo:iam-docker-policy-single-site` (AC 1, durability).** Consolidating 11 copies
without a gate lets copy #12 appear: the next Docker-backed suite can hand-roll
`if std::env::var_os("CI").is_some() { panic!(..) } … return None` and nothing reds. The repo
has exact precedent in `repo:redis-connect-single-site`, which bans raw Redis constructors in
`src/` *and* `tests/`. This gate greps for `var_os("CI")` / `env::var("CI")` under
`rs/crates/services/paigasus-iam/tests/` outside `support/docker.rs`, with narrow inputs
(`rs/crates/services/paigasus-iam/tests/**/*`), and joins the `CLAUDE.md` gate list.

**Unchanged.** Two new test binaries do not add a `paigasus_iam` link dependency — like
`support_docker_retry.rs` they reference only `docker.rs` — so `ci.yml`'s documented disk
pressure is not materially affected. No crate is added, so `repo:affected-smoke` and the
Cargo/Moon parity gate are untouched.

### 8. Documentation (AC 6)

`CLAUDE.md`'s gotcha and `docs/dev-setup.md`'s bullet both currently describe the silent skip as
an unavoidable hazard and prescribe `CI=1` as the only defence. Both are rewritten around the
canary and the two env vars, and must carry the three §6 limitations — especially that
`PAIGASUS_SKIP_DOCKER` is a per-invocation escape hatch, not a shell-profile setting.

### 9. Testing

**`tests/support_docker_policy.rs`** (new, Docker-free, mirroring `support_docker_retry.rs`):

* `env_flag` — `1`/`true`/`yes`/`TRUE`/`Yes` on; `0`/`""`/`no`/`maybe`/`None` off. Pure, so no
  `set_var`.
* `is_daemon_unreachable` positives, both freely constructible via the `testcontainers::bollard`
  re-export: `SocketNotFoundError(_)` (F1 row 1) and `IOError` with `ConnectionRefused`.
* **The F3 regression test**, the most important one:
  `Client(PullImage { err: DockerResponseServerError { status_code: 500, message: "… dial tcp
  …: connect: connection refused" } })` must be `false`. This is the fail-open the first draft
  shipped, constructible exactly as written.
* Further negatives: `WaitContainer` carrying a log line containing `connection refused`;
  `CopyToContainerError`; `Client(Init(DockerResponseServerError { .. }))` — F1's mis-tagged
  container-start failure, which must hard-fail.
* `IOError` with `PermissionDenied` must be `false` (§3's fail-closed claim, now testable).

**Not unit-testable, and stated as such:** `HyperLegacyError` wraps
`hyper_util::client::legacy::Error`, which has no public constructor. F1 row 2 is therefore
covered by the end-to-end check below, not by a fabricated string.

**End-to-end**, all three run manually before the PR:

* `DOCKER_HOST=tcp://127.0.0.1:1 cargo nextest run -p paigasus-iam` → exactly one failure
  (`docker_preflight`), everything else green. Covers F1 row 2.
* the same with `PAIGASUS_SKIP_DOCKER=1` → fully green.
* a normal run with Docker up → fully green, canary passing, no behaviour change.

**Gates**: the full `moon ci` list from `CLAUDE.md`, plus the new one, `--base origin/main
--include-relations`.

## Departures

| Source | Says | This design | Why |
|---|---|---|---|
| Issue | Skip iff `Client(ClientError::Init(..))` | Exhaustive type match, transport vs daemon-answered | F1 — the rule both over- and under-matches |
| Issue | `SKIP` wins regardless of `CI` | `CI` overrides `SKIP` | A stray var in a workflow must not green CI |
| Issue | A greppable marker satisfies visibility | Marker **plus** a hard-failing canary | F2 — a marker alone is discarded twice over |
| Issue | `start_redis_or_skip()` / `start_nats_or_skip()` | `start_redis_or_skip()` only | The two NATS sites use different images |
| Draft 1 | `Client(_)` + connect-marker substrings | Type-based match, no substrings | F3 — substrings fail open through `PullImage` |
| Draft 1 | Canary `retries = 0` | `retries = 1` | Rule 6 makes transient start failures red |
| Draft 1 | nats-permissions cannot use the canary | `--test` is repeatable; it now does | Simply wrong |
| Draft 1 | private `env_flag` | `pub`, and pure | `#[path]` modules hide privates from the parent |

## Out of scope

The 17 further `eprintln!("skipping …")` messages in `dead_letters_pg.rs`,
`outbox_retention_pg.rs`, `outbox_retention_concurrency_pg.rs` and
`outbox_dead_letter_columns_pg.rs` are *consumers* of a `None`, not copies of the decision, and
the issue scopes them out. They will print their own line beneath the uniform marker; AC 4's
"uniform" is read as applying to the policy's own output, which has exactly one format.

## Acceptance criteria mapping

| AC | Satisfied by | Note |
|---|---|---|
| 1 — one definition, no per-file copies | §1, §2, §7 | §7's new gate is what keeps it true |
| 2 — reachable-daemon failure is hard | §3, §4 rule 6 | See §4 on `keycloak_e2e` |
| 3 — both env vars, `0`/unset off, unit-tested | §4, §5, §9 | |
| 4 — uniform greppable marker | §4 | **Partial as literally worded.** The marker exists and is uniform, but F2 shows it stays invisible on the skip path; §6's canary is what makes a skipping run observable. Worth amending the AC rather than claiming it whole. |
| 5 — gate inputs updated iff the reference is added | §7 | |
| 6 — `CLAUDE.md` + `docs/dev-setup.md` | §8 | |
