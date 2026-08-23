# SMA-577 — Making the proto family publishable

**Status:** Approved (brainstorming 2026-08-23; **adversarial review incorporated — B1–B2, M1–M10, m1–m7**).
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
crates must land in the same commit that flips their `publish` flags (see §8, atomic group A1).

## 3. The chicken-and-egg — measured, and resolved

Parent design §3 flagged this as the reason "the PR implementing this would red on its own gate",
and §14 Q6 left open how the first dry-run could behave. Measurements against the pinned toolchain
(`rust-toolchain.toml` channel `1.95.0`, `cargo 1.95.0 (f2d3ce0bd 2026-03-21)`), run in the SMA-577
worktree with both crates temporarily flipped to `publish = true`:

| # | Invocation | Exit | Result |
| --- | --- | --- | --- |
| M1 | `cargo publish --dry-run --locked -p paigasus-proto` | **101** | `no matching package named 'paigasus-proto-derive' found` / `location searched: crates.io index` |
| M2 | `cargo publish --dry-run --locked -p paigasus-proto-derive -p paigasus-proto` | **0** | see below |
| M3 | as M2, but with `include = ["Cargo.toml"]` on the derive crate | **101** | `warning: ignoring library 'paigasus_proto_derive' as 'src/lib.rs' is not included in the published package` → `no targets specified in the manifest` |

M1 confirms the hazard is real. M2 resolves it: cargo's workspace-publish support (multiple `-p`
flags) computes the topological order itself and stages the to-be-published packages so the
downstream can resolve its upstream locally.

**M3 is the load-bearing measurement.** It establishes that the combined run consumes the
upstream's **packaged tarball**, not its workspace source — a deliberately broken `include` on
`paigasus-proto-derive` fails the run. Without M3, a combined dry-run could plausibly have been a
workspace shortcut that silently stopped proving anything about the artifact. It is not; it is
registry-faithful.

### What M2 actually did, line by line

Review finding **M1** asked whether the combined form runs the verify build for *every* named
package or only for those with a staged dependent. M2's log answers it — every package is
packaged, verified, and compiled from its own tarball, and cargo materializes a real temporary
registry to do it:

```
Packaging paigasus-proto-derive v0.0.0 (…/crates/libs/paigasus-proto-derive)
Packaging paigasus-proto        v0.0.0 (…/crates/libs/paigasus-proto)
Verifying paigasus-proto-derive v0.0.0 (…/crates/libs/paigasus-proto-derive)
Compiling paigasus-proto-derive v0.0.0 (…/target/package/paigasus-proto-derive-0.0.0)
Verifying paigasus-proto        v0.0.0 (…/crates/libs/paigasus-proto)
Unpacking paigasus-proto-derive v0.0.0 (registry `…/target/package/tmp-registry`)
Compiling paigasus-proto        v0.0.0 (…/target/package/paigasus-proto-0.0.0)
Uploading paigasus-proto-derive v0.0.0
Uploading paigasus-proto        v0.0.0
```

`Verifying` appears once per named package, and `Unpacking … (registry …/tmp-registry)` is the
mechanism behind M3.

### M4 — required before implementation

M1–M3 were run at `0.0.0` with only the two proto crates. Two things were therefore **not**
measured, and both must be before the gate change is trusted (review **M1**, **Q2**):

**M4:** `cargo publish --dry-run --locked -p paigasus-kernel -p paigasus-proto-derive -p
paigasus-proto`, at the shipped `0.1.0` with `rs/Cargo.toml`'s `[workspace.dependencies]` pins
already moved to `0.1.0`. Record the exit code and confirm a `Verifying` line for all three.

This is a verification task in the plan, not an assumption in this design.

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

Verified: neither crate has a `build.rs` or any non-`.rs` asset — the only non-Rust files in either
directory are `Cargo.toml` and `moon.yml` — so `src/**/*.rs` omits nothing the build needs.

