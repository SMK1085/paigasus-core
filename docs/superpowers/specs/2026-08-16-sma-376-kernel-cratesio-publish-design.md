# SMA-376 — crates.io publishing metadata for `paigasus-kernel`

**Date:** 2026-08-16
**Linear:** SMA-376 (blocks SMA-407; relates to SMA-357, SMA-388, SMA-428)
**Status:** Approved (design) — revised after adversarial review

## Problem

`paigasus-kernel` was bootstrapped in SMA-357 with `version = "0.0.0"`,
`publish = false`, and a `# TODO(SMA-376)` marking exactly this gap. Per ADR-0005
the kernel is the crates.io-bound half of the open-core boundary — `paigasus-cloud`
and the language bindings consume it as a *versioned, published* dependency edge
(ADR-0006), not as a path dep. Leaving it unpublishable indefinitely means the
boundary is untested until the day it matters.

Two concrete gaps today:

1. **The metadata is absent and nothing notices.** `cargo package` emits only
   `warning: manifest has no description, documentation, homepage or repository`
   and exits 0. crates.io rejects incomplete metadata *at upload time* — the worst
   possible moment to discover it.
2. **`publish = false` makes publishability unassertable.** `cargo publish
   --dry-run -p paigasus-kernel` refuses outright:
   ``error: `paigasus-kernel` cannot be published. `package.publish` must be set to
   `true` or a non-empty list``.

Flipping the flag also *arms* a hazard the issue names: a publishable crate at
`0.0.0` will publish `0.0.0` and burn the version namespace the moment a release
path exists.

## Outcome

`paigasus-kernel` carries complete crates.io metadata, `publish = true`, and no
`TODO(SMA-376)`. A new CI gate proves — on every Rust change — that every
publishable crate has metadata crates.io will actually accept, packages exactly the
files it should, and cannot be released by release-plz while still at the `0.0.0`
stub floor.

## Scope

**In scope**

- `rs/crates/libs/paigasus-kernel/Cargo.toml` — metadata fields, `include`,
  a `[lints.rust]` override, `publish = true`, TODO removal.
- `rs/crates/libs/paigasus-kernel/src/lib.rs` — the crate-level rustdoc, which
  currently says "Empty until real logic lands" and becomes the docs.rs landing page.
- `rs/crates/libs/paigasus-kernel/README.md` (new) — required by `readme`.
- `rs/crates/libs/paigasus-kernel/LICENSE` (new) — a real copy of the root `LICENSE`.
- `rs/release-plz.toml` — a `release = false` block while the floor is `0.0.0`.
- `ci/publish-metadata/run.sh` (new) — the gate script.
- `moon.yml` — a `publish-metadata` task on the root `repo` project.
- `.github/workflows/ci.yml` — add `:publish-metadata` to the `moon ci` target list.
- `CLAUDE.md` — add `:publish-metadata` to the documented full-graph command.
- **Linear** — move SMA-376's version-floor and release-tooling ACs onto SMA-407
  (see D1/D8). This is a deliverable, not a courtesy note.

**Out of scope**

- **The version floor.** `version` stays `0.0.0`. The `0.0.0 → 0.1.0` bump across
  crates.io / PyPI / npm is SMA-407 item 1, and SMA-407 item 2 is the lockstep that
  makes those three carry *one* version. Bumping only the Rust crate here would skew
  it against `py/packages/paigasus-kernel` and `@paigasus/kernel` (both `0.0.0`) —
  the exact drift SMA-407 exists to prevent. Precedent: SMA-378, SMA-428.
- **Release tooling / live workflows** — SMA-407 item 3 (see D8).
- **The other eleven crates.** They stay `publish = false`. `paigasus-proto` has its
  own issue (SMA-388); bindings and services are not crates.io-bound.
- **Removing `sum()`** — see D5.

## Design

### Unit 1 — the crate manifest

