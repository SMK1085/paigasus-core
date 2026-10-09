# SMA-735: no compile in the job that holds the crates.io and App credentials

- Linear: SMA-735 (related: SMA-684, SMA-602, SMA-580)
- Date: 2026-10-09
- Status: draft for GATE 1. Sven approved approach A and design sections 1 and 2 on 2026-10-09.
  Revision 2 folds in the spec challenge (§13). Revision 3 folds in decision D8 (Sven,
  2026-10-09).

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
code runs while the three credentials are on the runner. release-plz also passes
`CARGO_REGISTRY_TOKEN` to `cargo publish` in the child environment (F4), so a build script sees
the token directly.

SMA-684 removed every compile from the `release-pr` job for the same reason, and kept the
`release` job out of scope (SMA-684 spec D2). This spec closes that gap for the `release` job.
It does not close it for `publish-npm` (§7, §9).

The job runs only after a person approves `approve-release`. The approval does not reduce the
risk: the person approves a release, not the third-party code in the dependency graph.

## 2. Decisions

| ID | Decision | Source |
|---|---|---|
| D1 | The isolation level is the **job**, the same as SMA-684 D1. Every step of a job can read every secret that the job references (SMA-684 spec §1). | SMA-684, kept |
| D2 | Approach **A**: the `release` job compiles nothing. A new job without credentials verifies the crates before the approval. | Sven, 2026-10-09 |
| D3 | `--no-verify` is a **CLI flag** on the `release-plz release` line, not the `publish_no_verify` key in `rs/release-plz.toml`. `release_guard.py` reads the workflow file, so it can pin the flag. The key would need a second gate on a different file. | Sven, 2026-10-09 (section 1) |
| D4 | The verify job calls a new mode of `ci/publish-metadata/run.sh`, not an inline loop. The group logic and the publishable-set logic then have one copy each. | Sven, 2026-10-09 (section 1) |
| D5 | New guards in `release_guard.py`: the V18 engine becomes table-driven with one row per job; V19 is the row for `release`, V20 is the row for `verify-crates` plus a job-level credential ban. | Sven, 2026-10-09 (section 2); V20 shape from the spec challenge |
| D6 | Approach B (a separate tag job) and approach C (move only the App mint) are rejected. See §8. | Sven, 2026-10-09 |
| D7 | V19 and V20 run only when the checked file is the release workflow (`RELEASE_WORKFLOW_NAME`), the same scope as V11. They are tested by direct calls, not by `FIXTURES` rows. So the `FIXTURES` count and the fixture floor in `ci/actionlint/run.sh` do not change. | spec challenge |
| D8 | `verify-crates` sets exactly `permissions: contents: read`, and V20 enforces it. This replaces the earlier rule "no `permissions:` key". V20 refuses every other value (§5.5). | Sven chose option A, 2026-10-09 (§13 addendum) |

## 3. Goal and acceptance

1. **A1.** No step of the `release` job compiles code or runs a build script or a proc macro.
2. **A2.** Every crate that `release` can publish is verified by a build before the human approval,
   by a job that holds no credential. The build uses the tree of the commit that the run checks
   out. For the merge-commit case, see §7 (row "a different commit").
3. **A3.** A gate reds when a change removes `--no-verify`, adds a step to `release` that is
   outside the V19 allowlist, renames the job away from V19, removes the verify job, removes its
   path to `approve-release`, changes its verify command, gives it a credential, or gives it a
   `permissions:` value other than exactly `contents: read`.
4. **A4.** The release still publishes the same crates, cuts the same tags and makes the same
   GitHub releases.

A1 does **not** say that the job runs no third-party code. The job still trusts a fixed set of
tools (§4.1).

Non-goals:
- The `publish-npm` job (§7, §9).
- A gate for the "no build downstream of `release`" rule in the `release.yml` header (F7).
- Any change to the App installation, its permissions or the token scopes.
- The `release-pr` job (SMA-684 owns it). V18 must not change what it accepts.

## 4. Facts this design depends on

Labels: READ (from source), MEASURED (run here or in a CI log), INFERRED.