`paigasus-proto` keeps `tests/**/*.rs` for the same reason the kernel keeps its proptest files — a
vendoring consumer can run the suite. Its `tests/auditable_derive_drift.rs` parses
`src/generated/**`, which the allowlist also ships, so the suite stays runnable from the tarball.
**`paigasus-proto-derive` deliberately omits `tests/**/*.rs`: it has no `tests/` directory** (its
expansion assertions are unit tests under `src/`). The asymmetry is intentional, and is recorded
here so that adding a `tests/` dir later is understood to require an allowlist edit (review **m6**).

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

### `AuditableExample` becomes semver-locked — accepted deliberately

`rs/crates/libs/paigasus-proto/Cargo.toml:10-13` flags this as an open consequence of the flip, and
review finding **M9** is right that neither this design nor the parent addressed it.
`AuditableExample` is generated from `contracts/proto/paigasus/common/v1/auditable_example.proto`,
whose own header calls it *"Not a domain type"* — it exists to prove the cross-language
`AuditMetadata` embedding path generates correctly and to back the per-language `Auditable`
conformance tests against a generated type.

**Decision: accept the commitment.** It is a two-field message that is already generated and
committed in all three languages, so the semver surface it adds is trivial and additive. The
alternatives — moving it behind a feature, or relocating it to a proto package excluded from the
published crate — are contracts restructures that would break the conformance tests SMA-438 built
around it, for no benefit proportional to a `string` and an `AuditMetadata`. The stale
`TODO(SMA-388)` prose recording it as open is replaced by a note recording it as decided.

## 5. Gate changes — `ci/publish-metadata/run.sh`

### 5.1 Check 2 becomes one invocation **per publish group**

`check_package` currently does both Check 2b and Check 2 per package, driven by a `while read`
loop over `metadata_checks`'s output. That loop "cannot currently express a two-crate publish", as
the issue puts it. The split:

- **Check 2b stays per-package.** `cargo package --list` performs no build, so it has no
  chicken-and-egg (verified: it succeeds for `paigasus-proto` today, with the derive crate absent
  from crates.io). Its per-package loop is unchanged.
- **Check 2 runs once per *publish group*.**

Review finding **M2** is correct that a single all-crates invocation would **weaken** the guarantee
`paigasus-kernel` has today. The current contract is "compiles standalone with no unversioned path
dependency" (`run.sh:18-19`) — publishable *against the registry as it exists now*. M3 shows the
combined form resolves in-set dependencies from a locally staged tarball instead. For a crate with
no in-set dependency, folding it into a combined run trades a stronger assertion for a weaker one
and buys nothing.

**A publish group is a connected component of the in-set dependency graph**, computed from the
`cargo metadata` already loaded: nodes are the publishable crates, and an edge joins A→B when A
depends on B and both are publishable. For today's set that yields:

```
{paigasus-kernel}                            → cargo publish --dry-run -p paigasus-kernel
{paigasus-proto-derive, paigasus-proto}      → cargo publish --dry-run -p paigasus-proto-derive -p paigasus-proto
```

`paigasus-kernel` keeps exactly the registry-faithful assertion it has today, and the proto family
gets the only form that can work. The grouping is derived, not declared, so it needs no coupling to
`release-plz.toml`'s `version_group` and cannot go stale when a dependency is added or removed —
which matters because the two-`version_group` design does not guarantee the families release
atomically.

`classify_cargo_failure` keeps its 1-vs-2 exit-code contract; cargo names the offending crate in
its own error output.

### 5.2 `--allow-dirty` must be decided per group, not per package

`check_package` computes `dirty` per package from `git status --porcelain -- "$pkg_dir"`
(`run.sh:362-365`), and the comment above it records that `--allow-dirty` *changes what gets
packaged* — untracked files are swept in. Under the split, Check 2b keeps a per-package flag while
a group's Check 2 needs one flag for the whole group, so a dirty `paigasus-proto` would silently
cause `paigasus-proto-derive` to be packaged dirty too (review **M3**).