```toml
[package]
name = "paigasus-kernel"
# 0.0.0 is the pre-release stub floor. SMA-407 moves every package to the 0.1.0
# floor together (crates.io/PyPI/npm in lockstep) and lets release-plz cut the
# first tag; publishing THIS version would burn the crates.io version namespace.
# `rs/release-plz.toml` therefore carries `release = false`, and
# `repo:publish-metadata` fails if that block is ever removed while a publishable
# crate is still at 0.0.0.
version = "0.0.0"
description = "Pure-logic behavioral kernel for Paigasus — resource names (PRN), UUIDv7 minting, and Cedar entity UIDs."
repository = "https://github.com/SMK1085/paigasus-core"
homepage = "https://github.com/SMK1085/paigasus-core#readme"
readme = "README.md"
keywords = ["paigasus", "kernel", "prn", "uuid7", "cedar"]
categories = ["data-structures", "parser-implementations"]
# ALLOWLIST, not a denylist: cargo's default include is "every non-ignored file in
# the package dir", which today ships the monorepo's `moon.yml` to crates.io
# consumers and would ship whatever the dir gains next (a pyproject.toml, a fixture
# tree). Enumerating what belongs in the artifact is the version that cannot leak.
include = ["src/**/*.rs", "tests/**/*.rs", "Cargo.toml", "README.md", "LICENSE"]
edition.workspace = true
license.workspace = true
rust-version.workspace = true
authors.workspace = true
publish = true

# NOT `workspace = true` — see D7. Cargo INLINES the resolved lint table into the
# published manifest, and docs.rs builds a published crate as the ROOT package on
# nightly, where `--cap-lints allow` does not apply. Inheriting the workspace's
# `warnings = "deny"` would let the first new rustc warning silently kill docs.rs
# builds of a released crate. CI strictness is unaffected: the Moon `lint` task
# passes `-D warnings` explicitly, which is already how clippy is handled.
[lints.rust]
warnings = "warn"

[lints.clippy]
all = "warn"
```

`license` stays inherited (`Apache-2.0`) — that SPDX expression is what crates.io
records; no `license-file` key is needed. `documentation` is omitted (docs.rs is
inferred). `rust-version` (1.95) is inherited and surfaces as the MSRV badge.

