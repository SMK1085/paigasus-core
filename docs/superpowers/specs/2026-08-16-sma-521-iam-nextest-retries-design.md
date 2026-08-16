# SMA-521 — a retry budget and container-concurrency cap for `paigasus-iam`'s Docker-backed tests

**Issue:** [SMA-521](https://linear.app/smaschek/issue/SMA-521/repo-paigasus-iam-rstest-has-no-retries-and-is-flaky-under-docker)
**Date:** 2026-08-16
**Status:** design approved, pending adversarial challenge

## Problem

`paigasus-iam` has 339 `#[tokio::test]` functions across 59 integration test binaries, and
nearly every one starts its own Postgres, Redis, Keycloak or NATS container. They race each
other on startup. Under that contention `support::connect_when_ready` exhausts its 60-second
budget and the test dies with `postgres did not accept connections within 60s`, reddening a
branch whose diff is unrelated to the failure.

The task CI actually executes is the one inherited from `.moon/tasks/rust.yml`:

```yaml
test:
  command: 'cargo nextest run --no-tests=pass'
```

No retry budget, no concurrency bound. Verifying SMA-505 (PR 124) cost six `moon ci` runs:
five different, unrelated, pre-existing tests failed across the first five, and isolated
re-runs of the worst offender passed cleanly — infrastructure contention, not a regression.

### Two premises in the issue that do not survive contact with the repo

1. **AC3 assumes `CLAUDE.md` documents `CI=1 cargo nextest run -p paigasus-iam --retries 2`.
   It does not.** No `*.md` in the repo contains that command; it exists only in an agent's
   session memory. AC3 is therefore *adding* the correct policy, not reconciling a
   contradiction. The spirit of AC3 — documented invocation and executed invocation must
   agree — is preserved and is what the design satisfies.
2. **The issue estimates the duplicated skip policy at "the same code path".** It is
   11 near-identical copies across 10 files (see §3).

### The contention is worse locally than in CI

This asymmetry decides how much work each lever is doing:

| | logical CPUs | nextest default parallelism | Docker capacity |
|---|---|---|---|
| Dev machine (observed) | 18 | 18 concurrent tests | 8-CPU / 8 GB VM |
| CI (`ubuntu-latest`) | 4 | 4 concurrent tests | shares those 4 vCPUs |

Locally a bare `cargo nextest` fires **18 concurrent container startups at an 8-CPU daemon**.
CI is already only ~4-way, and its contention comes mostly from `moon ci` running other tasks
(cargo builds, clippy, wasm-pack) against the same 4 vCPUs. A concurrency cap therefore buys
a great deal locally and comparatively little in CI, where retries carry most of the weight.
The design uses both levers deliberately, with retries as the safety net rather than the
primary mechanism.

## Decisions

| # | Decision | Rationale |
|---|---|---|
| D1 | Both levers: a concurrency cap **and** a retry budget | The cap attacks the cause; retries absorb the residue. Either alone leaves AC1 probabilistic or the cause untouched. |
| D2 | Policy lives in `rs/.config/nextest.toml` on `profile.default` | Verified to resolve from the Cargo workspace root regardless of invocation directory, which is what makes AC2 true (§1). |
| D3 | Scope: `package(paigasus-iam) and kind(test)` | Exactly the Docker-backed integration binaries. `paigasus-iam`'s own `--lib` unit tests and every other crate keep `retries = 0`, so a flaky pure-logic test still reds immediately. Self-maintaining as tests are added. |
| D4 | Exponential backoff with jitter, not a flat retry | The failure is contention; an immediate retry re-enters the same jam and co-failing tests would retry in lockstep. |
| D5 | Cap value chosen by measurement, ≤ ~30% wall-clock regression | The optimum depends on Docker VM capacity, not on a number anyone can reason to a priori. |
| D6 | Consolidate the 11 skip-policy copies into one helper | One definition of a policy that currently drifts across 10 files. |
| D7 | `PAIGASUS_REQUIRE_DOCKER=1` makes local skips hard-fail | The only *reliable* guard, given nextest discards passing tests' output (§3). |
| D8 | A reachable daemon plus a failed container is a **hard failure**, not a skip | Makes "skip" mean what its name says (§3). |
| D9 | Add `/rs/.config/nextest.toml` to the Rust `test` task's Moon `inputs` | Without it the profile is outside every project's input set: stale cache hits and no affected-graph selection (§4). |

## 1. The nextest profile

New file, `rs/.config/nextest.toml` — the repo's first nextest profile:

```toml
[test-groups.docker-containers]
max-threads = N            # the ONLY value left open by this spec; §5 fixes the
                           # procedure that determines it during implementation

[[profile.default.overrides]]
filter     = 'package(paigasus-iam) and kind(test)'
retries    = { backoff = "exponential", count = 2, delay = "2s", jitter = true }
test-group = 'docker-containers'
```

Every other value above is final. `count = 2` means up to three attempts, matching the retry
budget the issue's manual workaround used.

### Verified, not assumed

Against the proto-pinned `cargo-nextest 0.9.136`:

- **The config parses** — every key and the filterset. A **negative control** (`kindzz(test)`)
  was rejected with a caret diagnostic, proving nextest validates filtersets at config-load
  time and that the clean parse was not vacuous.
- **AC2's crux:** invoked from `rs/crates/services/paigasus-iam` — precisely where Moon runs
  the task — nextest still resolved the config at the workspace root, `rs/.config/nextest.toml`.
  So Moon, the manual command, and a developer's bare `cargo nextest` all read one definition.

### Why `profile.default` and not `[profile.ci]`

A `ci` profile only applies with `--profile ci`, which the inherited
`cargo nextest run --no-tests=pass` does not pass. Adding one would recreate the exact
"documented ≠ executed" split this issue exists to close, violating AC2.

### Why this cannot mask a genuine failure

nextest retries only a failing attempt. A test that eventually passes is reported **FLAKY**
and the run stays green; a test that fails all three attempts is **FAIL** and the run exits
non-zero. Combined with D3's narrow filter, nothing outside `paigasus-iam`'s integration
tests gains any retry budget at all.

**Accepted residual risk.** A test that degrades from "rare flake" to "fails roughly half the
time" will pass as flaky rather than red. The signal is nextest's per-occurrence `FLAKY` line.
nextest 0.9.x has no "fail the run on flaky" knob, and building one is out of scope here; if
this bites, it earns its own issue.

## 2. Container-concurrency cap

`[test-groups.docker-containers] max-threads = <N>` bounds how many tests in the group run
concurrently, on top of (not instead of) nextest's global `test-threads`. The global setting
is left at its default so non-container crates keep full parallelism.

Expected effect, given the table above: local parallelism drops from 18-way to N-way against
an 8-CPU daemon — the large win. In CI, where nextest is already only 4-way, a cap at or above
4 is a no-op by construction; this is stated plainly rather than hidden, because it means
**CI's reliability comes primarily from D1's retries**, not from the cap.

## 3. Skip-policy consolidation and hardening

### Measured scale

11 copies of `Docker unavailable → panic if CI, else eprintln + return None`:

| File | Copies | Has `mod support;` |
|---|---|---|
| `tests/support/mod.rs` (`start_migrated_postgres`, `start_raw_postgres`) | 2 | n/a |
| `tests/api_key_cache_connection.rs` | 1 | yes |
| `tests/authz_acceptance.rs` | 1 | yes |
| `tests/keycloak_e2e.rs` | 1 | yes |
| `tests/nats_publisher.rs` | 1 | yes |
| `tests/api_key_cache_redis.rs` | 1 | **no** |
| `tests/authz_cache_redis.rs` | 1 | **no** |
| `tests/authz_generations_redis.rs` | 1 | **no** |
| `tests/nats_permissions.rs` | 1 | **no** |
| `tests/redis_jwks_cache.rs` | 1 | **no** |

52 of 59 test files already declare `mod support;`, so adding it to the five that do not
follows the established pattern rather than introducing one.

### One shared helper

The 11 sites differ only in a human-readable label, so a single generic helper in
`support/mod.rs` replaces all of them. `testcontainers 0.27` and `testcontainers-modules 0.15`
are both in scope, so `Postgres`, `Redis` and `GenericImage` unify under one signature:

```rust
pub async fn start_or_skip<T, I>(image: T, what: &str) -> Option<ContainerAsync<I>>
where
    T: Into<ContainerRequest<I>> + Send,
    I: Image,
```

### Why "make the skip loud" cannot mean "improve the message"

Every one of the 11 sites **already** prints `eprintln!("skipping …")`. Nobody ever sees it,
because nextest defaults to `success-output = "never"`: stderr from a *passing* test is
captured and discarded. That capture **is** the silent-skip mechanism. Two levers work:

- **`PAIGASUS_REQUIRE_DOCKER` (D7)** makes the helper panic exactly as it does under `CI`.
  This is the reliable guard. CI behaviour is unchanged, since `CI` is already set there.
  Semantics are **presence-based** (`std::env::var_os(..).is_some()`), matching the existing
  `CI` check it sits beside — so `PAIGASUS_REQUIRE_DOCKER=0` also enables strict mode. Docs
  spell the variable as `PAIGASUS_REQUIRE_DOCKER=1`; to disable it, unset it.
- A **uniform, greppable marker** on the skip line, so `--no-capture` and
  `--success-output immediate` have a stable string to match. The marker is the literal
  prefix `SKIP[docker-unavailable]`, followed by the caller's label and the underlying error —
  one format, emitted from the single helper, so `grep 'SKIP\[docker-unavailable\]'` finds
  every skip in a run and nothing else.

Scoping `success-output = "immediate"` through the same override was considered and
**rejected**: it would dump output from all 339 passing tests, not only the skipping ones.

### The daemon probe (D8)

Today *any* `start()` error skips locally — an image-pull failure, a full disk, or **the very
container-startup contention this issue exists to fix**. On an 18-way/8-CPU machine the local
suite is therefore liable to report passes for tests it never ran, under exactly the load
SMA-521 is about.

The helper probes daemon reachability **once per test binary** (cached in a `OnceCell`, so the
cost is paid once, not 339 times) and splits the cases:

| Daemon reachable | `CI` or `PAIGASUS_REQUIRE_DOCKER` set | Outcome |
|---|---|---|
| no | no | skip — return `None`, greppable marker (today's behaviour) |
| no | yes | panic (today's behaviour) |
| **yes** | either | **hard failure** — the container failed for a real reason |

This composes with §1: retries absorb transient startup contention first, so only a failure
that survives all three attempts reaches the hard-failure path.

## 4. Moon wiring (D9)

`rs/` is not a Moon project — `.moon/workspace.yml` globs cover `contracts`, `rs/crates/libs/*`,
`rs/crates/bindings/*`, `rs/crates/services/*`, `py`, `ts` and their packages. `rs/.config/`
therefore belongs only to the root `repo` project.

`paigasus-iam-rs:test` inherits `inputs: ['@group(sources)', '@group(tests)', 'Cargo.toml']`,
all project-relative, plus `implicitInputs`, which lists only `.moon/*` files. The new profile
is in neither set. Left alone, that means:

- editing the retry/cap policy does **not** bust `paigasus-iam-rs:test`'s Moon cache — a stale
  cached PASS can be served; and
- a profile-only PR does **not** select the test task under `moon ci --affected`.

Fix: add `/rs/.config/nextest.toml` (workspace-relative) to the `test` task's `inputs` in
`.moon/tasks/rust.yml`, which covers every Rust project's test task in one edit.

## 5. Choosing the cap

Measured on the dev machine's real Docker VM (8 CPUs / 8 GB), with `CI=1` exported so skips
panic and the timing reflects a run that genuinely ran:

1. Establish an uncapped baseline on unmodified `origin/main` first, so the diff is never
   blamed for pre-existing flakiness.
2. Time `cargo nextest run -p paigasus-iam` at caps 8, 6, 4, 3, 2.
3. Repeat the finalists to average out the known "a different random subset fails each run"
   noise. Use `cargo nextest` directly, not `moon run`, so Moon's task cache cannot serve a
   previous timing.
4. Choose the **lowest cap whose wall-clock stays within ~30% of the uncapped baseline**.

The chosen number, its measurements, and the machine they came from are recorded in a comment
beside the setting, so a future reader can re-derive it instead of guessing.

## 6. Verification

Each check targets a specific way this change could silently do nothing.

| Risk | Proof |
|---|---|
| Override parses but matches no test | `cargo nextest show-config test-groups` lists the iam integration tests under `docker-containers`. The same override block carries `retries`, so a match proves both apply. |
| Moon serves a stale cached pass | Touch only `rs/.config/nextest.toml`; `moon query tasks --affected` must list `paigasus-iam-rs:test`. |
| Skip policy regresses unnoticed | Extract the decision as a pure function (`daemon_up`, `ci`, `strict` → skip / panic / run) and unit-test every row of §3's table. |
| Retries never actually engage | The measurement runs in §5 surface `FLAKY` lines when contention occurs; a run reporting flaky-but-green is direct evidence. |

The skip-policy unit tests go in **one dedicated test binary**, not in `#[test]` functions
inside `support/mod.rs` — the latter would compile and run the same tests in all 52 binaries
that declare `mod support;`.

Full-graph gates, per `CLAUDE.md` (a new file plus a `.moon/tasks/rust.yml` edit means
per-project tasks are not sufficient):

```
moon ci :build :test :lint :fmt :deny :osv :machete :typecheck :breaking :affected-smoke \
  :parity-corpus-drift :next-env-drift :wasm-getrandom-free :redis-connect-single-site \
  :promtool :observability-drift :nats-permissions :release-parity :release-parity-py \
  :release-parity-ts :publish-metadata --base origin/main --include-relations
```

## 7. Documentation (AC3)

`CLAUDE.md` gains, in the Gotchas section:

- the retry/concurrency policy lives in `rs/.config/nextest.toml` and every entry point —
  Moon, the manual command, a bare `cargo nextest` — picks it up from there;
- `PAIGASUS_REQUIRE_DOCKER=1` makes a Docker-less local run hard-fail instead of skipping;
- the tell: an iam suite that finishes in under a second **skipped**, it did not pass.

`docs/dev-setup.md` gains the developer-facing version of the same three points.

## Out of scope

- A "fail the run on flaky" gate (see §1's accepted residual risk).
- Reworking `connect_when_ready`'s 60-second budget or the testcontainers log-based
  ready-condition. The retry budget makes the existing race tolerable; replacing the readiness
  mechanism is a larger change with its own risk surface.
- Reusing one container across tests within a binary. A real wall-clock win, but it would
  change the per-test isolation that `support/mod.rs` documents as load-bearing (each test
  runs against its own freshly migrated Postgres, which is why a per-binary counter suffices
  for grant ids).
- Any change to CI runner sizing or `moon ci`'s own task concurrency.

## Acceptance criteria mapping

| AC | Satisfied by |
|---|---|
| 1. `moon ci :test` retries transient container-startup failures | §1 retry budget on `profile.default`, plus §2's cap reducing incidence |
| 2. Policy lives where every entry point picks it up | §1 — verified that workspace-root config resolution holds from the crate directory Moon runs in; §4 wires it into Moon's input set |
| 3. Documented command matches what CI executes | §7 — and note the premise correction: there was no conflicting documented command to begin with |
| Note: must not mask a genuinely failing test | §1 (FLAKY vs FAIL, narrow filter) and §3's D8 hardening, which converts a class of today's silent skips into hard failures |