**Rule:** the flag is computed as the union over the group's package dirs, and when it is set the
run prints which crates forced it, naming each. This is local-only — CI sets `CI`, which skips the
dirty check entirely — but it keeps a local run from diverging from CI in the one direction that
hides a packaging defect.

### 5.3 Non-vacuity: the invoked-set assertion

Moving Check 2 creates new production call sites in `main()`. Deleting them would disable Check 2
while every `--negative-control` fixture — which exercises the check *functions*, never their
*invocation* — stayed green. That is the SMA-542 guard-the-guard failure shape.

Review finding **M4** showed the first draft talked itself out of a guarantee available for free.
The shape is therefore mandated, not left to the implementer:

1. The Check 2 helper **records into a script-scope variable the set of package names it was
   actually invoked with**, appending on each call.
2. After the group loop, `main()` asserts that recorded set equals the set the Check 2b loop
   enumerated, and fails 2 (infrastructure) on mismatch.
3. The helper refuses an empty package list and exits 2.

Because the record is written *by the helper*, deleting an invocation leaves the recorded set short
and the assertion fires. A one-line deletion is closed. What remains open is deleting the
invocation **and** the assertion together — a two-site edit, the same bounded residual the
`T`-array and release-parity cycles carry.

A true external pin would mean adding `PUBLISH_METADATA_SH_CALL_SITES` to
`ci/affected-graph/ci_targets.py` *and* adding `ci/publish-metadata/run.sh` to
`repo:affected-smoke`'s `inputs` — verified absent today (`moon.yml:165-202`), and without that
input the pin would serve a cached pass on exactly the PR that breaks it. That stays out of scope:
pinning one check while the file's other four stay unpinned would misrepresent the coverage.
Uniform call-site pinning for this file is follow-up work.

Note that mechanism 3 is defence-in-depth rather than a true analogue of `assert_package_list`'s
guard: Check 0 already exits 2 on an empty publishable set (`run.sh:104-110`) and pins the set by
strict equality, so an empty list is unreachable from `main()` today (review **m5**).

### 5.4 Where Checks 1c and 1d live

Review finding **B2**: the first draft left this undecided, and the natural reading was broken.
`metadata_checks`'s fixtures share a `base` package object whose `manifest_path` is
`/nowhere/Cargo.toml` (`run.sh:443-446`), and roughly 22 negative-control rows derive from it —
including the positive control `_expect_rc 0 "clean fixture passes"`. Putting manifest-reading
logic *inside* `metadata_checks` would make every one of those rows fail on a missing file rather
than on the rule it names.

**Checks 1c and 1d are therefore standalone shell functions**, `assert_lint_table <manifest>` and
`assert_include_allowlist <manifest>`, called from `main()` in the per-package loop after
`metadata_checks` returns — the same shape as `assert_package_list` and `assert_freshness_call_site`.
Each takes a path so `--negative-control` can drive it with fixtures, and each carries the file's
standard contract: **0** pass, **1** the repo is wrong, **2** infrastructure failed (an unreadable
or malformed `Cargo.toml` is a 2, not a 1). Their invocations are covered by §5.3's recorded-set
assertion only for Check 2; extending that machinery to 1c/1d is explicitly *not* claimed.

### 5.5 Check 1c — per-crate lint table, and no `deny`

Review finding **M6**: asserting only "declares its own table" enforces the rule but not the hazard
the rule exists to prevent — a crate declaring its own `[lints.rust] warnings = "deny"` would pass
while carrying the docs.rs failure in full. Check 1c asserts **both**:

1. The manifest declares its own `[lints.*]` table and does **not** inherit
   (`[lints] workspace = true`).
2. Neither `lints.rust.warnings` nor `lints.clippy.all` resolves to `deny` or `forbid`.

