# SMA-376 — crates.io publishing metadata for `paigasus-kernel`

**Date:** 2026-08-16
**Linear:** SMA-376 (blocks SMA-407; relates to SMA-357, SMA-428)
**Status:** Approved (design)

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
   and exits 0. crates.io rejects incomplete metadata *at upload time* — which is
   the worst possible moment to discover it.
2. **`publish = false` makes the publish path unverifiable.** `cargo publish
   --dry-run -p paigasus-kernel` refuses outright:
   ``error: `paigasus-kernel` cannot be published. `package.publish` must be set to
   `true` or a non-empty list``. So there is no way to prove the crate would
   package and build standalone.

Flipping the flag also *arms* a hazard the issue names: a publishable crate at
`0.0.0` will publish `0.0.0` and burn the version namespace the moment a release
workflow exists.

## Outcome

`paigasus-kernel` carries complete crates.io metadata, `publish = true`, and no
`TODO(SMA-376)`. A new CI gate proves — on every Rust change — that every
publishable crate has valid metadata, packages cleanly, and cannot be published
while still at the `0.0.0` stub floor.

## Scope

**In scope**

- `rs/crates/libs/paigasus-kernel/Cargo.toml` — metadata fields, `exclude`,
  `publish = true`, TODO removal.
- `rs/crates/libs/paigasus-kernel/README.md` (new) — required by `readme`.
- `rs/crates/libs/paigasus-kernel/LICENSE` (new) — a real copy of the root
  `LICENSE`.
- `ci/publish-metadata/run.sh` (new) — the gate script.
- `moon.yml` — a `publish-metadata` task on the root `repo` project.
- `.github/workflows/ci.yml` — add `:publish-metadata` to the `moon ci` target list.
- `CLAUDE.md` — add `:publish-metadata` to the documented full-graph command.

**Out of scope**

- **The version floor.** `version` stays `0.0.0`. The `0.0.0 → 0.1.0` bump across
  crates.io / PyPI / npm is SMA-407 item 1, and SMA-407 item 2 is the lockstep that
  makes those three carry *one* version. Bumping only the Rust crate here would
  skew it against `py/packages/paigasus-kernel` and `@paigasus/kernel` (both `0.0.0`)
  — creating the exact drift SMA-407 exists to prevent. This mirrors SMA-378
  ("versions stay `0.0.0`") and SMA-428.
- **Release tooling / live workflows.** `rs/release-plz.toml` is deliberately
  dormant (SMA-398); no release-plz workflow exists. Adding one is SMA-407 item 3.
  SMA-376's "set up release tooling" bullet is therefore re-scoped to SMA-407,
  which already owns it verbatim. Note this on the Linear issue when closing.
- **The other eleven crates.** They stay `publish = false`. `paigasus-proto` has
  its own issue (SMA-388); the bindings and services are not crates.io-bound.
- **Removing `sum()`** — see *Decisions* D5.

## Design

### Unit 1 — the crate manifest

```toml
[package]
name = "paigasus-kernel"
# 0.0.0 is the pre-release stub floor. SMA-407 moves every package to the 0.1.0
# floor together (crates.io/PyPI/npm in lockstep) and lets release-plz cut the
# first tag; publishing THIS version would burn the crates.io version namespace.
# `repo:publish-metadata` enforces that: it fails if a live publish path appears
# in .github/workflows/ while a publishable crate is still at 0.0.0.
version = "0.0.0"
description = "Pure-logic behavioral kernel for Paigasus — resource names (PRN), UUIDv7 minting, and Cedar entity UIDs."
repository = "https://github.com/SMK1085/paigasus-core"
homepage = "https://github.com/SMK1085/paigasus-core#readme"
readme = "README.md"
keywords = ["paigasus", "kernel", "prn", "uuid7", "cedar"]
categories = ["data-structures", "parsing"]
# moon.yml is monorepo build-system config; it has no meaning to a crates.io
# consumer and would otherwise ship inside the .crate (cargo's default include is
# "every non-ignored file in the package dir").
exclude = ["moon.yml"]
edition.workspace = true
license.workspace = true
rust-version.workspace = true
authors.workspace = true
publish = true
```

