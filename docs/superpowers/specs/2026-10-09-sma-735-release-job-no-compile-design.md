# SMA-735: no compile in the job that holds the crates.io and App credentials

- Linear: SMA-735 (related: SMA-684, SMA-602, SMA-580)
- Date: 2026-10-09
- Status: draft for GATE 1. Sven approved approach A and design sections 1 and 2 on 2026-10-09.

## 1. Problem

The `release` job in `.github/workflows/release.yml` (lines 563-628) holds three credentials:

1. `CARGO_REGISTRY_TOKEN`, the crates.io token from the OIDC exchange
   (`rust-lang/crates-io-auth-action`). It is in the `env:` of the `Release` step.
2. `GIT_TOKEN`, an App installation token with `contents: write`. It is in the same `env:`.
3. The App private key `PAIGASUS_BOT_PRIVATE_KEY`. It is on the runner for the whole job, because
   the mint step references it.

The `Release` step runs `release-plz release --output json`. For each publishable crate,
release-plz runs `cargo publish` without `--no-verify` (F1). Cargo then packages the crate,
unpacks it and compiles it, with every build script and proc macro of its dependency graph. That
code runs while the three credentials are on the runner.

SMA-684 removed every compile from the `release-pr` job for the same reason, and kept the
`release` job out of scope (SMA-684 spec D2). This spec closes that gap.

The job runs only after a person approves `approve-release`. The approval does not reduce the
risk: the person approves a release, not the third-party code in the dependency graph.

## 2. Decisions

| ID | Decision | Source |
|---|---|---|
| D1 | The isolation level is the **job**, the same as SMA-684 D1. Every step of a job can read every secret that the job references (SMA-684 spec §1). | SMA-684, kept |
| D2 | Approach **A**: the `release` job compiles nothing. A new job without credentials verifies the crates before the approval. | Sven, 2026-10-09 |
| D3 | `--no-verify` is a **CLI flag** on the `release-plz release` line, not the `publish_no_verify` key in `rs/release-plz.toml`. `release_guard.py` reads the workflow file, so it can pin the flag. The key would need a second gate on a different file. | Sven, 2026-10-09 (section 1) |
| D4 | The verify job calls a new mode of `ci/publish-metadata/run.sh`, not an inline loop. The group logic then has one copy. | Sven, 2026-10-09 (section 1) |
| D5 | Two new guards: V19 (a step allowlist for `release`) and V20 (the verify job exists, has no credentials and gates the approval). | Sven, 2026-10-09 (section 2) |
| D6 | Approach B (a separate tag job) and approach C (move only the App mint) are rejected. See §8. | Sven, 2026-10-09 |

## 3. Goal and acceptance

1. **A1.** No step of the `release` job compiles code or runs a build script or a proc macro.
2. **A2.** Every crate that `release` publishes is verified by a build before the human approval,
   on the same commit, by a job that holds no credential.
3. **A3.** A gate reds when a change removes `--no-verify`, adds a step to `release` that is
   outside the V19 allowlist, removes the verify job, removes its edge to `approve-release`, or
   gives it a credential.
4. **A4.** The release still publishes the same crates, cuts the same tags and makes the same
   GitHub releases.

A1 does **not** say that the job runs no third-party code. The job still trusts a fixed set of
tools (§4.1).

Non-goals:
- The `publish-npm` job (see §9, follow-up).
- A gate for the "no build downstream of `release`" rule in the `release.yml` header (F7).
- Any change to the App installation, its permissions or the token scopes.
- The `release-pr` job (SMA-684 owns it).

## 4. Facts this design depends on

Labels: READ (from source), MEASURED (run here or in a CI log), INFERRED.

