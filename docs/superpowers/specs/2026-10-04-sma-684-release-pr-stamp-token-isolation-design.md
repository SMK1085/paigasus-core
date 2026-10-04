# SMA-684: no compile in the job that holds the release-PR App credentials

- Linear: SMA-684 (related: SMA-680, SMA-667, SMA-685)
- Date: 2026-10-04
- Status: draft, revised after the spec challenge, waits for approval (GATE 1)
- Replaces: the 2026-09-27 draft. That file was in a session scratchpad and is lost. Its Linear
  comment is the only record. This spec does not depend on it.

## 1. Problem

The `release-pr` job in `.github/workflows/release.yml` holds two credentials:

1. **The App private key** `PAIGASUS_BOT_PRIVATE_KEY`, an environment secret of the
   `release-pr` environment. The mint step uses it (line 270). It is long-lived. With it, a person
   can mint installation tokens with every permission that the App installation holds, offline,
   until someone rotates the key.
2. **The minted installation token**, with `contents: write` and `pull-requests: write`. It is
   valid for one hour.

The step "Stamp the lockstep manifests onto the release PR branch" (line 355) puts the token in
its `env:` as `GH_TOKEN_FOR_PUSH`. The step runs `bash ci/version-lockstep/run.sh --write`.
`run_write` (`run.sh:988-1015`) runs `pnpm --filter @paigasus/kernel exec napi build`. That
command compiles the napi crate, the kernel, and every build script and proc macro in their
dependency graph. Each of these programs gets the token in its environment.

The same job runs `pnpm --dir ts install --frozen-lockfile` (line 316) before the token steps.

**The key is the larger stake.** The runner receives every secret that a job references when the
job starts. On a hosted runner, a step can use `sudo` to read the memory of the `Runner.Worker`
process. CVE-2025-30066 (tj-actions/changed-files) took secrets this way from steps that did not
use them (INFERRED from that incident; not measured here). So code that runs in **any** step of
this job can take the key, not only code in a step that has the token in `env:`.

## 2. Decisions

| ID | Decision | Source |
|---|---|---|
| D1 | The isolation level is the **job**, not the step. | Sven, 2026-10-04 |
| D2 | The `release` job (`cargo publish` compiles with `CARGO_REGISTRY_TOKEN` and `GIT_TOKEN`) is **out of scope**. A follow-up Linear issue records it. | Sven, 2026-10-04 |
| D3 | Approach **D**: the job compiles nothing. Approach C (a three-job split) is the fallback. | Sven, 2026-10-04 |
| D4 | V18 is an **allowlist** for the `release-pr` job, not a denylist. | spec challenge; was a denylist in the design that Sven approved in Section 2 |
| D5 | Exit codes of the new writer: E3 (the file is inconsistent) and lowering exit as "the repo is wrong". The format and internal checks (E1, E2, E4, E5, E6, a bad version) exit 2. | spec challenge; follows the `run.sh:7` contract and the `cargo-package` arm |
| D6 | `moon setup` stays. A narrower install (for example `proto install uv` plus rustup) would duplicate the toolchain pins that `.moon/toolchains.yml` holds. It would also remove only downloads, and each download is already pinned by proto. A separate issue can change this. | coordinator; Sven can overrule at GATE 1 |

**Why job level (D1).** All steps of one job run on one runner. A step without a token that runs
compiled code can change state that a later step uses: `.git/hooks`, `$GITHUB_ENV`,
`$GITHUB_PATH`, `~/.gitconfig`, or a binary under `~/.proto`. A later step with the token then
runs code that the attacker controls. It can also read the private key from the runner process
(§1). A split inside one job (the fix that the issue proposes) meets the text of the acceptance
criteria, but it does not stop either attack.

## 3. Goal and acceptance

1. **A1.** No step of the `release-pr` job compiles code, or runs a build script, a proc macro, or
   an npm or pip lifecycle or install script. This includes the issue's criterion: no step that
   compiles code holds an App token.
2. **A2.** The release PR is still stamped and pushed. The stamped `index.js` is byte-identical to
   the output of `napi build` for the stamped version.
3. **A3.** A gate reds when a change adds a step to `release-pr` that is outside the V18
   allowlist, or adds `napi`, `pnpm` or `node` back to `run_write`, or removes the napi-glue
   writer from the write path.

