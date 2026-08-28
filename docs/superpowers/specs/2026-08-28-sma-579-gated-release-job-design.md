# SMA-579 — Release activation D: the gated `release` job, npm activation, and the re-founded release guard

Fourth increment of SMA-407. Input specs: §7/§9 of
`docs/superpowers/specs/2026-08-22-sma-407-release-activation-design.md` (the umbrella) and
**§9 of** `docs/superpowers/specs/2026-08-28-sma-578-maturin-wheel-matrix-design.md`, which is a
reviewed specification for this issue's PyPI half, not a sketch.

**Status:** design, revision 2. Revision 1 was reviewed adversarially and returned NEEDS REWORK
with six blocking findings; §13 records what changed and what was rejected. Nothing here is
implemented yet.

---

## 0. What this issue does, and what it deliberately does not

It ships the **complete but inert** irreversible half of the release path: tags, crates.io, PyPI
and npm, behind `vars.PAIGASUS_RELEASE_ENABLED == 'true'`. SMA-580 flips that variable after its
pre-flight. **This issue publishes nothing.**

It also settles three decisions SMA-578 refused to make by default, because making them by default
is how they would have been made wrongly:

1. the napi ↔ release-plz **tagging boundary** (§2),
2. the **crates.io credential mechanism** (§4),
3. `py/packages/paigasus-proto`'s **PyPI ownership** (§5.4).

### Scope decision (2026-08-28)

A split was offered — moving the npm *publish path* to a follow-up, since `prebuild.yml` is not a
reusable workflow. **Sven chose all six sub-scopes in one PR.** That decision stands and this spec
carries the full scope.

**New information since that decision, recorded because it postdates it:** the adversarial review
found the npm half carries five unresolved items the PyPI half does not — the `wasm-pack`
clobber (§7.2), tarball-vs-disk assertion (§7.5), npm's non-idempotency (§6), npm Trusted
Publishing (§4.4), and per-platform provenance (§7.6). The PyPI half is fully specified by
SMA-578 §9 and carries none. If the PR proves unreviewable, **the npm half is the seam**, and it
is pre-identified here so the split does not have to be re-derived under pressure.

### Not in scope

`@paigasus/kernel` / `@paigasus/proto` npm publish. There is no JS emit anywhere in `ts/` — every
build task is `tsc --noEmit` — so those need a TypeScript build pipeline first. Tracked separately.

---

## 1. The job graph

```
.github/workflows/release.yml        on: push → branches: [main]

  release-pr                                          ← existing, UNTOUCHED, ungated
  plan          if: vars.PAIGASUS_RELEASE_ENABLED == 'true'
                  mint App token → release-plz release --dry-run --output json
                  outputs: kernel_release, proto_release, kernel_version, proto_version
  wheels        needs: plan     uses: ./.github/workflows/wheels.yml
  prebuild      needs: plan     uses: ./.github/workflows/prebuild.yml
  proto-dist    needs: plan     builds py/packages/paigasus-proto
  release       needs: [plan, wheels, prebuild, proto-dist]   environment: release-publish
                  mint App token + crates.io OIDC → release-plz release
                                                      ← FIRST IRREVERSIBLE STEP
  publish-pypi  needs: [plan, wheels, proto-dist, release]    environment: release-publish
  publish-npm   needs: [plan, prebuild, release]              environment: release-publish
```

### 1.1 Why this order

**Everything reversible precedes the first irreversible step** (SMA-578 review **B3**). The
umbrella's first draft ran `release → wheels → publish-pypi`, so `release-plz release` completed
the crates.io upload *and* cut the tags before a single wheel was built. A failure in the six-leg
matrix — a zig regression, a runner image change — would leave crates.io permanently published,
tags permanently cut, and `paigasus-kernel` missing from PyPI while pinning
`paigasus-py-bindings==X.Y.Z`.

Nothing forces the bad order: the release commit on `main` already carries the bumped versions.
**`release.yml` must carry this rationale as a comment**, because the order looks arbitrary
otherwise and a future editor would "simplify" it.

**Revision 2 extends the principle inside the jobs, which revision 1 violated.** Revision 1 had
`publish-npm` run `wasm-pack` (a full wasm32 compile) and `publish-pypi` run
`uv build --package paigasus-proto` — both *after* `release-plz release` had published to
crates.io and cut tags. A rustup or toolchain failure there produced exactly the half-published
state B3 exists to prevent.

**Rule, now explicit: no job downstream of `release` may build anything.** `publish-pypi` and
`publish-npm` may only download, assert, and upload. Every artifact they need is produced in the
reversible stage:

| Artifact | Produced by | Consumed by |
|---|---|---|
| `wheel-<platform>` ×7, `sdist` | `wheels.yml` | `publish-pypi` |
| `face-paigasus-kernel` | `wheels.yml` | `publish-pypi` |
| `proto-dist-py` | `proto-dist` | `publish-pypi` |
| `prebuild-<platform>` ×7 | `prebuild.yml` | `publish-npm` |
| `npm-dirs` (assembled `npm/`) | `prebuild.yml` | `publish-npm` |
| `wasm-dist` (glue + `.wasm`) | `prebuild.yml` | `publish-npm` |

### 1.2 Why `plan` exists, and why it does **not** gate the build jobs

`release-plz release` is idempotent — it publishes only packages not already on the registry at
that version — so the standard two-job pattern runs it on every push to `main`. `plan` runs the
dry-run first so a non-release push does not build twelve matrix legs.

**`plan` is the only job carrying the gate directly.** Everything else is gated *transitively*
through an unbroken `needs:` chain rooted at `plan`. This is exactly the topology §8.2's verdict
reasons about, so the guard's `needs:`-walking is load-bearing on the real file, not merely
exercised by fixtures.

**A per-family optimisation was designed and then rejected — the reason is worth recording.** The
review correctly observed that a proto-only release still builds the 12-leg kernel matrix, and
proposed gating `wheels`/`prebuild` on a `kernel_release` output. **That cannot be implemented
without defeating this issue's own guard.** In GitHub Actions a job whose `needs:` dependency was
*skipped* is itself skipped; reviving it requires `if: always() && …` or `if: !cancelled() && …`,
and §8.2 **bans every status-check function** on any job on a registry-reaching `needs:` path.
The optimisation and the guard are mutually exclusive as long as the build jobs sit on that path.

**Decision: correctness over cost.** All three build jobs run whenever *anything* is releasable.
`plan`'s per-family outputs are still emitted and are consumed at **step level** inside
`publish-pypi` / `publish-npm`, where a skip does not propagate through `needs:`. A proto-only
release therefore builds kernel wheels nobody uploads — wasted minutes on a rare event, in
exchange for a guard with no `always()` hole in it.

### 1.3 `plan`'s three unmeasured premises

Revision 1 measured one and assumed two. All three are Task-1 preconditions.