| ID | Fact | Label |
|---|---|---|
| F1 | In `release-plz release` 0.3.158, the only step that can compile is `cargo publish` (`release_plz_core/src/command/release.rs:1154-1200`). It adds `--no-verify` when `publish_no_verify` is true for the package (`release.rs:1185-1187`). The CLI flag `--no-verify` (`release_plz/src/args/release.rs:43-45`) sets that value for the workspace and every package, and overrides every package key (`config.rs:100-138`). The default is `false` (`release.rs:372-373`). The other subprocesses are `cargo metadata --no-deps`, `cargo info` and git queries. No `cargo package`, `cargo build`, rustdoc, semver check or git-cliff call runs in `release`. | READ, tag `release-plz-v0.3.158` |
| F2 | `cargo publish --dry-run --no-verify --workspace` and `cargo package --no-verify --workspace` ran no `build.rs` and no proc macro. The control, the same commands without `--no-verify`, ran both. | MEASURED, cargo 1.95.0, a two-crate fixture with a sentinel in `build.rs` and in a proc macro (2026-10-09) |
| F3 | `release-plz release` always needs a git token. It calls `get_git_client` before any other work (`release.rs:543`, `:1110-1115`), also when tags and GitHub releases are off. Tags and releases go through the GitHub API (`forge.rs:1041-1063`, `:319-335`), one package at a time, directly after its publish. | READ |
| F4 | release-plz passes `CARGO_REGISTRY_TOKEN` to `cargo publish` and `cargo info` in the child environment (`release.rs:1189-1199`, `cargo.rs:140-151`). | READ |
| F5 | `ci/publish-metadata/run.sh` Check 2 (`check_publish_group`, lines 864-910) runs `cargo publish --dry-run --locked -p <pkg>…` once per publish group. `publish_groups` (lines 702-740) computes the groups from the names in `EXPECTED_PUBLISHABLE` only (lines 713-714): today `{paigasus-kernel}` and `{paigasus-proto, paigasus-proto-derive}`. `classify_cargo_failure` (lines 682-696) returns 2 for a network error and 1 for a defect. `assert_check2_covered_everything` (line 919) checks that every publishable crate was in a group. `check_publish_group` returns 2 when `CI` is unset and `PKG_DIR` has no entry for a package (lines 881-887). The dispatch (lines 1903-1917) reads only `$1` and has no mode that runs Check 2 alone. | READ |
| F5b | `main()` gets the publishable set from the stdout of `metadata_checks` (line 1849; printed at lines 414-415). That one Python heredoc also runs Check 0 (lines 198-213), Checks 1 and 1b (217-270), Check 3 (272-319) and Check 5 (321-405), and loads the category snapshot with today's date (line 175). The load fails when the snapshot is older than 90 days (`categories.py:199-205`). The loop that fills `PKG_DIR` also runs Checks 1c, 1d, 2b and 2c (lines 1866-1876). | READ (spec challenge) |
| F6 | Check 2 runs only on a PR whose changed files select `repo:publish-metadata`. The release PR selects it, because it changes `rs/crates/**` (`ci/affected-graph/ci_targets.py:296-311`). Nothing verifies the crates on the commit that the `release` job checks out. | READ |
| F7 | No check in `release_guard.py` holds the rule "no job downstream of `release` may build anything" (`release.yml:105-107`). | READ |
| F8 | `release_guard.py` V18 (lines 1584-1918) holds `release-pr` to a step allowlist. It is driven by `ungated_job_violations` over `UNGATED_JOBS`, which is pinned to `{"release-pr"}` (lines 4453-4468). V18 also refuses a step `shell:` (1890-1891), the job keys `container`, `services`, `defaults`, `env` and `uses` (1606, 1881-1884), and a workflow-level `defaults:` or `env:` (1873-1876). Its fixtures are `_SMA684_V18_CASES` with a strict count of 147 (lines 4554-4708); every message carries `": V18: "` (4718-4719). | READ |
| F9 | V9b requires every direct consumer of `plan` to carry `if: needs.plan.outputs.nothing_to_release != 'true'`. V3 and V4 ban status functions and `continue-on-error` on every job on a gated job's `needs:` path, but V3 reads only the job `if:` (line 2165), not a step `if:`. V11 allows `id-token: write` only in `OIDC_PUBLISH_JOBS`. V14's `_holds_write_capability` (1239-1268) ignores `contents: write` (1230-1233) and an App mint without `permission-contents: write`. No check pins the exact `needs:` list of any job. `self_test` runs every `FIXTURES` row through `check_main(doc, "fixture")` (4772-4787); the `release` job in `_OK_MAIN` has no `--no-verify` (2395-2398). | READ |
| F10 | `rs/rust-toolchain.toml` pins `channel = "1.95.0"` with `rustfmt` and `clippy`. rustup reads it for every cargo call, so the `release` job and the verify job use the same cargo. | READ |
| F11 | `ci/publish-metadata/run.sh` uses `declare -A` and needs bash 4 or later (root `CLAUDE.md`). The hosted Linux runner has bash 5. | READ |
| F12 | release-plz writes the cargo argv and cargo's stderr only at debug level (`release_plz_core/src/cargo.rs:33, 46-47`). The default level is INFO (`release_plz/src/log.rs:17-19`). So a `release` log shows no `Compiling` line, with or without `--no-verify`. | READ (spec challenge) |
| F13 | A `cargo publish --dry-run` of a version that is already on crates.io gives a warning and passes. Every PR run of Check 2 since 0.1.0 does this, because the versions on `main` are published. | READ (Check 2 history on `main`) |
| F14 | When the release PR head is on `main` (the merge-commit case), `should_release` returns `YesWithCommit(<release PR head>)`, and release-plz checks out and publishes from that commit (`release.rs:553-562, 718-747`). The repository allows squash, merge-commit and rebase merges. | READ; MEASURED (`gh api repos/SMK1085/paigasus-core`, 2026-10-09) |
| F15 | The `release-approval`, `release-publish` and `release-pr` environments each have a custom branch policy that allows only `main`. `release-approval` also has required reviewers. | MEASURED (`gh api …/environments`, 2026-10-09) |
| F16 | pnpm 11.3.0 blocks dependency build scripts unless `allowBuilds` allows them. `ts/pnpm-workspace.yaml` allows only `sharp`. | READ, `ts/pnpm-workspace.yaml:5-30` |