Both TOML spellings must be handled: the string form `warnings = "warn"` and the table form
`warnings = { level = "warn", priority = -1 }`. The error message carries the docs.rs rationale,
since the failure it prevents is invisible for months.

Assertion 1 fires even on a crate with *no* `[lints]` table at all, which is stricter than the
hazard argument alone requires (a crate inheriting nothing carries no `deny`). That is deliberate:
the rule is discipline, not hazard-avoidance, so a future crate cannot drift into workspace
inheritance by deletion (review **m2**).

### 5.6 Check 1d — `include` allowlist

Review finding **M7**: "covers `README.md` and `LICENSE`" was ambiguous enough to yield two
different implementations. Fixed semantics:

1. `[package] include` is present, is a **list**, and is non-empty.
2. `include.workspace = true` is **rejected explicitly.** It is a workspace-inheritable field, so
   it parses as `{"workspace": true}` — non-empty and truthy, and it would pass a naive check
   vacuously.
3. Every entry is a plain string; a non-string entry is a failure.
4. Membership is **literal**: the list must contain the exact strings `README.md` and `LICENSE`.

Literal membership is chosen over glob-aware matching for two reasons. It is far less code, and
glob matching would accept `include = ["**/*"]` — which "covers" both files while reinstating
exactly the `moon.yml` leak Check 2b exists to catch. The cost is that a legitimate `["*.md", …]`
is rejected; that is an acceptable false-red for a three-crate set with a documented reference
implementation.

Check 2b already fails on the *observable consequence* of a missing allowlist (a leaked
`moon.yml`), but only after the tarball listing exists, and only for files someone thought to add
to `FORBIDDEN_PACKAGED`. 1d asserts the discipline itself. The two are complementary — 1d is the
rule, 2b is the outcome.

### 5.7 Negative-control coverage

`--negative-control` gains fixture rows proving each new assertion can report red:

| Fixture | Asserts |
| --- | --- |
| manifest with `[lints] workspace = true` | 1c.1 fires |
| manifest with no `[lints]` table at all | 1c.1 fires |
| manifest with own `[lints.rust] warnings = "deny"` (string form) | 1c.2 fires |
| manifest with own `warnings = { level = "forbid", priority = -1 }` (table form) | 1c.2 fires |
| manifest with no `include` key | 1d.1 fires |
| manifest with `include = []` | 1d.1 fires |
| manifest with `include.workspace = true` | 1d.2 fires |
| manifest whose `include` omits `LICENSE` | 1d.4 fires |
| manifest that is unreadable / malformed TOML | 1c and 1d exit **2**, not 1 |
| empty package list handed to the Check 2 helper | exits 2, not 0 |
| a recorded invoked-set shorter than the 2b-enumerated set | §5.3's assertion fires |

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

`rs/release-plz.toml` is an input to all three `repo:release-parity*` tasks (`moon.yml:86`), so this
edit schedules all three. That is harmless: `ci/release-parity/ecosystems/release-plz.sh`'s
`_derive_config` greps only `features_always_increment_minor`, so `[[package]]` blocks do not enter
the derived fixture (review **m7**).

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

**`ts/packages/paigasus-proto/package.json` stays at `0.0.0` and is deliberately not a site.**
Parent design §13.4 records `@paigasus/kernel` and `@paigasus/proto` sitting at `0.0.0` while their
family siblings move, as a temporary S1 exception that rejoins at the family's *current* version —
npm activation is SMA-579's scope. Recorded locally so that §6.3's "arbitrary asymmetry" argument
is not read as applying to the npm sibling too (review **m4**).

### 6.3 Lock coverage for the proto family