| ID | Fact | Label |
|---|---|---|
| F1 | In `release-plz release` 0.3.158, the only step that can compile is `cargo publish` (`release_plz_core/src/command/release.rs:1154-1200`). It adds `--no-verify` when `publish_no_verify` is true for the package. The CLI flag `--no-verify` (`release_plz/src/args/release.rs:43-45`) sets that value for the workspace and every package (`config.rs:100-138`). The default is `false` (`release.rs:372-373`). No `cargo package`, `cargo build`, rustdoc or semver check runs in `release`. | READ, tag `release-plz-v0.3.158` |
| F2 | `cargo publish --dry-run --no-verify --workspace` and `cargo package --no-verify --workspace` ran no `build.rs` and no proc macro. The control, the same commands without `--no-verify`, ran both. | MEASURED, cargo 1.95.0, a two-crate fixture with a sentinel in `build.rs` and in a proc macro (2026-10-09) |
| F3 | `release-plz release` always needs a git token. It calls `get_git_client` before any other work (`release.rs:~540`, `:1110-1115`), also when tags and GitHub releases are off. Tags and releases go through the GitHub API (`forge.rs:1041-1063`, `:319-335`), one package at a time, directly after its publish. | READ |
| F4 | release-plz passes `CARGO_REGISTRY_TOKEN` to `cargo publish` and `cargo info` in the child environment (`release.rs:1189-1199`, `cargo.rs:140-151`). So a build script in the verify build sees the token in its environment, not only in the runner memory. | READ |
| F5 | `ci/publish-metadata/run.sh` Check 2 (`check_publish_group`, lines 864-910) runs `cargo publish --dry-run --locked -p <pkg>…` once per publish group. `publish_groups` (lines 702-740) computes the groups: today `{paigasus-kernel}` and `{paigasus-proto, paigasus-proto-derive}`. `classify_cargo_failure` (lines 682-696) returns 2 for a network error and 1 for a defect. `assert_check2_covered_everything` (line 919) checks that every publishable crate was in a group. The dispatch (lines 1903-1917) has no mode that runs Check 2 alone. | READ |
| F6 | Check 2 runs only on a PR whose changed files select `repo:publish-metadata`. Nothing verifies the crates on the release commit itself. | READ (`moon.yml` inputs) |
| F7 | No check in `release_guard.py` holds the rule "no job downstream of `release` may build anything" (`release.yml:105-107`). | READ |
| F8 | `release_guard.py` V18 (lines 1584-1918) holds `release-pr` to a step allowlist. It is driven by `ungated_job_violations` over `UNGATED_JOBS`, which is pinned to `{"release-pr"}` (lines 4453-4468). Its fixtures are `_SMA684_V18_CASES` with a strict count of 147 (lines 4554-4708). | READ |
| F9 | V9b requires every direct consumer of `plan` to carry `if: needs.plan.outputs.nothing_to_release != 'true'`. V3 and V4 ban status functions and `continue-on-error` on every job on a gated job's `needs:` path. V11 allows `id-token: write` only in `OIDC_PUBLISH_JOBS`. V14 requires the chain approval on the `needs:` path of every job that holds a capability. No check pins the exact `needs:` list of any job. | READ |
| F10 | `rs/rust-toolchain.toml` pins `channel = "1.95.0"`. rustup reads it for every cargo call, so the `release` job and the verify job use the same cargo. | READ |
| F11 | `ci/publish-metadata/run.sh` uses `declare -A` and needs bash 4 or later (root `CLAUDE.md`). The hosted Linux runner has bash 5. | READ |
| F12 | `semver_check = false` is already set in `rs/release-plz.toml` (SMA-684). It has no effect on `release` (F1). | READ |

### 4.1 The tools that the `release` job still trusts

After the change, the job runs these programs. Each one can read the three credentials.

| Program | Where | How it is pinned |
|---|---|---|
| `rust-lang/crates-io-auth-action` | crates.io auth | commit SHA |
| `actions/create-github-app-token` | mint step | commit SHA |
| `actions/checkout` | checkout | commit SHA |
| `moonrepo/setup-toolchain` | setup step | commit SHA |
| proto, then `release-plz` | `proto install release-plz` | proto plugin, HTTPS from GitHub, **no checksum** (`.proto/plugins/release-plz.toml`) |
| rustup, then cargo 1.95.0 | the first cargo call | `rs/rust-toolchain.toml` channel; runner image rustup |
| `git` | inside release-plz | runner image |