Category slugs validated against `GET /api/v1/categories`: `data-structures` and
`parser-implementations` (6476 crates, *"Parsers implemented for particular formats
or languages"*) — the latter replaces `parsing`, which is crates.io's category for
parser *tooling* (generators/combinators), not for a crate that parses a format.

`repository` and `homepage` are safe to publish: the GitHub repo is **public**
(`gh repo view` → `"visibility":"PUBLIC"`). The "this private repo needs auth"
comment at `.github/workflows/ci.yml:58-60` is stale and describes a fetch concern,
not visibility.

### `src/lib.rs` — rustdoc correction

The crate-level doc comment currently reads *"Empty until real logic lands."* That
string becomes the docs.rs landing page for a crate whose new `description`
advertises PRN, UUIDv7 and Cedar UIDs. Rewrite the `//!` block to describe the real
surface. Do **not** use `#![doc = include_str!("../README.md")]`: the README opens
with an H1 that would render as a duplicate title inside rustdoc, and the two
documents have different audiences.

### `README.md` (new, in the crate dir)

`readme` must name a file packaged *inside* the distribution — cargo hard-errors
otherwise (verified: ``readme `NOPE.md` does not appear to exist``). Plain Markdown,
no SPDX header (that convention covers source files, not docs — consistent with
`rs/README.md` and SMA-378's package READMEs):

```markdown
# paigasus-kernel

Pure-logic behavioral kernel for Paigasus — the cross-language primitives that must
behave identically everywhere: Paigasus Resource Names (`Prn`), UUIDv7 minting from
injected bytes, and Cedar entity UIDs.

No I/O, no FFI, no adapters. The Python, Node and browser bindings live in
[`rs/crates/bindings/`](https://github.com/SMK1085/paigasus-core/tree/main/rs/crates/bindings)
and call into this crate rather than reimplementing it (ADR-0005).

Licensed under the Apache License, Version 2.0.
```

### `LICENSE` (new, in the crate dir)

A real copy of the root `LICENSE` (Apache-2.0, 201 lines) — not a symlink, matching
SMA-378's rationale. It must be named in `include` (the allowlist replaces cargo's
default sweep).

### `rs/release-plz.toml` — the release block

```toml
[workspace]
# Publishing is BLOCKED while packages sit at the 0.0.0 stub floor: releasing 0.0.0
# would permanently burn that version on crates.io. SMA-407 removes this line as
# part of moving every package to the 0.1.0 floor. `repo:publish-metadata` asserts
# the pairing — the block cannot be removed while a publishable crate is at 0.0.0.
release = false
features_always_increment_minor = true
dependencies_update = true
```

Verified against the pinned release-plz 0.3.158: `[workspace] release = false` (and
the per-package `[[package]] name = "..." release = false` form) parse cleanly,
while an unknown key is rejected at config-load time — so acceptance is meaningful,
not silently ignored.

This does not disturb the SMA-398 parity harness: `ci/release-parity/ecosystems/release-plz.sh`
derives its fixture by grepping the single `features_always_increment_minor` line
and writes its own config, so an added key is inert there.

### Unit 2 — `ci/publish-metadata/run.sh`

Bash + `python3` (the repo has no `jq`; `ci/osv/run.sh` and `ci/affected-graph/run.sh`
already parse JSON this way; `tomllib` covers the release-plz config). `set -euo pipefail`.

**All cargo invocations run from `rs/`, not the repo root.** `rs/rust-toolchain.toml`
(`channel = "1.95.0"`) and `rs/.cargo/config.toml` are discovered by walking up from
**CWD**, not from `--manifest-path` — `rs/.cargo/config.toml:9-12` says so verbatim and
`rs/rust-toolchain.toml:1-7` records the E0514 incident (SMA-389) caused by exactly
this mismatch. Every other cargo-invoking gate does the same (`wasm-getrandom-free`
`cd rs` at `moon.yml:219`; `parity-corpus-drift` at :143; `observability-drift` at :162).
There is also no repo-root `Cargo.toml`, so a bare `cargo metadata` from the root
would simply fail.

**Check 0 — the publishable set is what we expect (non-vacuity control).**

```bash
EXPECTED_PUBLISHABLE=(paigasus-kernel)
```

`cargo metadata --format-version 1 --no-deps` reports `publish: null` for a
publishable package and `publish: []` for `publish = false`. Discovering the set at
runtime is convenient — but the set is governed by the very flag this gate protects:
revert the kernel to `publish = false` and every remaining check would iterate an
empty list and exit 0 green. That is the same shape as the traps this repo has
already paid for (`ci/osv/run.sh:59-76` "0 packages scanned", `moon.yml:299-302`
empty-`expected`, `ci/next-env/run.sh:67-72` "typegen emitted nothing").

So: assert the discovered set **equals** `EXPECTED_PUBLISHABLE` (strict equality,
the `ci/affected-graph/run.sh` shape). Empty → exit 2. Mismatch → exit 1 with
*"add the crate to EXPECTED_PUBLISHABLE, or you have just silently disabled this
gate"*. SMA-388 edits one line; that is the point.

**Check 1 — metadata crates.io will actually accept.**

For each publishable package, assert `description`, `license`, `repository`,
`readme`, `keywords`, `categories` are present and non-empty — and additionally
enforce crates.io's real upload-time validation rules, which are cheap:

| Rule | Limit |
|---|---|
| `keywords` | ≤ 5 entries; each ≤ 20 chars, `[A-Za-z0-9_-]` only, must start alphanumeric |
| `categories` | ≤ 5 entries |
| `description` | ≤ 1000 chars |

Presence alone would leave the gate claiming more than it delivers: the problem
statement's whole premise is *not being surprised at upload*. What remains
undiscoverable locally — name availability, ownership, the 10 MiB size cap — is
stated as such rather than implied away.

Workspace inheritance is already resolved at this layer (verified: with
`license.workspace = true`, `cargo metadata` reports `license: "Apache-2.0"` and
`rust_version: "1.95"`), so no special handling is needed. Absent fields come back
as `null` or `[]` — hence *non-empty*, not merely present.

Report **every** violation across **every** crate before exiting, so one CI run
gives the full picture.

This check is the one that earns the gate: `cargo publish --dry-run` only *warns*
about missing `description`/`repository` and exits 0 — verified against this crate.

**Check 2 — `cargo publish --dry-run -p <pkg> --locked`, per publishable crate.**

Proves the crate is publishable *at all* (it refuses under `publish = false`, which
makes it a second, independent non-vacuity control), packages, and compiles
standalone from the packaged copy with no unversioned path dependency.
`paigasus-kernel` qualifies because it is a leaf: only `uuid` + `thiserror` (plus
dev-only `proptest`), all crates.io-resolvable.

`--locked` so the verify build resolves against the packaged lockfile rather than
whatever the registry serves that minute.

*On `--allow-dirty`:* passed **only** when the package directory is actually dirty,
and **never** when `CI` is set. The earlier rationale ("a dry run uploads nothing,
so the flag surrenders no safety") was wrong about what the flag does: it changes
*what gets packaged*. Cargo enumerates via git, so with `--allow-dirty` an untracked
`README.md`/`LICENSE` is packaged and `.cargo_vcs_info.json` is stamped
`"dirty": true`. The gate would then pass on files that were never `git add`ed. So:
locally, apply the flag conditionally (with a printed warning) so a developer can
run the gate on uncommitted work; in CI, refuse it, so the assertion is about a
committed tree.

**Check 2b — the packaged file list.**

From the same run: `cargo package --list` must contain `README.md` and `LICENSE`,
and must **not** contain `moon.yml`. D4 argues against hand-verification; leaving
this as a manual step would have been the spec contradicting itself. It is also
what makes `include` self-defending.

**Check 3 — the `0.0.0` release block.**

If any publishable crate is at `0.0.0`, assert `rs/release-plz.toml` blocks its
release: either `[workspace] release = false`, or a `[[package]]` entry naming that
crate with `release = false`. Parsed with `tomllib`, not grepped.

The rejected alternative was grepping `.github/workflows/` for a live publish
command. It fails on every realistic activation path: release-plz's canonical wiring
is `uses: release-plz/action@v0` with `with: command: release` (no
`release-plz release` substring), and `cargo release`, a composite action, a
reusable workflow, or `run: ./scripts/release.sh` all evade it — while
`run: |` continuations and YAML comments produce false positives. A heuristic that
the *most probable* future change walks straight past is worse than none, because it
will be trusted.

The structural check is tool-honored: release-plz reads this key regardless of how
it is invoked. **Stated limitation:** it does not stop a human running
`cargo publish` by hand, and it is crates.io-only — `@paigasus/kernel` (`private: true`)
and the PyPI packages carry the identical `0.0.0` hazard with their own separate
guards (`prebuild.yml:145` already runs `napi prepublish --dry-run`). The invariant
this gate enforces is *"release-plz cannot release a `0.0.0` crate"*, not *"nothing
anywhere can publish `0.0.0"*.

**Failure-mode classification.** Assertion failures exit 1. Infrastructure failures
— `cargo metadata` erroring, an unreadable `release-plz.toml`, a registry/network
error from the dry run — exit 2 with a distinct message, so a broken invocation can
never read as "all checks passed" (the `repo:wasm-getrandom-free` posture).

**`--negative-control` mode**, matching `ci/affected-graph/run.sh` and
`ci/release-parity/run.sh`: mutate a temp copy of the manifest and assert each check
reports red. This is what keeps the gate from rotting into a vacuous pass.

### Moon task

```yaml
  publish-metadata:
    description: 'Assert every publishable crate carries crates.io-valid metadata, packages exactly the intended files, and cannot be released by release-plz while still at the 0.0.0 stub floor (SMA-376).'
    script: 'bash ci/publish-metadata/run.sh'
    toolchain: 'system'
    inputs:
      - 'ci/publish-metadata/run.sh'
      - 'rs/Cargo.toml'
      - 'rs/Cargo.lock'
      - 'rs/crates/**/*'
      - 'rs/rust-toolchain.toml'
      - 'rs/.cargo/config.toml'
      - 'rs/release-plz.toml'
      - '.gitignore'
```

On the root `repo` project, `toolchain: 'system'` — the `repo:wasm-getrandom-free`
shape, which also shells out to cargo.

**`inputs` are deliberately broad** (`rs/crates/**/*`): the publishable set is
discovered at runtime, so per-crate globs would go stale the day a new crate flips
`publish = true`, and Moon would serve a cached pass over sources it never tracked.
A vacuous gate is worse than a slow one. Beyond the sources, the list now includes
every input that *determines the answer* — the toolchain pin and cargo config (they
drive the verify build), `release-plz.toml` (Check 3), and `.gitignore` (it drives
cargo's file enumeration). Omitting those is the precise failure
`ci/next-env/run.sh:11-16` documents for the missing `ts/pnpm-lock.yaml`.

`.github/workflows/**/*` is **not** an input — with Check 3 restructured around
`release-plz.toml`, workflow edits no longer affect the result, and keeping them
would have re-keyed a cold cargo build on every unrelated CI tweak.

**Cost is not yet measured.** The `~4s` figure in the first draft was a warm local
`cargo publish --dry-run`; the real cost in CI is a *cold* verify build in
`rs/target/package/<crate>-<ver>/target`, which shares nothing with the workspace
`rs/target` cache, plus a crates.io index fetch. Measure it on the implementation PR
and record the number. If it proves material against the job's 30-minute budget and
the documented disk pressure (`ci.yml:23-43`), the mitigation is to split Check 3
into its own `publish-version-floor` task (a ~50 ms TOML read) and narrow this task's
inputs to `rs/**/Cargo.toml` plus per-publishable-crate globs, with a script-side
assertion that the declared globs cover the discovered set.

### CI and documentation wiring

- `.github/workflows/ci.yml:184` — append `:publish-metadata` to the `T=(...)` array.
  Without this the task exists but never runs.
- `CLAUDE.md` — add `:publish-metadata` to the documented full-graph `moon ci`
  command in *Gotchas*.
- **`:affected-smoke` is unaffected.** `moon.yml` and `.github/workflows/ci.yml` are
  in that gate's `inputs` (`moon.yml:120-125`), but `repo` is filtered out of every
  expected set (`ci/affected-graph/run.sh:25-30`), so adding a `repo` task changes no
  expectation. Stated so the next reader need not re-derive it.

## Key decisions & rationale

**D1 — Version stays `0.0.0`.** SMA-376's "choose a real `0.x` version" overlaps
SMA-407 item 1, which owns the floor bump *across all three ecosystems* plus the
lockstep. Moving only the Rust crate breaks that lockstep before it exists.
Precedent: SMA-378, SMA-428.

**D2 — Why flip `publish = true` now.** The flip is what the issue asks for and what
makes the open-core edge real; it is *not* a prerequisite for packaging verification
(`cargo package` works fine under `publish = false` — the first draft's claim that
the flip "unlocks Check 2" was wrong). What the flip does buy the gate is that
`cargo publish --dry-run` asserts **publishability**, not merely packageability — so
Check 2 doubles as an independent control against someone quietly reverting the flag.
Safety today rests on Check 3 and the `release = false` block, not on the flag.

**D3 — Full metadata, not the minimal set.** Mirrors `@paigasus/kernel` on the npm
side. A crate on crates.io with no keywords and no category listing is unsearchable;
the marginal cost is two array literals.

**D4 — A permanent gate, not one-off verification.** SMA-378 verified by hand and
added no gate, so nothing stops its metadata from being emptied. Applied
consistently here: the packaged-file-list assertion (Check 2b) is automated for the
same reason, and Check 0 plus `--negative-control` exist so the gate cannot pass
vacuously.

**D5 — `sum()` stays in the public API.** Documented as a placeholder, but all three
bindings call it (`paigasus-node-bindings`, `paigasus-wasm`, `paigasus-py-bindings`)
along with their committed napi/wasm glue and the parity corpus. Removing it is a
multi-crate change with FFI-glue churn, outside a metadata issue. Forward-flagged:
SMA-407 should decide whether `sum` belongs in the first *published* surface, since
that is the point of no return.

**D6 — `include` allowlist, not `exclude` denylist.** An `exclude` list ships
whatever the crate dir gains next; the binding crates already carry
`pyproject.toml`/`package.json` alongside their Rust sources. The allowlist keeps
today's contents minus `moon.yml`, including the three proptest files (kept
deliberately — they let a vendoring consumer run the property suite).

**D7 — Override `[lints.rust]` to `warn` in the published crate.** Verified on disk:
cargo inlines the resolved lint table into the packaged manifest
(`rs/target/package/paigasus-kernel-0.0.0/Cargo.toml` carries
`[lints.rust] warnings = "deny"`). Registry consumers are shielded by
`--cap-lints allow`; **docs.rs is not**, because it builds the crate as the root
package, on nightly — exactly where new rustc warnings surface first. The workspace
chose `deny` for an unpublished workspace; publishing changes that calculus. CI
strictness is preserved by the Moon `lint` task's explicit `-D warnings`, which is
already the documented arrangement for clippy (`rs/Cargo.toml:200-201`).

**D8 — Two of SMA-376's four bullets move to SMA-407, as a tracked deliverable.**
The version bullet is D1. The "set up release tooling (release-plz) / crates.io
publishing" bullet is SMA-407 item 3 verbatim — SMA-407 already owns it, and doing
it here would mean a Low-priority metadata issue landing the riskiest release step.
The mitigation is not a note-to-self: updating the Linear ACs is in the Definition of
Done, so the issue cannot close with two ACs silently unmet.

**D9 — No `PUBLISH_METADATA_SKIP_VERIFY` escape hatch.** Considered, because Check 2
is network-dependent inside a required check on a strict-up-to-date-branches repo.
Rejected as inconsistent: `:deny` and `:osv` are equally network-dependent and ship
without one, so this gate would not be the thing that blocks a merge during a
registry outage. Revisit for all three together if it ever bites.

## Verification

Run from the repo root with the proto shims on `PATH`.

1. **Gate passes:** `moon run repo:publish-metadata` exits 0.
2. **Negative control passes:** `bash ci/publish-metadata/run.sh --negative-control`
   reports every check red on a mutated temp manifest.
3. **Deliberate-break checks** — each must fail with the stated exit code, each
   reverted afterwards:
   - blank `description` → Check 1, exit 1, naming `paigasus-kernel: description`;
   - a sixth keyword, or a 21-char keyword → Check 1, exit 1;
   - `readme = "NOPE.md"` → Check 2, exit 1. Verified empirically that cargo
     hard-errors here (``readme `NOPE.md` does not appear to exist``), so Check 1
     needs no file-existence assertion of its own;
   - revert the kernel to `publish = false` → **Check 0**, exit 1 (this is the
     vacuity control; without Check 0 the whole gate would go green);
   - drop `release = false` from `rs/release-plz.toml` → Check 3, exit 1 naming
     `paigasus-kernel` and `0.0.0`;
   - remove `LICENSE` from `include` → Check 2b, exit 1.
4. **Toolchain provenance:** the verify build uses the pinned 1.95.0 — confirm the
   script `cd`s into `rs/` and that `cargo --version` inside the script reports the
   pinned toolchain, not the host default.
5. **Cost measurement:** record the cold CI wall-clock of `repo:publish-metadata` on
   the implementation PR and write it into this spec.
6. **No regression in the wider graph** — the full CI list, since this touches a
   workspace manifest, `moon.yml`, and `release-plz.toml`:
   `moon ci :build :test :lint :fmt :deny :osv :machete :typecheck :breaking
   :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free
   :redis-connect-single-site :promtool :observability-drift :nats-permissions
   :release-parity :release-parity-py :release-parity-ts :publish-metadata
   --base origin/main --include-relations`. `:release-parity` in particular must stay
   green across the `release-plz.toml` edit.

## Forward notes for SMA-407

- **Removing `release = false` from `rs/release-plz.toml` is gated:** Check 3 reds
  until every publishable crate is off `0.0.0`. That enforces SMA-407's own stated
  ordering (floor first, workflow last) rather than trusting it.
- **Check 0 will red when SMA-388 flips `paigasus-proto`** until it is added to
  `EXPECTED_PUBLISHABLE` — and Check 1 will then demand the same metadata fields of
  it. Worth a cross-note on SMA-388.
- **`sum()`** — see D5. Decide before the first real publish.
- **The `[workspace.dependencies]` pin** `paigasus-kernel = { path = ..., version =
  "0.0.0" }` must move in the same commit as the floor bump (`^0.0.0` matches only
  `0.0.0`), or resolution breaks.
- **Reconsider `[lints.rust] warnings = "warn"`** (D7) if SMA-407 adds
  `[package.metadata.docs.rs]` handling or a docs.rs build check.
- **crates.io credential custody is unaddressed here.** SMA-407 must decide between
  crates.io Trusted Publishing (OIDC, no long-lived secret) and a
  `CARGO_REGISTRY_TOKEN` repository secret, and who owns the crate.

## Open question for the issue owner (not decided here)

The name `paigasus-kernel` is **unregistered** on crates.io, and this spec publicly
documents the intent to take it. D1 treats publishing `0.0.0` as burning the version
namespace — the concrete cost is one permanently-recorded, unyankable `0.0.0`
release. Weighed against losing the name to a squatter, deliberately publishing a
placeholder to reserve it is the standard move, and it would directly contradict D1
and Check 3. This spec assumes name-squatting risk is negligible for this name; if
that assumption is wrong, say so before implementation, because it inverts the design.

## Considered and not done

- **Bumping to `0.1.0`** — D1.
- **Metadata + `publish = true` alone (~20 lines), leaving the gate to SMA-407**,
  where `cargo publish --dry-run` would be a natural pre-publish step in the release
  workflow rather than a standing cost on every Rust PR. Rejected: it leaves the
  metadata unprotected in exactly the way D4 criticizes, and SMA-376 is the change
  that arms the `0.0.0` hazard, so it should be the change that contains it.
- **Grepping `.github/workflows/` for a live publish command** — rejected; see
  Check 3.
- **Hoisting `repository`/`homepage` into `[workspace.package]`** — only 2 of 6
  fields are hoistable. Revisit when a second crate becomes publishable.
- **A `dry-run`-only gate** — proven to miss missing metadata entirely.
- **Unconditional `--allow-dirty`** — rejected; see Check 2.
- **Narrow per-crate `inputs`** — rejected now, with a documented trigger to
  reconsider if the measured cost is material.
- **`#![doc = include_str!("../README.md")]`** — duplicate H1 in rustdoc.
- **A `documentation` field** — docs.rs is inferred.

## Definition of done

- `paigasus-kernel/Cargo.toml` carries `description`, `repository`, `homepage`,
  `readme`, `keywords`, `categories`, `include`, `[lints.rust] warnings = "warn"`,
  and `publish = true`; the `TODO(SMA-376)` block is gone, replaced by the
  `0.0.0`/SMA-407 note.
- The crate-level rustdoc in `src/lib.rs` no longer says "Empty until real logic lands".
- `README.md` and `LICENSE` exist in the crate dir; the packaged `.crate` contains
  both and excludes `moon.yml`.
- `rs/release-plz.toml` carries `release = false` with the explanatory comment.
- `ci/publish-metadata/run.sh` exists with an SPDX header, runs cargo from `rs/`,
  implements Checks 0–3 plus `--negative-control`, and separates assertion failures
  (exit 1) from infrastructure failures (exit 2).
- `repo:publish-metadata` is defined in `moon.yml` with the full `inputs` list,
  listed in `.github/workflows/ci.yml`, and documented in `CLAUDE.md`.
- Every deliberate-break check and the negative control behave as specified.
- The measured cold CI cost is recorded in this spec.
- SMA-376's version-floor and release-tooling ACs are moved onto SMA-407 in Linear.
- The full `moon ci ...` graph is green.
