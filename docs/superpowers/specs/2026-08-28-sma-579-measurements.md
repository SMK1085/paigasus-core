# SMA-579 Task 1 — measurements

Measurement only, no production code. All commands ran from the worktree at
`.claude/worktrees/sma-579`, on the pinned toolchain (`export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`).
Pinned versions: `release-plz 0.3.158` (depends on `release_plz_core ^0.36.14`), `@napi-rs/cli 3.7.2`,
Node `24.16.0`, npm `11.11.0`. One host, one run each unless stated. `gh auth token` (a
read-scoped, keyring-backed OAuth token, scopes `gist, read:org, repo`) was available throughout,
so no measurement here was blocked on credentials.

---

## M1 — does `release-plz release --dry-run` succeed, and what does it cost?

**Command run** (the `--git-token` value came from `GIT_TOKEN=$(gh auth token)` env-var form
instead of the brief's `--git-token "$(gh auth token)"` inline-substitution form — the harness's
worktree-isolation checker refused the inline form as "too complex to verify"; `release-plz release
--help` documents `--git-token` as `[env: GIT_TOKEN]`, so the two forms are equivalent):

```
cd rs
GIT_TOKEN=$(gh auth token) release-plz release --dry-run > out 2> err
```

**Result: exit=1, near-immediate (sub-2-second; fails before any `cargo publish` build work
starts).**

```
2026-08-28T18:57:23.019331Z ERROR Package `paigasus-node-bindings` has `publish = false` or
`publish = []` in the Cargo.toml, but it has `publish = true` in the release-plz configuration.
Error: Package `paigasus-node-bindings` has `publish = false` or `publish = []` in the Cargo.toml,
but it has `publish = true` in the release-plz configuration.
```

**This is not the failure §1.3b anticipated, and the real cause is more urgent.** §1.3b worried
about `exit 101, no matching package named 'paigasus-proto-derive'` — a per-package ordering
problem that only bites once `paigasus-proto-derive` is absent from crates.io. That was never
reached. The actual failure is a **pre-existing config defect in the committed
`rs/release-plz.toml`**, unrelated to crate publish order:

- `rs/release-plz.toml`'s `[[package]]` entries for the three binding crates
  (`paigasus-node-bindings`, `paigasus-wasm`, `paigasus-py-bindings`) set `release = true` but
  never set `publish = false`, even though each crate's own `Cargo.toml` sets `publish = false`
  (verified: `grep -n '^publish' rs/crates/bindings/*/Cargo.toml` shows `publish = false` in all
  three).
- Read from `release-plz-0.3.158/src/config.rs:331`: `let is_publish_enabled = value.publish !=
  Some(false);` — an **unset** `publish` field in a `[[package]]` block defaults to
  `publish = true`. There is no `[workspace.packages_defaults]` block in `rs/release-plz.toml`
  either, so nothing overrides that default downward.
- Read from `release_plz_core-0.36.14/src/command/release.rs:210-225`,
  `ReleaseRequest::check_publish_fields()`: it walks every Cargo package that is **not**
  publishable (`Cargo.toml publish = false`) and, if that package also has an **explicit
  override entry** in the release-plz config (i.e. appears in `[[package]]` at all — every field
  becomes an override struct, not just the fields literally written), errors if that override's
  computed `publish` is `true`. Every `[[package]]` entry in this repo's config becomes such an
  override, whether or not it sets `publish` explicitly.
- This check is called from `release-plz-0.3.158/src/args/release.rs:134`,
  `req.check_publish_fields()?`, **unconditionally**, as the last step of building the
  `ReleaseRequest` — **before** the dry-run/live branch in `release_plz_core` is ever reached.
  So `--dry-run` does not protect against it: a real `release-plz release` (no `--dry-run`) would
  fail identically, at the identical point, with the identical message.
- **Why this was never caught before:** `check_publish_fields()` is called only from the
  `release` subcommand's arg-building path. `release-plz release-pr` (the update/PR path) never
  calls it. This repo's `rs/release-plz.toml` comment records a **measured** fact from an earlier
  issue — "`version_group`... DOES apply to crates whose Cargo manifest says `publish = false`"
  — but that measurement was made against `release-pr`, which never exercises this guard.
  `release-plz release` was never run against this config until this task.

**Consequence for §1.3b's decision tree.** The brief's Step 2 offered three outcomes (exit 0 →
`plan` stands; exit 101 for the derive-crate reason → delete `plan`; wall-clock > ~5 min → record
cost, `plan` still stands). The actual result fits none of them: it is a **fourth outcome** — a
blocking config defect that fires regardless of which job-graph shape (`plan` vs. direct
`vars.PAIGASUS_RELEASE_ENABLED` gating) is chosen, and regardless of `--dry-run` vs. live. **Fix
required before either shape can be exercised for real:** add `publish = false` to the three
binding-crate `[[package]]` entries in `rs/release-plz.toml`. That fix is out of scope for this
measurement-only task (no production code) but is a hard prerequisite for whichever later task
first runs `release-plz release` against this repo — most likely wherever `plan`/`release` gets
implemented. **The original §1.3b question — does the derive-crate-absent-from-registry ordering
problem also bite `release-plz release --dry-run`? — remains genuinely unmeasured**, because this
earlier-firing blocker prevents ever reaching that code path. It must be re-measured once the
`publish = false` fix lands.