A1 does **not** say that the job runs no third-party code. The job still runs a fixed set of
tools (§4.1). The job trusts them with the key and the token.

Non-goals: the `release` job (D2); narrowing `moon setup` (D6); any change to the token scope or
the App installation.

## 4. Facts this design depends on

Labels: READ (from source), MEASURED (run here or in a CI log), INFERRED.

| ID | Fact | Label |
|---|---|---|
| F1 | In `--write`, only `napi build` compiles. The other processes are `python3` text edits (`stamp_sites`), `cargo update -w`, and `uv lock`. No `moon run` and no repo module import. | READ, `run.sh:756-1015` |
| F2 | The only committed output of `napi build` that `--write` needs is the version in `rs/crates/bindings/paigasus-node-bindings/index.js`. The file has 27 guards (26 native, 1 WASI at `index.js:662-666`). Each guard has two literals: `bindingPackageVersion !== '<v>'` and `expected <v> but got`. The file has 54 occurrences of the version string, so 2 per guard and no other. `index.d.ts` has no version. `package.json` is read by napi, not written. | READ and MEASURED (grep), `index.js:92-93` |
| F3 | `committed-napi-glue.test.ts` (SMA-667, `paigasus-kernel-ts:test`) requires the committed `index.js` and `index.d.ts` to equal fresh `napi build` output byte for byte. CI runs it on the release PR. | READ, `committed-napi-glue.test.ts:187` |
| F4 | `release-plz release-pr` 0.3.158 calls only `cargo metadata`, `cargo update`, `cargo package --list` and `cargo info`. Its one verifying `cargo package` call (`next_ver.rs:169`) is on the `git_only` path, and the live config sets no `git_only`. | READ, release-plz tag `release-plz-v0.3.158` |
| F5 | `release-plz` runs `cargo-semver-checks` (which builds rustdoc) for library crates when the binary is installed. `semver_check` is not set in `rs/release-plz.toml`, so the default (on) applies. No repo file installs the binary. | READ (`updater.rs:867-876`); absence on the runner INFERRED |
| F6 | `cargo update -w` runs no build script. | MEASURED: a crate whose `build.rs` writes a sentinel; after a version bump and `cargo update -w --offline` the sentinel did not exist |
| F7 | `uv lock` in `py/` runs no build backend today. Every member, and the maturin crate `paigasus-py-bindings`, declares a static `version` and static dependencies. No locked package is sdist-only. | MEASURED: `uv lock -v` with a fresh `UV_CACHE_DIR`, on an unchanged tree and on a tree with the versions bumped; zero `build`/`maturin`/`backend` lines; uv logged "Found static `requires-dist`" |
| F8 | `moon setup` installs toolchains only. | MEASURED: run 37113088606 (2026-10-03), job "release PR": the summary lists `SetupProto` and five `SetupToolchain` actions and nothing else; the log has no `.venv`, `node_modules` or `uv sync` line |
| F9 | Nothing else in the job needs node, pnpm or `ts/node_modules`. Without `pnpm install`, `git commit` still works: the `prepare` script installs hooks only when `lefthook` is on PATH (`ts/package.json:15`), and both lefthook hooks exit 0 for a `[bot]` email (`lefthook.yml:14-16, 30-32`). | READ |
| F10 | `release_guard.py` has no assertion on the stamp step, its name, its `env:`, `GH_TOKEN_FOR_PUSH` or `pnpm install`. `UNGATED_JOBS == {"release-pr"}` is pinned by equality (line 4106). V7 runs the publish detector on `release-pr`. V14 skips `UNGATED_JOBS`. The last check is V17. `command_segments` (line 603) splits a `run:` line on `&&`, `\|\|`, `;`, `\|` and strips a `#` comment. It does not parse quotes. | READ |
| F11 | `stamp_sites` writes only the kinds `pyproject`, `pyproject-dep`, `packagejson` and `cargo-package` (`run.sh:761-769`, `*) continue`). It reads the target version from the head `Cargo.toml` that release-plz wrote, for every row (`run.sh:772`). | READ |
| F12 | The `cargo-package` arm refuses a version that is not plain `X.Y.Z` (`plain()`, `run.sh:934-938`) and refuses to lower a version with rc 3, which `stamp_sites` maps to rc 1 (`run.sh:922-925`). | READ |

