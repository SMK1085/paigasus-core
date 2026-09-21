# paigasus-core — `.github/`

<!-- Project memory. Claude Code loads this file only when it reads a file in
this directory, so it costs nothing in a session that stays out of .github/.
The root CLAUDE.md holds the repo-wide rules and the two gate-checked blocks. -->

## release-plz

- `rs/release-plz.toml` declares releasability **per package**, never workspace-wide. A
  `[workspace] release = false` makes release-plz hard-error (`no public packages found`), and
  simply deleting it is worse: `dependencies_update = true` cascades a patch bump into every
  transitive dependent — a crate neither in the version group nor touched by the commit still
  gets bumped ("dependencies changed") — and Cargo's `publish = false` suppresses publishing but
  **not tagging**, so the first release would permanently tag most of the workspace. Per-package
  `release = false` removes a package from the proposal entirely; every non-family crate needs
  one explicitly. `paigasus-gateway` / `paigasus-iam` are versioned BY HAND (SMA-658, option
  V-a) and are at `0.1.0` as of this PR. `release = false` stays, and release-plz neither bumps
  nor tags them: `packages_to_process()` filters on Cargo's own `publish` field, and both crates
  sit in NO `version_group` (SMA-658 M7, measured on 0.3.158). Read that as the scoped claim it
  is — a `publish = false` crate INSIDE a group whose head is publishable still gets its
  `[package] version` written, which the version-lockstep entry below records as measured for
  the three kernel binding crates. M7's fixture had a group with only unpublishable members and
  produced `version groups: {}`; it did not test the mixed group, and neither result disproves
  the other. What `publish = false` always excludes is tagging and publishing.
  `env!("CARGO_PKG_VERSION")` still feeds `ServiceInfo`, and ADR-0020 skew reporting is still
  parked on that value (SMA-505 R7).
- release-plz's `release_pr()` does all its work in a **tempdir copy** (`copy_to_temp_dir`,
  measured against the pinned 0.3.158) — it never touches the local working tree or `HEAD`. This
  nearly shipped a direct push to `main`: deriving the push target with `git rev-parse
  --abbrev-ref HEAD` after `release-plz release-pr` still reads `main`, so `git push origin
  "HEAD:$BRANCH"` becomes an unreviewed push to protected `main` (the `Protect main` ruleset has
  no `pull_request` rule and a `bypass_actors` entry for admin). Always derive the branch from
  `release-plz release-pr --output json`'s `.prs[0].head_branch`; the `prs` array is empty
  whenever no release is needed — see the next entry.
- release-plz's version baseline is the **crates.io registry**, not git tags (no `git_only` is
  set in `rs/release-plz.toml`). Measured on this repo at the `0.1.0` floor: it logs `WARN
  Package 'paigasus-kernel@*.*.*' not found`, then proposes `next version is 0.1.0` — the
  manifest version, no bump. Measured live on 2026-08-28: it still OPENS a PR (`chore: release
  v0.1.0`) listing all three packages — "empty" means it proposes no version CHANGE, not that no
  PR appears — so "the release PR is the acceptance evidence" does not hold for the first run. The real hazard here is name
  squatting — release-plz performs a crates.io lookup for every workspace member name, so a
  squatted name silently becomes the comparison baseline — not a runaway version proposal.
- **Standing rule: release-plz tags only what it PUBLISHES. `release = true` keeps a `publish =
  false` crate in the version group and does NOT get it tagged.**
  release-plz owns every tag it cuts (`<package>-v<version>`, its default), but it **only tags what
  it PUBLISHES**. MEASURED on the first live release (SMA-580): three tags, not six. The three
  `publish = false` kernel-family binding crates were never mentioned in the `release` job log at
  all — not even as skipped — so `release = true` keeps a crate in the version group and does NOT
  get it tagged. `rs/release-plz.toml`'s comment claimed otherwise and is corrected. Cosmetic: those
  crates' versions come from `version_group` + `repo:version-lockstep`, neither of which reads a
  tag. `napi prepublish` always
  carries `--no-gh-release` — a flag its own `--help` does not list. Two invocations exist:
  the real publish in `release.yml`'s `publish-npm` job, with the requirement recorded in the
  comment directly above it; and the dry run in `prebuild.yml`'s `assemble` job.
  `ci/actionlint/release_guard.py`'s V5 asserts **both** carry it. That sentence used to be
  aspirational: V5 was inlined in `check_main`, which `main()` runs on `argv[0]` only, so every
  CALLED workflow — `prebuild.yml` included — got `check_called`, which had no V5 at all. It is
  now a shared helper invoked from both (SMA-579 fix round 3). Exactly two GitHub Releases land
  per release commit — the two family heads — not one per published package (SMA-579).
