# SMA-577 — Making the proto family publishable

**Status:** Approved (brainstorming 2026-08-23).
**Date:** 2026-08-23
**Linear:** [SMA-577](https://linear.app/smaschek/issue/SMA-577/release-activation-b-make-the-proto-family-publishable-absorbs-sma-388) — child of [SMA-407](https://linear.app/smaschek/issue/SMA-407). **Absorbs [SMA-388](https://linear.app/smaschek/issue/SMA-388).**
**Branch:** `feature/sma-577-release-activation-b-make-the-proto-family-publishable`
**Targets:** `main` (currently `e2007d5`).
**Parent design:** `docs/superpowers/specs/2026-08-22-sma-407-release-activation-design.md` — this document implements its **§6** in full, plus the derive→proto publish ordering from **§3** and the proto half of the version model in **§4**. It answers that document's open questions **Q6** and **Q7**.
**References:** ADR-0011 (S1 lockstep, S3 `0.1.0` floor, S4 dormant-until-real, S5 file-path attribution); ADR-0005; SMA-376 (`repo:publish-metadata`); SMA-529 (Check 1b); SMA-530 / SMA-542 (guard-the-guard); SMA-576 (the kernel-family precedent this mirrors); SMA-438 (`#[derive(Auditable)]`).

---

## 1. Problem

`paigasus-proto` and `paigasus-proto-derive` are both `publish = false` at the `0.0.0` stub floor
with no crates.io metadata whatsoever. ADR-0011 S4 gates activation on a package having a real
public API; the proto family cleared that bar when the generated code was committed in all three
languages, and SMA-388 has been open ever since to "flip `publish = false`".

It is not a flip. Measured against `ci/publish-metadata/run.sh`, both crates fail four of its
checks, and the gate's own structure cannot express the publish order the two crates require.

## 2. The publishable set is three, not four

Parent design §3 states "the publishable set is **four** crates, not two" and then diagrams three:

```
paigasus-proto-derive ──▶ paigasus-proto      (proto family, ordered)
paigasus-kernel                                (kernel family, independent)
```

Three is correct. The three binding crates (`paigasus-py-bindings`, `paigasus-node-bindings`,
`paigasus-wasm`) are Cargo `publish = false` — they ship as maturin/napi/wasm byproducts, not to
crates.io — so they are outside `EXPECTED_PUBLISHABLE` by construction. They are in the *kernel
version group*, which is what §4's site inventory counts, and that is where the miscount comes
from.

**Correction, recorded:** `EXPECTED_PUBLISHABLE` becomes exactly

```bash
EXPECTED_PUBLISHABLE=("paigasus-kernel" "paigasus-proto" "paigasus-proto-derive")
```

Check 0 compares this set to the runtime-discovered one with **strict equality**, so both new
crates must land in the same commit that flips their `publish` flags.

## 3. The chicken-and-egg — measured, and resolved

Parent design §3 flagged this as the reason "the PR implementing this would red on its own gate",
and §14 Q6 left open how the first dry-run could behave. Three measurements against the pinned
toolchain (`rust-toolchain.toml` channel `1.95.0`, `cargo 1.95.0 (f2d3ce0bd 2026-03-21)`), run in
the SMA-577 worktree with both crates temporarily flipped to `publish = true`:

| # | Invocation | Exit | Result |
| --- | --- | --- | --- |
| M1 | `cargo publish --dry-run --locked -p paigasus-proto` | **101** | `no matching package named 'paigasus-proto-derive' found` / `location searched: crates.io index` |
| M2 | `cargo publish --dry-run --locked -p paigasus-proto-derive -p paigasus-proto` | **0** | compiles `paigasus-proto` from `target/package/paigasus-proto-0.0.0`, then `Uploading` both in dependency order |
| M3 | as M2, but with `include = ["Cargo.toml"]` on the derive crate | **101** | `warning: ignoring library 'paigasus_proto_derive' as 'src/lib.rs' is not included in the published package` → `no targets specified in the manifest` |

M1 confirms the hazard is real. M2 resolves it: cargo's workspace-publish support (multiple `-p`
flags) computes the topological order itself and stages the to-be-published packages so the
downstream can resolve its upstream locally.