`license` stays inherited (`Apache-2.0` from `[workspace.package]`) — that SPDX
expression is what crates.io records; no `license-file` key is needed. `documentation`
is omitted: docs.rs is inferred for crates.io crates, and hard-coding it would go
stale. `rust-version` (1.95) is inherited and surfaces as the MSRV badge.

Both category slugs were validated against `GET /api/v1/categories`:
`data-structures` and `parsing` exist. `keywords` is five entries, crates.io's cap.

### `README.md` (new, in the crate dir)

`readme` must name a file packaged *inside* the distribution; nothing outside the
crate dir is included. Plain Markdown, no SPDX header (the SPDX convention applies
to source files, not docs — consistent with `rs/README.md` and SMA-378's package
READMEs). It renders as the crates.io landing page:

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
SMA-378's rationale (symlinks interact badly with archive packaging). Unlike the
Python side, **no manifest key is needed**: cargo's default include already sweeps
every non-ignored file in the package dir, so the copy alone ships it. The text is
the standard Apache-2.0 boilerplate and effectively never changes, so the
duplication carries no sync burden.

### Unit 2 — `ci/publish-metadata/run.sh`

Bash + `python3` (the repo has no `jq`; `ci/osv/run.sh` and `ci/affected-graph/run.sh`
already parse JSON this way). Runs from the repo root. Three checks, in order:

**Check 1 — required fields on every publishable crate.**

`cargo metadata --format-version 1 --no-deps` reports `publish: null` for a
publishable package and `publish: []` for `publish = false` — a clean, exact
discriminator that needs no crate allowlist. For each publishable package, assert
`description`, `license`, `repository`, `readme`, `keywords`, `categories` are
present and non-empty; report *every* missing field across *every* crate before
exiting non-zero, so one CI run gives the full picture.

Workspace inheritance is already resolved at this layer — verified: with
`license.workspace = true`, `cargo metadata` reports `license: "Apache-2.0"` (and
`rust_version: "1.95"`), so the assertion needs no special handling for inherited
keys. Absent fields come back as `null` (`description`, `readme`) or `[]`
(`keywords`, `categories`), which is why the check tests for *non-empty*, not
merely present.

This check is the one that earns the gate. `cargo publish --dry-run` **only warns**
about missing `description`/`repository` and exits 0 — verified empirically against
this very crate. Without an explicit assertion, the exact gap SMA-376 fixes could
silently regress.

It is also self-extending: when SMA-388 flips `paigasus-proto` to `publish = true`,
the gate covers it with no edit to this script.

**Check 2 — `cargo publish --dry-run -p <pkg> --allow-dirty`, per publishable crate.**

Proves the crate packages, compiles standalone from the packaged copy, and carries
no unversioned path dependency. `paigasus-kernel` qualifies today precisely because
it is a leaf: its only deps are `uuid` and `thiserror` (plus dev-only `proptest`),
all crates.io-resolvable.

`--allow-dirty` is deliberate. Cargo refuses a dry run when the *package directory*
has uncommitted changes; without the flag the gate cannot run on the working tree it
is meant to verify — the developer would have to commit first to find out the commit
is wrong. A dry run uploads nothing, so the flag surrenders no safety. (Scope
confirmed empirically: cargo's dirty check lists only files under the package dir,
not the whole repo.)

**Check 3 — the `0.0.0` tripwire.**

```bash
# POSIX ERE has no negative lookahead — grep for publish commands, then filter out
# the dry runs.
hits="$(grep -rnE 'cargo publish|release-plz release' .github/workflows/ || true)"
live="$(printf '%s\n' "$hits" | grep -v -- '--dry-run' || true)"
```