- `release-plz release` requires `publish = false` in `rs/release-plz.toml` for every package
  whose Cargo manifest already says so. An absent key reads as `publish = true` and hard-errors.
  This blocked `release-plz release` entirely and stayed invisible because `release-pr` never
  reaches that validation. Ten packages needed the key added. `release = false` stops the
  release-PR proposal; `publish = false` is what `release-plz release` itself checks — the two
  settings govern different phases, and neither substitutes for the other (SMA-579).
- `release-plz release --dry-run` creates **no** tags: `create_git_tag_and_release` is reachable
  only from the non-dry-run arm (`release_plz_core` 0.36.14, `release.rs:888` and `:959`) — but
  `get_git_client()` still runs unconditionally at `release.rs:543` (SMA-579 wording corrected
  SMA-603: this is not merely "requires a token" — it makes a LIVE authenticated
  `GET /repos/{owner}/{repo}/commits/{sha}/pulls` call, and dies 401 on a bad token).
  `git_release_enable = false` does not remove that call.
- **Standing rule: The reason the release job graph carries no dry-run plan stage is the
  empty-`releases` finding two entries below (SMA-603), not the derive-crate publish order
  described here.**
  The dry-run cannot pass until `paigasus-proto-derive` is published on crates.io: a per-package
  `cargo publish --dry-run` cannot resolve a workspace sibling absent from the index. This is
  true, and permanent, but it applies to the `proto` version group ONLY —
  `paigasus-kernel` is a publish group of one with no in-tree dependency, so its dry-run resolves
  fine — and it is no longer the operative reason the release job graph carries no dry-run-based
  `plan` stage; see the entry below (SMA-603) for the reason that generalizes. A live run is
  expected to work, since derive publishes before proto — which makes the first live release the
  first genuine test of that path (SMA-579).
- `release-plz release --dry-run --output json` prints `{"releases":[]}` at exit 0 **even when it
  would publish** (SMA-603, spec M6). Measured with only the `kernel` version group bumped:
  release-plz logged that it would publish `paigasus-kernel` and cut `paigasus-kernel-v0.1.1`,
  and still reported an empty array. The array records PERFORMED releases, and a dry run performs
  none — so it cannot distinguish "nothing to release" from "a release is pending", and reading
  it would silently skip every kernel-group release. `ci/release-plan/` replaces it: it decides
  on TAG EXISTENCE instead, the same thing release-plz itself short-circuits on before touching a
  registry or running cargo. This measurement (spec M6) is pinned to release-plz 0.3.158 and
  must be re-measured on a version bump. The derive-crate bullet above (spec M3) was measured
  against the same 0.3.158, but its validity turns on whether `paigasus-proto-derive` has been
  published, not on a release-plz version — do not read it as sharing this re-measure caveat.
- `release-plz release --output json` is `{"releases":[{package_name,prs,tag,version}]}` — key
  `releases`, field `package_name`. This is **not** `release-pr`'s `prs`/`package` shape. A
  package with Cargo `publish = false` never appears in `releases`, which is why any version
  assertion in `release.yml` binds to `paigasus-kernel` (SMA-579).

## Publishing, wheels and version lockstep