`ci/version-lockstep/run.sh`'s `cargo-lock` and `uv-lock` handlers hardcode **kernel** member
names, so the proto family has no lock-file coverage at all. `py/uv.lock` carries `paigasus-proto`
at `0.0.0` (line 607) and drifts **silently**: no gate reads that entry, and no `uv` invocation in
the repo passes `--locked` — the one that touches the lock is
`py/packages/paigasus-kernel/moon.yml`'s `uv sync --reinstall-package paigasus-py-bindings`. A
stale entry is therefore re-resolved on demand without ever reddening.

#### The two handlers span two namespaces (review **B1**)

The first draft said the handlers hardcode "the kernel member names" and then stated the two sets
**swapped**. Verified against the script:

| Handler | Line | Hardcoded set |
| --- | --- | --- |
| `cargo-lock` | `run.sh:160` | `{paigasus-kernel, paigasus-py-bindings, paigasus-node-bindings, paigasus-wasm}` |
| `uv-lock` | `run.sh:185` | `{paigasus-kernel, paigasus-py-bindings}` |

More importantly, these are **not the same fact viewed twice**. `cargo-lock` names are Cargo crate
names; `uv-lock` names are Python distribution names. `paigasus-node-bindings` and `paigasus-wasm`
are npm artifacts and appear nowhere in `py/uv.lock`; `paigasus-proto-derive` is a proc-macro crate
with no Python distribution at all (verified: zero occurrences in `py/uv.lock`). "The name set of
the row's group" is therefore **undefined**, and an implementer taking the first draft literally
would either demand `paigasus-proto-derive` in `py/uv.lock` — a permanent false red — or silently
weaken the check.

**Change:** a per-**(group, kind)** membership table, keyed that way *because the namespaces
differ*:

```bash
declare -A LOCK_MEMBERS=(
  [kernel:cargo-lock]="paigasus-kernel paigasus-py-bindings paigasus-node-bindings paigasus-wasm"
  [kernel:uv-lock]="paigasus-kernel paigasus-py-bindings"
  [proto:cargo-lock]="paigasus-proto paigasus-proto-derive"
  [proto:uv-lock]="paigasus-proto"
)
```

`read_version` currently takes only `kind` and `target` (`run.sh:71`, called at `:253`, `:260-261`,
`:471`); it gains the row's `group` as a parameter so it can key this table. Two rows are added:

```
proto|cargo-lock|rs/Cargo.lock
proto|uv-lock|py/uv.lock
```

`EXPECTED_SITE_COUNT` moves `18` → `20`. That literal is the deliberate out-of-band anchor
described in the script's own comment — it can only ever false-red, never silently absorb a
deletion — so it is updated together with the `SITES` edit and never independently.

The `cargo-lock` handler's presence-plus-uniformity discipline (SMA-576 review finding 4 — a name
absent from the lock must not be masked by the survivors' versions agreeing) is preserved
unchanged; only the source of the `names` set moves.

#### The new table needs its own non-vacuity coverage (review **M5**)

`EXPECTED_SITE_COUNT` anchors the *number of `SITES` rows* only; it says nothing about the
*contents* of a name set. And the existing negative control drifts exactly one site — the
node-bindings `package.json` (`run.sh:302-308`) — so no lock handler is exercised at all.
`ci/version-lockstep/README.md` already records this as limitation **L2**: *"It does NOT prove each
of the eight `read_version` kinds is itself honest."* Dropping `paigasus-proto-derive` from
`[proto:cargo-lock]` would therefore be a silent false-green **on the very change that introduces
the table**.

Two additions close it:

1. A `site_verdict`-style **self-test table for the lock readers**: feed each a synthetic lockfile
   with one member missing and assert the reader returns `""` (MISMATCH), and one with all members
   at a uniform version and assert it returns that version. `SELF_TEST_COUNT` moves `1` → `2`.
2. A **second negative-control drift on a lock-file row**, so at least one lock handler is
   exercised end-to-end by the control.