### 4.1 The tools that the `release` job still trusts

After the change, the job runs these programs. Each one can read the three credentials.

| Program | Where | How it is pinned |
|---|---|---|
| `rust-lang/crates-io-auth-action` | crates.io auth | commit SHA |
| `actions/create-github-app-token` | mint step | commit SHA |
| `actions/checkout` | checkout | commit SHA |
| `moonrepo/setup-toolchain` | setup step | action code by commit SHA; it downloads proto and moon at run time |
| proto, its schema WASM plugin, then `release-plz` | `proto install release-plz` | proto plugin `.proto/plugins/release-plz.toml`, HTTPS from GitHub, **no checksum** |
| rustup, then cargo 1.95.0 with `rustfmt` and `clippy` | the first cargo call | `rs/rust-toolchain.toml` channel; runner image rustup |
| `git` | inside release-plz | runner image |

Two repository files can change what cargo runs in this job, and no gate pins them:
`rs/.cargo/config.toml` (for example a `credential-provider`) and `rs/rust-toolchain.toml` (for
example a toolchain `path`). §7 records both.

## 5. Design

### 5.1 `.github/workflows/release.yml`, job `release`

- The `Release` step runs `release-plz release --output json --no-verify`. The rest of the step
  stays as it is.
- `needs:` becomes `[wheels, prebuild, proto-dist, verify-crates, approve-release]`.
- A comment above the step says: the job compiles nothing; `--no-verify` is load-bearing; the
  verify build is in `verify-crates`; V19 holds the step set; this spec's §4.1 lists the trusted
  tools.

### 5.2 `.github/workflows/release.yml`, new job `verify-crates`

```yaml
verify-crates:
  name: verify the crates.io packages
  needs: [plan]
  if: needs.plan.outputs.nothing_to_release != 'true'
  runs-on: ubuntu-latest
  permissions:
    contents: read
  timeout-minutes: 30
  steps:
    - name: Checkout
      uses: actions/checkout@<same SHA as the other jobs>
      with:
        persist-credentials: false
    # Only if the plan's measurement (below) shows that the runner's rustup and python3 are not
    # enough:
    - name: Set up proto + Moon
      uses: moonrepo/setup-toolchain@<same SHA>
      with:
        cache: false
    - name: Install Moon-managed toolchains
      run: moon setup
    - name: Verify every publish group
      run: bash ci/publish-metadata/run.sh --verify-publish-groups
```

- The job sets exactly `permissions: contents: read` (D8). It has no `environment:`, no
  `secrets` reference, no `github.token` reference and no App token. `actions/checkout` gets the
  job `GITHUB_TOKEN` as its default `token` input. With exactly `contents: read`, that token can
  only read the repository. §7 records that the token stays on the runner during the build.
- `approve-release` changes `needs:` to `[wheels, prebuild, proto-dist, verify-crates]`. A crate
  that does not build then stops the run before the human approval, in the reversible stage.
- **The plan measures**, on a hosted runner or in an `ubuntu:24.04` container: which tools the new
  mode needs (cargo through rustup and `rs/rust-toolchain.toml`, python3, bash 4 or later); whether
  `moonrepo/setup-toolchain` and `moon setup` can be dropped; and the time of a cold run of both
  groups against `timeout-minutes: 30`. The job keeps only the steps that the measurement needs,
  and the V20 row allows only those steps.