**M3 is the load-bearing measurement.** It establishes that the combined run consumes the
upstream's **packaged tarball**, not its workspace source — a deliberately broken `include` on
`paigasus-proto-derive` fails the run. Without M3, a combined dry-run could plausibly have been a
workspace shortcut that silently stopped proving anything about the artifact. It is not; it is
registry-faithful.

### The deviation this leaves, stated rather than hidden

release-plz's real publish is **sequential per package**, waiting for each crate to appear in the
crates.io index before publishing its dependent. The combined dry-run is therefore a *proxy* for
that sequence, not a replay of it. What it does prove — each crate packages, each packaged crate
compiles, and the dependent compiles against the dependency's packaged form with a resolvable
version requirement — is the entire failure surface that the sequential publish could hit. What it
cannot prove is index-propagation timing, which no local check can reach. Recorded here so the
proxy is not later mistaken for a replay.

## 4. Manifest changes

Both crates mirror `paigasus-kernel/Cargo.toml`, which is the reference implementation for a
publishable crate in this repo.

| Field | `paigasus-proto` | `paigasus-proto-derive` |
| --- | --- | --- |
| `version` | `0.1.0` | `0.1.0` |
| `description` | Generated protobuf message types and tonic gRPC service stubs for Paigasus. | Derive macro for Paigasus audit metadata — `#[derive(Auditable)]` for generated protobuf messages. |
| `repository` | `https://github.com/SMK1085/paigasus-core` | same |
| `homepage` | `…#readme` | same |
| `readme` | `README.md` | `README.md` |
| `keywords` | `paigasus`, `protobuf`, `grpc`, `tonic`, `prost` | `paigasus`, `protobuf`, `derive`, `macro`, `audit` |
| `categories` | `network-programming`, `encoding` | `development-tools::procedural-macro-helpers`, `development-tools` |
| `include` | `src/**/*.rs`, `tests/**/*.rs`, `Cargo.toml`, `README.md`, `LICENSE` | `src/**/*.rs`, `Cargo.toml`, `README.md`, `LICENSE` |
| `[lints.rust] warnings` | `"warn"` — per-crate, **not** inherited | same |
| `[lints.clippy] all` | `"warn"` | same |
| `publish` | `true` | `true` |

All four category slugs were verified present in the committed
`ci/publish-metadata/crates-io-categories.txt` snapshot. Every keyword is ≤ 20 chars, starts
alphanumeric, and matches `[A-Za-z0-9_-]+` — Check 1's constraints.

Both crates gain a `README.md` and a `LICENSE` that is a byte-identical copy of the repo-root
Apache-2.0 text, exactly as `paigasus-kernel` does it. Cargo has no mechanism to package a file
from outside the crate directory, so the copy is required rather than preferred.

The `include` list is an **allowlist**, not a denylist, for the reason recorded on the kernel: cargo's
default is "every non-ignored file in the package dir", which today sweeps `moon.yml` into both
tarballs (verified: `cargo package --list -p paigasus-proto` prints `moon.yml`) and would ship
whatever the directory gains next.

`paigasus-proto` keeps `tests/**/*.rs` for the same reason the kernel keeps its proptest files — a
vendoring consumer can run the suite. Its `tests/auditable_derive_drift.rs` parses
`src/generated/**`, which the allowlist also ships, so the suite stays runnable from the tarball.

### `CHANGELOG.md` — parent design §14 Q7, answered

**It does not ship.** release-plz writes a per-crate `CHANGELOG.md` at release time;
`paigasus-kernel`'s allowlist already excludes it, and both new allowlists follow. Consistency
across the publishable set matters more than the marginal value of a changelog in the tarball, and
an excluded file has no `repo:input-liveness` or Check 2b consequence.

### Why the per-crate lint table is not optional