If `live` is non-empty *and* any publishable crate is at `0.0.0`, fail with a
message naming the crate and pointing at SMA-407. Verified inert on the current
tree: that grep matches nothing today (exit 1), so the check is skipped. It fires
the instant SMA-407 adds a release job, and self-disarms once the floor moves to
`0.1.0`.

This converts SMA-407's *ordering assumption* (bump the floor as item 1, wire the
workflow as item 3) into an enforced invariant, which is the point: SMA-376 is the
change that arms the hazard, so SMA-376 should be the change that contains it.

**Failure modes are loud.** If `cargo metadata` itself fails, exit 2 with a distinct
message — a broken invocation must never read as "all fields present" (the same
posture as `repo:wasm-getrandom-free`).

### Moon task

```yaml
  publish-metadata:
    description: 'Assert every publishable crate carries complete crates.io metadata, packages cleanly (`cargo publish --dry-run`), and is not still at the 0.0.0 stub floor once a live publish path exists (SMA-376).'
    script: 'bash ci/publish-metadata/run.sh'
    toolchain: 'system'
    inputs:
      - 'ci/publish-metadata/run.sh'
      - 'rs/Cargo.toml'
      - 'rs/Cargo.lock'
      - 'rs/crates/**/*'
      - '.github/workflows/**/*'
```

On the root `repo` project, `toolchain: 'system'` — same shape as
`repo:wasm-getrandom-free`, which also shells out to cargo.

**`inputs` are deliberately broad.** `rs/crates/**/*` rather than per-crate globs:
the set of publishable crates is discovered at *runtime* from `cargo metadata`, so
narrow inputs would go stale the day a new crate flips `publish = true` — and Moon
would serve a cached pass over sources it never tracked. A vacuous gate is worse
than a slow one. Cost is one extra ~4s task on Rust-touching PRs (a verify build of
the packaged kernel plus a crates.io index fetch).

### CI and documentation wiring

- `.github/workflows/ci.yml:184` — append `:publish-metadata` to the `T=(...)`
  target array. Without this the task exists but never runs in CI.
- `CLAUDE.md` — add `:publish-metadata` to the documented full-graph
  `moon ci ...` command in *Gotchas*, so the pre-push instruction stays complete.

## Key decisions & rationale

**D1 — Version stays `0.0.0`.** SMA-376's "choose a real `0.x` version" bullet
overlaps SMA-407 item 1, which owns the floor bump *across all three ecosystems*
plus the lockstep wiring. Moving only the Rust crate would break that lockstep
before it exists. Precedent: SMA-378 and SMA-428 both landed metadata and explicitly
deferred the version.

**D2 — `publish = true` at `0.0.0` is safe today, and guarded for tomorrow.** No
workflow can publish anything (`rs/release-plz.toml` is dormant; no release job
exists). Check 3 keeps it that way. The flip is also what *unlocks* Check 2 — with
`publish = false` the dry run refuses to run at all.

**D3 — Full metadata, not the minimal set.** Mirrors what `@paigasus/kernel` already
declares on the npm side (description, keywords, homepage, repository). A crate that
lands on crates.io with no keywords and no category listing is unsearchable; the
marginal cost here is two array literals.

**D4 — A permanent gate, not one-off verification.** SMA-378 verified by hand and
added no gate, so nothing stops its metadata from being emptied. Here the gate is
cheap, self-extending to future publishable crates, and — via Check 2 — the only
standing proof that the open-core dependency edge actually works.

**D5 — `sum()` stays in the public API.** It is documented as a placeholder, but all
three bindings call it (`paigasus-node-bindings`, `paigasus-wasm`,
`paigasus-py-bindings`), along with their committed napi/wasm glue and the parity
corpus. Removing it is a multi-crate change with FFI-glue churn, well outside a
metadata issue. Flagged forward: SMA-407 should decide whether `sum` belongs in the
first *published* surface, since that is the point of no return.