### 5.3 `ci/publish-metadata/run.sh`: the `--verify-publish-groups` mode

- **One publishable set, two callers.** Move the publishable-set logic (`is_publishable`, lines
  188-192, and the Check 0 strict equality against `EXPECTED_PUBLISHABLE`, lines 198-213) out of
  the `metadata_checks` heredoc into a function, for example `publishable_set`. `metadata_checks`
  calls it, so the normal run does not change. The new mode calls it too.
- **The new mode** `--verify-publish-groups`:
  1. `cd "$RS_DIR"`.
  2. Run `cargo metadata --no-deps` once and call `publishable_set`. A set that is not equal to
     `EXPECTED_PUBLISHABLE` exits 1, as Check 0 does. So a crate that release-plz would publish but
     that is not in the list cannot skip the verify.
  3. Fill `PKG_DIR` from the same metadata.
  4. Call `publish_groups`, `check_publish_group` for each group, and
     `assert_check2_covered_everything`.
  5. It does **not** load the category snapshot, and does not run Checks 1, 1b, 1c, 1d, 2b, 2c, 3,
     4, 5 or the PyPI arm. A stale snapshot can then never stop a release.
- **Exit codes** follow the script's contract: 0 when every group passes, 1 for a defect, 2 for
  an infrastructure fault (network, empty group, missing manifest dir).
- **Arguments.** The mode exits 2 when it gets any extra argument. The dispatch reads only `$1`,
  so `--negative-control --verify-publish-groups` already runs the negative control; V20 pins the
  exact words, so that form cannot reach the workflow (§5.5).
- The usage line lists the new mode.

### 5.4 `ci/actionlint/release_guard.py`: the table-driven engine, V18 and V19

- **The engine.** `v18_line_segments`, `v18_split`, `v18_segment_verdict` and
  `_v18_step_config_violations` become driven by a table. One row holds the allowlist of one job:
  actions, command words, prefixes, exact commands, step keys, env names, working directories,
  `with:` keys, a rule label (`V18`, `V19`, `V20`) and a hint text.
- **Rules for every row.** The checks that V18 makes today on the job and the workflow apply to
  every row: no step `shell:`; no job key `container`, `services`, `defaults`, `env` or `uses`;
  no workflow-level `defaults:` or `env:`. The engine never merges the sets of two rows. Each
  message carries its own label (`": V19: "`, `": V20: "`).
- **V18** keeps its row for `release-pr`. What it accepts does not change.
  `_SMA684_V18_CASES` and its count of 147 stay unchanged and pass.
- **V19, the `release` row.**
  - Actions: `rust-lang/crates-io-auth-action`, `actions/create-github-app-token`,
    `actions/checkout`, `moonrepo/setup-toolchain`. A local `./…` action is not allowed.
  - Command words: `set`, `echo`.
  - Prefixes: `proto install release-plz`, `release-plz release`. The prefix match is bounded, so
    `release-plz release-pr` does not match `release-plz release`.
  - Env names: `CARGO_REGISTRY_TOKEN`, `GIT_TOKEN`. Working directory: `rs`.
  - The plan derives the exact sets from the real job, and adds an entry only when the real job
    needs it.
  - **The `--no-verify` rule.** Every segment whose command is `release-plz release` must carry the
    exact token `--no-verify`. A token that starts with `--no-verify=` is refused.
  - V19 does not use the dry-run exemption.
- **V19 liveness.** For the release workflow: a job named `release` must exist, and every job that
  uses `rust-lang/crates-io-auth-action` or runs `release-plz release` must be in the V19 job set.
  The V19 job set is pinned by equality to `{"release"}`, in the same style as the `UNGATED_JOBS`
  pin.
- **Scope (D7).** V19 runs only when the checked file is the release workflow
  (`RELEASE_WORKFLOW_NAME`), as V11 does (lines 1061-1062).

### 5.5 `ci/actionlint/release_guard.py`, V20: the verify job

V20 has two parts. It runs only for the release workflow (D7).

1. **The `verify-crates` row of the engine.**
   - Actions: `actions/checkout`, plus `moonrepo/setup-toolchain` only if the plan's measurement
     keeps it (§5.2).
   - Exact commands: `moon setup` only if the measurement keeps it, and the verify step. After
     `v18_split`, the verify step's words must equal
     `["bash", "ci/publish-metadata/run.sh", "--verify-publish-groups"]`. Nothing else is allowed in
     that step: no `&`, no second segment, no extra argument.
   - Step keys: no step `if:`, no step `env:`, no step `working-directory:`, no `shell:`,
     no `continue-on-error` (V4 also covers the last one).
   - The checkout step must set `persist-credentials: false`.