**(a) Does `--dry-run` push tags? — MEASURED, NO.** Settled by reading `release_plz_core`
**0.36.14**, the version the pinned CLI `release-plz 0.3.158` depends on (`^0.36.14`, from
crates.io's dependencies API — the CLI and core are versioned separately). In
`src/command/release.rs`, `create_git_tag_and_release(...)` is reachable **only** from the `else`
arm of `if input.dry_run` — at both call sites, `release_package` (line 888) and
`release_package_git_only` (line 959). Under `--dry-run` the function logs and returns
`Ok(false)` without touching git. **R1 closes; no fallback needed.**

**(b) Does `--dry-run` succeed at all, and what does it cost? — MEASURED, and the answer is a
fourth outcome nobody anticipated.** `release-plz release --dry-run --git-token "$(gh auth
token)"` run from `rs/` against this repo: **exit 1, near-immediate (sub-2-second) failure**,
before any `cargo publish` build work starts:

```
ERROR Package `paigasus-node-bindings` has `publish = false` or `publish = []` in the Cargo.toml,
but it has `publish = true` in the release-plz configuration.
```

This is **not** the exit-101 derive-crate-ordering risk umbrella §14 Q6 recorded — that code path
was never reached. The real cause is a **pre-existing defect in the committed
`rs/release-plz.toml`**: its `[[package]]` entries for the three binding crates
(`paigasus-node-bindings`, `paigasus-wasm`, `paigasus-py-bindings`) set `release = true` without an
explicit `publish = false`, even though each crate's `Cargo.toml` sets `publish = false`. An unset
`publish` field defaults to `true` (`release-plz-0.3.158/src/config.rs:331`), and
`ReleaseRequest::check_publish_fields()` (`release_plz_core-0.36.14/release.rs:210-225`) — called
**unconditionally**, before the dry-run/live branch, from `release-plz-0.3.158/src/args/release.rs:134`
— rejects that combination. `--dry-run` does not protect against it: a live `release-plz release`
fails identically, at the identical point. This check exists only on the `release` subcommand's
path; `release-plz release-pr` never calls it, which is why the config's own comment ("`version_group`
… DOES apply to crates whose Cargo manifest says `publish = false`") — measured against
`release-pr` in an earlier issue — never caught it.

**Consequence:** this is a hard prerequisite fix (`publish = false` added to the three binding-crate
`[[package]]` entries in `rs/release-plz.toml`) that blocks `release-plz release` from running at
all, **independent of which fallback below is chosen** — the `plan`-exists shape and the
direct-`PAIGASUS_RELEASE_ENABLED`-gate shape both need it. It is out of scope for Task 1 (no
production code) and must land before whichever task first exercises `plan`/`release` for real.
**The original exit-101 derive-crate-ordering question is still genuinely unmeasured** — the
config defect fires first and prevents ever reaching that code path. Re-measure it once the
`publish = false` fix lands, before trusting `plan`'s `--dry-run` step in production. See
`docs/superpowers/specs/2026-08-28-sma-579-measurements.md` M1 for the full source trace.

**(c) `release-plz release` requires a git token EVEN under `--dry-run`. — MEASURED, YES.**
`release()` calls `get_git_client(input)?` **unconditionally** at `src/command/release.rs:543`,
before `should_release`; it hard-errors `git release not configured. Did you specify git-token and
forge?` when `input.git_release` is `None`. This is **not** suppressed by `git_release_enable =
false` — tried at both `[workspace]` and `[[package]]` scope, still errored, exit status 1
(measured **unpiped**; a first attempt read `tail`'s status through a pipe and wrongly recorded
0). **Consequence: `plan` must mint a GitHub App installation token exactly as `release-pr` does.**
Revision 1 did not budget this step.

### 1.4 `concurrency:`

`release.yml:13-15` currently holds `concurrency: { group: release-pr, cancel-in-progress: false }`
at the **workflow** level. A multi-leg matrix under that group would serialize every subsequent
push to `main` behind a full wheel build. **Design:** move the existing group to the `release-pr`
job; give the release path its own job-level group. Neither cancels in progress.

Two semantics to verify rather than assume (§12 Q4):

- **`cancel-in-progress: false` does not mean "never cancel".** GitHub still cancels a previously
  **pending** run in the group when a third arrives. Three rapid pushes to `main` therefore cancel
  the middle release path. `release-plz release` is idempotent so the outcome is probably benign,
  but the current rationale does not cover it and the behaviour must be recorded.
- **`concurrency:` on a job that uses `uses:`** — the allowed-key set for reusable-workflow caller
  jobs is narrower than for normal jobs, and `wheels`/`prebuild` are such jobs. Verify the key is
  accepted at all, not merely that its group does not collide.

---

## 2. The tagging boundary — **release-plz owns every tag**

`prebuild.yml:244-245` assigns this decision here in as many words. `napi prepublish` defaults
`ghRelease: true` and cuts a GitHub release plus a lerna-style tag
(`@paigasus/node-bindings@0.1.0`). release-plz also cuts tags. Two tools tagging one repo is
precisely the ADR-0011 S3 failure mode — *"the tool owns every tag"*, singular — and the SMA-385
trap that motivated it.

**Decision.** release-plz owns every tag. `napi prepublish` runs `--no-gh-release` in the live
path, exactly as in `prebuild.yml`'s dry-run. `@paigasus/wasm` publishes with a plain
`npm publish`, which never tags.

> `napi prepublish --help` lists only `--gh-release` (opt-in) and **not** `--no-gh-release`.
> `prebuild.yml:241-243` already records why: the negation exists and is required, and this "is
> not visible from `--help` — the Task-1 spike misread this; CI confirmed." **Do not "correct"
> the invocation on the strength of `--help`.**

**`git_tag_name` stays unset**, i.e. the default `<package>-v<version>` — confirmed from
`release-plz release --help` at the pinned 0.3.158. A group-collapsing name such as `v{{version}}`
is **not** an option: all four kernel-family packages share one version and would collide.

A release commit cuts up to six tags at the same commit (`paigasus-kernel-v…`,
`-py-bindings-v…`, `-node-bindings-v…`, `-wasm-v…`, `-proto-v…`, `-proto-derive-v…`). Four are
redundant by construction. Accepted deliberately: the alternative is a collision.

### 2.1 GitHub Releases — release-plz makes exactly two

Revision 1 settled who cuts *tags* and never said whether release-plz also creates *GitHub
Releases*. It does, by default, per released package — which would mean six release pages per
release commit.

**Decision:** `git_release_enable = false` for every package except the two family heads,
`paigasus-kernel` and `paigasus-proto`, which each get one carrying that family's changelog. Two
pages per release commit, each meaningful.

The exact TOML key spelling (`git_release_enable`, per-package) **MEASURED**: confirmed exactly as
spelled, at `release-plz-0.3.158/src/config.rs:414`, a field of `pub struct PackageConfig` (line
390) whose doc comment states it is "Configuration that can be specified both at the `[workspace]`
and at the `[[package]]` level." Both `Workspace` and `PackageSpecificConfig` (the `[[package]]`
shape) flatten this same `PackageConfig`, so `git_release_enable` is valid at **both** scopes — the
per-package placement this spec assumes is confirmed correct. Note that disabling it does **not**
remove the token requirement (§1.3c).

**This decision is enforced, not merely documented.** §8.2's guard fails if any `napi prepublish`
invocation lacks `--no-gh-release`.

---

## 3. `release` — crates.io and tags

Runs `release-plz release` from `rs/` (config and manifest discovery is CWD-relative with no
upward search).

