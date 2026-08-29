<!-- SPDX-License-Identifier: Apache-2.0 -->

# `cargo-lock-integrity`

Asserts that `rs/Cargo.lock` satisfies every workspace manifest in `rs/`. It runs as a
plain `ci.yml` step, not a `repo:*` Moon task — see below for why.

`run.sh` runs `cargo metadata --locked --format-version 1` from `rs/` and reports one
of three outcomes: the lock is consistent, the lock is not consistent, or the check
could not be completed at all.

## The three modes, and why `ci.yml` runs all of them

`run.sh` takes `--self-test`, `--negative-control`, or no argument. The `ci.yml` step
runs all three, in that order, under an explicit `set -euo pipefail`.

The bare mode alone is not enough. Delete `--locked` from the `cargo metadata` line and
that command exits `0` **and repairs the lock itself** — the gate then prints "satisfies
every manifest" and becomes the first repairer, defeating this whole gate silently and
permanently. Only `--negative-control` catches that: it mutates a copy of the lock in a
tempdir and requires the assertion to report `rc=1`. This is the same "control that
actively lies" shape SMA-530 closed for `release-parity`, and the same reason
`version-lockstep` and `workflow-credentials` run their controls in CI too.

Check 8f in `ci/actionlint/run.sh` pins both halves: `T_CARGO_LOCK_STEP_REQUIRED` pins
the step's six lines in `ci.yml` — in order, inside the step's own window, with no `if:`
and no suppressing `continue-on-error:` — and `T_CARGO_LOCK_SH_CALL_SITES` pins six
whole lines inside this `run.sh`, including the `cargo metadata --locked` line itself.
`repo:actionlint` carries `inputs: ['**/*']`, so that check is scheduled on every PR.

## The measured root cause

Dependabot cargo PRs have five times shipped a truncated `rs/Cargo.lock` (PRs 83, 96,
140, 149, 181) that then merged through a green required `moon ci` check.

SMA-601 first attributed the green report to `repo:publish-metadata`'s publish groups
happening to be satisfied by the surviving subset. That is true but it does not explain
the report, because `.moon/tasks/rust.yml`'s `lint` task already runs
`cargo clippy --locked --all-targets` on all thirteen crates and declares
`/rs/Cargo.lock` among its `inputs`.

Measured on PR 181's truncated commit `72c0ddb52` — 176 packages against `main`'s 543,
holding 5 of the 13 workspace members:

| Measurement | Result |
| -- | -- |
| `cargo metadata --locked` from `rs/` | exit 101 |
| Same, from `rs/crates/services/paigasus-gateway/` | exit 101 |
| `paigasus-gateway-rs:lint` on PR 181 (`cargo clippy --locked`) | pass, 23s 958ms, not cached |
| `paigasus-iam-rs:lint` on PR 181 (`cargo clippy --locked`) | pass, 1m 11s 740ms, not cached |

Both `--locked` clippy runs executed for real and passed. They should have failed. The
reason is that an earlier task repaired the lock on disk:

| Command | `--locked`? | Lock before to after | Exit |
| -- | -- | -- | -- |
| `cargo tree -p paigasus-wasm --target wasm32-unknown-unknown -e no-dev` (`repo:wasm-getrandom-free`) | no | 176 to 548 | 0 |
| `cargo deny --manifest-path rs/Cargo.toml check` (`repo:deny`) | no | 176 to 548 | 0 |

An unlocked cargo invocation re-resolves and rewrites an inconsistent lock in place,
mid-run, before any `--locked` task reaches it. The repaired lock exists only in the
runner's workspace and is never committed, so `main` keeps the truncated file.

Ordering, re-derived from task start timestamps in job 99064479471 (never from
durations, which are invalid under a parallel scheduler):

| Task | Start | Note |
| -- | -- | -- |
| `repo:deny` | 06:37:55 | unlocked `cargo deny` |
| `repo:wasm-getrandom-free` | 06:37:55 | unlocked `cargo tree` |
| `paigasus-kernel-rs:lint` | 06:38:07 | first `--locked` task, 12s later |
| `paigasus-kernel-ts:build` | 06:41:03 | unlocked cargo via `napi` / `wasm-pack` |
| `paigasus-kernel-py:test` | 06:41:24 | unlocked cargo via `uv sync` / maturin |
| `paigasus-gateway-rs:lint` | 06:41:25 | |
| `paigasus-iam-rs:lint` | 06:41:41 | |