- All three `repo:release-parity*` tasks run `ci/release-parity/run.sh --negative-control`
  before their real run, under an explicit `set -euo pipefail` (SMA-530). Each carries its
  own control because their *ecosystem-specific* `inputs` are distinct — a PR touching only
  a `.releaserc.json` selects `-ts` alone. They are not disjoint overall: all three also
  list `ci/release-parity/**/*` and `.prototools`, so an edit there schedules all three. Two pins guard it, both living in `ci/affected-graph/ci_targets.py`
  and both running inside `repo:affected-smoke`: `SELF_SCHEDULED_GATES` pins every
  self-scheduled gate's `moon.yml` invocation lines — `set -euo pipefail` included — for
  `input-liveness`, the three `release-parity*` tasks, `version-lockstep`,
  `publish-metadata`, `error-code-single-site`, `affected-smoke`, `actionlint`, (SMA-587)
  `http-extractor-envelope` and (SMA-593) `workflow-credentials` (whole
  lines, compared after stripping — reordering a flag or adding a trailing comment still
  reds it; a bare number here would only rot again as the registry grows, which is why this
  names its current membership instead), and `RELEASE_PARITY_SH_CALL_SITES` pins five discrete
  lines inside `run.sh` itself — the flag parse, the `NEGATIVE` guard, the assertion body,
  and both report arms — because pinning the span as one block left two MEASURED bypasses
  with different failure shapes: neutering the flag parse (dropping `NEGATIVE=1`) leaves
  `NEGATIVE` at its initialized 0, so the control branch is never entered and the invocation
  falls through to the real suite, which then just runs twice and proves nothing; gutting
  the assertion body (replacing the `check_case` call with a bare `ec=1`) never calls the
  harness at all yet still prints "reported red as expected" — a control that actively lies
  rather than one that merely no-ops. Those are the two bypasses closed by pinning five lines
  instead of one span, not an exhaustive list — see `ci/release-parity/README.md`'s
  Limitations section L5 for a residual (an inserted `NEGATIVE=0` before the guard, or all
  five lines parked in a never-executed heredoc) that survives all five pins, and why closing
  it generally is out of scope. That second pin is reachable only because
  `repo:affected-smoke` lists `ci/release-parity/**/*` in its `inputs` — do not remove it. A
  script-pinned gate needs either a `SELF_TASK_EXPECTED_GLOBS` entry or a reasoned
  `SELF_TASK_GLOBS_EXEMPT` one. Note a `moon.yml`-only edit does NOT select the
  `release-parity*` tasks (their own `script:` is not among their inputs), so a PR changing
  those blocks should also touch `ci/release-parity/**` if it wants CI to execute them.
  `repo:affected-smoke`'s OWN `moon.yml` block is the one member of that registry pinned
  twice over (SMA-572/SMA-573): its invocation lines are pinned here as above, but its
  `inputs` are deliberately NOT — those, and its invocation lines' ORDER, are pinned instead
  by check 8e in `ci/actionlint/run.sh`, a gate scheduled independently of
  `repo:affected-smoke` itself, since a pin living inside `ci_targets.py` would make that gate
  the sole judge of its own reachability; `repo:actionlint`'s own `inputs: ['**/*']` — the
  premise check 8e (and 8/8b/8c/8d) runs on every PR at all — is pinned the ordinary way, from
  `SELF_TASK_EXPECTED_GLOBS["actionlint"]` in `ci_targets.py`.
- The kernel family (`paigasus-kernel` + the three binding crates + their `pyproject.toml` /
  `package.json` faces) carries **one version** across eighteen sites, asserted by
  `repo:version-lockstep` (`ci/version-lockstep/run.sh`). release-plz owns every Cargo
  `[package] version` — via per-package `version_group` — **and** the `[workspace.dependencies]`
  version requirements; both were measured against the pinned 0.3.158, as was the fact that
  `version_group` applies to crates whose Cargo manifest says `publish = false`. The script owns
  the six sites Cargo cannot reach (`--write`) and checks all eighteen, because a `version_group`
  that silently stopped applying would otherwise go unnoticed. Two of the sites drift SILENTLY
  without it: `py/uv.lock` (its `moon.yml` runs bare `uv sync`, not `--locked`) and the 26
  `bindingPackageVersion` guards in the committed napi glue (the codegen-drift gate covers only
  the three `**/generated` proto dirs). `repo:version-lockstep` is script-pinned the same way the
  `release-parity*` tasks are — `SELF_SCHEDULED_GATES` pins its **four** `moon.yml` lines
  (`--self-test`, `--negative-control`, the real run, and `set -euo pipefail`; one more than the
  `release-parity*` tasks, which have no self-test invocation) — and takes the
  `SELF_TASK_EXPECTED_GLOBS` route through the
  pairing rule above, listing all sixteen of its literal `inputs`, so it needs no
  `SELF_TASK_GLOBS_EXEMPT` entry (holding both would itself be reported).
- Any crate flipping `publish = true` must carry **its own `[lints.*]` table** and **its own
  `include` allowlist** — enforced by `repo:publish-metadata` Checks 1c/1d (SMA-577). Cargo
  inlines the resolved lint table into the published manifest and docs.rs builds published
  crates as the root package on nightly, where `--cap-lints allow` does NOT apply, so an
  inherited `warnings = "deny"` silently kills docs.rs builds on the first new rustc lint —
  months after the PR. 1d's membership is LITERAL: `include = ["**/*"]` is rejected, since it
  would "cover" README.md/LICENSE while reinstating the `moon.yml` leak Check 2b catches.
  Check 2 runs one `cargo publish --dry-run` per **publish group** (a connected component of
  the in-set dependency graph), NOT per package: a per-package dry-run of `paigasus-proto`
  exits 101 (`no matching package named 'paigasus-proto-derive'`) until the derive crate is on
  crates.io, while `-p paigasus-proto-derive -p paigasus-proto` exits 0. That combined form is
  registry-faithful, not a workspace shortcut — measured by breaking the derive crate's
  `include` and watching the run fail. Grouping keeps `paigasus-kernel` in a group of one so
  it retains its standalone assertion.
- `paigasus-py-bindings` ships to PyPI as **`cp312-abi3` wheels (six matrix legs, seven wheels)
  plus a source-verified sdist**, built by `.github/workflows/wheels.yml` (SMA-578) — a
  *reusable* workflow (`on: workflow_call`) that SMA-579's gated `release` job will consume. It
  must **never** declare `secrets:` or `id-token: write`: it carries a `pull_request` trigger, so
  a same-repo PR would receive the credential — `repo:workflow-credentials` asserts this, and it
  applies the same ban to EVERY `pull_request`/`pull_request_target`-triggered workflow, not to
  `wheels.yml` alone (SMA-593; it was `repo:publish-metadata`'s P-D6 until then). Four facts
  that cost a measurement each: (1) maturin injects the apple-darwin `-undefined dynamic_lookup`
  args **itself**, so an sdist builds on macOS without `rs/.cargo/config.toml` — that file exists
  for plain `cargo build`, as its own comment says, and the old "no sdist" rule rested on a false
  premise (measured on ONE host / maturin 1.9.6 / one target, natively — which is why the sdist
  is verified on three platforms rather than trusted); (2) maturin builds the sdist from `cargo
  package --list`, so the crate's **Cargo** `include` allowlist is what keeps `moon.yml` out —
  `[tool.maturin] include` is not needed, and Checks 1c/1d/2b/2c never reach this crate because it
  is `publish = false`, so the only assertion holding that allowlist honest lives in `wheels.yml`;
  (3) the sdist ships the **workspace** `Cargo.toml` verbatim, `[workspace.lints.rust] warnings =
  "deny"` included, and a consumer builds as the ROOT package where `--cap-lints allow` does NOT
  apply — so every sdist-shipped crate needs its own non-denying `[lints.rust]` table, the
  Check-1c rule extended past `publish = true`; (4) `pyo3`'s `abi3-py312` means one wheel per
  (OS, arch) covers CPython 3.12+, so the matrix never multiplies by Python version. maturin also
  relocates `pyproject.toml` to the sdist **root**, not the crate dir, so the sdist content
  assertions match on basename.
- All four **Linux** wheel legs cross-compile with `--zig` — not only the musl ones, unlike
  `prebuild.yml`. `ubuntu-latest` ships glibc 2.39, so a *native* build tags `manylinux_2_39`,
  which almost nothing can install. The floor comes from **`--zig` together with
  `--compatibility`, both passed as FLAGS** (maturin's own `--help`: "`--zig` … Default to
  manylinux2014/manylinux_2_17 if you do not specify a `--compatibility`"). It does **not** come
  from cargo-zigbuild's decorated triple: maturin hands `--target` straight to `cargo metadata`,
  so `x86_64-unknown-linux-gnu.2.17` dies with `could not find specification for target` —
  measured, it failed both manylinux legs on this workflow's first CI run while the other ten
  jobs passed.
  Pass `--compatibility` explicitly so maturin's auditwheel **errors** instead of silently
  emitting a PyPI-rejected `linux_*` tag, and set `-C target-feature=-crt-static` on musl (the
  target defaults to a static CRT a cdylib cannot use). A wheel's **tag is not its binary**:
  assert the compressed tag *set* (split on `.` — `manylinux_2_17_x86_64.manylinux2014_x86_64` is
  ONE platform FIELD carrying TWO tags, so a cardinality check that counts fields as tags is
  wrong), and separately assert the binary via `otool -l`'s minimum-macOS on darwin and a
  max-`GLIBC_` symbol check on manylinux. An ELF-class check proves only the machine type and
  passes for a wheel that fails at import. **Only the `aarch64-apple-darwin` wheel and the macOS
  sdist path have been built locally.** The macOS / Windows / manylinux / musllinux tag sets, the
  `macosx_10_12` minimum-macOS value have all now been **MEASURED green on CI**. The GLIBC floor
  is per-arch and the two values legitimately differ: x86_64 tops out at **`GLIBC_2.14`** (its
  base is `GLIBC_2.2.5`; 2.14 is `memcpy`'s versioned symbol) while aarch64 reaches
  **`GLIBC_2.17`** — do not harmonise them. x86_64 was pinned at 2.17 on the first run and the
  assertion red with *"needs only [GLIBC_2.14] … safe, but re-pin"*, which is the intended
  behaviour: a wheel needing LESS than its `manylinux_2_17` tag promises is correct, since the
  tag declares a minimum platform. When one of these reds, read what the tool produced, confirm
  it is correct, and re-pin the constant — never loosen the comparison to an inequality.

## Workflow credentials and release guards

- `release.yml` authenticates with a **GitHub App installation token minted per run**
  (`actions/create-github-app-token`), never a stored secret: an installation token lives one
  hour, so it CANNOT be a repository secret, and the original `RELEASE_PLZ_TOKEN` shape could
  only ever have held a long-lived PAT (SMA-589). Three traps. The secret `PAIGASUS_BOT_APP_ID`
  holds the App's **Client ID**, not the numeric App ID — the NAME is the only stale thing about
  it, so do not "correct" it by storing the numeric id. The token must request
  `permission-contents: write` + `permission-pull-requests: write` **explicitly**: without the
  `permission-*` inputs it inherits every permission the installation holds, so granting the App
  an unrelated scope later silently widens it (zizmor `github-app`) — and because the requested
  set must actually be granted, an under-granted App reds at mint time rather than half-working
  later. And the preflight makes the whole job skip **green** when the App id is absent, so a
  broken token path is invisible in CI: the only proof is a real run on `main`.
- `release_guard.py`'s `UNGATED_JOBS` exempts a job from the GATING rule (V1) and from nothing
  else. V7 applies the publish detector to every member, because the exemption's premise is that
  the job cannot reach a registry — and `release-pr`, the only member, runs on every push to
  `main` with a `contents: write` App token. Measured before V7 existed: a `release-pr` job whose
  steps ran `cargo publish`, `npm publish` and `pypa/gh-action-pypi-publish` passed the guard at
  **exit 0**, while the same steps in any other job correctly exited 1. The detector's markers are
  REGEXES, not substrings, and the reason is one entry: `release-plz release` is a strict prefix
  of `release-plz release-pr`, which is exactly what the real job runs — a substring test reds the
  real repository, so the marker is bounded with `(?![-\w])` (SMA-579).
  V7 is NOT the last check: the roster has since grown through V8 to V12 (SMA-602), so read
  `ci/actionlint/release_guard.py`'s `^# V` comments rather than this entry alone.