`--output json`'s schema, **measured** from `release_plz_core` 0.36.14:

```rust
pub struct Release        { releases: Vec<PackageRelease> }
pub struct PackageRelease { package_name: String, prs: Vec<Pr>, tag: String, version: Version }
```

So `{"releases": [{"package_name": …, "prs": […], "tag": …, "version": …}, …]}`. **The array key
is `releases` and the field is `package_name`** — *not* `prs`/`package`, which is `release-pr`'s
different shape. `release()` returns `Option<Release>` and yields `None` when nothing is
releasable; **MEASURED (source-confirmed, §5.3): the CLI prints `{"releases":[]}`** for that case
— `Release` derives `Default`, and `main.rs` does `.unwrap_or_default()` on the `Option`, so
`None` becomes `Release { releases: vec![] }`, matching `release-pr`'s `{"prs":[]}` precedent.

### 3.1 Credentials for the tag push

Every `actions/checkout` in this repo sets `persist-credentials: false` (artipacked hardening:
`release.yml:102`, `wheels.yml:128`, `prebuild.yml:82`), so there are **no ambient git
credentials**. `release-plz release` pushes tags and creates the two GitHub Releases of §2.1.

**Decision:** `release` mints its own GitHub App installation token, mirroring `release-pr`'s step
(`release.yml:151`), and passes it as `GIT_TOKEN`. Tokens are per-job and live one hour; each job
mints its own. `plan` does the same, for the reason in §1.3c.

Revision 1 showed only `contents: write` and `CARGO_REGISTRY_TOKEN` and would have failed on the
tag push **after** the crates.io upload — a half-completed irreversible step, the one outcome
§1.1 exists to prevent.

---

## 4. Credentials

**All credentials live in `release.yml` and nowhere else** (umbrella §7 review **M2**).
`prebuild.yml` and `wheels.yml` both carry a `pull_request` trigger, and same-repo PRs receive
repository secrets — so a contributor with push access could exfiltrate a registry token in a PR
that never merges.

**Two independent mechanisms enforce this, and it is worth naming both:**

1. **The call site.** Secrets are **not** passed automatically to a called workflow; the caller
   must use `secrets:` or `secrets: inherit`. `release.yml` uses neither, so `wheels.yml` and
   `prebuild.yml` receive no secrets regardless of what they declare.
2. **The callee's declaration**, asserted by `repo:publish-metadata`'s P-D6.

> **Constraint for every future editor of `release.yml`:** it must never gain a `pull_request` or
> `pull_request_target` trigger. It genuinely reads secrets, so it would red the credential gate —
> correctly. Do not add such a trigger, and do not reach for that gate's allowlist to silence it.

**R7 dependency, stated honestly and corrected 2026-08-28.** Revision 1 said SMA-593 *widens*
`repo:publish-metadata`'s P-D6 arm to every workflow with a `pull_request` trigger. **That is no
longer what SMA-593 does.** Confirmed directly with the session implementing it: P-D6 is being
**deleted** from `ci/publish-metadata/run.sh` — `assert_wheels_has_no_credentials`,
`strip_comments`, the `PATTERNS` table and its six fixture rows — and re-founded as a **new narrow
gate, `repo:workflow-credentials`**. The reason is cost: widening `repo:publish-metadata`'s
`inputs` to every workflow would make each `ci.yml` edit pay for a `cargo publish --dry-run` on a
required check.

Two consequences for this issue:

- The credential assertion protecting `release.yml` will live in `repo:workflow-credentials`, not
  in `repo:publish-metadata`. Any comment this issue writes must name the new gate.
- `.github/workflows/wheels.yml` is being **removed** from `repo:publish-metadata`'s `inputs`
  (`moon.yml:548`), since P-D6 was its only reader — so
  `SELF_TASK_EXPECTED_GLOBS["publish-metadata"]` **shrinks**. This issue changes neither.

**Neither gate is on `main` yet.** Until one lands, nothing reds when a `pull_request` trigger is
added to `release.yml`. **Fallback, unchanged:** if SMA-593 has not landed when this issue is
ready, §8's guard adds the trigger assertion itself — it already parses `release.yml`'s `on:`
block, so the marginal cost is one fixture row.

### 4.1 Environments — **two, not one**

Revision 1 put `environment: release` on all three publishing jobs, so that required reviewers
could later be added by settings (umbrella §9 **M12**). **The review showed that breaks §1.1.**
If reviewers are added, GitHub pauses *each* job entering the environment: `release` → approve →
crates.io published and tags cut → `publish-pypi` pauses again, with a 30-day default timeout. A
rejected or expired second approval leaves crates.io published and PyPI empty, permanently — the
exact split state B3 removed.

**Design: separate the approval from the OIDC claim.**

| Environment | Used by | Purpose |
|---|---|---|
| `release-approval` | a no-op `approve-release` job that `release` depends on | the single place to add required reviewers |
| `release-publish` | `release`, `publish-pypi`, `publish-npm` | scopes the OIDC claim only; **never** gets reviewers |

One approval gates the whole irreversible stage. **SMA-580 must add reviewers to
`release-approval` and never to `release-publish`** — recorded here so the wrong one is not
chosen.

### 4.2 crates.io — OIDC, not a stored token

release-plz 0.3.158 authenticates with `CARGO_REGISTRY_TOKEN` and has no native OIDC support.

```yaml
permissions:
  id-token: write     # crates.io OIDC exchange
  contents: write     # release-plz pushes tags
steps:
  - uses: rust-lang/crates-io-auth-action@c6f97d42243bad5fab37ca0427f495c86d5b1a18  # v1.0.5
    id: cratesio
  - uses: actions/create-github-app-token@…                  # §3.1
    id: app_token
  - run: release-plz release
    env:
      CARGO_REGISTRY_TOKEN: ${{ steps.cratesio.outputs.token }}
      GIT_TOKEN:            ${{ steps.app_token.outputs.token }}
```

Resolved: output name is `token`; the action's post-step revokes it at job end. **There is no
moving `v1` tag ref** (`git/ref/tags/v1` 404s), so the README's `@v1` snippet has no resolvable
ref in this repo's SHA-pinning style — hence the versioned pin above.

### 4.3 PyPI — trusted publishing

OIDC, `id-token: write`. **The claim binds to the *calling* workflow's filename**, so SMA-580
registers the pending publisher against **`release.yml`**, not `wheels.yml`. Easy to get wrong;
fails only at first publish.

### 4.4 npm — **measure before accepting a long-lived token**

Revision 1 asserted `NPM_TOKEN` was unavoidable. That was reasoned from memory, in the one
decision that creates the highest-value long-lived secret in the repo — against §4's whole
argument.