2. **Job rules.**
   - A job named `verify-crates` exists.
   - `verify-crates` is on the transitive `needs:` path of `approve-release` (`gated_path_jobs`).
   - The job-level `permissions:` value is a mapping with exactly one key, `contents`, and the
     value of that key is the string `read` (D8). V20 refuses a missing `permissions:` key,
     `read-all`, `write-all`, `{}`, `contents: write`, a second scope (also a `read` scope), and
     every value that is not a mapping.
   - The job has no `environment:` key.
   - The job has no `secrets` context reference (`secret_refs`, line 363) and no `github.token`
     reference, in any step or key.
   - The job has no `uses: actions/create-github-app-token` step, with or without `permission-*`
     inputs.

The message names the job and the failed condition, and points to this spec.

### 5.6 Tests

- **`_SMA735_V19_CASES`**, with a strict count, driven by a new self-test that calls the V19 check
  directly with the release workflow name, in the style of `_sma684_v18_allowlist_bites`. Red
  cases: no `--no-verify`; `--no-verify=false`; `--no-verify` only in a comment; `cargo publish`;
  `cargo build`; `cargo package`; `moon setup`; `pnpm --dir ts install`;
  `release-plz release --no-verify && cargo build`; `release-plz release --no-verify &`;
  `uses: actions/setup-node@…`; `uses: ./local-action`; an env name outside the set; a step
  `shell:`; a job `container:`; a job renamed so that it uses `crates-io-auth-action` outside the
  V19 set. Clean controls: the real `Release` step, `proto install release-plz`, and the real job.
- **`_SMA735_V20_CASES`**, with a strict count. Red cases: the job is missing; the path to
  `approve-release` is removed; no `permissions:` key; `permissions: {id-token: write}`;
  `permissions: {contents: write}`; `contents: read` plus `id-token: write`; `contents: read` plus
  `actions: read`; `permissions: {}`; `permissions: read-all`; `permissions: write-all`; a
  `permissions:` value that is a string, not a mapping; `environment: release-publish`; an App
  mint with no `permission-*` input; a step env that names `secrets.X`; a `github.token` reference; a checkout without `persist-credentials: false`; the
  verify step with `&`; the verify step with a second command; the verify step with an extra
  argument; a step `if: false`; a step `shell:`; a step `env: BASH_ENV`. Clean control: the real
  job, with `permissions: contents: read`.
- **Cross-row cases.** V18 is silent on `release` and on `verify-crates`. V19 is silent on
  `release-pr` and `verify-crates`. V20 is silent on `release-pr` and `release`.
- **Scope cases.** V19 and V20 are silent when the file name is not the release workflow.
- **The fixture floor.** The new cases are not `FIXTURES` rows (D7), so `--fixture-count` and the
  floor `[ "$n" -ge 170 ]` in `ci/actionlint/run.sh` do not change. The same line is pinned in
  `ci_targets.py` (lines 1116, 2961, 3215); it does not change either.
- **`ci/publish-metadata/run.sh --negative-control`, new case for the mode.** Put a stub `cargo`
  first on `PATH`. The stub records its argv and returns scripted output. Call the real dispatch
  `bash run.sh --verify-publish-groups`, and assert:
  1. The recorded argv holds both groups: `-p paigasus-kernel`, and `-p paigasus-proto-derive`
     with `-p paigasus-proto` in one call. Each call has `publish --dry-run --locked`.
  2. Scripted "could not compile" gives rc 1. Scripted "spurious network error" gives rc 2.
     Scripted success gives rc 0.
  3. An extra argument gives rc 2.
  The `cargo metadata` call goes to the real cargo (the stub passes it through), so the
  publishable set comes from the real workspace. The case does not change the tree.

### 5.7 Documentation

| File | Change |
|---|---|
| `release.yml` header | The job order becomes `{wheels, prebuild, proto-dist, verify-crates} -> approve-release -> release -> {publish-pypi, publish-npm}`. The `release` job compiles nothing. |
| `release.yml:932-934` | The comment that quotes the old `needs:` of `release`. |
| `.github/workflows/prebuild.yml:239-242` | The comment that quotes the old `needs:` of `release`. |
| `.github/CLAUDE.md` | A new entry under "Workflow credentials and release guards": `release` runs `--no-verify`, `verify-crates` verifies before the approval, V19 and V20, and the residuals in §7. |
| `ci/actionlint/README.md` | V19, V20, the table-driven engine, and a new L-entry for the residuals in §7. |
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
   the same packages? What do we lose?** Yes (§5.1, §5.2). We lose:
   - The build at the moment of the upload. The verify job builds the same tarball from the same
     tree with the same cargo (F10), and with `--locked`. The registry index can change between the
     verify and the upload, and the approval can wait up to 30 days. A dependency that is yanked in
     that window can give a published crate that a consumer cannot build. That risk exists after
     every publish too.
   - In the merge-commit case, release-plz publishes from the release PR head, not from the commit
     that `verify-crates` builds (F14). §7 records this.