`ci_targets.py`'s `SELF_TASK_EXPECTED_GLOBS["version-lockstep"]` needs **no** change — verified:
`rs/Cargo.lock` and `py/uv.lock` are already among its 16 entries. `SELF_SCHEDULED_GATES`
["version-lockstep"] pins only `moon.yml` lines, which this change does not touch.

## 7. What this does *not* change

Each claim below was verified against code, not inferred.

- **`fileGroups.upstreams` / A6.** `cargo_moon_parity.py:357-360` derives the expected set as
  `{src}/src/**/*` plus `{src}/Cargo.toml` per upstream. Adding `README.md` and `LICENSE` to the
  derive crate's directory cannot enter that set, so `paigasus-proto`'s declared group
  (`rs/crates/libs/paigasus-proto/moon.yml:19-22`) is unchanged and A6's strict equality holds.
- **`ci/affected-graph/run.sh`'s expected sets.** The `lockfile->all-lint` and `kernel->bindings`
  cases (`run.sh:271-272`, `330-331`) key on synthetic paths and add no crate. No new crate is
  introduced here, so neither set changes.
- **Check 3.** It is **already** vacuous on `main` — the only publishable crate is
  `paigasus-kernel` at `0.1.0`, and `run.sh:178` skips the block when `stubs` is empty. This change
  does not make it vacuous; it leaves it that way. Parent design §9's intended replacement (an
  `ci/actionlint/run.sh` verdict function guarding `PAIGASUS_RELEASE_ENABLED`) **was not built by
  SMA-576**: that variable appears only in the two spec documents and one comment in
  `.github/workflows/release.yml:5`. Owner: **SMA-580**, which flips the variable and should carry
  its guard (review **m1**).
- **PyPI / npm metadata.** Parent design §14 Q8 asks whether to extend gating to PyPI metadata.
  Out of scope here and left open; this issue's `EXPECTED_PUBLISHABLE` is crates.io only.

## 8. Atomic groups — which edits must share a commit

Review finding **M8**: the first draft recorded two atomic pairings in passing and never stated the
full set, leaving sequencing to whoever writes the plan. Each group below has a measured
consequence if split.

| # | Must land together | Consequence of splitting |
| --- | --- | --- |
| A1 | `EXPECTED_PUBLISHABLE` += both crates **and** both `publish = true` flips | Check 0 is strict equality — either half alone reds it |
| A2 | The Check 2 group restructure **and** A1 | M1 measured exit 101: per-package `-p paigasus-proto` fails while the derive crate is off crates.io |
| A3 | The `0.1.0` bumps (sites 5, 6) **and** sites 8, 9 **and** both regenerated lock files | Check 2 passes `--locked` (`run.sh:386`); a stale lock or an unmoved pin fails the dry-run |
| A4 | `release-plz.toml`'s `release = true` **and** A3 | Check 3 errors on a publishable `0.0.0` crate that is not `release = false` (`run.sh:204-224`) |
| A5 | `SITES` rows **and** `EXPECTED_SITE_COUNT` 18→20 **and** the `LOCK_MEMBERS` table | The count anchor false-reds; a missing table key is an unset-variable failure |
| A6 | Checks 1c/1d **and** their `--negative-control` rows | A gate that cannot report red is worse than no gate — the repo's standing rule |

A1–A4 are mutually entangled and are best treated as one commit; A5 and A6 are independent of them
and of each other.

## 9. Documentation