**npm shipped Trusted Publishing (OIDC, workflow-filename-bound, like PyPI's), which removes the
token and enables provenance implicitly.** Whether it applies here is a **Task-1 measurement**:

1. Does npm Trusted Publishing cover a **scoped org package** (`@paigasus/*`)?
2. Does `napi prepublish` publish through the npm CLI in a way that picks it up?

If both hold, there is no `NPM_TOKEN` at all. If either fails, `NPM_TOKEN` stays — **and §4.5
records that it was rejected for a measured reason, not merely unconsidered.**

**MEASURED: both (1) and (2) hold, but `NPM_TOKEN` stays anyway, for a reason neither question
asked.**

1. **YES.** npm Trusted Publishing (GA since 2025-07-31) explicitly covers "all npm private and
   public packages, both scoped and unscoped." Requires npm ≥ 11.5.1 / Node ≥ 22.14.0; this repo's
   pinned toolchain (Node 24.16.0, npm 11.11.0) clears that floor.
2. **YES, structurally.** `napi prepublish` (measured against the installed `@napi-rs/cli@3.7.2`)
   shells out with `execSync(\`${npmClient} publish\`, { cwd: pkgDir, env: process.env, stdio:
   "pipe" })`, `npmClient` defaulting to `"npm"` — a real `npm publish` invocation per per-platform
   package directory, inheriting the full, unfiltered parent environment. Exactly the shape OIDC
   auto-detection needs.

**The decisive fact, found while researching (1), not asked by either sub-question:** npm Trusted
Publishing **cannot be configured for a package that does not yet exist on the registry** — the
npmjs.com UI used to register a Trusted Publisher requires the package to already exist
(`npm/cli#8544`, open and unresolved as of 2026 — the issue itself contrasts this with PyPI's
"pending publisher," which supports pre-registration, the exact mechanism §4.5's PyPI row already
relies on). **Every npm package this issue would publish is a first-ever publish** —
`@paigasus/node-bindings`, its seven per-platform packages, and `@paigasus/wasm` have never been on
the npm registry. Trusted Publishing structurally cannot cover any of their first releases.

**Decision: `NPM_TOKEN` stays**, as a bootstrap credential for the first publish of each package.
Once every package exists on the registry, a **later, separate** change could migrate subsequent
releases to Trusted Publishing — nothing else blocks it. See
`docs/superpowers/specs/2026-08-28-sma-579-measurements.md` M4 for sources, including a 2026-05-20
npm change (Trusted Publisher registration now requires explicitly selecting at least one allowed
action) relevant to that future migration, not to this decision.

### 4.5 What SMA-580's pre-flight must create

| Kind | Name | Purpose |
|---|---|---|
| Repository variable | `PAIGASUS_RELEASE_ENABLED` | the gate; `'true'` activates |
| Environment | `release-approval` | **the only** place reviewers go (§4.1) |
| Environment | `release-publish` | OIDC claim scope; never gets reviewers |
| crates.io | Trusted Publisher | repo + `release.yml` (+ environment) |
| PyPI | pending publisher ×3 | `paigasus-py-bindings`, `paigasus-kernel`, `paigasus-proto`, all against **`release.yml`**; environment field per §12 Q3 |
| npm | `NPM_TOKEN` | **MEASURED (§4.4): Trusted Publishing cannot cover a package's first-ever publish** (npmjs.com requires the package to already exist before OIDC can be registered), and every npm package here is a first publish. `NPM_TOKEN` is a bootstrap credential; a later, separate change may migrate to Trusted Publishing once every package exists. |

`PAIGASUS_BOT_APP_ID` / `PAIGASUS_BOT_PRIVATE_KEY` are unchanged and now serve `release-pr`,
`plan` and `release`.

---

## 5. `publish-pypi`

Downloads, asserts, uploads. **Builds nothing** (§1.1).

### 5.1 Upload order

**`paigasus-py-bindings` first, then `paigasus-kernel`** — the face pins `==`, so the reverse
leaves it uninstallable in the window between uploads (umbrella §3). `paigasus-proto` is
independent and may go in any position.

`wheels.yml` carries a standing comment warning that `face-paigasus-kernel` is deliberately
outside the `wheel-*` namespace, precisely so the natural implementation — one `download-artifact`
with `pattern: wheel-*` and `merge-multiple: true`, then a single upload — cannot silently violate
that ordering. **Honour it: three downloads, three uploads.**

### 5.2 Idempotency

`skip-existing: true` on every upload (review **M9**). The upload is many distributions; if a
later one fails, a retry re-uploads the earlier ones and PyPI returns 400 *"file already exists"*,
so an un-skipped retry can never succeed unaided. PyPI is delete-but-never-reuse.

### 5.3 Version binding — bind to `paigasus-kernel`, not `paigasus-py-bindings`

Review **M10** requires asserting the built wheel's version equals the version release-plz
reports, as a hard precondition of upload. Revision 1 named `paigasus-py-bindings`.

**That package is Cargo `publish = false`** (it ships as a maturin byproduct), so release-plz
uploads nothing for it and **it may not appear in `--output json` at all**. If it does not, the
assertion is unsatisfiable and the natural repair under pressure is "assert if present" — which is
vacuous, and is exactly the control-that-lies failure this repo has paid for.

**Design:** bind against **`paigasus-kernel`**'s reported version. The family is version-locked by
`version_group` and asserted by `repo:version-lockstep`, so it is the same number, and it is a
package release-plz definitely reports.

**MEASURED (source-confirmed, since the live dry-run cannot complete — see §1.3b): a Cargo
`publish = false` package can never appear in `--output json`'s `releases` array, under any
release-plz.toml configuration.** `release_packages()` builds its output only from
`Project::publishable_packages()`, which filters on Cargo's own `publish` manifest field
(`cargo_metadata::Package.publish`) — a check that runs before release-plz's own per-package
config is even consulted (`release_plz_core-0.36.14/project.rs:110-115`,
`next_ver.rs:456-469`). So `paigasus-py-bindings`, `paigasus-node-bindings`, and `paigasus-wasm`
are structurally excluded. **"Binding against both" is not an available option** — one side of
that binding can never appear — so `paigasus-kernel` is not merely the better choice, it is the
only one.

**Also measured, for Task 6's `has_releases` key:** when nothing is releasable, `--output json`
prints `{"releases":[]}` — not `null`, not empty output. Source-confirmed at both layers: `release_packages()`
returns `None` when its result vec is empty (`release.rs:613`), and the CLI does
`release_plz_core::release(&request).await?.unwrap_or_default()` (`main.rs:63-65`) — `Release`
derives `Default` as `{ releases: vec![] }`, matching the `release-pr`/`{"prs":[]}` precedent
exactly.

### 5.4 `py/packages/paigasus-proto` — **published here**

SMA-578 review **M8** required this be decided rather than left to omission. It is version-locked
with the **proto** family, its name is reserved on PyPI, and no publish path uploads it. Left
unowned, every proto-family release burns a PyPI version that is never uploaded, so the Python
`paigasus-proto` permanently trails crates.io and can never be published at a matching version.

**Decision: publish it in this issue.**

**The condition, spelled out because the name means two different things.** release-plz only ever
sees the **Rust crate** `paigasus-proto`. The thing uploaded is the **Python distribution** built
from `py/packages/paigasus-proto`, which release-plz knows nothing about. The condition is
therefore:

> If `release-plz release --output json` reports a release for the **Rust crate**
> `paigasus-proto`, upload the **Python distribution** built from `py/packages/paigasus-proto` at
> the version `repo:version-lockstep` stamped there.

Written loosely, one engineer keys on a name that can never appear (silently uploading nothing)
and another drops the condition (uploading on every run).

**This requires editing `ci/publish-metadata/run.sh`** — see §9.

---

## 6. `publish-npm` — and npm's missing idempotency

Downloads, asserts, publishes. **Builds nothing** (§1.1).

`napi prepublish` publishes **eight** packages (seven platform + the main one) and **npm has no
`--skip-existing`**. A re-run after a partial success hits `403 You cannot publish over the
previously published versions` on the ones that landed. §5.2's own argument — *"an un-skipped
retry can never succeed unaided"* — applies verbatim to npm and revision 1 did not make it there.

`publish-pypi` and `publish-npm` also run **in parallel** off `needs: release`, so partial failure
across three registries is the normal shape, not the exceptional one.

**Design:** before each publish, check `npm view <pkg>@<version> version` and skip if present,
giving npm the `skip-existing` semantics it lacks. §11 carries the per-registry recovery
procedure, including npm's 72-hour unpublish window.

---

## 7. npm activation

### 7.1 Package metadata

Both packages are `private: true`, which `npm publish` refuses.

- **`@paigasus/node-bindings`** — drop `private: true`. Everything else is already correct.
- **`@paigasus/wasm`** — drop `private: true`; add `publishConfig.access: public` (a scoped
  package without it publishes **restricted**); add `description`, `repository`, `homepage`,
  `keywords`.

### 7.2 `wasm-pack` destroys the `package.json` it is about to publish

`rs/crates/bindings/paigasus-wasm/.gitignore:4-10` records the measured behaviour:

> `wasm-pack build` **CLEANS its `--out-dir` each run** (overwrites `.gitignore` with a bare `*`,
> **DELETES `package.json`**, even with `--no-pack`).

Revision 1's step list said `wasm-pack build (paigasus-wasm)`. Following that literally destroys
every bit of §7.1's metadata moments before publishing. Best case the publish hard-fails; worse,
wasm-pack regenerates a `package.json` **without** `publishConfig.access: public` and the scoped
package publishes **restricted, irreversibly, at that version**.

**The repo already has the pattern** (`ts/packages/paigasus-kernel/moon.yml:43`): build into a
gitignored scratch dir, copy only `paigasus_wasm*` back, never build into the crate root. The
`build` task owns `.wasmpack-out` and `test` owns `.wasmpack-test-out`, so the release path needs
a **third** name to avoid racing either.

**Design:** the wasm build moves into `prebuild.yml` (also satisfying §1.1), builds into
`.wasmpack-release-out`, and uploads a `wasm-dist` artifact containing the glue plus the `.wasm`.
`publish-npm` downloads it over a clean checkout. The crate-root `package.json` is never at risk
because nothing in the publish path runs `wasm-pack`.

### 7.3 `prebuild.yml` → reusable

`prebuild.yml` has `workflow_dispatch`, `push` and `pull_request` but **no `workflow_call`**. Add
it. It stays credential-free (§4).

It also gains the wasm build (§7.2) and uploads the assembled `npm/` dirs, so the publish stage
only downloads. Its `permissions: contents: read` comment — *"SMA-407 adds publish creds at
activation"* — must be corrected: creds are **not** added there, by design.

**`prebuild.yml` has no SPDX header** (line 1 is `name: prebuild`), unlike `release.yml` and
`wheels.yml`. The file is being edited anyway; add it.

### 7.4 The publish steps

```
download prebuild-* / npm-dirs / wasm-dist        (built in the reversible stage)
napi prepublish --no-gh-release --npm-dir npm --cwd $CRATE     ← §2's invariant
npm publish --provenance   (@paigasus/wasm, from wasm-dist)
```

### 7.5 Assert the tarball, not the working tree

Revision 1 said "assert the `.wasm` exists and is non-empty" — which checks the *disk*, not what
`npm publish` will ship. `prebuild.yml:250-262` already carries the correct idiom: `npm pack
--dry-run --json`, parse `files`, assert membership. Use it. This is the same "a tag is not a
binary" rule §7.2 cites, applied to the artifact that actually leaves the machine.

### 7.6 Provenance on the platform packages

`napi prepublish` exposes no `--provenance` flag, so the seven platform packages would publish
without provenance while `@paigasus/wasm` gets it. **Likely remedy:** `NPM_CONFIG_PROVENANCE=true`
in the job env, which napi's internal `npm publish` calls inherit.

**MEASURED against `@napi-rs/cli@3.7.2` (the installed, pinned version): it works.** `napi
prepublish`'s internal call is `execSync(\`${npmClient} publish\`, { cwd: pkgDir, env: process.env,
stdio: "pipe" })` — `process.env` is the full, unfiltered parent environment, and npm CLI's
standard `NPM_CONFIG_<KEY>` env-to-flag mapping treats `NPM_CONFIG_PROVENANCE=true` as equivalent
to `--provenance`. A job-level `NPM_CONFIG_PROVENANCE=true` reaches all eight `npm publish`
invocations `napi prepublish` makes (main package + seven platform packages). No asymmetry to
record — provenance applies uniformly.

Related, and a §12 question: `npm publish --provenance` requires a `repository` field matching the
building repo. `@paigasus/node-bindings/package.json:9-13` has one. **MEASURED: the seven
`napi create-npm-dirs`-generated children do too.** Ran `napi create-npm-dirs --cwd
rs/crates/bindings/paigasus-node-bindings` (napi-rs/cli 3.7.2): all seven generated `package.json`
files (`darwin-arm64`, `darwin-x64`, `linux-arm64-gnu`, `linux-arm64-musl`, `linux-x64-gnu`,
`linux-x64-musl`, `win32-x64-msvc`) carry
`"repository": {"type":"git","url":"git+https://github.com/SMK1085/paigasus-core.git","directory":"rs/crates/bindings/paigasus-node-bindings"}`,
matching the building repo exactly. No remediation needed here.

---

## 8. The re-founded release guard

Umbrella §9 specifies it; SMA-576 scoped it but had no job to guard; SMA-578 left it open for the
same reason. **This issue has the job, so the guard lands here.**

### 8.1 Why the obvious rubric is wrong

SMA-578 review **B4** rejected transplanting `assert_freshness_call_site`'s test. That rubric
guards **a check that must be able to report red**; this guard must **prevent execution**, and its
bypasses differ:

- `publish-pypi` is gated only *transitively*. An added `if: always()` un-gates the upload while
  the pinned `release` guard stays byte-identical and green.
- `continue-on-error: true` on `release` does not suppress a red — it makes a **failed** release
  job count as success for `needs:`, so a failed crates.io publish still lets wheels reach PyPI.
- The verdict must find a **job-level** `if:` in a file carrying **eight** identical step-level
  ones (`if: steps.preflight.outputs.configured == 'true'`) and **zero** job-level ones.

> **Correction.** SMA-578 §9.2 says "seven step-level ones (`release.yml:45,63,77,81,85,106,125`)"
> and revision 1 of this spec repeated it verbatim. **Both the count and every line number are
> wrong** — SMA-589's App-token work shifted the file. Measured: eight, at 72, 87, 105, 119, 123,
> 127, 148, 167. The cited numbers now point at a shell `if`, four comments, `client-id:`, a
> `uses:` and a blank line. **This spec deliberately states the shape, not the numbers**, because
> line numbers rot on the next edit — which is precisely what happened.

### 8.2 The verdict — inverted subject set

Revision 1 defined the subject set by **detecting** registry-reaching jobs. The review showed
detection ∪ pin does not close the class it claims: a new job publishing by an unrecognised
mechanism (`JS-DevTools/npm-publish`, `cargo publish` rather than `release-plz release`, a local
composite action) is simply *not detected*, so the detected set still equals the pin, strict
equality holds, and the gate is green. That is this repo's own "looks complete while leaving a
simpler hole open" standard, applied to the central strengthening.

**Design: invert it.** The subject is every job, and the exemption is the pin.

> **(V1)** Every job in `release.yml` is gated on `PAIGASUS_RELEASE_ENABLED` — directly, or
> through an unbroken `needs:` chain from a gated job — **except** members of
> `UNGATED_JOBS = {"release-pr"}`.
>
> **(V2)** The gating expression equals a **pinned literal string**, accepted in both its bare and
> `${{ }}`-wrapped forms and no others.
>
> **(V3)** No **status-check function** (`always`, `cancelled`, `success`, `failure`) appears in
> the `if:` of any job on a gated path.
>
> **(V4)** No `continue-on-error:` other than literal false appears on any such job **or on any
> step of one**.
>
> **(V5)** No `napi prepublish` invocation omits `--no-gh-release`.

V1 is strictly stronger than detection, cannot rot, and reduces the pin to one direction — a new
job is gated or it is exempted by name, and there is no third outcome. V2 closes
`if: vars.PAIGASUS_RELEASE_ENABLED != 'disabled'` and
`… == 'true' || github.actor == 'x'`, which revision 1's "gated on `PAIGASUS_RELEASE_ENABLED`"
admitted. V3 replaces revision 1's two literal spellings — the real class is any status function,
and `success() || failure()`, `!failure()` and `${{ ! cancelled() }}` all evaded a two-string
test. V4 adds the step level, which is §8.1's second bullet one level down.

This follows `wheels.yml:15-18`'s standing rule: **exact equality, never a substring.**

### 8.3 Called workflows — a conservative transfer rule

Revision 1 said a publish step added to `wheels.yml` "is reachable from `release.yml` and gated."
**That is false and dangerously so.** Both files carry their own `push:` and `pull_request:`
triggers. A `twine upload` added to `wheels.yml`'s `build` job would be judged **green** — the
calling job is gated through `needs: plan` — while running on every PR and every push to `main`,
with `PAIGASUS_RELEASE_ENABLED` irrelevant. The guard would report green on precisely the ungated
publish it exists to prevent, and with a *stronger* claim than "we do not check that file."

> **(V6)** A workflow reachable by `uses: ./.github/workflows/*.yml` may contain a
> registry-reaching job **only if** its `on:` block contains `workflow_call` **and nothing else**.

Both files today would fail that test *if they ever gained a publish step*, which is the correct
verdict. Detection (a step invoking `release-plz release`, `npm publish`, `napi prepublish`,
`twine upload`, or a PyPI publish action) is retained **only here**, where V1's whitelist has no
meaning.

> **This overlaps `repo:workflow-credentials` (SMA-593) and the overlap is deliberate — do not
> delete either as redundant.** That gate's rule is **trigger-derived**: a workflow whose `on:`
> block contains `pull_request`/`pull_request_target` may declare no credentials. V6 is
> **reachability-derived**: a workflow called from `release.yml` may publish only if it is
> `workflow_call`-only. They share no predicate and catch different failures — a credential added
> to a PR-triggered workflow versus a publish step added to a workflow the release path calls. Both
> would red on `wheels.yml`/`prebuild.yml` gaining a publish step, for different reasons. A future
> reader who removes one on the grounds that the other covers it will silently reopen the half it
> did not cover.

Where a set of jobs must be named across files it is keyed by **`(workflow_file, job_id)` pairs**,
never bare strings: `wheels.yml` and `prebuild.yml` both have a job literally named `build`.

### 8.4 Implementation — real YAML, obtained through the pinned uv

SMA-578 §9.2 offered four routes on the premise that *"`repo:publish-metadata` runs under
`toolchain: 'system'`, where PyYAML is **not guaranteed** to be importable."* **That premise is
measured false:** `uv run --no-project --with 'pyyaml==6.0.3' python3 …` → **0.068s warm**. The
constraint holds only for a bare `import yaml`.

**Decision: `ci/actionlint/release_guard.py`, a real parser. Not a vendored one.**

The decisive argument is the defect class, not the timing. SMA-593 exists **because**
`ci/publish-metadata` hand-rolled a partial YAML scanner: it tracked quotes but not backslash
escapes, so `\"` closed a string early. That session measured 14 distinct bypasses. A second
hand-rolled scanner would recreate exactly the defect class SMA-593 is removing, in a guard whose
verdict depends on distinguishing job-level from step-level keys and walking `needs:` chains.

> **Precision on the YAML-alias argument, since it was overstated twice and is checkable.**
> GitHub **added anchor/alias support in September 2025**, so the alias bypass (`x: &w write` …
> `id-token: *w`) is real in a workflow GitHub will run, and a real parser resolves it.
> **Merge keys are NOT supported by GitHub Actions**, so a merge-key bypass cannot exist in a
> runnable workflow at all — it is not, as earlier discussion had it, "caught by accident." One
> real class, one impossible class. The adversarial review's counter-claim that anchors are
> unsupported is itself out of date.

**§9.1 fixes the dependency problem this creates**, which is not free.

### 8.5 Fail-closed contract

Stated explicitly because a gate that silently does nothing is worse than the regex it replaced:

| Condition | Result |
|---|---|
| `uv` not on `PATH` | **exit 2** (infra), never a skip |
| `pyyaml` unobtainable | **exit 2** |
| workflow file missing / unreadable | **exit 2** |
| YAML unparseable, or `jobs:` not a mapping | **exit 2** |
| verdict violated | **exit 1** (fail) |

Never a pass, never a skip, in any row.

### 8.6 YAML coercions — every one gets a fixture row

PyYAML is a YAML 1.1 parser and GitHub's schema collides with it. All measured:

| Source | Parses to | Consequence |
|---|---|---|
| `on:` (top-level key) | `True` (bool) | must read `doc.get("on", doc.get(True))` |
| `if: false` | `False` (bool) | a job disabled outright is not the string `"false"` |
| `continue-on-error: false` | `False` (bool) | the literal-false test must accept the **boolean** |
| `continue-on-error: "false"` | `"false"` (**str**) | GitHub treats it as false; the test must accept **both** |
| `needs: release` (scalar) | `str`, not `list` | **iterating it yields characters** and silently walks nothing |

Measured on `release.yml`: top-level keys are `['name', True, 'concurrency', 'permissions',
'jobs']`, `'on' in doc` **False**, `True in doc` **True**.

The `needs:` row is the most dangerous: it makes the **transitive half of V1 quietly vacuous**,
and that half is load-bearing because §1.2 makes every job but `plan` transitively gated.

Also required, each with a fixture: `!!str`-tagged scalars; multi-document files
(`yaml.safe_load_all`); the closed set of accepted `if:` forms (bare, `${{ }}`-wrapped, folded
`>-` with varying whitespace); and an explicit verdict for a job-level `if: false` on a
registry-reaching job — **more restrictive than the gate, so it passes** (it cannot cause a
publish), recorded so it is not read as an oversight.

### 8.7 Guard-the-guard obligations

Per the repo's doctrine (*"That script cannot assert its own invocation"*), this is a **new**
verdict function against a **new** file:

1. A new `release_guard_self_test` table driving the verdict through pass and fail fixtures — one
   per bypass in §8.1–8.3, one per coercion in §8.6.
2. **`SELF_TEST_COUNT` 10 → 11.** SMA-578 says "9 → 10"; **that is stale** — SMA-572 already added
   `affected_smoke_block_self_test` as the tenth. Check 9 asserts invocations **and** definitions
   (`run.sh:4044` counts bash `*_self_test` definitions; `:4079-4090` derives the battery by awk
   over `run_self_tests`' body), so both must move together.
3. A whole-line `ACTIONLINT_SH_CALL_SITES` entry in `ci/affected-graph/ci_targets.py`, **at column
   0** (review **N5**) — that haystack matches at column 0 deliberately, so a call site nested in
   a function or an `if` cannot satisfy it.
4. The battery grows from eleven subprocesses to **twelve** (ten mutants + one unmutated becomes
   eleven + one). `moon.yml:635` phrases it as subprocesses; keep that phrasing. The measured cost
   tables in `ci/actionlint/README.md` and `moon.yml` must be **re-measured, not estimated** — and
   via interleaved A/B sweeps, since sequential min-of-N is invalid on this shared host.
5. **NEW — an arity floor on the Python fixture table.** This obligation exists because the verdict
   is Python and the machinery is bash. Check 9 only ever sees bash functions, so a **single** bash
   `release_guard_self_test` wrapping a Python `--self-test` means **emptying the Python table is
   invisible to every existing check** — the gate passes having asserted nothing. That is the same
   hole check 8e's two arity floors (`ci_targets.py:585,597`) exist to close, and it was **measured**
   there: with the array replaced by `()`, the verdict emitted zero lines against a fully-wired
   file. Add an `ACTIONLINT_SH_CALL_SITES` floor entry of the form
   `[ "${#RELEASE_GUARD_FIXTURES[@]}" -ge N ] || infra …`.

   The alternative — N separate bash fixture rows — was **rejected on cost**: each spawns a
   `uv run`, and at 8–20 rows × 12 concurrent `--self-test` mutants that is 96–240 `uv run`
   invocations per gate run, which is not what §11's R6 sizes.

### 8.8 What the guard still does not protect

Unchanged from umbrella §9 **M12**: the guard asserts the `if:` exists and is not defeated. The
*decision* remains a repository variable a maintainer can flip in the UI. §4.1's
`release-approval` environment is where that gap can be closed later, by settings rather than code.

---

## 9. CI bookkeeping

- **No new `repo:*` gate.** This extends `repo:actionlint`, already in `ci.yml`'s `T=(…)` array —
  so **no `T` change and no CLAUDE.md marker edit**, which also avoids conflicting with two
  concurrent sessions in `ci_targets.py`.
- `repo:actionlint` already declares `inputs: ['**/*']`, pinned by
  `SELF_TASK_EXPECTED_GLOBS["actionlint"]`, so `ci/actionlint/release_guard.py` is covered with no
  input change, and `repo:input-liveness` is satisfied for the same reason.