Both crates currently carry `[lints] workspace = true`, which resolves to the workspace's
`warnings = "deny"`. Cargo **inlines the resolved lint table into the published manifest**, and
docs.rs builds a published crate as the *root* package on nightly, where cargo's `--cap-lints
allow` does not apply. An inherited `deny` therefore lets the first new rustc warning silently kill
docs.rs builds of an already-released crate — months after the PR that caused it. CI strictness is
unaffected: the Moon `lint` task passes `-D warnings` explicitly.

## 5. Gate changes — `ci/publish-metadata/run.sh`

### 5.1 Check 2 becomes one combined invocation

`check_package` currently does both Check 2b and Check 2 per package, driven by a `while read`
loop over `metadata_checks`'s output. That loop "cannot currently express a two-crate publish", as
the issue puts it. The split:

- **Check 2b stays per-package.** `cargo package --list` performs no build, so it has no
  chicken-and-egg (verified: it succeeds for `paigasus-proto` today, with the derive crate absent
  from crates.io). Its per-package loop is unchanged.
- **Check 2 becomes one `cargo publish --dry-run --locked -p <each>` over the whole set**, run once
  after the 2b loop completes.

`classify_cargo_failure` keeps its 1-vs-2 exit-code contract; cargo names the offending crate in
its own error output, so the combined form loses no diagnosability.

Including `paigasus-kernel` in the same invocation is deliberate. It has no in-tree path
dependency, so the combined form is equivalent for it today — and if a future in-family dependency
is added, the combined form is the one that stays correct, because that is how the crates would
actually be published.

### 5.2 Non-vacuity: the count assertion

Moving Check 2 creates a new production call site in `main()`. Deleting that one line would
disable Check 2 while every `--negative-control` fixture — which exercises the check *functions*,
never their *invocation* — stayed green. That is the SMA-542 guard-the-guard failure shape.

This spec buys the **internal** guard, not an external pin:

1. The combined dry-run helper refuses an empty package list and exits 2 — the same non-vacuity
   shape `assert_package_list` already uses for its rule lists.
2. `main()` records the set of package names the Check 2b loop actually enumerated and asserts the
   combined Check 2 was invoked over exactly that set.

**What this does and does not close, stated plainly.** It closes the realistic regression: a
refactor that drops a crate from the combined run, or that lets the loop iterate zero times, while
leaving the invocation in place. It does **not** close a deletion of the invocation itself, because
the assertion lives beside it and dies with it. A true external pin would mean adding
`PUBLISH_METADATA_SH_CALL_SITES` to `ci/affected-graph/ci_targets.py` *and* adding
`ci/publish-metadata/run.sh` to `repo:affected-smoke`'s `inputs` — without that input the pin
serves a cached pass on exactly the PR that breaks it. That is deliberately out of scope: today's
per-package `check_package` call is equally unpinned, so the restructure introduces no new
exposure, and pinning one check while the file's other four stay unpinned would misrepresent the
coverage. Uniform call-site pinning for this file is follow-up work.

### 5.3 New Check 1c — per-crate lint table

Neither `[lints]` nor `include` appears in `cargo metadata` output, so both new checks read the
publishable crates' `Cargo.toml` directly with `tomllib`. The manifest paths are already available:
`metadata_checks` prints `<name>\t<manifest-dir>` for each publishable crate.

**Check 1c:** every publishable crate declares its own `[lints.*]` table and does **not** inherit
(`[lints] workspace = true`). The error message carries the docs.rs rationale, since the failure it
prevents is invisible for months.

### 5.4 New Check 1d — `include` allowlist

**Check 1d:** every publishable crate declares a non-empty `[package] include`, and that list
covers `README.md` and `LICENSE`.

Check 2b already fails on the *observable consequence* of a missing allowlist (a leaked
`moon.yml`), but only after the tarball listing exists, and only for files someone thought to add
to `FORBIDDEN_PACKAGED`. 1d asserts the discipline itself: an allowlist that exists and is not
empty. The two are complementary — 1d is the rule, 2b is the outcome.

### 5.5 Negative-control coverage

`--negative-control` gains fixture rows proving each new assertion can report red:

| Fixture | Asserts |
| --- | --- |
| a publishable crate with `[lints] workspace = true` | 1c fires |
| a publishable crate with no `[lints]` table at all | 1c fires |
| a publishable crate with no `include` key | 1d fires |
| a publishable crate with `include = []` | 1d fires |
| a publishable crate whose `include` omits `LICENSE` | 1d fires |
| an empty package set handed to the combined dry-run helper | exits 2, not 0 |
| a 2b-enumerated set that disagrees with the Check 2 set | the count assertion fires |

The last two are what keep §5.2's guard honest.

## 6. Release activation

### 6.1 `rs/release-plz.toml`

`paigasus-proto` and `paigasus-proto-derive` move out of the "Not releasable" block into a proto
family block:

```toml
[[package]]
name = "paigasus-proto"
version_group = "proto"
release = true