## 5. Design

### 5.1 `.github/workflows/release.yml`, job `release`

- The `Release` step runs `release-plz release --output json --no-verify`. The rest of the step
  stays as it is.
- `needs:` becomes `[wheels, prebuild, proto-dist, verify-crates, approve-release]`.
- A comment above the step says: the job compiles nothing; `--no-verify` is load-bearing; the
  verify build is in `verify-crates`; V19 holds the step set; §4.1 lists the trusted tools.

### 5.2 `.github/workflows/release.yml`, new job `verify-crates`

```yaml
verify-crates:
  name: verify the crates.io packages
  needs: [plan]
  if: needs.plan.outputs.nothing_to_release != 'true'
  runs-on: ubuntu-latest
  timeout-minutes: 30
  steps:
    - name: Checkout
      uses: actions/checkout@<same SHA as the other jobs>
      with:
        persist-credentials: false
    - name: Set up proto + Moon
      uses: moonrepo/setup-toolchain@<same SHA>
      with:
        cache: false
    - name: Install Moon-managed toolchains
      run: moon setup
    - name: Verify every publish group
      run: bash ci/publish-metadata/run.sh --verify-publish-groups
```

- The job has no `environment:`, no `permissions:` block (the workflow default is
  `contents: read`), no secret and no App token.
- `approve-release` changes `needs:` to `[wheels, prebuild, proto-dist, verify-crates]`. A crate
  that does not build then stops the run before the human approval, in the reversible stage.
- The plan measures which tools the new mode needs on the runner (cargo, python3, jq, bash 4). If
  `moon setup` is not needed, the plan uses a narrower install, in the style of the `plan` job.

### 5.3 `ci/publish-metadata/run.sh`: the `--verify-publish-groups` mode

- A new dispatch case `--verify-publish-groups` runs only what Check 2 needs: the enumeration of
  the publishable crates, `publish_groups`, `check_publish_group` for each group, and
  `assert_check2_covered_everything`. It does not run Checks 0, 1, 3, 4, 5 or the PyPI arm.
- Exit codes follow the script's contract: 0 when every group passes, 1 for a defect, 2 for an
  infrastructure fault (network, empty group, missing manifest dir).
- The usage line lists the new mode.
- The mode reuses the existing functions. It does not copy them. If `main()` holds the enumeration
  loop inline, the plan moves that loop into a function that both `main()` and the new mode call.

### 5.4 `ci/actionlint/release_guard.py`, V19: the `release` job allowlist

- The V18 engine (`v18_line_segments`, `v18_split`, `v18_segment_verdict`,
  `_v18_step_config_violations`) becomes table-driven. One table row holds the allowlist for one
  job: actions, command words, prefixes, step keys, env names, working directories, `with:` keys.
  V18 keeps its row for `release-pr`, with no change in what it accepts. V19 adds a row for
  `release`.
- The V19 row:
  - Actions: `rust-lang/crates-io-auth-action`, `actions/create-github-app-token`,
    `actions/checkout`, `moonrepo/setup-toolchain`. A local `./…` action is not allowed.
  - Command words: `set`, `echo`.
  - Prefixes: `proto install release-plz`, `release-plz release`.
  - Env names: `CARGO_REGISTRY_TOKEN`, `GIT_TOKEN`. Working directory: `rs`.
  - The plan derives the exact sets from the real job, and adds an entry only when the real job
    needs it.
- An extra V19 rule: every segment whose command is `release-plz release` must carry the exact
  token `--no-verify`. A token that starts with `--no-verify=` is refused. The prefix match is
  bounded, so `release-plz release-pr` does not match the `release-plz release` prefix.
- The set of jobs that V19 checks is pinned by equality to `{"release"}`, in the same style as
  the `UNGATED_JOBS` pin.
- V19 does not use the dry-run exemption.

### 5.5 `ci/actionlint/release_guard.py`, V20: the verify job