---

## M2 — does `--output json` list Cargo `publish = false` packages?

**Command run** (same env-var substitution note as M1):

```
cd rs
GIT_TOKEN=$(gh auth token) release-plz release --dry-run --output json > out.json 2> err
```

**Result: exit=1, identical error to M1** (`out.json` empty). The live run cannot answer this
question either, for the same reason as M1.

**Answered from source instead** (release_plz_core 0.36.14, matching the brief's own fallback of
reading source when a live run can't produce clean data):

- `release_packages()` (`release.rs:586`) builds its output only from `project.publishable_packages()`.
- `Project::publishable_packages()` (`project.rs:110-115`) filters on `Package::is_publishable()`.
- `Package::is_publishable()` (`next_ver.rs:456-469`) reads Cargo's **own** `publish` manifest
  field directly (`cargo_metadata::Package.publish`) — `publish.is_empty()` (true for `false` or
  `[]`) makes a package non-publishable. This check runs **before** release-plz's own
  per-package config is even consulted.

**Answer: NO.** A Cargo `publish = false` package (`paigasus-py-bindings`, `paigasus-node-bindings`,
`paigasus-wasm`) can **never** appear in `release()`'s output or `--output json`'s `releases`
array, regardless of anything in `rs/release-plz.toml`. §5.3's decision to bind the version
assertion to `paigasus-kernel` is **confirmed as the only viable choice** — "binding to both,"
which the brief's step 3 asked to check the possibility of, is not an available option, since one
side of that binding can structurally never appear.

---

## M3 — how does the CLI serialize `release()`'s `None`?

Could not be produced live (M1/M2's blocker prevents any run from completing). Answered from
source, reading the pinned CLI tag directly, exactly as the brief's fallback allows:

- `release_packages()` (`release_plz_core-0.36.14/release.rs:613`):
  `(!package_releases.is_empty()).then_some(Release { releases: package_releases })` — an empty
  result is `None`, not `Some(Release { releases: vec![] })`.
- `release-plz-0.3.158/src/main.rs:63-65`:
  `let output = release_plz_core::release(&request).await?.unwrap_or_default();` — `None` becomes
  `Release::default()`.
- `Release` derives `Default` (`release.rs:514`: `#[derive(Serialize, Default, Debug)]`), so
  `Release::default()` is `{ releases: vec![] }`.
- `print_output()` (`main.rs:83-90`) serializes whatever `output` is with `serde_json::to_string`.

**Answer: `{"releases":[]}`.** Matches the `release-pr` precedent's `{"prs":[]}` shape exactly.
Not `null`, not empty stdout. Task 6 can key `has_releases` on `releases` being a non-empty array.

---

## M4 — does npm Trusted Publishing remove `NPM_TOKEN`?

Two sub-questions from §4.4, both checked, plus one thing neither sub-question anticipated that
turned out to be decisive.

**1. Does npm Trusted Publishing cover a scoped org package (`@paigasus/*`)?**

Checked npm's current Trusted Publishing docs (`docs.npmjs.com/trusted-publishers`) and 2026
coverage reporting. **Answer: YES.** Trusted Publishing with OIDC is generally available (GA since
2025-07-31) and explicitly covers "all npm private and public packages, both scoped and
unscoped." Requires npm CLI ≥ 11.5.1 and Node ≥ 22.14.0. This repo's pinned toolchain has npm
11.11.0 on Node 24.16.0 (`npm --version` via the proto shim) — well above the floor.

**2. Does `napi prepublish` publish through the npm CLI in a way that picks it up?**

Measured against the installed `@napi-rs/cli@3.7.2`
(`ts/node_modules/.pnpm/@napi-rs+cli@3.7.2.../dist/index.js:3452`):

```js
const output = execSync(`${npmClient} publish`, {
  cwd: pkgDir,
  env: process.env,
  stdio: "pipe"
});
```

with `npmClient` defaulting to `"npm"` (`index.js:346`). **Answer: YES, structurally.** This is a
real `npm publish` CLI invocation per per-platform package directory, inheriting the full,
unfiltered parent environment — exactly the shape npm's OIDC auto-detection needs (no special env
var required beyond what GitHub Actions itself sets when `id-token: write` is granted).

**3. The decisive caveat neither sub-question asked about, found while researching (1):** npm
Trusted Publishing **cannot be configured for a package that does not yet exist on the
registry** — the npmjs.com settings UI used to register a Trusted Publisher requires the package
to already exist (`npm/cli#8544`, open and unresolved as of 2026; the issue explicitly contrasts
this with PyPI, which supports a "pending publisher" for not-yet-existing projects — the exact
mechanism §4.5's PyPI row already assumes). **Every npm package this issue would publish is a
first-ever publish**: `@paigasus/node-bindings`, its seven `napi create-npm-dirs`-generated
per-platform packages, and `@paigasus/wasm` have never been on the npm registry before SMA-579/580.

**Answer to M4, combined: `NPM_TOKEN` cannot be removed.** It stays as a bootstrap credential —
required for the first publish of each of the (currently) nine npm packages, regardless of (1)
and (2) both holding — because Trusted Publishing structurally cannot cover a package's first
release. Once every package exists on the registry, a **later, separate** change could migrate
subsequent releases to Trusted Publishing, since (1) and (2) hold and nothing else blocks it. This
contradicts the hoped-for "if both hold, no `NPM_TOKEN` at all" outcome the brief posed — the two
sub-questions passing was necessary but not sufficient.

**One more 2026 detail worth carrying into §4.5, for whenever Trusted Publishing migration
happens:** as of 2026-05-20, npm changed Trusted Publisher registration to require explicitly
selecting at least one allowed action (e.g. "npm publish"); configurations created before that
date defaulted to publish-only. Not relevant to this issue's `NPM_TOKEN` decision, but relevant to
whoever configures Trusted Publishing later.

Sources consulted: `docs.npmjs.com/trusted-publishers`, GitHub Changelog "npm trusted publishing
with OIDC is generally available" (2025-07-31), `github.com/npm/cli` issue #8544.

---

## M5 — the `git_release_enable` TOML key

**Command run:**

```
curl -sSL "https://static.crates.io/crates/release-plz/release-plz-0.3.158.crate" \
  -H 'User-Agent: paigasus-sma579' -o cli.crate
tar xzf cli.crate
grep -rn 'git_release_enable\|git_tag_enable' release-plz-0.3.158/src/
```

**Result:** the key is spelled exactly `git_release_enable`
(`release-plz-0.3.158/src/config.rs:414`), a field of `pub struct PackageConfig` (line 390),
whose doc comment (line 388) reads: "Configuration that can be specified both at the
`[workspace]` and at the `[[package]]` level." Confirmed structurally: `Workspace` (line 160)
flattens `PackageConfig` as `packages_defaults`, and `PackageSpecificConfig` (line 298, the
`[[package]]` shape) flattens the same `PackageConfig` as `common`.

**Answer: `git_release_enable` is correct as spelled in §2.1, and it is valid at both `[workspace]`
and `[[package]]` scope.** No change needed to §2.1.

---

## M6 — `NPM_CONFIG_PROVENANCE` and the generated `repository` field

**Part (a) — does `NPM_CONFIG_PROVENANCE=true` reach napi's internal `npm publish` calls?**
Answered from the same source read as M4's sub-question 2: `execSync(\`${npmClient} publish\`, {
cwd: pkgDir, env: process.env, stdio: "pipe" })` passes `process.env` — the full, unfiltered
parent environment — to the child process. npm CLI's standard `NPM_CONFIG_<KEY>` env-to-flag
mapping treats `NPM_CONFIG_PROVENANCE=true` as equivalent to `--provenance`. **Answer: YES** — a
job-level `NPM_CONFIG_PROVENANCE=true` reaches every one of the eight `npm publish` invocations
`napi prepublish` makes (the main package plus seven platform packages), no extra wiring needed.

**Part (b) — do the generated per-platform `package.json` files carry `repository`?**

**Command run:**

```
cd ts/packages/paigasus-kernel
napi create-npm-dirs --cwd ../../../rs/crates/bindings/paigasus-node-bindings
cat ../../../rs/crates/bindings/paigasus-node-bindings/npm/*/package.json
```

(Ran via `pnpm exec napi create-npm-dirs` through the proto-pinned pnpm/napi toolchain, from the
existing worktree `node_modules` — no install needed.)

**Result:** seven directories created (`darwin-arm64`, `darwin-x64`, `linux-arm64-gnu`,
`linux-arm64-musl`, `linux-x64-gnu`, `linux-x64-musl`, `win32-x64-msvc`). **All seven** generated
`package.json` files carry a `repository` field, e.g.:

```json
"repository": {
  "type": "git",
  "url": "git+https://github.com/SMK1085/paigasus-core.git",
  "directory": "rs/crates/bindings/paigasus-node-bindings"
}
```

matching the building repo. **Answer: YES, all seven carry a matching `repository` field.** §12
Q7's "may not" concern is settled — no remediation needed for `--provenance`'s `repository`
requirement on the generated children. Generated `npm/` directory removed after inspection
(`rm -rf`), confirmed clean via `git status --short`.

---

## Scope limits

- One host (macOS/Darwin), one run per live command, pinned toolchain versions listed above.
- M1/M2's live runs never reached the code path they were meant to measure (the derive-crate
  ordering question), because of the config defect found instead; that specific question is
  carried forward as unresolved, not silently treated as answered.
- M4's npm-docs research reflects npmjs.com's documentation and the linked GitHub issue as read on
  2026-08-28; both are living platform behavior, not a version-pinned artifact like the Rust
  source reads in M1/M2/M3/M5.
- M6 was measured against `@napi-rs/cli@3.7.2` as currently pinned in `ts/pnpm-lock.yaml`; a future
  napi major bump is not covered by this measurement.
