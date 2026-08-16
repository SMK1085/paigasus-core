# SMA-521 — a retry budget and container-concurrency cap for `paigasus-iam`'s Docker-backed tests

**Issue:** [SMA-521](https://linear.app/smaschek/issue/SMA-521/repo-paigasus-iam-rstest-has-no-retries-and-is-flaky-under-docker)
**Date:** 2026-08-16
**Status:** approved (GATE 1). Revised after adversarial challenge; scope split per the
"middle path" — the skip-policy consolidation and hardening are deferred to a follow-up issue.

## Problem

`paigasus-iam` has 339 `#[tokio::test]` functions across 59 integration test binaries, and
nearly every one starts its own Postgres, Redis, Keycloak or NATS container. nextest runs each
test in its **own process**, in parallel, so they race each other on container startup. Under
that contention `support::connect_when_ready` exhausts its 60-second budget and the test dies
with `postgres did not accept connections within 60s`, reddening a branch whose diff is
unrelated to the failure.

The task CI actually executes is the one inherited from `.moon/tasks/rust.yml`:

```yaml
test:
  command: 'cargo nextest run --no-tests=pass'
```

No retry budget, no concurrency bound. Verifying SMA-505 (PR 124) cost six `moon ci` runs:
five different, unrelated, pre-existing tests failed across the first five, and isolated
re-runs of the worst offender passed cleanly — infrastructure contention, not a regression.

### A premise in the issue that does not survive contact with the repo

**AC3 assumes `CLAUDE.md` documents `CI=1 cargo nextest run -p paigasus-iam --retries 2`. It
does not.** No `*.md` in the repo contains that command; it exists only in an agent's session
memory. `CLAUDE.md:20` documents `cargo nextest run --workspace` and nothing more. AC3 is
therefore *adding* the correct policy, not reconciling a contradiction. The spirit of AC3 —
documented invocation and executed invocation must agree — is what the design satisfies.

### Two failure classes, not one

This is the correction that most changes the design. The first draft modelled only the slow
class and was wrong about the fast one.

| Class | Where | Time to fail | Retry behaviour |
|---|---|---|---|
| **Slow** — connect races container readiness | `connect_when_ready` (60s budget), `nats_publisher`/`nats_permissions` (90s), `keycloak_e2e` (240s startup) | tens of seconds | a retry lands well outside the contention burst |
| **Fast** — host port mapping not yet published | 11 unguarded `get_host_port_ipv4(..).unwrap()/.expect()` sites | **milliseconds** | three attempts can land inside one burst |

The repo already documents both. `tests/nats_publisher.rs:46-50` describes the fast class
exactly — `AsyncRunner::start` returns once the server logs that it is listening, but the
runtime publishes the host-side port mapping independently, so an inspect in that gap returns
`PortNotExposed`; "rare for one container and reproducible when this suite races eight of
them". And `tests/nats_publisher.rs:51-58` measures the contention window: **a full-suite run
that normally takes 3.7s took 33s** under load, which is why its budget is 90s.

A flat `delay = "2s"` retry gives attempts at t, t+2s, t+6s — a ~6-second window against a
measured 33-second burst. For the fast class, all three attempts fail together and **AC1 is
not met**. §1 and §3 both exist to close this.

### The contention is worse locally than in CI

| | logical CPUs | nextest default parallelism | Docker capacity |
|---|---|---|---|
| Dev machine (observed) | 18 | 18 concurrent tests | 8-CPU / 8 GB VM |
| CI (`ubuntu-latest`) | 4 | 4 concurrent tests | shares those 4 vCPUs |

The repo is **public** (`gh repo view` → `isPrivate: false`), so `ubuntu-latest` is the 4-vCPU
standard runner. Note `ci.yml:59`'s comment calls the repo "private" — stale and misleading;
correcting it is out of scope here but worth a follow-up.

Locally a bare `cargo nextest` fires **18 concurrent container startups at an 8-CPU daemon**.
CI is already only ~4-way, and its contention comes mostly from `moon ci` running other tasks
against the same 4 vCPUs. So the cap buys a great deal locally and, by design (D5a), nothing in
CI — where **retries and §3 carry the reliability**.

## Scope

**In this issue:** the nextest profile (§1), the concurrency cap (§2), the `mapped_port` fix
for the fast failure class (§3), Moon input wiring (§4), and docs (§7).

**Deferred to a follow-up issue:** consolidating the 11 duplicated skip-policy copies, the
`PAIGASUS_REQUIRE_DOCKER` / `PAIGASUS_SKIP_DOCKER` env vars, and the error-variant hardening
that turns a live-daemon container failure into a hard error. Rationale in §8. That work is
real and the analysis behind it is preserved in the follow-up issue; it is simply not needed to
satisfy any of SMA-521's acceptance criteria, and it is not independently revertible if bundled.

## Decisions

| # | Decision | Rationale |
|---|---|---|
| D1 | Both levers: a concurrency cap **and** a retry budget | The cap attacks the cause; retries absorb the residue. |
| D2 | Policy lives in `rs/.config/nextest.toml` on `profile.default` | Verified to resolve from the Cargo workspace root regardless of invocation directory — what makes AC2 true (§1). |
| D3 | Scope: `package(=paigasus-iam) and kind(test)` | Exactly the Docker-backed integration binaries; every other crate keeps `retries = 0`. |
| D4 | Backoff sized against the **measured** 33s contention window, not a guess | A 6-second retry window does not clear a 33-second burst. |
| D4a | Fix the fast class at source: a retrying `mapped_port` helper for the 11 unguarded sites | Retries alone cannot fix a failure that recurs in milliseconds. Generalizes what `nats_publisher.rs:61-72` already does locally. |
| D5 | Cap value chosen by measurement, ≤ ~30% wall-clock regression locally | The optimum depends on Docker VM capacity. |
| D5a | **Cap floored at ≥ 4** | CI is 4-way already, so a cap ≥4 is provably a no-op there, removing any risk of serializing CI into the 30-minute job timeout. The cap is a local-only win, stated plainly. |
| D9 | Wire the profile into Moon `inputs` for **both** affected nextest tasks | `paigasus-iam-rs:test` and `repo:nats-permissions` (§4). |
| D10 | Emit JUnit from `profile.default` and upload it in CI | Without it the `FLAKY` signal this design's safety argument rests on is invisible (§6). |

## 1. The nextest profile

New file, `rs/.config/nextest.toml` — the repo's first nextest profile:

```toml
nextest-version = { required = "0.9.136" }

[test-groups.docker-containers]
max-threads = N            # measured (§5); floored at 4 per D5a

[[profile.default.overrides]]
filter     = 'package(=paigasus-iam) and kind(test)'
retries    = { backoff = "exponential", count = 2, delay = "15s", max-delay = "60s", jitter = true }
test-group = 'docker-containers'

# keycloak_e2e starts a 240s-startup-timeout Keycloak; three attempts of a genuinely failing
# run would be ~18 minutes against ci.yml's 30-minute job budget. One retry, not two.
[[profile.default.overrides]]
filter  = 'package(=paigasus-iam) and test(keycloak)'
retries = 1

[profile.default.junit]
path = 'junit.xml'
```

`delay = "15s"` with jitter puts attempts at roughly t, t+15s, t+45s — spanning the measured
33-second contention window rather than sitting inside it. `nextest-version` makes an older
developer-installed nextest fail loudly rather than erroring on an unknown key, which matters
precisely because AC2's argument is that a *bare* `cargo nextest` picks this policy up.

### Verified, not assumed

Against the proto-pinned `cargo-nextest 0.9.136`:

- **The config parses** — every key and the filterset. A **negative control** (`kindzz(test)`)
  was rejected with a caret diagnostic, proving nextest validates filtersets at config-load
  time and that the clean parse was not vacuous.
- **AC2's crux:** invoked from `rs/crates/services/paigasus-iam` — precisely where Moon runs
  the task — nextest still resolved the config at the workspace root, `rs/.config/nextest.toml`.
- **`package()` does not over-match.** The challenge raised that `package(paigasus-iam)` might
  match `paigasus-iam-core` by substring, silently granting retries to a proptest suite. Tested:
  `cargo nextest list -E 'package(paigasus-iam)'` matches **0** `paigasus-iam-core` tests, while
  a bare listing matches 116 — the control proves the check bites. The claim is **refuted**.
  D3 still writes the explicit `=` form so no reader has to know the matcher's semantics.

### Why `profile.default` and not `[profile.ci]`

A `ci` profile only applies with `--profile ci`, which the inherited
`cargo nextest run --no-tests=pass` does not pass. Adding one would recreate the exact
"documented ≠ executed" split this issue exists to close, violating AC2.

### Why this cannot mask a genuine failure

nextest retries only a failing attempt. A test that eventually passes is reported **FLAKY**
and the run stays green; a test that fails all attempts is **FAIL** and the run exits non-zero.
D3's narrow filter means nothing outside `paigasus-iam`'s integration tests gains any retry
budget. D10 makes the FLAKY signal actually observable (§6).

**Accepted residual risk.** A test that degrades from "rare flake" to "fails roughly half the
time" passes as flaky rather than red. nextest 0.9.x has no "fail the run on flaky" knob.
Mitigation is D10's JUnit artifact; escalation threshold in §6.

## 2. Container-concurrency cap

`[test-groups.docker-containers] max-threads = N` bounds how many tests in the group run
concurrently, on top of (not instead of) nextest's global `test-threads`.

**The cap is per nextest process, not global.** `moon ci` runs `paigasus-iam-rs:test` and
`repo:nats-permissions` (`moon.yml:180-183`, itself `cargo nextest run -p paigasus-iam --test
nats_permissions`) as separate Moon tasks, each spawning its own `cargo nextest` with its own
budget. The real bound under `moon ci` is therefore `N × (concurrent nextest tasks)`, and
`nats_permissions` runs **twice** per graph — once inside the crate suite, once as the repo
gate. That duplication is deliberate and documented at `moon.yml:170-176`; deduplicating it is
out of scope, but N must be sized knowing it exists.

Given D5a floors N at 4 and CI is already 4-way, the cap is a **local-only** improvement:
18-way against an 8-CPU daemon becomes N-way. This is stated rather than hidden, because it
means CI's reliability comes from D1's retries and D4a's fast-class fix, not from the cap.

## 3. `mapped_port` — fixing the fast failure class (D4a)

Eleven sites call `get_host_port_ipv4(..)` and immediately `.unwrap()`/`.expect()`, so they die
in milliseconds when the runtime has not yet published the port mapping:

`tests/support/mod.rs:94` · `tests/redis_jwks_cache.rs:35` · `tests/authz_cache_redis.rs:41` ·
`tests/authz_generations_redis.rs:42` · `tests/api_key_cache_redis.rs:37` ·
`tests/authz_acceptance.rs:87` · `tests/api_key_cache_connection.rs:48` ·
`tests/authz_system_retirement_pg.rs:413` · `tests/outbox_retention_concurrency_pg.rs:70,160` ·
`tests/keycloak_e2e.rs:94`

`tests/nats_publisher.rs:61-72` already solves this locally with a retry loop against a load
budget. Generalize it into a small shared helper:

```rust
#[allow(dead_code)]
pub async fn mapped_port(node: &ContainerAsync<impl Image>, port: u16) -> u16
```

It retries until the mapping is published or a load budget expires, then panics with a
diagnostic — so a genuinely missing port still fails, loudly, rather than hanging.

### Where it lives

A **standalone** `tests/support/docker.rs`, depending on nothing else in `support`:

- The 7 affected files that already declare `mod support;` reach it as `support::docker::mapped_port`
  (`support/mod.rs` gains `pub mod docker;`).
- The 4 that do not — `redis_jwks_cache.rs`, `authz_cache_redis.rs`,
  `authz_generations_redis.rs`, `api_key_cache_redis.rs` — use
  `#[path = "support/docker.rs"] mod docker;`, pulling in that one small file and nothing else.

**Not `support/mod.rs` itself**, for two independently fatal reasons established during the
challenge:

1. **It would not compile.** `rs/Cargo.toml:217` sets `[workspace.lints.rust] warnings = "deny"`,
   so `dead_code` is a hard error. `start_migrated_postgres` (`support/mod.rs:65`) and
   `connection_url` (`:93`) are the only `pub` items in that file **without**
   `#[allow(dead_code)]`, and the four Redis-only files call neither. All four binaries would
   fail to build and `moon ci :lint` would red. The file's own header (`:22-24`) documents this
   exact mechanism — hence every item in `docker.rs` carries `#[allow(dead_code)]`.
2. **It would stale-cache a security gate.** `moon.yml:205-211` narrows
   `repo:nats-permissions`'s inputs and *deliberately excludes* `tests/support/**/*`, reasoning
   that "nats_permissions.rs has no `mod support;` and never references it". Pulling the whole
   support surface into more binaries erodes that boundary.

`nats_permissions.rs` is **not** among the 11 sites — it already retries its port lookup — so it
is untouched by this change and `moon.yml`'s exclusion comment remains true as written.

## 4. Moon wiring (D9)

`rs/` is not a Moon project — `.moon/workspace.yml` globs cover `contracts`, `rs/crates/libs/*`,
`rs/crates/bindings/*`, `rs/crates/services/*`, `py`, `ts` and their packages. `rs/.config/`
therefore belongs only to the root `repo` project.

**Two tasks run nextest against `paigasus-iam` and neither has the profile in its input set:**

1. `paigasus-iam-rs:test` inherits `inputs: ['@group(sources)', '@group(tests)', 'Cargo.toml']`,
   all project-relative, plus `implicitInputs`, which lists only `.moon/*` files.
   Fix: add `/rs/.config/nextest.toml` (workspace-relative) to the `test` task's `inputs` in
   `.moon/tasks/rust.yml`, covering every Rust project's test task in one edit.
2. `repo:nats-permissions` is a task on the root `repo` project (`language: 'bash'`), so
   `.moon/tasks/rust.yml`'s `inheritedBy: languages: ['rust']` does not attach. Its narrow
   `inputs` (`moon.yml:205-211`) do not list the profile, yet its script runs
   `cargo nextest run -p paigasus-iam --test nats_permissions` — matched by D3's override.
   Fix: add `rs/.config/nextest.toml` to that task's `inputs`.

`repo:observability-drift` also invokes nextest, but against `-p paigasus-observability`, which
D3's filter does not match — the profile cannot change its result, so adding the input would
only cause needless cache busts. Deliberately not changed.

## 5. Choosing the cap

Measured on the dev machine's real Docker VM (8 CPUs / 8 GB), with `CI=1` exported so skips
panic and the timing reflects a run that genuinely ran:

1. Establish an uncapped baseline on unmodified `origin/main` first, so the diff is never
   blamed for pre-existing flakiness.
2. Time `cargo nextest run -p paigasus-iam` at caps 8, 6, 4 (D5a's floor).
3. Repeat the finalists to average out the known "a different random subset fails each run"
   noise. Use `cargo nextest` directly, not `moon run`, so Moon's task cache cannot serve a
   previous timing.
4. Choose the **lowest cap ≥ 4** whose wall-clock stays within ~30% of the uncapped baseline.

The chosen number, its measurements, and the machine they came from are recorded in a comment
beside the setting, so a future reader can re-derive it instead of guessing.

**CI cost is measured too, not assumed.** The implementation PR records the `moon ci`
wall-clock delta against `origin/main`. D5a makes the cap a CI no-op by construction, so the
only CI cost is retries on failure; if the observed delta is material, §8's rollback applies.

## 6. Verification

Each check targets a specific way this change could silently do nothing.

| Risk | Proof |
|---|---|
| Override matches nothing | `cargo nextest show-config test-groups` — the `docker-containers` membership set must **equal** the iam integration tests, asserted as equality, not containment. |
| Override matches too much | The same listing must contain **zero** `paigasus-iam-core` or other-crate tests. (Already refuted as a live risk in §1; the check is free and pins it.) |
| `paigasus-iam-rs:test` serves a stale cached pass | Touch only `rs/.config/nextest.toml`; `moon query tasks --affected` must list it. |
| `repo:nats-permissions` serves a stale cached pass | Same touch; the same query must list `repo:nats-permissions` too. |
| `mapped_port` doesn't actually retry | Unit-test the loop against a stub that fails N times then succeeds — the assertion must fail if the retry is removed. |

The retry test lives in its own small integration binary, `tests/support_docker_retry.rs`, which
includes `docker.rs` via `#[path]`. **Not** a `#[cfg(test)]` module inside `docker.rs`: `cfg(test)`
is not enabled when compiling an integration-test binary, so such a module would be silently
compiled out and never run — a vacuous test. Nor plain `#[tokio::test]` functions inside
`docker.rs`, which would duplicate across every binary that includes it. To keep the stub
Docker-free, `mapped_port` is generic over a tiny `PortSource` trait implemented for
`ContainerAsync<I>` in production and by a fail-N-times counter in the test.
| Retries never engage | D10's JUnit artifact records flaky reruns; §5's measurement runs surface them directly. |

**Observability (D10).** `.moon/tasks.yml:25-26` sets `taskOptions.outputStyle:
'buffer-only-failure'`, so a task that goes green-with-flakes prints **nothing** — the FLAKY
signal the safety argument in §1 depends on is invisible in the one environment that matters.
The profile therefore emits JUnit and `ci.yml` uploads it as an artifact on the `moon ci` job.
Escalation threshold: if a single test appears as flaky in more than half the runs of a week,
it stops being a container-contention flake and gets its own issue rather than a retry.

Full-graph gates, per `CLAUDE.md` (new files plus `.moon/tasks/rust.yml` and `moon.yml` edits
mean per-project tasks are not sufficient):

```
moon ci :build :test :lint :fmt :deny :osv :machete :typecheck :breaking :affected-smoke \
  :parity-corpus-drift :next-env-drift :wasm-getrandom-free :redis-connect-single-site \
  :promtool :observability-drift :nats-permissions :release-parity :release-parity-py \
  :release-parity-ts :publish-metadata --base origin/main --include-relations
```

## 7. Documentation (AC3)

`CLAUDE.md` gains, in the Gotchas section:

- the retry/concurrency policy lives in `rs/.config/nextest.toml`; Moon, the manual command and
  a bare `cargo nextest` all pick it up from there — but **`cargo test` does not**, since
  nextest config is nextest-only. Stated explicitly so AC2's "every entry point" is not
  over-read.
- the tell: an iam suite that finishes in under a second **skipped**, it did not pass (the
  deferred follow-up addresses the cause; the tell is worth documenting now regardless).

`docs/dev-setup.md` gains the developer-facing version of the same points.

## 8. Rollback and the scope split

`rs/.config/nextest.toml` plus the two `inputs` lines revert as a unit with no code impact — if
CI wall-clock or timeout regressions appear, that is the first move. §3 is a code change but a
narrow one: a new standalone file plus eleven one-line call-site substitutions, with no
behavioural change on the happy path.

The deferred work (skip-policy consolidation, the two env vars, error-variant hardening) was
split out precisely because it is *not* narrow: 10 test files, a shared module carrying real
policy, a documented `moon.yml` gate boundary to renegotiate, and a local-dev behaviour change.
Bundled, a CI regression from either half would force reverting both. It satisfies none of
SMA-521's acceptance criteria on its own, so it loses nothing by shipping separately.

## Out of scope

- Skip-policy consolidation, `PAIGASUS_REQUIRE_DOCKER` / `PAIGASUS_SKIP_DOCKER`, and
  error-variant hardening — **deferred to a follow-up issue** (see §8).
- A "fail the run on flaky" gate (see §1's accepted residual risk).
- Reworking `connect_when_ready`'s 60-second budget or the testcontainers log-based
  ready-condition.
- Reusing one container across tests within a binary — a real wall-clock win, but it would
  change the per-test isolation `support/mod.rs` documents as load-bearing.
- Deduplicating `nats_permissions`'s two runs per `moon ci` graph (§2).
- Correcting `ci.yml:59`'s stale "private repo" comment.
- Any change to CI runner sizing or `moon ci`'s own task concurrency.

## Acceptance criteria mapping

| AC | Satisfied by |
|---|---|
| 1. `moon ci :test` retries transient container-startup failures | §1's retry budget sized against the measured 33s window, **plus §3's `mapped_port`** for the millisecond-scale class retries alone cannot fix, plus §2's cap reducing incidence locally |
| 2. Policy lives where every entry point picks it up | §1 — verified workspace-root resolution from Moon's invocation directory; §4 wires both affected Moon tasks; §7 documents the `cargo test` exception |
| 3. Documented command matches what CI executes | §7 — with the premise correction that there was no conflicting documented command to begin with |
| Note: must not mask a genuinely failing test | §1 (FLAKY vs FAIL, narrow filter, verified non-over-matching) and D10 making the FLAKY signal observable |