### 4.1 The tools that the job still trusts

After the change, the job runs these programs. Each one can read the key (§1).

| Program | Where | How it is pinned | Holds the token in `env:` |
|---|---|---|---|
| `actions/create-github-app-token` | mint step | commit SHA | (makes it) |
| `actions/checkout` | checkout | commit SHA | no |
| `moonrepo/setup-toolchain` | setup step | commit SHA | no |
| proto, then `release-plz` | `proto install release-plz` | release-plz: proto plugin, HTTPS from GitHub, **no checksum** (`.proto/plugins/release-plz.toml:6-8`) | no |
| node, pnpm, rust, python, uv | `moon setup` | proto, versions pinned in `.moon/toolchains.yml` | no |
| `release-plz` | `Open or update the release PR` | as above | `GIT_TOKEN` |
| `git`, `jq`, `python3`, `cargo`, `uv` | stamp step | runner image (`git`, `jq`); proto (`cargo`, `uv`, `python3`) | `GH_TOKEN_FOR_PUSH` |

## 5. Design

### 5.1 `ci/version-lockstep/run.sh`: the napi-glue writer

**Write path.**
- Add `napi-glue` to the `stamp_sites` filter (`run.sh:762`), so that `stamp_sites` passes row 36
  (`index.js`) to `write_site`.
- Add a `napi-glue)` arm to `write_site`. Correct the comment at line 984: the napi glue is no
  longer a file that a tool owns.
- Remove the `pnpm … napi build` call and its comment from `run_write` (lines 1000-1007).
  `cargo update -w` and `uv lock` stay (F6, F7).

**The arm.** It is a `python3` heredoc that edits text only, the same style as the other arms. It
receives the target version of the `kernel` group from `stamp_sites` (F11). It reads the file as
UTF-8 and does all checks **in memory, before it writes**. It writes in place (open for writing,
not a rename), because a rename breaks the pnpm hard link (ts/CLAUDE.md).

The two forms, with the version as the only capture group:

```
G1 = bindingPackageVersion !== '(<ver>)'
G2 = expected (<ver>) but got
```

`<ver>` is `[0-9]+\.[0-9]+\.[0-9]+`. A version that goes into a regex is always passed through
`re.escape`.

Checks, in order:

| ID | Check | On failure |
|---|---|---|
| V | The target version is plain `X.Y.Z` (the same rule as `plain()`, F12). | rc 2 |
| E1 | G1 has at least one match. | rc 2: the napi format changed |
| E2 | G1 and G2 have the same count. | rc 2: the napi format changed |
| E6 | The old version string occurs in the file exactly 2 × (G1 count) times. | rc 2: napi added or removed a version-bearing literal |
| E3 | All G1 and G2 captures are one value (`<old>`). | rc 1: the repo is wrong |
| L | `<old>` is not higher than the target. | rc 3, which `stamp_sites` maps to rc 1 (the `cargo-package` convention, F12) |
| E4 | In the new text, every G1 and G2 capture equals the target. | rc 2: writer defect |
| E5 | Mask check: replace every G1 and G2 capture by a fixed mask in the old text and in the new text. The two results are equal. | rc 2: the edit changed a byte outside the literals |

E5 uses masks, not a reverse edit on positions. A reverse edit shares code with the forward edit,
and its positions move when the version length changes (`0.9.9` to `0.10.0`).

When `<old>` equals the target, the arm writes nothing and prints `0`. Otherwise it writes the
file and prints `1`, the same as the other arms.

**Comments.** Correct "napi regenerates 26 … guards" (line 223): 27 guards, written by the text
writer, read back here. Correct `run.sh:80` (the `SELF_TEST_COUNT` comment) and `run.sh:993-995`
(rows 16-20, "three derived files" becomes two).

### 5.2 `ci/version-lockstep/run.sh`: static uv metadata check

F7 holds only while uv can read every local package's metadata without a build. A
`dynamic = ["version"]` or `dynamic = ["dependencies"]` in one of them makes `uv lock` run the
build backend. For `paigasus-py-bindings` that backend is maturin, which compiles Rust in the
token job.