- **`ci/publish-metadata/run.sh` IS edited by this issue**, contrary to revision 1's §8. §5.4's
  decision cannot be implemented otherwise: `EXPECTED_PYPI_PUBLISHABLE` at **line 119** is
  strict-equality (Check P0), and the discovery scan is runtime-based, so adding
  `[tool.paigasus] pypi = true` to `py/packages/paigasus-proto/pyproject.toml` reds the gate until
  that array moves. The file's own comment at lines 116-118 says **"SMA-579 owns that decision"** —
  it was always this issue's edit. It is a **one-line array change**.

  **The marker and the array entry must land in the SAME COMMIT.** Check P0 compares a runtime
  discovery scan against the array by strict equality, so either edit alone reds
  `repo:publish-metadata`. Splitting them across two branches would red `main` for whichever landed
  first. That is what makes the line structurally this issue's rather than a courtesy hand-off.
  §9.2 records the boundary.
- CLAUDE.md gains: the tagging boundary and the two-GitHub-Release decision; the `--dry-run`
  measurements (no tags, but a token still required); the YAML 1.1 coercions; the wasm-pack
  clobber as it applies to the release path; and the "never add a `pull_request` trigger to
  `release.yml`" constraint.

### 9.1 `pyyaml` must be locked and scanned

`uv run --with 'pyyaml==6.0.3'` as written adds an **unlocked, unscanned, network-dependent** input
to a required-check gate. `pyyaml` would appear in no lockfile, so `repo:osv` and dependabot cannot
see it. The 0.068s figure is **warm on one host**; the cold path is unmeasured.