1. **CLAUDE.md** — a Gotchas entry recording the rule from the issue ("any crate flipping
   `publish = true` must carry its own lint table and its own `include` allowlist"), now enforced
   as Checks 1c/1d, plus the per-group dry-run fact and its M3 justification.
2. **Parent design** (`2026-08-22-sma-407-release-activation-design.md`) — mark Q6 and Q7 answered
   with their measurements, and correct §3's "four crates" to three with the reason.
3. **`ci/publish-metadata/README.md`** and the **`run.sh` header comment block** (`run.sh:4-26`,
   which enumerates Checks 0/1/1b/2/2b/3/4) — both must gain 1c/1d and the per-group Check 2.
   Neither is an input to `repo:publish-metadata` (`moon.yml:518-531` lists only `run.sh`,
   `categories.py`, `crates-io-categories.txt`), so nothing reds when they go stale (review **M10**).
4. **`ci/version-lockstep/README.md`** — hardcodes the site count in two places (`:8`, `:39`) and
   records limitation **L2**, which §6.3's self-test partially closes. Not an input to its own gate
   either (`moon.yml:555-571`).
5. **In-repo TODO comments this issue obsoletes** — `rs/crates/libs/paigasus-proto/Cargo.toml:8-13`,
   `rs/crates/libs/paigasus-proto-derive/Cargo.toml:10-11`, and `rs/Cargo.toml:136`, all carrying
   `TODO(SMA-388)` / PUBLISH ORDER prose (review **m3**).
6. **ADR-0011 amendment** (Notion) — the proto family joins activation; its S1 lockstep is realized
   structurally via the committed generated code plus S5 file-path attribution, with no contract
   version introduced.

## 10. Testing

Every command below is run and its output read, not assumed.

1. **M4** (§3) — the three-crate dry-run at `0.1.0`. This must pass *before* the gate restructure is
   trusted, since the gate will run it.
2. `bash ci/publish-metadata/run.sh --negative-control` — the eleven new fixture rows plus the
   existing ~22. Proves the new assertions can report red **before** the real run proves they pass.
3. `bash ci/publish-metadata/run.sh` — the real gate over the three-crate set, in two groups. This
   is the check the issue predicted would red; it must now pass.
4. `bash ci/version-lockstep/run.sh --self-test`, `--negative-control`, then `--check` — the proto
   group at `0.1.0` across all 20 sites, with the new lock-reader self-test table running.
5. `cargo package --list -p paigasus-proto` and `-p paigasus-proto-derive` — confirm `README.md`
   and `LICENSE` present, `moon.yml` absent.
6. The full CI gate line from CLAUDE.md, `--base origin/main --include-relations`. Per-project Moon
   tasks do not run the repo-level gates, and this change touches manifests, lock files and gate
   scripts that several of them key on.

## 11. Risks

| Risk | Mitigation |
| --- | --- |
| The per-group dry-run is mistaken for a replay of release-plz's sequential publish | §3 states the deviation explicitly, and M3 establishes what it *does* prove. |
| Folding `paigasus-kernel` into a combined run silently weakens its assertion | §5.1's publish groups keep it in a group of one, preserving today's registry-faithful form. |
| Deleting Check 2's invocation disables it silently | §5.3's helper-recorded invoked-set closes single-line deletion; the two-site residual is stated, with the external-pin route named for follow-up. |
| The new `LOCK_MEMBERS` table is wrong and nothing notices | §6.3's lock-reader self-test table and second negative-control drift; `SELF_TEST_COUNT` 1→2. |
| Check 1c enforces the rule but not the hazard | §5.5 asserts `deny`/`forbid` absence in both TOML spellings, not just non-inheritance. |
| Check 1d passes vacuously on `include.workspace = true` | §5.6 rejects it explicitly and requires a list of plain strings with literal membership. |
| A category slug is real but wrong for the crate | Check 1b validates slugs against the snapshot; correctness-of-fit is a human judgement, made here and reviewable in the diff. |
| `EXPECTED_SITE_COUNT` false-reds a later legitimate `SITES` edit | Intended failure direction; the script's own comment records that it is updated only alongside a deliberate `SITES` change. |
| Relocking `py/uv.lock` drags in unrelated updates | `uv lock` without `--upgrade` only resolves what changed; the diff is reviewed, and §6.3's new `uv-lock` row makes future drift red instead of silent. |
| Publishing semver-locks `AuditableExample` | §4 records the decision to accept, with the reason and the rejected alternatives. |
