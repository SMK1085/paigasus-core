# SMA-407 — Release activation: `0.1.0` floor, kernel/proto lockstep, live release workflows

**Status:** Approved (brainstorming 2026-08-22; **adversarial review incorporated — B1–B6, M1–M14**; **decomposition accepted 2026-08-22 — §12**). **SMA-576 implemented 2026-08-22** — kernel-family `0.1.0` floor, `repo:version-lockstep`, per-package release-plz releasability, and the live `release-pr` job. **SMA-577–580 remain open** (§12).
**Date:** 2026-08-22
**Linear:** umbrella [SMA-407](https://linear.app/smaschek/issue/SMA-407/release-activation-000-010-floor-kernelproto-lockstep-wiring-live) → children SMA-576 … SMA-580 (§12)
**Implements here:** **SMA-576** only — the kernel-family floor, the lockstep gate, and the `release-pr` job.
**Branch:** `feature/sma-576-release-activation-a-kernel-family-010-floor-repoversion`
**Targets:** `main` (currently `14b8603`).
**References:** ADR-0011 (S1 hybrid lockstep, S3 `0.1.0` floor + tool owns every tag, S4 dormant-until-real, S5 file-path attribution, S6 canonical contract); ADR-0005; ADR-0006; ADR-0010; ADR-0020 (service version skew); SMA-398; SMA-376; SMA-378; SMA-388; SMA-385; SMA-307; SMA-419 / SMA-427 / SMA-428; SMA-505 (R7 — service version parked on `0.0.0` "until E-activate"); SMA-529; SMA-541; SMA-553.

---

## 1. Problem

Every package sits at the `0.0.0` stub floor with publishing structurally blocked.
`rs/release-plz.toml` carries `[workspace] release = false`; `repo:publish-metadata` Check 3
holds that line in place *because* a publishable crate is still at `0.0.0`. No release workflow
exists.

ADR-0011 calls this **E-activate** and flags it as the riskiest step: the first tag and first
upload are irreversible, and a hand-placed tag permanently breaks release-plz's bump tracking
(the SMA-385 failure this strategy was designed around).

## 2. What activates, and what does not

ADR-0011 S4 gates activation on a package having **a real public API**. Only two families clear
that bar: **kernel** (PRN canonicalization + UUIDv7 minting across Rust/PyO3/napi/wasm) and
**proto** (generated code committed in all three languages).

`paigasus-ml`, `paigasus-workflows`, `@paigasus/sdk` and `@paigasus/ui` are 1–3 line stubs with
no public API and stay at `0.0.0`.

> **Correction (review M6).** An earlier draft claimed `repo:publish-metadata` Check 3 "keeps
> guarding" those four. It does not and never has: Check 3 operates on `cargo metadata` filtered
> to crates with `publish != false` (`ci/publish-metadata/run.sh:92-99,177`), and none of the four
> is a Cargo crate. **Nothing structurally holds them at `0.0.0`.** That is acceptable — they have
> no release workflow to run away with — but it must not be recorded as a safety net that exists.

### Out of scope, each with a reason

| Deferred | Reason |
| --- | --- |
| `@paigasus/kernel`, `@paigasus/proto` npm publish | No JS emit exists anywhere in `ts/` — every build task is `tsc --noEmit`. A TypeScript build pipeline is a subsystem in its own right. |
| `paigasus-ml`, `paigasus-workflows`, `@paigasus/sdk`, `@paigasus/ui` | No public API (ADR-0011 S4). |
| Decision **G** (sdk/ui sub-1.0 lifecycle) | Unreachable while those packages are stubs. |
| Actual registry uploads | The release job ships gated (§7). |

## 3. The publishable set, and the publish order

Review finding **B1** — the largest gap in the first draft. `paigasus-proto` depends on
`paigasus-proto-derive` (`rs/crates/libs/paigasus-proto/Cargo.toml:24` →
`rs/Cargo.toml:137`), and the repo already documents the consequence in two places:

- `rs/Cargo.toml:136` — *"PUBLISH ORDER (SMA-388): this crate must publish BEFORE paigasus-proto, which depends on it."*
- `rs/crates/libs/paigasus-proto/Cargo.toml:10-11` — *"paigasus-proto-derive must publish FIRST."*

So the publishable set is **three** crates, not two (the diagram below was already right; the
miscount here came from counting the kernel *version group*'s four members — `paigasus-kernel`
plus its three binding crates — rather than the publishable set, which is `paigasus-kernel`
alone):

```
paigasus-proto-derive ──▶ paigasus-proto      (proto family, ordered)
paigasus-kernel                                (kernel family, independent)
```

`cargo publish -p paigasus-proto` cannot succeed until `paigasus-proto-derive` is on crates.io at
a matching version — and neither can `cargo publish --dry-run --locked`, which is exactly what
`repo:publish-metadata` Check 2 (`ci/publish-metadata/run.sh:385`) runs the moment
`EXPECTED_PUBLISHABLE` gains `paigasus-proto`. **The PR implementing this would red on its own
gate.** How that first dry-run is expected to behave before `paigasus-proto-derive` exists on the
registry is an open item — see §12.

## 4. The version model

Two release-plz `version_group`s, each with one Cargo crate as source of truth.

**Verified against the pinned release-plz `0.3.158`** (`.prototools`), not assumed. Three
measurements in a disposable fixture:

1. `version_group` is accepted, and acceptance is meaningful — release-plz uses
   `deny_unknown_fields`, so a misspelled `version_grouppp` fails hard at TOML parse.
2. `version_group` **does** hold a crate whose *Cargo manifest* says `publish = false`: a group
   member untouched by the commit (release-plz logged it "already up to date") was still pulled
   from `0.1.0` to `0.2.0`. This is precisely what the three binding crates need, and it settles
   the ambiguity between Cargo's `publish` field and release-plz's own.
3. release-plz rewrites `[workspace.dependencies]` version requirements (see below).

### Site inventory, by owner

The first draft claimed 11 sites. The true count is **18**, and the four missed classes
(review **B3**) are the highest-value part of this review — each is a silent drift channel, and
two are load-bearing for the publish itself.

| # | Site | Group | Owner |
| --- | --- | --- | --- |
| 1 | `rs/crates/libs/paigasus-kernel/Cargo.toml` | kernel | release-plz |
| 2 | `rs/crates/bindings/paigasus-py-bindings/Cargo.toml` | kernel | release-plz |
| 3 | `rs/crates/bindings/paigasus-node-bindings/Cargo.toml` | kernel | release-plz |
| 4 | `rs/crates/bindings/paigasus-wasm/Cargo.toml` | kernel | release-plz |
| 5 | `rs/crates/libs/paigasus-proto/Cargo.toml` | proto | release-plz |
| 6 | `rs/crates/libs/paigasus-proto-derive/Cargo.toml` | proto | release-plz |
| 7 | `rs/Cargo.toml` `[workspace.dependencies] paigasus-kernel.version` | kernel | release-plz (**measured**) |
| 8 | `rs/Cargo.toml` `[workspace.dependencies] paigasus-proto.version` | proto | release-plz (**measured**) |
| 9 | `rs/Cargo.toml` `[workspace.dependencies] paigasus-proto-derive.version` | proto | release-plz (**measured**) |
| 10 | `rs/crates/bindings/paigasus-py-bindings/pyproject.toml` | kernel | `--write` |
| 11 | `py/packages/paigasus-kernel/pyproject.toml` | kernel | `--write` |
| 12 | `py/packages/paigasus-proto/pyproject.toml` | proto | `--write` |
| 13 | `rs/crates/bindings/paigasus-node-bindings/package.json` | kernel | `--write` |
| 14 | `rs/crates/bindings/paigasus-wasm/package.json` | kernel | `--write` |
| 15 | `py/packages/paigasus-kernel` dep pin → `paigasus-py-bindings==X.Y.Z` | kernel | `--write` |
| 16 | `rs/Cargo.lock` (13 × `version = "0.0.0"`) | both | `cargo update -w` |
| 17 | `py/uv.lock` (5 × `version = "0.0.0"`, plus the `requires-dist` specifier) | kernel | `uv lock` |
| 18 | `rs/crates/bindings/paigasus-node-bindings/index.js` (26 × `bindingPackageVersion !== '0.0.0'`) | kernel | `napi build` |

Sites 7–9 are **version requirements**, not versions: they are what cargo embeds in the published
manifest. If they stay at `0.0.0` while the crates move to `0.1.0`, `cargo publish -p paigasus-proto`
resolves against a crates.io version that will never exist.

**Measured against `0.3.158`** in a three-crate fixture: release-plz **does** rewrite them
(`probe-a = { path = "crates/a", version = "0.2.0" }` after a `feat:` on `a`). They are
release-plz-owned; `--write` does not touch them. `--check` still verifies them — that is the point
of checking sites the tool owns.

Site 17 drifts **silently**: `py/packages/paigasus-kernel/moon.yml:36` runs bare `uv sync`, not
`--locked`, so nothing reds. Site 18 is regenerated by `napi build` on every
`paigasus-kernel-ts:build`, and `ci.yml`'s codegen-drift gate covers only the three
`**/generated` proto dirs — so nothing reds there either.

`--check` verifies **all 18**, including the ones release-plz and the regeneration commands own.
A gate that trusted release-plz to have done its half would not notice a `version_group` that
silently stopped applying. (SMA-577 later added a `proto`-group `cargo-lock` row and a
`proto`-group `uv-lock` row alongside the kernel-group ones already in this inventory, bringing
the live total to **20** — see
`docs/superpowers/specs/2026-08-23-sma-577-proto-family-publishable-design.md` §6.3. The
18-row inventory below is the original SMA-576 baseline and is not re-numbered here.)

### The dependency-pin site

`py/packages/paigasus-kernel` declares `dependencies = ["paigasus-py-bindings"]` — unpinned — and
reaches the local crate through `[tool.uv.sources]`, which is development-only metadata that uv
strips from the built distribution. The published wrapper would otherwise float against *any*
bindings version. **Decision:** pin exactly (`==X.Y.Z`). Under lockstep the bindings can never
release independently, so a range buys nothing and an exact pin makes the lockstep legible in the
published metadata.

### Why proto's source of truth is the crate

ADR-0011 S1 says the proto family is "versioned to track the proto contract"; no contract version
exists. It does not need to: the generated Rust lives inside `rs/crates/libs/paigasus-proto/src/generated`,
so a `contracts/` change regenerates it, changes the crate's files, and release-plz attributes the
bump **by file path** — ADR-0011 S5 exactly. This keeps the crate inside release-plz's model so the
SMA-398 parity gate keeps covering it. Recorded as an S1 clarification (§11).

A comment-only `.proto` edit shifts the embedded `FILE_DESCRIPTOR_SET` and so does bump the family.
That is correct — the wire artifact changed.

## 5. Lockstep mechanism

release-plz has **no hook mechanism**, and this is now measured rather than inferred: against the
pinned `0.3.158`, a config carrying `pre_release_hook` fails with `unknown field 'pre_release_hook'`.

> Review **M10** asserted the opposite. It is wrong for this version. Note the argument would hold
> regardless: a *release-time* hook runs after the manifests must already be correct, so it could
> never stamp them during the update step.

Everything Cargo cannot reach is therefore stamped by one script with three modes:

**`ci/version-lockstep/run.sh`**

| Mode | Behaviour |
| --- | --- |
| `--check` (default) | Compares all 20 sites (18 below, plus the two SMA-577 proto lock rows) against each group's source of truth. The `repo:version-lockstep` Moon gate. |
| `--write` | Rewrites sites 10–15 and invokes the regeneration commands for 16–18. |
| `--negative-control` | Proves the checker can still report red. |

One implementation, two operating modes, so writer and checker cannot disagree about what "in
lockstep" means — the same argument that makes `ci/publish-metadata/run.sh` own both its assertion
and its `--refresh-categories` path.

**Exit codes:** `0` pass, `1` the repo is wrong, `2` infrastructure failed. `--write`
additionally distinguishes *wrote something* from *already in lockstep* via stdout (the release-PR
job needs that to decide whether to commit) while keeping `0` for both.

## 6. Making the proto family publishable

Review **B2** — the first draft called this "a two-line change". It is not. Measured against
`ci/publish-metadata/run.sh`, both new crates need:

| Requirement | Check | Current state |
| --- | --- | --- |
| `description`, `repository`, `readme`, `keywords`, `categories` | 1 (`run.sh:123-147`) | **none present** on either crate |
| Categories are real crates.io slugs | 1b | not chosen |
| `README.md` + `LICENSE` in the packaged tarball | 2b (`REQUIRED_PACKAGED`, `run.sh:50`) | **neither file exists** in either crate dir |
| `moon.yml` *not* in the tarball | 2b (`FORBIDDEN_PACKAGED`) | **no `include` allowlist** → cargo's default sweeps it in |
| Per-crate lint table | — | both carry `[lints] workspace = true` |

That last row is review **M5** and is a real regression risk: `paigasus-kernel/Cargo.toml:33-44`
deliberately uses `[lints.rust] warnings = "warn"` rather than inheriting, with a recorded
rationale — cargo inlines the resolved lint table into the published manifest, and docs.rs builds
published crates as the root package on nightly where `--cap-lints allow` does not apply, so an
inherited `warnings = "deny"` lets the first new rustc warning silently kill docs.rs. Both new
crates must mirror the kernel's per-crate table.

**Recorded as a rule, not a one-off:** any crate flipping `publish = true` must carry its own lint
table and its own `include` allowlist.

## 7. Release workflow

`.github/workflows/release.yml`, release-plz's two-job pattern (SMA-307), split so the reversible
half runs live and the irreversible half ships complete but inert.

### `release-pr` — live

Runs on push to `main`; opens/updates the rolling release PR, then runs
`ci/version-lockstep/run.sh --write` and commits onto release-plz's branch. Ordering is fixed:
release-plz first, `--write` second, same job, every time. `concurrency: { group: release-pr,
cancel-in-progress: false }` — release-plz force-updates that branch, so two rapid merges to main
would otherwise race (review **M9**).

**Token strategy (review B6).** GitHub does not trigger `pull_request` workflows for PRs opened by
the default `GITHUB_TOKEN`, and `moon ci` — the required check on the `Protect main` ruleset — runs
only on `pull_request`/`push`. A release PR opened with `GITHUB_TOKEN` would sit permanently with a
missing required check and could never merge. The job therefore uses a **GitHub App installation
token** (or a fine-grained PAT).

### `release` — shipped, gated

Guarded by `if: vars.PAIGASUS_RELEASE_ENABLED == 'true'`. Cuts tags and publishes.

**Credentials live here and nowhere else (review M2).** The first draft put npm credentials in
`prebuild.yml`, which carries a `pull_request` trigger — same-repo PRs receive repository secrets,
so any contributor with push access could exfiltrate a registry token in a PR that never merges.
Registry tokens are the highest-value secret this project will hold and their compromise is not
reversible. `release.yml` has no `pull_request` trigger. Prefer **OIDC trusted publishing** (PyPI,
crates.io) and **npm provenance** (`permissions: id-token: write`) over long-lived tokens.

**Trigger filters must be block sequences**, never the inline `branches: [main]` form — `repo:actionlint`'s
extractor fails all four keys loudly on inline flow.

### Two unresolved workflow questions

- **The napi ↔ release-plz tagging boundary (review M1).** `prebuild.yml:244-245` explicitly assigns
  it here: *"SMA-407 owns the napi/release-plz tagging boundary + the real --gh-release path"*.
  `napi prepublish` defaults `ghRelease: true` and cuts a GitHub release + tag from `package.json`.
  Two tools tagging the same repo is precisely the ADR-0011 S3 failure mode — *"the tool owns every
  tag"*, singular. Must be settled before the release job is written: which tool tags npm artifacts,
  whether `--no-gh-release` stays on, and what `--tag-style` resolves to against release-plz's
  `git_tag_name` (which `rs/release-plz.toml` does not currently set).
- **`@paigasus/wasm` is not publishable as it stands (review M4).** `private: true`; no
  `publishConfig.access: public` (so a scoped package would publish *restricted*, unlike its
  sibling); and its `files` list includes `paigasus_wasm_bg.wasm`, which is **gitignored** — so
  publishing from a fresh checkout ships a package with no wasm binary unless the release job runs
  `wasm-pack` first.

### The PyPI wheel problem (review M3)

"Wheels only, no sdist" closes the macOS sdist trap — `rs/crates/bindings/paigasus-py-bindings/pyproject.toml`
notes a published sdist would not carry `rs/.cargo/config.toml`, whose apple-darwin
`-undefined dynamic_lookup` flags the `extension-module` cdylib needs to link.

But **no maturin wheel matrix exists.** `prebuild.yml`'s matrix builds the *napi addon*, not
wheels; there is no cibuildwheel or maturin-action anywhere. A single-runner build yields one
manylinux-x86_64 wheel, and with no sdist there is no fallback — so `pip install paigasus-kernel`
(pinned `==` to the bindings) fails outright on macOS, Windows and linux-aarch64.

This is a subsystem of the same size SMA-428 was for napi. It is **not** solvable inside this
issue; see §12.

## 8. Replacing `[workspace] release = false`

Review **B4**, and the finding with the widest blast radius. The first draft said the line "goes
away". It is **workspace-scoped**: deleting it makes every member releasable — `paigasus-gateway`,
`paigasus-iam`, `paigasus-logging`, `paigasus-observability`, `paigasus-service-info`,
`paigasus-iam-core`, `paigasus-kernel-parity` — all at `0.0.0`. Cargo's `publish = false`
suppresses `cargo publish`, **not tagging**, so release-plz would bump and permanently tag roughly
nine crates nobody intended to release, in a repo whose ADR-0011 S3 says the tool owns every tag.

**And it is worse than "the crates nobody touched get tagged" — measured.** `rs/release-plz.toml`
sets `dependencies_update = true`. In the fixture, a crate that was **neither in the version group
nor touched by the commit** was still bumped `0.1.0 → 0.1.1`, logged as *"dependencies changed"*,
purely because a dependency of it moved. In this repo `paigasus-kernel` is depended on by
`paigasus-iam-core`, `paigasus-observability`, both services and the three bindings — so the
**first** kernel release cascades a patch bump, and therefore a permanent tag, across most of the
workspace. The blast radius is not the ~9 crates the review named; it is "the transitive dependents
of whatever moved".

There is a second consequence. `rs/crates/services/paigasus-iam/src/service_info.rs:25` and
`…/paigasus-gateway/src/service_info.rs:16` both read `env!("CARGO_PKG_VERSION")`, and SMA-505's
spec (R7) explicitly parks ADR-0020 skew reporting on "version is permanently `0.0.0` until
E-activate". This *is* E-activate — so the service crates' advertised version changes as a side
effect, which the first draft never mentioned.

**Design:** replace the workspace-level blanket with **explicit per-package settings** — `release = true`
only for the seven family members, `release = false` for everything else. Given the cascade above,
this is not defence-in-depth; it is the only thing standing between the first kernel release and a
workspace-wide tag sweep. The plan must also state what version the service crates sit at afterwards
and what that means for ADR-0020.

**Two more measurements settle that this is required, not optional, and required *now*:**

- Per-package `release = false` removes a package from the release-PR proposal **entirely** — the
  fixture's `probe-b` was neither bumped nor listed. So it genuinely controls the cascade rather
  than merely suppressing a tag.
- `[workspace] release = false` — today's config — makes release-plz **hard-error**:
  `no public packages found. Are there any public packages in your project?`. A live `release-pr`
  job would therefore **fail outright**, not quietly do nothing. The replacement is a precondition
  for §7's job existing at all, which is why it lands with SMA-576 rather than later.

An alternative worth measuring in the plan: `dependencies_update = false`. It would stop the
cascade at its source rather than suppressing its symptom — but it also changes the classification
contract the SMA-398 parity harness derives its fixture config from (`rs/release-plz.toml` is the
source), so it is not a free switch.

## 9. CI bookkeeping

- **`repo:version-lockstep`** in **both** `ci.yml`'s `T=(…)` array and CLAUDE.md's marker-delimited
  command (SMA-541), with `inputs` satisfying `repo:input-liveness` (SMA-553).
- **`repo:publish-metadata`**: `EXPECTED_PUBLISHABLE` gains `paigasus-proto` **and**
  `paigasus-proto-derive` — Check 0 is strict-equality, so this is mandatory.
- **Re-founding the guard.** Check 3 asserts a publishable crate at `0.0.0` is release-blocked.
  At the `0.1.0` floor it goes **vacuously satisfied** (`run.sh:177` skips the block when no
  publishable crate is at `0.0.0`) and stops holding anything in place. The replacement is a new
  assertion in `ci/actionlint/run.sh` that the `release` job's `if:` guard is present, references
  `PAIGASUS_RELEASE_ENABLED`, and is not defeated by a `continue-on-error:` or a discarded exit
  status.
- **Guard-the-guard obligations (review M11), which the first draft omitted.** Per the repo's own
  doctrine (`ci_targets.py:225-323` — *"That script cannot assert its own invocation"*), this is a
  **new** verdict function against a **new** file, not an extension of check 8 (whose scanning is
  keyed on `ci.yml` specifically). It therefore requires: a new self-test table, `SELF_TEST_COUNT`
  bumped 9 → 10 (the gate asserts invocations **and** definitions), and a new whole-line
  `ACTIONLINT_SH_CALL_SITES` entry pinning its production call site.
- **The re-founded guard is weaker than what it replaces (review M12), and that is deliberate.**
  Check 3 made publishing `0.0.0` structurally impossible. The replacement asserts only that the
  `if:` expression exists; the *decision* is then a repository variable any maintainer can flip in
  the UI with no PR and no review. The guard protects the **mechanism**, not the **decision**. If
  the decision needs to be reviewable, the standard tool is a GitHub Environment with required
  reviewers — an option, not part of this design.

## 10. First-release behaviour

The first draft's analysis here was **wrong**, and correcting it is review **B5**.

It asserted that release-plz determines what has been released from git tags, so with none present
it treats all history as unreleased and may propose `0.2.0`+. The repo's own harness contradicts
the premise: `ci/release-parity/ecosystems/release-plz.sh:43` sets `git_only = true` with the
comment *"avoids crates.io registry lookup for nonexistent fixture crates"*. In the real
configuration `rs/release-plz.toml` sets no `git_only`, so release-plz's baseline is the
**crates.io registry**, not tags.

For a package absent from the registry, the expected behaviour is therefore to treat it as new and
propose the **manifest version** — `0.1.0`, no bump.

**Confirmed by SMA-576 (was a prediction; now measured).** Running `release-plz release-pr` on
this repo at the `0.1.0` floor logs `WARN Package 'paigasus-kernel@*.*.*' not found`, then
proposes `next version is 0.1.0` — the manifest version, no bump, exactly as predicted. The
`release-pr --output json` result's `prs` array is empty: **the first release PR is empty.**

Two consequences the first draft missed:

1. **The §7 job split was justified by a hazard that probably does not exist in that form.** The
   split is still right — for the reasons in §7 (token strategy, credential isolation, observing
   real behaviour before publishing) — but the justification must be restated honestly.
2. **The acceptance criterion is unsatisfiable as written, confirmed.** Release-plz proposes no
   change, so there is no release PR and therefore no "end-to-end evidence" from the first run —
   this is no longer a hypothetical to plan around, it is what SMA-576 actually observed. Evidence
   for that run is the empty `prs` array plus the log lines above, not a PR diff.

**The real hazard is name squatting.** With `git_only` unset, release-plz performs a crates.io
lookup for **every** workspace member name — `paigasus-logging`, `paigasus-observability`, … — and a
squatted name would silently become the comparison baseline.

**Pre-flight checklist, before the first `--write` lands (review M7, M8):**

- [ ] `paigasus-kernel`, `paigasus-proto`, `paigasus-proto-derive` free on crates.io
- [ ] `paigasus-kernel`, `paigasus-proto`, `paigasus-py-bindings` free on PyPI
- [ ] the `@paigasus` npm scope owned by this project
- [ ] no workspace member name squatted on crates.io
- [ ] the repository is **public** — `docs/ops/RUNBOOK-go-public.md` documents a flip that has not
      been executed, so every published `repository`/`homepage` URL currently 404s

**What we never do, under any circumstance:** hand-place a `*-vX.Y.Z` tag to seed the tracking.
Manual tags lack release-plz's metadata and silently stop all future bumps — the SMA-385 failure and
the direct motivation for ADR-0011 S3.

**Rollback (review, MINOR).** Irreversibility is concrete, not abstract: crates.io supports only
`cargo yank` (never delete, never reuse); PyPI supports delete-but-never-reuse; npm supports
unpublish within 72 hours only. A wrong publish is a permanent version burn in two of three
registries.

## 11. Testing

- **`ci/version-lockstep/run.sh --self-test`**: each of the 18 sites drifted individually; the
  dependency pin left unpinned; a malformed version; a missing manifest (exit `2`, not `1`); both
  groups drifting at once; and `--write` idempotence (a second run writes nothing).
- **`--negative-control`** before the real check under an explicit `set -euo pipefail` — Moon does
  not enable errexit for `script:` blocks, so without it a failing control is masked by the passing
  real run. Precedent on `main`: `ci/affected-graph/run.sh --negative-control` (`moon.yml:127`) and
  `ci/publish-metadata/run.sh --negative-control` (`moon.yml:464`).
  > Review **M14**: the first draft cited SMA-530's three `repo:release-parity*` controls. Verified
  > against this branch's base — `moon.yml:57-83` shows plain `script:` lines with no control.
  > **SMA-530 is not on `main`.** The precedent stands; that citation did not.
- **`repo:release-parity*` is unchanged** — nothing here alters classification.

## 12. Decomposition — accepted 2026-08-22

The first draft assumed the gated scope was deliverable as one unit. The review shows it is not.
Three findings are each a subsystem:

| Blocked-out work | Size |
| --- | --- |
| Proto family publishability (§6) — 2 crates × (metadata + README + LICENSE + `include` + lint table), plus the derive→proto publish order and the Check 2 dry-run chicken-and-egg (§3) | a full issue |
| The maturin wheel matrix (§7) — no matrix exists; comparable to SMA-428's napi work | a full issue |
| The napi ↔ release-plz tagging boundary + `@paigasus/wasm` packaging (§7) | a full issue |

**The split, as filed under SMA-407:**

| Issue | Scope |
| --- | --- |
| **[SMA-576](https://linear.app/smaschek/issue/SMA-576)** — kernel-family floor + lockstep gate | Sites 1–4, 7, 10–11, 13–18; `ci/version-lockstep`; per-package release-plz settings (§8); the `release-pr` job with its App token. **No publish.** |
| **[SMA-577](https://linear.app/smaschek/issue/SMA-577)** — proto-family publishability | §6 in full, plus the derive→proto ordering. Absorbs SMA-388. |
| **[SMA-578](https://linear.app/smaschek/issue/SMA-578)** — maturin wheel matrix | §7's PyPI half. |
| **[SMA-579](https://linear.app/smaschek/issue/SMA-579)** — npm activation | Tagging boundary, `@paigasus/wasm` packaging, the `release` job's npm path. |
| **[SMA-580](https://linear.app/smaschek/issue/SMA-580)** — flip the variable | Pre-flight checklist (§10), then `PAIGASUS_RELEASE_ENABLED`. |

Ordering: **576 → (577 ‖ 578 ‖ 579) → 580**, recorded as Linear blocking relations.

**This document remains the umbrella design for all five.** Sections 6, 7's PyPI/npm halves, and 10's
pre-flight are inputs to 577–580 and are deliberately *not* implemented by 576 — they are kept here
rather than split across five documents so the activation is readable as one strategy.

## 13. Documentation

An **ADR-0011 amendment** recording four things:

1. **S1 clarification** — proto's lockstep is realized structurally via the committed generated code
   plus S5 file-path attribution; no contract version is introduced.
2. **S4 activation shape** — `release-pr` live, `release` gated behind a repository variable; the
   guard moves from Check 3's pairing into `ci/actionlint/run.sh`, and protects the mechanism, not
   the decision.
3. **Decision G deferred** — with its reason.
4. **A temporary, deliberate S1 exception (review M13)** — `@paigasus/kernel` and `@paigasus/proto`
   sit at `0.0.0` while their family siblings move to `0.1.0`+. Record what that means and how they
   rejoin: they jump to the family's *current* version, not `0.1.0`.

## 14. Open questions for the plan

1. ~~Does release-plz rewrite `[workspace.dependencies]` version requirements (sites 7–9)?~~
   **Answered — yes** (measured, §4). release-plz-owned.
2. ~~Does `version_group` hold crates whose *Cargo manifest* says `publish = false`?~~
   **Answered — yes** (measured, §4). The three binding crates can join the kernel group.
3. **New, from the same measurement:** keep `dependencies_update = true` and suppress the cascade
   with per-package `release = false`, or set it to `false` and stop the cascade at source? The
   latter changes what the SMA-398 parity fixture derives from the real config (§8).
4. What is `git_tag_name`? `rs/release-plz.toml` sets none; the default is `{package}-v{version}`.
   Does it collide with napi's `lerna` style (`@paigasus/node-bindings@0.1.0`)?
5. What schedules the `release` job, and is `release_always` on or off?
6. ~~How does the *first* `cargo publish --dry-run -p paigasus-proto` behave before
   `paigasus-proto-derive` exists on crates.io — and can `check_package`'s per-package loop
   (`run.sh:665-668`) express a two-crate publish at all?~~
   **Answered.** It cannot: measured at exit 101, `no matching package named
   'paigasus-proto-derive' found`. cargo 1.95's *multi*-package `cargo publish --dry-run -p
   paigasus-proto-derive -p paigasus-proto` resolves the publish order itself — flag order is
   irrelevant, cargo computes the topological order — and M3 proved it consumes the upstream
   crate's locally staged, packaged tarball rather than replaying release-plz's sequential
   registry publish. See `docs/superpowers/specs/2026-08-23-sma-577-proto-family-publishable-design.md`
   §3.
7. ~~Does the generated per-crate `CHANGELOG.md` ship? `paigasus-kernel`'s `include` allowlist
   (`Cargo.toml:19`) excludes it, and new tracked files have `repo:input-liveness` and Check 2b
   consequences.~~
   **Answered — no.** `paigasus-proto` and `paigasus-proto-derive`'s `include` allowlists
   (Check 1d) exclude `CHANGELOG.md` the same way `paigasus-kernel`'s does; it is generated but
   does not ship.
8. `paigasus-py-bindings/pyproject.toml` has essentially no PyPI metadata and no LICENSE/README.
   Nothing gates PyPI/npm metadata the way `repo:publish-metadata` gates crates.io — extend the new
   gate, or accept the asymmetry explicitly?

## 15. Risks

| Risk | Mitigation |
| --- | --- |
| A missed version site drifts silently | `repo:version-lockstep` checks all 20, including release-plz's and the regeneration commands'; the negative control proves it can still red — on both a packagejson drift and a lock-row drift, each staged into its own pristine tree. |
| Removing `release = false` tags ~9 unintended crates | Per-package settings instead of a workspace blanket (§8). |
| `paigasus-proto` reds Check 2 on its own PR | §6's work list + resolving the derive publish order first (§3, §14 Q5). |
| Registry tokens exfiltrated via a PR | Credentials only in `release.yml` (no `pull_request` trigger); prefer OIDC (§7). |
| The release PR can never merge | GitHub App token, not `GITHUB_TOKEN` (§7). |
| PyPI package uninstallable off linux/x86_64 | Blocked out to SMA-407c; **not** claimed as closed (§7). |
| Two tools cutting tags | Unresolved — settle the napi boundary before writing the release job (§7). |
| A squatted crates.io name becomes the version baseline | Pre-flight checklist (§10). |
| Published URLs 404 | Repo must be public first (§10). |
| The release decision is flipped without review | Acknowledged: the guard protects the mechanism, not the decision (§9). |