- `release.yml` must never gain a `pull_request` or `pull_request_target` trigger (SMA-579).
- Each service image releases through **its own chain** in `release.yml`: `images-build-<svc>` →
  `approve-images-<svc>` → `publish-images-<svc>` → `tag-<svc>`, for `iam` and `gateway`. The
  chains are independent of the kernel chain and of each other, so a kernel-only release, an
  image-only release and a combined release all work, and a failed image chain does not stop the
  kernel release. `release_guard.py` V8 asserts that a publisher's job depends, in the job graph,
  on the approval job of ITS OWN chain — a kernel approval job never gates an image push job. All
  three approval jobs (`approve-release`, `approve-images-iam`, `approve-images-gateway`) share the
  one `release-approval` environment, though. GitHub approves a pending deployment by environment,
  not by job, so one human approval releases every chain that waits for approval in the same run.
  This comes from the shape of GitHub's approval API; it has not yet been observed on a live run.
  V14 asserts the same job-graph rule for the CAPABILITY (`packages: write`, `id-token: write`,
  `attestations: write`, the `release-images` or `release-publish` environment, an App token with
  `contents: write`), so a publish with a tool no marker names still reds. V13 allows
  `DOCKERHUB_TOKEN` only in a job whose environment is `release-images`, compares environment names
  case-folded, and fails closed on an `environment:` built from an expression.
- **The first digest published under `:<version>` is final** (D10). A later run adopts it and
  discards its own build. A rebuild never reproduces a digest, because `chisel cut` resolves the
  live Ubuntu archive on every build, so "push the same digest again" is not available as a
  recovery. `:<major>.<minor>` and `:latest` move only forward, compared as numbers. `:<major>`
  moves the same way, but only once the service leaves `0.x` — a `0.x` release writes no
  `:<major>` tag at all.
- A service version is set **by hand**, in a normal pull request, with a `CHANGELOG.md` section.
  The two service crates are `publish = false` and sit in no `version_group`, so `release-plz
  update` never sees them, and `git_only` hard-errors on the second release because each has an
  unpublished workspace dependency (MEASURED, SMA-658 M7). See the release-plz entry above for
  the bound on that claim: a `publish = false` crate inside a group with a publishable head IS
  version-written. `ci/release-plan/release_plan.py
  --assert`, which `repo:actionlint` check 11 runs on every pull request, fails when a bumped
  service has no changelog section.