V20 reds when one of these is false:

1. A job named `verify-crates` exists.
2. `verify-crates` is on the `needs:` path of `approve-release`.
3. `verify-crates` holds no capability, by the V14 definition (`_holds_write_capability`), and has
   no `environment:`.
4. `verify-crates` has exactly one `run:` step, and that step runs exactly one command:
   `bash ci/publish-metadata/run.sh --verify-publish-groups`.

The message names the job and the failed condition, and points to this spec.

### 5.6 Tests

- **`_SMA735_V19_CASES`**, with a strict count, driven by a new self-test in the style of
  `_sma684_v18_allowlist_bites`. Red cases: no `--no-verify`; `--no-verify=false`;
  `--no-verify` only in a comment; `cargo publish`; `cargo build`; `cargo package`; `moon setup`;
  `pnpm --dir ts install`; `release-plz release --no-verify && cargo build`;
  `uses: actions/setup-node@…`; `uses: ./local-action`; an env name outside the set. Clean
  controls: the real `Release` step, `proto install release-plz`, and the real job.
- **V20 cases**: the job is missing; the `approve-release` edge is removed; the job gets
  `id-token: write`; the job gets `environment: release-publish`; the job gets an App token mint;
  the step gets a second command. Clean control: the real job.
- **V18 regression**: `_SMA684_V18_CASES` and its count stay unchanged and pass after the engine
  becomes table-driven.
- **`ci/actionlint/run.sh`**: raise the fixture floor (`-ge 170`) per its own comment, and the
  matching README row.
- **`ci/publish-metadata/run.sh --negative-control`**: a case that breaks one group (for example a
  removed `include` entry that the crate needs) must red `--verify-publish-groups` with rc 1, and
  the restored tree must pass.

### 5.7 Documentation

| File | Change |
|---|---|
| `release.yml` header | The job order becomes `{wheels, prebuild, proto-dist, verify-crates} -> approve-release -> release -> {publish-pypi, publish-npm}`. The `release` job compiles nothing. |
| `.github/CLAUDE.md` | A new entry under "Workflow credentials and release guards": `release` runs `--no-verify`, `verify-crates` verifies before the approval, V19 and V20. |
| `ci/actionlint/README.md` | V19, V20, and a new L-entry for the residuals in §7. |
| `ci/publish-metadata/README.md` | The `--verify-publish-groups` mode, and that `release.yml` calls it. |
| `docs/ops/RUNBOOK-release-activation.md` | Line 659: `verify-crates` runs before `approve-release`. The text at line 322 stays true (it is about dependency resolution, not the verify build). |
| `release_guard.py` | The `check_main` docstring lists V19 and V20. |

## 6. Issue questions, answered

1. **Can the App token mint and the tag step move to a separate job that compiles nothing?**
   Yes, but this design does not do it. After §5.1 the `release` job itself compiles nothing, so
   the App credentials are already in a job without a compile. A separate tag job would need a
   second release-plz configuration with `publish = false`. That path tags a crate also when its
   publish failed (release-plz `release_package_git_only`, READ). It adds new logic to the
   irreversible path. Its only gain is against a compromised trusted tool (§4.1), and that tool
   could still read the crates.io token in its own job.
2. **Can `cargo publish` run with `--no-verify` after an earlier job without credentials verified
   the same packages? What do we lose?** Yes (§5.1, §5.2). We lose only the build at the moment
   of the upload. The verify job builds the same tarball from the same commit with the same cargo
   (F10), and with `--locked`. The registry index can change between the verify and the upload.
   The approval can wait up to 30 days. A dependency that is yanked in that window can give a
   published crate that a consumer cannot build. That risk exists after every publish too, and the
   verify build at upload time does not remove it for later consumers.