The existing `pyproject` reader does not cover this. It exits 2 on a missing `version`
(`run.sh:114-124`), but only for the three `pyproject.toml` files in SITES. `paigasus-ml` and
`paigasus-workflows` are uv workspace members (`py/pyproject.toml:2`, `members = ["packages/*"]`)
and are not in SITES.

So `--check` gets a new assertion: no uv workspace member, and no path source in any
`[tool.uv.sources]`, lists `version`, `dependencies` or `optional-dependencies` in
`[project].dynamic`. A violation exits 1 and names the file and the field.

### 5.3 `ci/version-lockstep/run.sh`: changed-path check after `--write`

`run_write` records the paths that `git status --porcelain` reports before it starts. After it
finishes, every path that is new in that list must be in the write set: the SITES paths,
`rs/Cargo.lock` and `py/uv.lock`. Otherwise it exits 2 and names the paths. The stamp step runs
`git add -A` (release.yml:395), so this check is the boundary of what the job commits. It is the
same control that fallback C (§8) plans.

### 5.4 `.github/workflows/release.yml`, job `release-pr`

- Remove the step `Install JS workspace deps` (lines 314-316).
- Update the `Checkout` comment (lines 279-290), which lists `pnpm install`.
- Update the comment above `Open or update the release PR` (lines 318-322): "nine sites" becomes
  ten, "three derived files" becomes two, and `--write` no longer builds.
- Add a comment on the stamp step: it compiles nothing; V18 holds the job's step set; §4.1 lists
  the trusted tools.
- The stamp step keeps `GH_TOKEN_FOR_PUSH`.

### 5.5 `rs/release-plz.toml`

Add `semver_check = false` in `[workspace]`, with a comment. Reason: F5. Today
`cargo-semver-checks` does not run only because the runner does not have it. The key makes that a
property of the config. If a semver check is wanted later, it must run in a job without
credentials. The release-parity harness copies only `features_always_increment_minor` from this
file (`ci/release-parity/ecosystems/release-plz.sh:84-97`), so the new key does not affect it.

### 5.6 `ci/actionlint/release_guard.py`, new check V18 (allowlist)

For every job in `UNGATED_JOBS` (today only `release-pr`), every step must match the allowlist:

- **`uses:` steps.** The action path (the part before `@`, at any ref) is one of
  `actions/create-github-app-token`, `actions/checkout`, `moonrepo/setup-toolchain`. A local
  `./…` action is not allowed.
- **`run:` steps.** V18 splits each line with `command_segments` (F10). It removes leading
  variable assignments and a leading `$(` from each segment. Then the first command word of the
  segment must be in this set: `set`, `echo`, `printf`, `if`, `then`, `else`, `fi`, `[`, `exit`,
  `jq`, `git`. The following are allowed as exact command prefixes: `proto install release-plz`,
  `moon setup`, `release-plz release-pr`, `bash ci/version-lockstep/run.sh --write`.

V18 does not use the dry-run exemption (`_dry_run_exempts`, release_guard.py:669-729). The message
names the job, the step and the segment, and points to this spec.

The implementation plan derives the exact allowlist from the real job and adds an entry only when
the real job needs it. The real `release.yml` must pass V18.

**Residuals (recorded in `ci/actionlint/README.md` as a new L-entry, next to L20/L21):**
- `command_segments` does not parse quotes (F10). A command inside a quoted string is not seen.
- V18 cannot see inside an allowed program. `bash ci/version-lockstep/run.sh --write` is guarded
  by §5.7, `moon setup` by F8 (one measurement, not a gate).

**Tests.** One fixture case per rejected shape, driven straight into the V18 detector, the same
way as `_SMA658_MARKER_CASES` (release_guard.py:4154-4186). The rejected cases include
`pnpm --dir ts install --frozen-lockfile`, `napi build`, `node x.js`, `cargo build`, `cargo b`,
`cargo +1.95.0 build`, `cargo publish --dry-run`, `uvx foo`, `make`, `bash other.sh`,
`bash ci/version-lockstep/run.sh --write && bash other.sh`, `uses: actions/setup-node@…`,
`uses: PyO3/maturin-action@…`, `uses: pnpm/action-setup@…`, `uses: ./local-action`, and
`cargo update -w` as a direct step (the job does not run it directly; it runs inside
`run.sh`). Clean controls, which must pass: `moon setup`, `proto install release-plz`,
`release-plz release-pr --output json`, `git push "$AUTH_REMOTE" "HEAD:$BRANCH"`, and the real
job. Re-set the fixture floor in
`ci/actionlint/run.sh:4615` (`-ge 162`) per its own comment at lines 4595-4598.