**D6 — `exclude = ["moon.yml"]`.** Confirmed present in today's packaged file list.
It is monorepo build config with no meaning to a consumer.

## Verification

Run from the repo root with the proto shims on `PATH`.

1. **Gate passes:** `moon run repo:publish-metadata` exits 0.
2. **Deliberate-break checks** — each must fail, and each is reverted afterwards:
   - blank `description` in the kernel manifest → Check 1 fails naming
     `paigasus-kernel: description`;
   - remove `keywords` → Check 1 fails;
   - point `readme` at a nonexistent file → Check 2 fails. Verified empirically:
     cargo hard-errors with ``readme `NOPE.md` does not appear to exist (relative
     to ...)``, so Check 1 needs no file-existence assertion of its own;
   - append a line containing `cargo publish` (no `--dry-run`) to a workflow →
     Check 3 fails naming `paigasus-kernel` and `0.0.0`.
3. **Packaged contents:** `cargo package -p paigasus-kernel --allow-dirty --list`
   lists `README.md` and `LICENSE`, and **does not** list `moon.yml`.
4. **Metadata renders:** `cargo publish --dry-run -p paigasus-kernel --allow-dirty`
   completes with no `manifest has no description...` warning.
5. **No regression in the wider graph** — the full CI list, since this touches a
   workspace manifest and `moon.yml`:
   `moon ci :build :test :lint :fmt :deny :osv :machete :typecheck :breaking
   :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free
   :redis-connect-single-site :promtool :observability-drift :nats-permissions
   :release-parity :release-parity-py :release-parity-ts :publish-metadata
   --base origin/main --include-relations`.

## Forward notes for SMA-407

- **Check 3 will go red** the moment the release workflow lands, until every
  publishable crate is off `0.0.0`. That is intended, and it enforces SMA-407's own
  stated ordering (floor first, workflow last).
- **Check 1 will start covering `paigasus-proto`** automatically once SMA-388 flips
  it — so SMA-388 must add the same metadata fields, or the gate reds. Worth a
  cross-note on SMA-388.
- **`sum()`** — see D5. Decide before the first real publish.
- The `[workspace.dependencies]` pin `paigasus-kernel = { path = ..., version =
  "0.0.0" }` must move in the same commit as the floor bump, or resolution breaks.

## Considered and not done

- **Bumping to `0.1.0`** — D1.
- **Hoisting `repository`/`homepage` into `[workspace.package]`** — only 2 of 6
  fields are hoistable (description/keywords/categories are inherently per-crate),
  and it touches the shared table for a single-crate change. Revisit when a second
  crate becomes publishable.
- **A `dry-run`-only gate** — rejected: proven to miss missing metadata entirely.
- **`cargo publish --dry-run` without `--allow-dirty`** — rejected: unusable
  locally on uncommitted work, which is when the gate has the most value.
- **Narrow per-crate `inputs`** — rejected: risks a vacuous cached pass.
- **A `documentation` field** — docs.rs is inferred.

## Definition of done

- `paigasus-kernel/Cargo.toml` carries `description`, `repository`, `homepage`,
  `readme`, `keywords`, `categories`, `exclude`, and `publish = true`; the
  `TODO(SMA-376)` block is gone, replaced by the `0.0.0`/SMA-407 note.
- `README.md` and `LICENSE` exist in the crate dir; the packaged `.crate` contains
  both and excludes `moon.yml`.
- `ci/publish-metadata/run.sh` exists with an SPDX header and implements all three
  checks, failing loudly (exit 2) if `cargo metadata` itself errors.
- `repo:publish-metadata` is defined in `moon.yml`, listed in `.github/workflows/ci.yml`,
  and documented in `CLAUDE.md`.
- All four deliberate-break checks fail as specified and the gate passes on the
  restored tree.
- The full `moon ci ...` graph is green.