3. **Is the App installation limited to this repository, with the minimum permissions?** Not
   measured. The run log shows only the scopes that each token requests
   (`permission-contents: write`, `permission-pull-requests: write`, and "Creating token for this
   repository"). It does not show the installation's repository selection or its full
   permission set. A user token cannot read the installation (`GET /user/installations` returns
   403, `GET /repos/…/installation` needs an App JWT). See §10 Q1.

## 7. Risks and residuals

| Risk | Effect | Control |
|---|---|---|
| A change removes `--no-verify`. | The verify build runs again with the credentials. | V19 reds. |
| A change removes `verify-crates` or its edge to `approve-release`. | A broken crate reaches the irreversible step. | V20 reds. |
| The registry index changes between the verify and the upload. | A published crate can fail for consumers. | Residual (§6 Q2). `--locked` holds the workspace lock in the verify job. |
| `--verify-publish-groups` drifts from Check 2. | The two verify paths disagree. | One copy of the functions (§5.3). |
| A compromised `release-plz` download. | It runs with all three credentials. | Residual, the same as SMA-684 §7: the proto plugin has no checksum. |
| Any tool in §4.1 is compromised. | It can read the credentials from the runner. | Residual of D1. |
| The V18/V19 splitter does not see a command inside a quoted string. | A hidden command passes V19. | Residual, already an L-entry for V18; the new L-entry extends it to V19. |
| `verify-crates` hits a transient network error. | The run stops before the approval. | rc 2 names it as infrastructure. A re-run of the failed job is safe, because nothing irreversible ran. |

## 8. Rejected approaches

- **B: A plus a separate tag job.** See §6 Q1. More new logic in the irreversible path, for a
  gain only against a compromised trusted tool.
- **C: move only the App mint to another job, keep the verify build.** `CARGO_REGISTRY_TOKEN`
  stays on the runner during the compile, and release-plz passes it to `cargo publish` in the
  child environment (F4). It does not meet A1.

## 9. Follow-up issue

Open one Linear issue with project, milestone, priority and labels set at creation. Title:
"publish-npm: pnpm install runs lifecycle scripts while the job holds id-token: write". It also
records F7: no gate holds the "no build downstream of `release`" rule.

## 10. Verification

| ID | Check | Kind |
|---|---|---|
| V-1 | `bash ci/publish-metadata/run.sh --verify-publish-groups` passes on the real tree, and reports both groups. | local and CI |
| V-2 | Deletion checks, each must red a gate: remove `--no-verify` from the `Release` step; remove `verify-crates` from the `needs:` of `approve-release`; delete the `verify-crates` job; delete the `--verify-publish-groups` dispatch case. | one time |
| V-3 | `release_guard.py --self-test` passes, including the V18 cases unchanged and the new V19 and V20 cases. | local and CI |
| V-4 | `repo:actionlint` and `repo:publish-metadata` pass under the bash that each needs (root `CLAUDE.md`). | local and CI |
| V-5 | The full gate graph from the root `CLAUDE.md` passes before the push. | local and CI |
| V-6 | The first release after the merge: the `release` log shows `cargo publish … --no-verify` and no `Compiling` line; `verify-crates` ran before `approve-release`; the tags and GitHub releases exist as before. | live, after merge |

V-6 is the only full proof. The release path is gated on `vars.PAIGASUS_RELEASE_ENABLED` and on a
push to `main`, so no PR run proves it.

## 11. Rollback and first-run failure

Rollback: revert the PR. The old job shape works again; no data format changes.

First-run failure:
- If `verify-crates` fails, the run stops before `approve-release`. Nothing irreversible ran. Fix
  forward on `main`, or re-run the job for a transient fault.
- If the `release` job fails after the change, the failure is in packaging or upload, not in a
  build. The recovery is the same as today: release-plz is idempotent per package (it skips a
  version that is already on the registry and a tag that already exists).

## 12. Open questions for Sven (GATE 1)

- **Q1.** The App installation scope (issue question 3) is not measured (§6). Can you read it on
  the settings page and give the repository selection and the permission list? If not, this spec
  records it as an open item, and the runbook gets a manual check.
- **Q2.** Do you accept the residual in §6 Q2 (a registry change between the verify and the
  upload)?