`repo:deny` and `repo:wasm-getrandom-free` start together, 12 seconds before the first
`--locked` task and about three minutes before the FFI tasks. So the two named commands
are the actual first repairers, and the FFI tasks are not.

## Why this is a `ci.yml` step, not a `repo:*` Moon task

A Moon task would run inside the same graph as `repo:deny` and
`repo:wasm-getrandom-free`, and Moon's scheduler gives no ordering guarantee against
them. A `repo:*` task checking the lock would race those repairers exactly as the
existing `--locked` tasks do today, and could lose the race the same way. This step
instead runs in `ci.yml`, before the `moon ci` step, when nothing has run yet and the
working tree still holds the committed lock untouched. That is the same argument
CLAUDE.md records for the codegen-drift step, which is also an unconditional inline
`ci.yml` step rather than a `repo:*` task.

## Exit codes

`run.sh` uses the repo's usual three codes:

- `0` — pass. `rs/Cargo.lock` satisfies every workspace manifest.
- `1` — the lock does not satisfy the manifests. This is the real finding the gate
  exists to catch.
- `2` — infrastructure. The gate asserted nothing.

Code `2` exists because `cargo metadata` has no distinct exit code for "the registry is
down" versus "your lock is broken" — it exits `101` for a broken lock, a malformed
manifest, and a registry outage alike. `run.sh` classifies the failure by reading
cargo's captured stderr instead. Without that split, a crates.io outage would report
red on a REQUIRED check for a reason that has nothing to do with the lock, and a reviewer
would chase a phantom truncation. The `--locked` message is tested first and
unconditionally: a truncated-lock run also prints "Updating crates.io index" first, so a
network-pattern test evaluated before the `--locked` test would misfile every real
detection as infrastructure, and the gate would never report red at all.

## Limitations

* **`--locked` proves consistency, not correctness.** A lock that is complete but
  wrong — a version swapped for another that still satisfies every requirement, a
  tampered `checksum`, a removed `[patch]` — passes `cargo metadata --locked`. This
  gate detects truncation and any other inconsistency with the manifests. It is not a
  lockfile-tampering detector, and nothing here becomes one.
* **`napi build`, `uv sync`/maturin, and `wasm-pack build` cannot be locked**
  (measured). `napi build` exposes no `--locked` flag and no cargo passthrough; `uv
  sync` drives maturin with no flag path either. `wasm-pack build … -- --locked` looks
  fixable — the passthrough does reach the `cargo build` wasm-pack forwards to (a bad
  flag there, e.g. `-- --zzz-not-a-real-cargo-flag`, is rejected with exit 1) — but
  measured against a truncated 176-package lock it still exits 0 and rewrites the lock
  176 -> 548: wasm-pack makes its own unlocked cargo call BEFORE the forwarded build and
  repairs the lock there first, so the forwarded `--locked` sees an already-valid lock.
  Three tasks therefore still re-resolve. This gate has already reported before they
  run, so they cannot mask a truncated lock, but their own cargo work is not audited
  against the shipped resolution.
* **Gate scripts under `ci/**` are outside this gate's derived set.** A cargo call
  inside a `.sh` invoked by a Moon task is not in Moon's resolved command string.
  Today's instances are `ci/version-lockstep/run.sh`'s deliberate `cargo update -w`
  writers, which run only in `--write` mode, and `ci/publish-metadata/run.sh`'s
  `cargo metadata --no-deps`, which performs no resolution. A text scan over those files
  was measured to have an intolerable false-positive rate and is deliberately not
  built.
* **The sibling lockfiles are out of scope.** `ts` is already safe (`ci.yml:177`,
  `pnpm --dir ts install --frozen-lockfile`). `py` is not: CLAUDE.md records that `py`'s
  `moon.yml` runs bare `uv sync`, not `--locked`, and that `py/uv.lock` drifts silently.
  That is a separate issue.
* **`wheels.yml` and `prebuild.yml` are out of scope.** Both invoke cargo and maturin,
  and neither runs inside `moon ci`. A truncated lock would produce silently different
  published wheels. Separate issue.
* **A task whose script mentions cargo in a string but never runs it would be a false
  positive for a text-scan approach.** There are none today. When one appears it takes
  an `ALLOW_UNLOCKED_CARGO` entry with a reason.
* **Nothing asserts that a future cargo-invoking `repo:*` gate declares
  `rs/.cargo/config.toml` in its `inputs`.** Pre-existing, recorded in CLAUDE.md, not
  closed here.