[[package]]
name = "paigasus-proto-derive"
version_group = "proto"
release = true
```

The existing comment promising exactly this ("`paigasus-proto` / `paigasus-proto-derive` join the
'proto' version_group in SMA-577, once they carry publishable metadata") is updated to describe the
state rather than the plan.

Nothing publishes as a result. The `release` workflow stays gated behind
`PAIGASUS_RELEASE_ENABLED`, which SMA-580 flips. `release = true` here only makes the crates
eligible for the release **PR**.

### 6.2 The five version sites

Per the parent design's inventory, the proto group's sites are 5, 6, 8, 9 and 12. All move
`0.0.0` → `0.1.0`:

| # | Site | Owner in steady state |
| --- | --- | --- |
| 5 | `rs/crates/libs/paigasus-proto/Cargo.toml` | release-plz |
| 6 | `rs/crates/libs/paigasus-proto-derive/Cargo.toml` | release-plz |
| 8 | `rs/Cargo.toml` `[workspace.dependencies] paigasus-proto.version` | release-plz |
| 9 | `rs/Cargo.toml` `[workspace.dependencies] paigasus-proto-derive.version` | release-plz |
| 12 | `py/packages/paigasus-proto/pyproject.toml` | `--write` |

Sites 8 and 9 are **version requirements**, not versions: they are what cargo embeds in the
published manifest. Leaving them at `0.0.0` while the crates move to `0.1.0` would make
`cargo publish -p paigasus-proto` resolve against a crates.io version that will never exist. In
steady state release-plz rewrites them; in *this* PR release-plz is not running, so they are edited
by hand and then asserted by `repo:version-lockstep`.

`rs/Cargo.lock` and `py/uv.lock` are relocked (`cargo update -w`, `uv lock`) as a consequence.

### 6.3 Lock coverage for the proto family

`ci/version-lockstep/run.sh`'s `cargo-lock` and `uv-lock` handlers hardcode the **kernel** member
names (`{paigasus-kernel, paigasus-py-bindings}` and `{paigasus-kernel, paigasus-py-bindings,
paigasus-node-bindings, paigasus-wasm}` respectively), so today the proto family has no lock-file
coverage at all. `py/uv.lock` carries `paigasus-proto` at `0.0.0` (line 607) and drifts
**silently**: no gate reads that entry, and no `uv` invocation in the repo passes `--locked` — the
one that touches the lock is `py/packages/paigasus-kernel/moon.yml`'s
`uv sync --reinstall-package paigasus-py-bindings`. A stale entry is therefore re-resolved on
demand without ever reddening. That is precisely the silent-drift channel the parent design's risk
table exists to close, and closing it for the kernel family while leaving it open for the proto
family would be an arbitrary asymmetry.

**Change:** both handlers take their name set from the row's `<group>` rather than a hardcoded
literal, and two rows are added:

```
proto|cargo-lock|rs/Cargo.lock
proto|uv-lock|py/uv.lock
```

`EXPECTED_SITE_COUNT` moves `18` → `20`. That literal is the deliberate out-of-band anchor
described in the script's own comment — it can only ever false-red, never silently absorb a
deletion — so it is updated together with the `SITES` edit and never independently.

`ci_targets.py`'s `SELF_TASK_EXPECTED_GLOBS["version-lockstep"]` needs **no** change:
`rs/Cargo.lock` and `py/uv.lock` are already in `repo:version-lockstep`'s `inputs` (the kernel rows
read them), and the two new rows introduce no new path.

The `cargo-lock` handler's presence-plus-uniformity discipline (SMA-576 review finding 4 — a name
absent from the lock must not be masked by the survivors' versions agreeing) is preserved
unchanged; only the source of the `names` set moves.

## 7. What this does *not* change

- **Check 3 becomes vacuous for the whole set.** With no publishable crate left at `0.0.0`, its
  `stubs` list is empty. That is already true of `paigasus-kernel` post-SMA-576 and is the intended
  end state — Check 3 is a floor guard, not a permanent assertion. Recorded so a future reader does
  not mistake the silence for breakage.
- **`fileGroups.upstreams`.** `repo:affected-smoke`'s A6 derives the expected set as
  `{src}/src/**/*` plus `{src}/Cargo.toml` per upstream. Adding `README.md` and `LICENSE` to the
  derive crate's directory does not enter that set, so `paigasus-proto`'s declared group is
  unchanged and A6's strict equality still holds.
- **`ci/affected-graph/run.sh`'s expected sets.** No new crate is added, so neither the
  `lockfile->all-lint` set nor the `kernel->bindings` set changes.
- **PyPI / npm metadata.** Parent design §14 Q8 asks whether to extend gating to PyPI metadata.
  Out of scope here and left open; this issue's `EXPECTED_PUBLISHABLE` is crates.io only.

## 8. Documentation

1. **CLAUDE.md** — a Gotchas entry recording the rule from the issue ("any crate flipping
   `publish = true` must carry its own lint table and its own `include` allowlist"), now enforced
   as Checks 1c/1d, plus the combined-dry-run fact and its M3 justification.
2. **Parent design** (`2026-08-22-sma-407-release-activation-design.md`) — mark Q6 and Q7 answered
   with their measurements, and correct §3's "four crates" to three with the reason.
3. **ADR-0011 amendment** (Notion) — the proto family joins activation; its S1 lockstep is realized
   structurally via the committed generated code plus S5 file-path attribution, with no contract
   version introduced.

## 9. Testing

Every command below is run and its output read, not assumed.

1. `bash ci/publish-metadata/run.sh --negative-control` — the seven new fixture rows plus the
   existing ones. Proves the new assertions can report red **before** the real run proves they pass.
2. `bash ci/publish-metadata/run.sh` — the real gate over the three-crate set. This is the check
   the issue predicted would red; it must now pass, and its combined dry-run must appear in the
   output.
3. `bash ci/version-lockstep/run.sh --self-test`, `--negative-control`, then `--check` — the proto
   group at `0.1.0` across all 20 sites.
4. `cargo package --list -p paigasus-proto` and `-p paigasus-proto-derive` — confirm `README.md`
   and `LICENSE` present, `moon.yml` absent.
5. The full CI gate line from CLAUDE.md, `--base origin/main --include-relations`. Per-project Moon
   tasks do not run the repo-level gates, and this change touches manifests, lock files and gate
   scripts that several of them key on.

## 10. Risks

| Risk | Mitigation |
| --- | --- |
| The combined dry-run is mistaken for a replay of release-plz's sequential publish | §3 states the deviation explicitly, and M3 establishes what it *does* prove. |
| Deleting Check 2's invocation disables it silently | Partially closed by §5.2's count assertion; the residual is stated rather than implied, with the external-pin route named for follow-up. |
| A category slug is real but wrong for the crate | Check 1b validates slugs against the snapshot; correctness-of-fit is a human judgement, made here and reviewable in the diff. |
| `EXPECTED_SITE_COUNT` false-reds a later legitimate `SITES` edit | Intended failure direction; the script's own comment records that it is updated only alongside a deliberate `SITES` change. |
| Relocking `py/uv.lock` drags in unrelated updates | `uv lock` without `--upgrade` only resolves what changed; the diff is reviewed, and §6.3's new `uv-lock` row makes future drift red instead of silent. |