### 5.7 `run.sh` self-tests

`SELF_TEST_COUNT` goes from 4 to 6.

**`napi_glue_writer_self_test` (new).** It calls `write_site napi-glue` on temporary fixture
files:
1. A normal bump: all 54 literals hold the new version, and nothing else changed.
2. A bump that changes the length (`0.9.9` to `0.10.0`).
3. All literals already current: prints `0`, the file is unchanged.
4. A `<new>` string that already exists in the file outside the two forms: it is left alone.
5. A decoy `<old>` string outside a guard: E6, rc 2.
6. The WASI guard shape (no env condition, "WASI binding … mismatch").
7. E1, E2, E3, E4-shape, L and a non-plain target version: each gives its rc from §5.1, and the
   file is unchanged.

The same test holds the **call-site pins**:
- `write_site` has a `napi-glue)` arm.
- The `stamp_sites` filter case names `napi-glue`.
- The body of `run_write`, with comment lines removed, does not name `pnpm`, `napi` or a `node`
  command word. A bare `\bnode\b` is not used, because it matches `paigasus-node-bindings`.

The body is read with `sed -n '/^run_write() {$/,/^}$/p'` into a variable and matched with
`grep -E … < <(printf '%s' "$body")`. Two repo rules apply: `ci/actionlint/run.sh` check 13 bans
`… | grep -q` and `… | head` in tracked `*.sh` files, and a here-string over 512 bytes deadlocks
Homebrew bash 5.3.15 on the development Mac (ci/CLAUDE.md).

**`stamp_sites_self_test` (extended).** Add `napi-glue` to the sentinel loop (`run.sh:458`) and
read it back at 9.9.9 in the kernel loop (`run.sh:513`). This runs the writer on the staged real
`index.js` through the real write path. Because `index.js` is an input of `repo:version-lockstep`
(`ci/affected-graph/ci_targets.py:254`), a napi bump PR runs this test. So E1, E2 and E6 red on
that PR, before a release.

**`uv_static_metadata_self_test` (new).** Fixtures for §5.2: a member with
`dynamic = ["version"]`, one with `dynamic = ["dependencies"]`, a path source with
`dynamic = ["optional-dependencies"]` (each rc 1), and a clean tree (rc 0).

The changed-path check (§5.3) gets a case in `stamp_sites_self_test` or the negative control: a
write that also touches an extra file must exit 2.

### 5.8 Documentation

| File | Change |
|---|---|
| `.github/CLAUDE.md` | The `dependencies_update` entry (the stamp step "builds"); the version-lockstep entries: "`--write` owns nine sites" becomes ten; 26 guards becomes 27; the napi glue is written by the text writer. |
| `ts/CLAUDE.md:141-153` | `--write` no longer runs `napi build` in place and no longer replaces files by rename. Remove the `rm -rf ts/node_modules` instruction after `--write`. |
| `rs/release-plz.toml:75` | The `dependencies_update` comment. |
| `ci/version-lockstep/README.md` | Lines 21-22 (26 guards), 48 (nine sites, three derived files), L2 (`SELF_TEST_COUNT=4`, "napi-glue … no fixture"), 172-177 (the `pnpm --filter` note is obsolete); add §5.2 and §5.3. |
| `ts/packages/paigasus-kernel/moon.yml:25-27, 269-272` | "two writers only" and "`--write` is the only other writer": true for `index.js` only, and the writer is the text writer; `--write` no longer writes `index.d.ts`. |
| `ci/actionlint/README.md` | V18, and the new L-entry (§5.6). |
| `release_guard.py:1746` | The `check_main` docstring lists V18. |
| `release.yml` | The comments in §5.4. |

### 5.9 Follow-up issue