**And the cold path is not the rare case — it is every run.** `ci.yml:163-169` keys the uv cache on
`uv-${{ runner.os }}-${{ hashFiles('py/uv.lock') }}`. Under the `--with` form `py/uv.lock` never
changes, so that key never changes, so the restore is an **exact primary-key hit** — and
`actions/cache` **skips its save on an exact hit**. The pyyaml download therefore lands in the
cache directory and is *never persisted*: every CI run restores a cache without it, refetches it
from PyPI, and discards it again, permanently. A required check would depend on PyPI's
availability on every single run. *(Credit: this half was found by the SMA-593 session; it is the
same `actions/cache` behaviour recorded in this repo's notes as "widening what a cached job builds
without changing the key means the new output is never saved".)*

Adding `pyyaml` to `py/pyproject.toml` fixes both halves at once: it is then locked and scanned,
**and** `py/uv.lock`'s hash changes, so the cache key rotates and the save actually happens.

**Design:** add `pyyaml` as a `py/` dev dependency so it is locked by `py/uv.lock` and scanned by
`repo:osv`, and invoke through the project rather than `--with`. §8.5's fail-closed contract covers
the residual (a missing interpreter reds; it never skips).

### 9.2 Concurrency with other sessions

| Session | Issue | Shared file | Resolution |
|---|---|---|---|
| paigasus-core-2b | SMA-593 | `ci/publish-metadata/run.sh` | **Settled.** They **delete** the P-D6 block (~lines 1040-1120, 1711-1760) and re-found it as `repo:workflow-credentials`. This issue changes **only** `EXPECTED_PYPI_PUBLISHABLE` (line 119), ~900 lines clear of the nearest deletion. Agreed directly with that session. |
| paigasus-core-2b | SMA-593 | `ci/affected-graph/ci_targets.py` | they own `["publish-metadata"]`; this issue owns `SELF_SCHEDULED_GATES`, `ACTIONLINT_SH_CALL_SITES`, `["actionlint"]` |
| paigasus-core-3c | SMA-594/592/535 | `ci_targets.py`, CLAUDE.md | disjoint regions; whoever lands second rebases |

---

## 10. Testing

Nothing here can be tested by running it — the point is that it does not execute. The evidence is
structural:

1. **`release_guard_self_test`** — §8.7's fixture table.
2. **A negative control.** The guard must be observed reporting **red** on a deliberately broken
   `release.yml` (gate removed; `if: always()` added; step-level `continue-on-error: true` added;
   `--no-gh-release` dropped; a publish step added to `wheels.yml`), and green on the real one. A
   guard never observed reporting red is the control-that-lies failure this repo has paid for
   twice (SMA-542, SMA-530).
3. **`actionlint`** over the new workflow structure, including the `workflow_call` additions.
4. **`moon run repo:actionlint --force`** — `--force` matters, since check 5's branch half reads
   git ref state that is in no input hash.
5. **`repo:publish-metadata`** — §5.4 changes `EXPECTED_PYPI_PUBLISHABLE` and adds a marker.
6. **`repo:version-lockstep`** — the two `package.json` edits change no version, but the files are
   among its eighteen sites, so the edits **schedule** the gate.
7. **The JS workspace**: `pnpm --dir ts install --frozen-lockfile` (run by all three workflows) and
   **`moon run ts:fmt`**, a separate whole-tree Prettier gate.
8. **The full gate graph** before pushing, per CLAUDE.md's marker-delimited command.

### 10.1 What cannot be tested pre-merge

The publish path itself. No CI run can exercise a crates.io, PyPI or npm upload without publishing.
`--dry-run` covers part of the crates.io half; nothing covers the OIDC exchanges, which fail only
against a real registry with a real trusted-publisher registration. **SMA-580's pre-flight is the
first genuine test of §4.** Inherent, not an omission.

---

## 11. Rollback and recovery

Revision 1 had no such section; the umbrella has one and §1.1's whole rationale is about this class.

| Failure | Reversible? | Procedure |
|---|---|---|
| `plan` fails | yes | nothing happened; fix and re-push |
| a build job fails | yes | nothing irreversible has run; re-run |
| `release-plz release` fails **partway** — one crate published, next not, tags partial | **no** | crates.io cannot be unpublished (only yanked). Yank the published crate, delete the partial tags, fix, re-run: release-plz skips already-published packages, so a re-run converges |
| `publish-pypi` fails partway | **no** | `skip-existing: true` makes a re-run converge; PyPI is delete-but-never-reuse, so a bad file needs a **new version**, never a re-upload |
| `publish-npm` fails partway | **no** | §6's `npm view` pre-check makes a re-run converge; a genuinely bad publish must be unpublished **within 72 hours** or superseded by a new version |
| `release` succeeds, a publish job never runs | **no** | crates.io and tags are live while a registry is empty. Re-run the publish job; it is the reason both are independently re-runnable |

---

## 12. Open questions for the plan

1. ~~Does `release-plz release --dry-run` **succeed**, and what does it cost, with
   `paigasus-proto-derive` absent from crates.io?~~ **MEASURED, Task 1 (§1.3b) — but not this
   question.** It fails immediately for an unrelated, more urgent reason (a `release-plz.toml`
   config defect); the derive-crate ordering question remains unmeasured behind that blocker and
   must be re-measured once the config fix lands.
2. ~~Does `--output json` list Cargo `publish = false` packages?~~ **MEASURED (source-confirmed),
   Task 1 (§5.3): no, never — settling the binding target as `paigasus-kernel`, the only option.**
3. Does PyPI's pending-publisher registration need the `environment` field set? §4.1 says the
   environment appears in the OIDC claim; §4.5's PyPI row leaves it open. A mismatch fails only at
   first publish.
4. `workflow_call` semantics: does `github.workflow` resolve to caller or callee (§1.4)? Is
   `concurrency:` accepted at all on a `uses:` job? Do artifacts uploaded by a **called** workflow's
   jobs resolve for a sibling job in the **caller**? *(The last is near-certain — same run — but is
   the load-bearing assumption of §5 and §7.)*
5. ~~Does npm Trusted Publishing cover scoped org packages, and does `napi prepublish` pick it
   up?~~ **MEASURED, Task 1 (§4.4): yes to both, but `NPM_TOKEN` stays anyway** — Trusted
   Publishing cannot cover a package's first-ever publish, and every npm package here is one.
6. ~~Does `NPM_CONFIG_PROVENANCE=true` reach napi's internal `npm publish` calls?~~ **MEASURED,
   Task 1 (§7.6): yes**, via the full-environment `execSync` call napi makes.
7. ~~Do the seven `napi create-npm-dirs`-generated `package.json` files carry a `repository`
   field?~~ **MEASURED, Task 1 (§7.6): yes, all seven do**, matching the building repo.
8. ~~Is the per-package TOML key `git_release_enable`?~~ **MEASURED, Task 1 (§2.1): yes, confirmed
   exactly, and valid at both `[workspace]` and `[[package]]` scope.**
9. What happens on **"Re-run all jobs"** of an older run? `plan`/build jobs re-execute at the old
   commit while `main` has moved. §5.3's version binding catches the PyPI half; nothing catches the
   crates.io or npm half.

---

## 13. Revision 2 — what the adversarial review changed

Verdict on revision 1: **NEEDS REWORK**, six BLOCKERs.

**Accepted and folded in (all six blockers, all majors):** the called-workflow transfer rule
(§8.3); inverting the subject set (§8.2 V1); pinning the gate expression and banning all status
functions and step-level `continue-on-error` (V2–V4); the `wasm-pack` clobber (§7.2); the missing
git credential for the tag push (§3.1); the `§5.4`/`§9` contradiction about
`ci/publish-metadata/run.sh`; no builds downstream of `release` (§1.1); `plan`'s cost and
exit-101 risk (§1.3b); two environments instead of one (§4.1); npm Trusted Publishing as a
measurement (§4.4); binding the version assertion to `paigasus-kernel` (§5.3); the Rust-crate vs
Python-distribution conflation (§5.4); npm's non-idempotency (§6); `pyyaml` locked and scanned
(§9.1); the Python fixture table's arity floor (§8.7.5); the `needs:`-as-string coercion and the
rest of §8.6; SMA-593 named as an unlanded dependency with a fallback (§4); the concurrency
semantics (§1.4); the tarball-not-disk assertion (§7.5); `prebuild.yml`'s missing SPDX header
(§7.3); the JS workspace in testing (§10).

**Rejected, with reasons:**

- **"Anchors are unsupported by GitHub Actions, so §8.4's alias argument rests on a false
  premise."** The review is out of date. GitHub added anchor/alias support in September 2025; it
  is **merge keys** that remain unsupported. So the alias bypass is real and the argument stands —
  what changed is the merge-key half, corrected in §8.4.
- **Per-family gating of the build jobs (`kernel_release`).** Correct about the waste, but it
  cannot be built without `always()`/`!cancelled()`, which V3 bans. §1.2 records the tension and
  chooses the guard.

**Found independently of the review, during measurement:** `--dry-run` does not tag (§1.3a);
`release` needs a git token even under `--dry-run` (§1.3c); the `--output json` schema (§3); the
`crates-io-auth-action` pin and the absent `v1` ref (§4.2); `napi prepublish`'s `--help` omitting
`--no-gh-release` (§2); the stale step-level `if:` count (§8.1); artifacts resolving across a
`workflow_call` boundary (§1.1).