3. **Is the App installation limited to this repository, with the minimum permissions?** Not
   measured. The run log shows only the scopes that each token requests
   (`permission-contents: write`, `permission-pull-requests: write`, and "Creating token for this
   repository"). It does not show the installation's repository selection or its full permission
   set. A user token cannot read the installation (`GET /user/installations` returns 403,
   `GET /repos/…/installation` needs an App JWT). See §12 Q1.

## 7. Risks and residuals

| Risk | Effect | Control |
|---|---|---|
| A change removes `--no-verify`. | The verify build runs again with the credentials. | V19 reds. |
| A change renames `release`, or moves the publish to a new job. | V19 checks nothing. | V19 liveness: the job set is pinned, and every job with `crates-io-auth-action` or `release-plz release` must be in it. |
| A change removes `verify-crates`, its path to `approve-release`, or changes its command. | A broken crate reaches the irreversible step. | V20 reds. |
| A credential enters `verify-crates`. | Third-party build code runs with it. | V20 job rules. The environments' branch policies are a second, external control (F15). |
| The job `GITHUB_TOKEN` of `verify-crates`, with `contents: read`, stays on the runner while the crates build (D8). | Third-party build code can read the token. The token can read only this repository, and the repository is public. | Residual, accepted (D8). V20 refuses every grant other than exactly `contents: read`, so the token cannot write. |
| The registry index changes between the verify and the upload. | A published crate can fail for consumers. | Residual (§6 Q2). `--locked` holds the workspace lock in the verify job. |
| **A different commit:** the release PR is merged with a merge commit, and `main` has more commits than the release PR head. | release-plz publishes from the release PR head (F14), which `verify-crates` did not build. | Partial: PR CI ran Check 2 on that head, because the release PR changes `rs/crates/**` (F6). Residual: the registry window between that PR run and the upload. A squash merge avoids the case. This spec does not change the allowed merge methods. |
| `verify-crates` checks every group, also a group that does not release in this run. | A fault in that group stops the release of the other group. | Accepted: the direction is safe (fail closed, before the approval). |
| `--verify-publish-groups` drifts from Check 2. | The two verify paths disagree. | One copy of the functions (§5.3). |
| A transient network error in `verify-crates`. | The run stops before the approval. | rc 2 names it as infrastructure. A re-run of the failed job is safe, because nothing irreversible ran. |
| A compromised `release-plz` download. | It runs with all three credentials. | Residual, the same as SMA-684 §7: the proto plugin has no checksum. |
| Any tool in §4.1 is compromised. | It can read the credentials from the runner. | Residual of D1. |
| A change to `rs/.cargo/config.toml` or `rs/rust-toolchain.toml`. | cargo in `release` can run a chosen program (a credential provider, a toolchain path). | Residual. No gate pins these files; code review is the only control. |
| The engine's splitter does not see a command inside a quoted string. | A hidden command passes V19 or V20. | Residual, already an L-entry for V18; the new L-entry extends it to V19 and V20. |
| **`publish-npm`** holds `environment: release-publish` and `id-token: write` (`release.yml:840-850`) and runs `pnpm --dir ts install --frozen-lockfile` (line 975). | Install scripts run in a job that can request an OIDC token. INFERRED, not measured: crates.io and PyPI trusted publishing bind the repository, the workflow file and the environment, not the job. If so, such a script can publish to all three registries. | Narrower than it looks: pnpm 11 runs a dependency build script only when `allowBuilds` allows it, and only `sharp` is allowed (F16). The workspace's own lifecycle scripts still run. Not closed by this spec. Follow-up in §9, with the same priority as this issue. |

## 8. Rejected approaches

- **B: A plus a separate tag job.** See §6 Q1. More new logic in the irreversible path, for a
  gain only against a compromised trusted tool.
- **C: move only the App mint to another job, keep the verify build.** `CARGO_REGISTRY_TOKEN`
  stays on the runner during the compile, and release-plz passes it to `cargo publish` in the
  child environment (F4). It does not meet A1.
- **Rely on PR CI only, with no `verify-crates`.** PR CI already runs Check 2 on the release PR
  head (F6). It is not enough: the `Protect main` ruleset has an admin bypass actor; the dispatch
  lever (`release.yml:76-82`) can run on a later commit; and the registry window between the PR run
  and the upload is longer.
- **An exact snapshot of the `release` job's steps instead of an allowlist row.** It is simpler and
  cannot weaken V18. It is rejected because every comment or name edit in the job would red it,
  and because it cannot express the `--no-verify` rule or the V20 job rules. The table-driven
  engine keeps V18 unchanged by its own case table (§5.6).

## 9. Follow-up issue

Open one Linear issue with project, milestone, priority (the same as SMA-735) and labels set at
creation. Title: "publish-npm: pnpm install runs lifecycle scripts while the job holds id-token:
write". It records:
- the risk row in §7, and the INFERRED claim about the OIDC binding, to measure;
- the option `pnpm install --ignore-scripts` for that job, and what `napi prepublish` then needs;
- F7: no gate holds the "no build downstream of `release`" rule.

## 10. Verification

| ID | Check | Kind |
|---|---|---|
| M1 | The plan's runner measurement (§5.2): the tools, the steps that can be dropped, and the cold-run time. | one time, MEASURED |
| M2 | A fixture measurement of release-plz itself: a one-crate workspace whose `build.rs` writes a sentinel; `release-plz release --dry-run --no-verify` with `RELEASE_PLZ_LOG=debug`. The sentinel is absent, and the debug log shows `--no-verify` in the cargo argv. The control without `--no-verify` writes the sentinel. If the live GitHub call in `get_git_client` (F3) blocks the fixture, the plan records why and falls back to F1 (READ). | one time, MEASURED |
| V-1 | `bash ci/publish-metadata/run.sh --verify-publish-groups` passes on the real tree and reports both groups. | local and CI |
| V-2 | Deletion checks, each must red a gate: remove `--no-verify` from the `Release` step; remove `verify-crates` from the `needs:` of `approve-release`; delete the `verify-crates` job; change its command; delete the `--verify-publish-groups` dispatch case. | one time |
| V-3 | `release_guard.py --self-test` passes, including the V18 cases unchanged and the new V19, V20, cross-row and scope cases. | local and CI |
| V-4 | `repo:actionlint` and `repo:publish-metadata` pass under the bash that each needs (root `CLAUDE.md`). | local and CI |
| V-5 | The full gate graph from the root `CLAUDE.md` passes before the push. | local and CI |
| V-6 | Optional, before the merge, with Sven's consent: `gh workflow run release.yml --ref feature/sma-735-release-job-isolation`. `verify-crates` runs and passes. `approve-release` cannot deploy from the branch (F15), so nothing irreversible runs. The run also builds the full matrix, which costs runner time. | one time, live |
| V-7 | The first release after the merge: `verify-crates` ran before `approve-release`; the `Release` step's duration is clearly shorter than the last live release (a verify build of three crates is gone); the tags and GitHub releases exist as before. | live, after merge |

F12 explains why V-7 does not look for a `Compiling` line: the release-plz log never shows one.
M2 is the proof that `--no-verify` reaches cargo. V-7 is the proof that the live job still works.

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
- **Q2.** Do you accept the two residuals in §6 Q2: the registry window, and the merge-commit case?
  The alternative for the second is a rule that release PRs are squash-merged.
- **Q3.** Do you consent to V-6 (a dispatch from the feature branch, with the cost of a full build
  matrix)?

## 13. Spec challenge record (revision 2)

The challenger's verdict: APPROVE WITH CHANGES. All findings are folded in, except where noted.

| Finding | Severity | Result |
|---|---|---|
| V20 "one `run:` step" contradicts §5.2, and a V9e-style check fails open (`&`, flag order, step `if:`, `shell:`, `BASH_ENV`). | BLOCKER | Folded in: §5.5 uses an engine row with exact words and step-key bans; §5.3 refuses extra arguments. |
| The mode cannot reuse the enumeration without Checks 0-5 and the dated snapshot. | BLOCKER | Folded in: §5.3 extracts `publishable_set`, keeps Check 0, skips the snapshot. |
| The negative-control case proves nothing in CI (dirty tree). | MAJOR | Folded in: §5.6 uses a `cargo` stub on `PATH`. |
| V-6 (old) cannot see `--no-verify` at INFO level. | MAJOR | Folded in: F12, M2, V-7. The debug-log option in the live job is rejected: it adds a V19 env entry and a token-in-log risk. |
| The fixture floor is pinned in three `ci_targets.py` lines. | MAJOR | Folded in: D7, the new cases are not `FIXTURES` rows, so the floor does not change. |
| V19/V20 scope against the `_OK_MAIN` fixtures. | MAJOR | Folded in: D7, scope to the release workflow, as V11. |
| V20 rule 3 does not mean "no credential". | MAJOR | Folded in: §5.5 job rules. |
| The V19 row leaves out the job-wide checks. | MAJOR | Folded in: §5.4 rules for every row; cross-row cases in §5.6. |
| `publish-npm` keeps the capability. | MAJOR | Folded in as a §7 row and the §9 follow-up. Narrowed by F16 (pnpm 11 blocks dependency build scripts except `sharp`). |
| Stale comments in `release.yml` and `prebuild.yml`. | MINOR | Folded in: §5.7. |
| V19 has no liveness floor. | MINOR | Folded in: §5.4 V19 liveness. |
| `verify-crates` checks every group. | MINOR | Folded in: §7. |
| §4.1 is incomplete; `.cargo/config.toml` and `rust-toolchain.toml`. | MINOR | Folded in: §4.1 and §7. |
| "Same commit" can be false (merge commit). | MINOR | Folded in: F14, A2, §6 Q2, §7, §12 Q2. |
| §8 leaves out two alternatives. | MINOR | Folded in: §8. |
| The already-published case. | MINOR | Folded in: F13. |
| Measure `verify-crates` before the merge? | QUESTION | §10 V-6, with Sven's consent (§12 Q3). F15 shows that the approval cannot deploy from the branch. |
| V20 path: direct or transitive? | QUESTION | Transitive (`gated_path_jobs`), §5.5. |
| Which cargo does `verify-crates` run? | QUESTION | The plan measures it (M1). |
| A cold run against `timeout-minutes: 30`? | QUESTION | The plan measures it (M1). |

### Addendum: decision D8 (2026-10-09)

After revision 2, the coordinator gave Sven options for the `permissions:` key of
`verify-crates`. Option A: the job sets exactly `contents: read`, and V20 enforces it. Sven
chose option A on 2026-10-09. Revision 2 had the rule "no `permissions:` key", with the
workflow default `contents: read`. D8 replaces that rule.

The reason below is the coordinator's recommendation, which Sven accepted. This spec records
no other reason from Sven. `actions/checkout` gets the job `GITHUB_TOKEN` as its default `token`
input. With `contents: read`, that token can only read the public repository. V20 refuses every
other grant.

Changes: D8 in §2, A3, §5.2, §5.5, §5.6 and a new residual row in §7.

## 14. Measurement record

### M2 — release-plz 0.3.158 with `--no-verify` (2026-10-09)

A one-crate fixture with a sentinel in `build.rs`, a local forge stub that answers 404, and
`RELEASE_PLZ_LOG=debug release-plz release --dry-run`. Script: the plan, Task 1.

| Run | rc | `build.rs` ran | `cargo publish` argv in the debug log |
|---|---|---|---|
| A, `--no-verify` | 0 | no | `cargo publish --color always --manifest-path <…>/Cargo.toml --package sma735-m2-probe-<n> --dry-run --no-verify` |
| B, control | 0 | yes | `cargo publish --color always --manifest-path <…>/Cargo.toml --package sma735-m2-probe-<n> --dry-run` |
| C, `publish_no_verify = false` key and `--no-verify` flag | 0 | no | `cargo publish --color always --manifest-path <…>/Cargo.toml --package sma735-m2-probe-<n> --dry-run --no-verify` |

Result: the `--no-verify` flag reaches cargo and no build script runs, also when the package key is `false`.

### M1 — what `verify-crates` needs, and a cold run (2026-10-09)

Docker `ubuntu:24.04` (image `ubuntu@sha256:534baea6a22c03a63003dbc8dbe78fe34bc0d7e595d9a9dc9834884ff530eb55`,
x86_64, run under emulation on an arm64 host) with bash 5.2.21, python3 3.12.3, git, build-essential
and rustup with no toolchain. No proto and no Moon. `CI=true bash ci/publish-metadata/run.sh
--verify-publish-groups` exited 0 after 55 seconds, including the rustup install of 1.95.0
from `rs/rust-toolchain.toml` (cargo 1.95.0, rustc 1.95.0). Both groups passed. A first run took 60 seconds.

Decision (spec §5.2): `verify-crates` keeps only `Checkout` and the verify step. It drops
`moonrepo/setup-toolchain` and `moon setup`. `timeout-minutes: 30` stays. The container is a
proxy: the hosted runner is x86_64 and has rustup and python3 in its image. V-6 (Sven's consent)
or V-7 (the first release after the merge) measures the hosted runner.