Open one Linear issue with project, milestone, priority and labels set at creation. Title:
"release job: cargo publish compiles while CARGO_REGISTRY_TOKEN and the App credentials are on
the runner". It also records the open question from §10 Q1 if Sven does not answer it here.

## 6. Verification

| ID | Check | Kind |
|---|---|---|
| V-1 | Equivalence: on a scratch tree, bump the kernel family version with a length change (`0.2.0` to `0.10.0`), run the new `--write`, then run `napi build --js index.fresh.js --dts index.fresh.d.ts`. `index.js` equals `index.fresh.js`, and `index.d.ts` equals `index.fresh.d.ts`, byte for byte. Record the commands, the result and the `@napi-rs/cli` version in the PR. | one time, MEASURED |
| V-2 | Deletion checks, each must red the self-tests: remove the `napi-glue` arm; remove `napi-glue` from the `stamp_sites` filter; put `napi build` back into `run_write`. | one time |
| V-3 | V18 checks: add `pnpm --dir ts install`, `napi build` and `uses: actions/setup-node` to `release-pr` one at a time. `release_guard.py` reds for each. These are also fixture cases in CI (§5.6). | one time and CI |
| V-4 | `repo:version-lockstep` (`--self-test`, `--negative-control`, the real check) and `repo:actionlint` pass. Run them under the bash that each needs (root `CLAUDE.md`). | CI and local |
| V-5 | The full gate graph from the root `CLAUDE.md` passes before the push. | local and CI |
| V-6 | The first release PR after the merge is stamped, pushed and green, including `paigasus-kernel-ts:test` and `repo:version-lockstep`. | live, after merge |

V-6 is the only full proof. The job skips green without the App secrets, so no PR run proves the
job.

## 7. Risks

| Risk | Effect | Control |
|---|---|---|
| A future `@napi-rs/cli` changes the guard format, or adds or removes a version literal. | The writer exits 2 (E1, E2 or E6). | `stamp_sites_self_test` runs the writer on the real `index.js`, and `repo:version-lockstep` runs on the napi bump PR (§5.7). |
| A `pyproject.toml` gets a dynamic `version` or dependency field. | `uv lock` would run maturin and compile in the token job. | The §5.2 check reds `repo:version-lockstep`. |
| `cargo-semver-checks` appears on the runner image. | rustdoc builds in the token job. | §5.5 sets `semver_check = false`. |
| A compromised `release-plz` download. | It runs with `GIT_TOKEN` and can read the key. | Residual: the proto plugin has no checksum (§4.1). Not in scope; recorded here. |
| Any tool in §4.1 is compromised. | It can read the key from the runner. | Residual of D1: this spec removes the compile and the install, not the toolchain. Recorded here and in §10 Q1. |
| `command_segments` does not parse quotes. | A command hidden in a quoted string passes V18. | Residual, recorded as an L-entry (§5.6). |

## 8. Fallback

If V-1 fails and the cause cannot be fixed in the writer, use approach C: a job without
credentials runs `--write` with `napi build` and uploads the diff; a job with the token applies
it with a path allowlist and pushes. C needs a change to `UNGATED_JOBS` and to V14.

## 9. Rollback and first-run failure

Rollback: revert the PR. The old step shape works again, because the change removes code and adds
a writer; it does not change a data format.

First-run failure: if the writer exits non-zero on the first live run, release-plz has already
opened the release PR, but the PR has no stamp. It reds on `repo:version-lockstep` and
`committed-napi-glue.test.ts`. Nobody merges it. The recovery is a fix-forward PR to `main`;
release-plz writes the release branch again on the next push to `main`.

## 10. Open questions for Sven (GATE 1)

- **Q1.** Every tool in §4.1 can read `PAIGASUS_BOT_PRIVATE_KEY` from the runner. This spec
  accepts that as a residual. Do you accept it, and should the follow-up issue also check that the
  App installation is limited to this repository with the minimum permissions?
- **Q2.** D4 to D6 were decided after you approved the two design sections. D4 changes V18 from a
  denylist to an allowlist. Do you accept D4, D5 and D6?
- **Q3.** §5.2 (the static uv metadata check) and §5.3 (the changed-path check) are additions
  from the spec challenge. Keep both in this issue, or move them to a follow-up?
