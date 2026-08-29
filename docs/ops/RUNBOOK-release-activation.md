# RELEASE ACTIVATION RUNBOOK — the first publish to crates.io, PyPI and npm (SMA-580)

Operator-facing procedure for activating the release path in
[`.github/workflows/release.yml`](../../.github/workflows/release.yml). SMA-579 shipped that path
complete but **inert**, gated behind `vars.PAIGASUS_RELEASE_ENABLED`. This runbook sets the
credentials up, seeds crates.io, flips the variable, and verifies the result.

**Two steps here are irreversible. Every other step is not.**

| Registry | What a wrong publish costs |
| --- | --- |
| crates.io | `cargo yank` only. **Never delete, never reuse a version.** |
| PyPI | Delete is allowed. **Reuse of a version is not.** |
| npm | Unpublish within **72 hours** only. |

The design rationale — why crates.io needs a seed at all, why the seed tree must not be a git
repository, and what each measurement showed — lives in
[`docs/superpowers/specs/2026-08-29-sma-580-release-activation-e-design.md`](../superpowers/specs/2026-08-29-sma-580-release-activation-e-design.md).
This runbook does not repeat the reasoning. It carries the procedure.

**Read §4 (step D) in full before you start.** It is the step that must not be paraphrased.

---

## 1. The sequence

Steps **D** and **I** are irreversible. Work top to bottom. Do not reorder.

| Step | Action | Trigger | Owner | Reversible |
| --- | --- | --- | --- | --- |
| **A** | **Prepare** this issue's PR — runbook, `release.yml` comment + dispatch trigger, ADR draft | — | agent | yes |
| **B1** | Create both environments with the settings in §3 | — | agent or owner | yes |
| **B2** | Add the required reviewer to `release-approval` | — | agent or owner | yes |
| **B3** | Prove the App can push a tag and cut a Release | — | agent or owner | yes |
| **C** | `@paigasus` npm scope, `NPM_TOKEN`, three PyPI pending publishers | — | owner | yes |
| **D** | Publish the three `0.1.0-alpha.1` seeds | — | owner | **NO** |
| **E** | Configure crates.io Trusted Publishing for the three crates | — | owner | yes |
| **F** | Merge this issue's PR. **OBSERVATION GATE** | the merge in F | owner | yes |
| **G** | Merge the release PR. Still gated, so nothing publishes | the merge in G | owner | yes |
| **H** | **THE FLIP** — set `PAIGASUS_RELEASE_ENABLED` to `true` | — | owner | yes, until I |
| **I** | Dispatch `release.yml`. Approve when it pauses | the dispatch | owner | **NO** |
| **J** | Verify §7. **Then** yank the seeds and remove the dispatch trigger | — | owner | — |

Steps C, D and E use this runbook **from the PR branch**, before step F merges it.

---

## 2. Steps A and B — already done when you read this

**A** prepared the pull request carrying this file, the `release.yml` comment naming the Paigasus
bot App, and the temporary `workflow_dispatch` trigger. **That pull request is merged at step F,
not here** — you read this runbook from its branch while you work through steps C, D and E.

**B1 and B2** created the two environments. **B3** proved the App installation can push a tag and
create a GitHub Release, using a throwaway tag that was then deleted. The absence of tag protection
does not prove the App can push — that is why B3 exists.

If any of B1–B3 was not performed, stop and perform it now. §3 has the settings.

---

## 3. The two GitHub environments

**GitHub auto-creates a referenced environment on first use, with no protection rules.** So a
missing environment and a misconfigured one produce the same outcome: `approve-release` walks
straight through and the whole irreversible stage runs unattended. Verify both, do not assume.

### `release-approval`

| Setting | Value | Why |
| --- | --- | --- |
| Required reviewers | the repository owner | The one place a human can stop the run |
| Prevent self-review | **OFF** | The owner dispatches the run at step I **and** must approve it |
| Wait timer | 0 | Nothing to delay |
| Deployment branch policy | **`main` only** | Same reasoning as `release-publish` below |

### `release-publish` — no reviewers, deliberately

| Setting | Value | Why |
| --- | --- | --- |
| Required reviewers | **none** | See below |
| Wait timer | **0** | A wait timer delays *each* of the three jobs independently |
| Deployment branch policy | **`main` only** | This is the authorization boundary for the temporary `workflow_dispatch` trigger. See below |

**The `main`-only branch policy is load-bearing, not hygiene.** A dispatch runs the workflow
definition from the dispatched ref, so anyone with write access can dispatch an edited copy of
`release.yml`. Every in-workflow control is attacker-controlled on such a ref. Two things outside
this repository close that, and they hold in both directions:

- If the edited copy **keeps** `environment: release-publish`, this branch policy fails the job on
  any ref but `main`.
- If it **removes** the environment, the OIDC token carries no environment claim, and the crates.io
  and PyPI trusted publishers — configured in §5.2 and §5.3 to require `release-publish` — reject
  it. `NPM_TOKEN` is an environment secret on the same environment, so npm loses its credential.

**Do not relax the branch policy, and do not leave the environment field blank on any of the three
registry configurations.** Together they are the boundary.

**One path this boundary does NOT cover.** `release-pr` enters no environment, and mints an App
token with `contents: write` from **repository** secrets, which any run reaches regardless of ref.
So a dispatched ref can still reach those credentials. The escalation is narrow — the token is
repo-scoped, masked, and revoked at job end, granting about what a write-access holder already has
— and `release-pr` now carries `if: github.event_name == 'push'`, which stops an *accidental*
dispatch from minting it. That `if:` is **not** a boundary: an edited copy of the workflow deletes
it. Closing it properly means moving `PAIGASUS_BOT_APP_ID` and `PAIGASUS_BOT_PRIVATE_KEY` to
environment secrets on a `main`-only environment. **That is an open decision — see the spec
§3.3.1 and §10.2. Settle it before or together with step H.**

GitHub pauses **each** job that enters an environment, and three enter this one: `release`,
`publish-pypi` and `publish-npm`. A reviewer here would stop the run again between crates.io and
PyPI. A rejected or timed-out second approval leaves crates.io published and PyPI empty — the split
state the job order exists to prevent.

The environment must still exist. Both PyPI's and crates.io's OIDC claims bind to it.

---

## 4. Step D — the crates.io seed. IRREVERSIBLE

crates.io cannot pre-register a Trusted Publisher for a crate that does not exist (RFC 3691). The
`release` job holds no other crates.io credential. So three crates must exist before step E can be
performed at all.

The seed publishes `0.1.0-alpha.1` of each. It is the `0.1.0` code with a different version string.
Cargo never resolves a pre-release for a `0.1` requirement, so no consumer sees it.

### 4.1 The API token

Create a crates.io API token scoped to **`publish-new` and `yank` only**. Not `publish-update`, not
`change-owners`. It lives on your machine, never in a repository or environment secret, and **you
revoke it at step J**.

### 4.2 Build the scratch tree — it must NOT be a git repository

Cargo walks **upward** to find a repository. Extracting under a directory that is itself tracked
reintroduces the fault this guards against. Use `git archive`. Do not use `git worktree add`, and do
not clone.

```bash
rm -rf /tmp/seed && mkdir -p /tmp/seed
git archive HEAD | tar -x -C /tmp/seed

# MUST fail with "not a git repository". If it prints a path, STOP.
git -C /tmp/seed rev-parse --show-toplevel
```

**Why this matters.** `cargo publish` embeds `.cargo_vcs_info.json` whenever it runs inside a git
repository, recording the HEAD SHA1. release-plz reads that file from the published tarball and uses
the SHA1 as the boundary for its commit walk. A seed carrying it would truncate the first release's
changelog to "commits after the seed" — close to empty.

### 4.3 The six edits

Apply all six in `/tmp/seed`. Three package versions and **three** workspace dependency pins.

| # | File | Change |
| --- | --- | --- |
| 1 | `rs/crates/libs/paigasus-kernel/Cargo.toml` | `version = "0.1.0-alpha.1"` |
| 2 | `rs/crates/libs/paigasus-proto-derive/Cargo.toml` | `version = "0.1.0-alpha.1"` |
| 3 | `rs/crates/libs/paigasus-proto/Cargo.toml` | `version = "0.1.0-alpha.1"` |
| 4 | `rs/Cargo.toml:140` | `paigasus-proto-derive = { path = "crates/libs/paigasus-proto-derive", version = "=0.1.0-alpha.1" }` |
| 5 | `rs/Cargo.toml:143` | `paigasus-kernel = { path = "crates/libs/paigasus-kernel", version = "=0.1.0-alpha.1" }` |
| 6 | `rs/Cargo.toml:146` | `paigasus-proto = { path = "crates/libs/paigasus-proto", version = "=0.1.0-alpha.1" }` |

**Edits 5 and 6 are not optional.** Ten workspace members consume `paigasus-kernel` or
`paigasus-proto` through `[workspace.dependencies]`. A pre-release never satisfies a
non-pre-release requirement, so leaving those pins at `"0.1.0"` makes **every** cargo command in
`rs/` fail — including the derive publish, which touches neither crate. Measured: `cargo metadata`
exits 101 with *"failed to select a version for the requirement `paigasus-kernel = "^0.1.0"`"*.

Confirm the resolution before going further:

```bash
cd /tmp/seed/rs && cargo metadata --format-version 1 >/dev/null && echo "resolves OK"
```

`rs/Cargo.lock` is rewritten by the first cargo command. That is expected in a throwaway tree.
**Do not pass `--locked`.**

### 4.4 Assert no VCS info — two crates now, the third mid-publish

**`paigasus-proto` cannot be packaged yet, and that is not a fault.** `cargo package` resolves the
crate's dependencies against the **crates.io index**, so it needs `paigasus-proto-derive` at
`=0.1.0-alpha.1` to be published already. Before the derive is uploaded it fails with:

```
error: failed to prepare local package for uploading
Caused by:
  no matching package named `paigasus-proto-derive` found
  location searched: crates.io index
```

**Measured, both forms** — `--no-verify` does **not** help, because the failure is in dependency
resolution, not in the verification build. So the assertion interleaves with the publish: two
crates here, `paigasus-proto` in §4.5 after the index poll converges.

Save the check as a shell function; §4.5 calls it again.

```bash
cd /tmp/seed/rs
set -euo pipefail

assert_no_vcs () {
  c="$1"
  crate="target/package/$c-0.1.0-alpha.1.crate"
  # FAIL CLOSED. Without these two lines a failed `cargo package` leaves no archive,
  # `tar` writes nothing, `grep -c` prints 0, and the assertion reports OK on a crate it
  # never inspected. Measured: `tar tzf missing.crate | grep -c cargo_vcs_info` gives 0.
  cargo package -p "$c" || { echo "$c: cargo package FAILED — STOP"; return 1; }
  [ -f "$crate" ] || { echo "$c: no archive at $crate — STOP"; return 1; }
  n=$(tar tzf "$crate" | grep -c cargo_vcs_info || true)
  [ "$n" -eq 0 ] && echo "$c OK" || { echo "$c CARRIES VCS INFO — STOP"; return 1; }
}

assert_no_vcs paigasus-kernel
assert_no_vcs paigasus-proto-derive
```

Both must print `OK`. A non-zero count means the tree is inside a git repository — go back to §4.2.

**Expected file counts, so a surprise is visible:** `paigasus-kernel` 12 files,
`paigasus-proto-derive` 7. A count that jumps by one is the signature of an added
`.cargo_vcs_info.json`.

### 4.5 Publish

> **NEVER add `--allow-dirty` to any command in this section.**
>
> In the intended non-git tree the flag does nothing. In a git tree it is exactly what converts
> cargo's hard error into a **silent success that embeds the SHA1** — measured, with
> `"dirty": true` written into `.cargo_vcs_info.json`. The dirty-tree error is the guard. Removing
> it removes the only automatic detector of the §4.2 fault.
>
> If cargo refuses because the tree is dirty, that means §4.2 was not followed. Fix the tree.

```bash
cd /tmp/seed/rs

cargo publish -p paigasus-proto-derive

# Wait for the index. cargo publish -p paigasus-proto verifies against the REGISTRY.
# BOUNDED at ~3 minutes. An unbounded loop would hang forever on an outage or an auth
# failure, and you would never reach the fallback below.
ok=0
for i in $(seq 1 18); do
  if cargo info paigasus-proto-derive@0.1.0-alpha.1 >/dev/null 2>&1; then ok=1; break; fi
  echo "waiting for the index… ($i/18)"; sleep 10
done
[ "$ok" -eq 1 ] || { echo "index did not converge — use the fallback below"; exit 1; }

# paigasus-proto could not be asserted in §4.4 — the derive crate was not on the index
# yet. It is now. Assert BEFORE uploading it.
assert_no_vcs paigasus-proto        # must print OK before the upload below

cargo publish -p paigasus-proto
cargo publish -p paigasus-kernel
```

`paigasus-kernel` declares no in-tree dependency, so its position does not matter. Only the proto
pair is ordered.

**Fallback if the poll does not converge within a few minutes.** Use the combined form, which
resolves the order itself from the locally staged tarball and needs no poll:

```bash
cargo publish -p paigasus-proto-derive -p paigasus-proto
```

The sequential form is preferred because it is the **only rehearsal** the live derive→proto path
gets before step I. If you use the fallback, record that the rehearsal was given up.

**crates.io rate limits.** New crates are capped at a burst of 5 per account, then one per 10
minutes. Three fits. If a publish is refused, wait and retry — the earlier seeds stay valid.

---

## 5. Steps C and E — the registry configurations

### 5.1 npm — step C

**Already confirmed (2026-08-29):** `npm org ls paigasus` reports `smaschek - owner`, and
`npm access list packages @paigasus` is empty. Nothing to do here unless that changed.

To re-check: `npm whoami` then `npm org ls paigasus`. Do **not** rely on an unauthenticated probe —
`npmjs.com/org/paigasus` returns 403 and the registry org API 404 whether or not the org exists.

The first release creates **nine** packages under the scope: `@paigasus/node-bindings`, seven
platform packages, and `@paigasus/wasm`.

Create the token:

| Property | Value |
| --- | --- |
| Type | **Automation** |
| Scope | the `@paigasus` scope, read and write — **not** "only select packages", since nine packages do not exist yet |
| Stored as | an **environment secret on `release-publish`**, not a repository secret |

The type is not a free choice, and it is not hypothetical here: this account reports
`two-factor auth: auth-and-writes` (measured 2026-08-29), so 2FA **is** enforced for writes and a
classic publish token fails with *"2FA required for publishing"*.

`publish-npm` already declares `environment: release-publish`, so an environment secret resolves
with no workflow change, and the credential is not readable by every other workflow.

### 5.2 PyPI — step C

Create a **pending publisher** for each of the three project names. Pending publishers are PyPI's
mechanism for a project that does not exist yet.

| Field | Value |
| --- | --- |
| PyPI project name | `paigasus-py-bindings`, then `paigasus-kernel`, then `paigasus-proto` |
| Repository owner and name | this repository |
| Workflow filename | `release.yml` — **with the extension** |
| Environment name | `release-publish` |

**Confirm three slots are free**, not merely that a cap exists. PyPI caps pending publishers per
account. Verify the field labels against the live form; a wrong field fails *after* crates.io has
published.

### 5.3 crates.io Trusted Publishing — step E, after the seed

One configuration per crate: `paigasus-kernel`, `paigasus-proto`, `paigasus-proto-derive`.

| Field | Value |
| --- | --- |
| Repository owner and name | this repository |
| Workflow filename | `release.yml` — with the extension |
| Environment | `release-publish` |

The environment field matters. Leaving it blank makes the configuration broader than intended.
Filling it with `release-approval` makes the OIDC exchange fail — after the human approval and after
the full matrix build.

---

## 6. Steps F through I

### 6.1 Step F — the observation gate

Merging the SMA-580 pull request is a push to `main`, so `release-pr` runs automatically. That job
only opens or updates a pull request. It publishes nothing, so this observation is free.

**Read the `release-pr` job's `--output json` line in the run log. Do not judge by the visual state
of the release PR, and do not identify it by number — use `.prs[0].number`.** release-plz opens a
new pull request if the old one was ever closed.

| Observation | Meaning | Action |
| --- | --- | --- |
| `.prs[0]` proposes `0.1.0` for all three crates | Confirmed | **Proceed** |
| `.prs` is `[]`, or no PR is refreshed | The gate is **vacuous**, not passed | **Stop.** Diagnose before flipping |
| Any other version proposed | The version model is wrong | **Stop. Do not flip** |

Expect the log to say *"updating changelog for version 0.1.0"* rather than *"next version is …"*.
That wording is correct for a package whose manifest version already exceeds the registry baseline.

**Also read the changelog.** It should still cover the full history. A near-empty changelog means
the seed embedded `.cargo_vcs_info.json` — see §4.2.

**Record the run's wall-clock.** After the seed, release-plz's commit walk has no stopping boundary
and must compare the package at each commit across `main`'s history. `release-pr` carries
`timeout-minutes: 20`. If the margin looks thin, raise it before proceeding. The cost is
self-limiting: tags exist after step I.

### 6.2 Step G — merge the release PR

Merge it while the path is still gated. Nothing publishes. This puts the `CHANGELOG.md` files on
`main` so the two GitHub Releases carry real notes.

The branch ruleset requires a pull request to be up to date with `main` (`strict = true`). Step F's
merge leaves the release PR behind. This resolves itself: `release-pr` force-updates its branch on
that same push, which is the refresh step F reads.

### 6.3 Step H — the flip

```bash
gh variable set PAIGASUS_RELEASE_ENABLED --body true --repo SMK1085/paigasus-core
gh variable list --repo SMK1085/paigasus-core
```

Nothing publishes yet. `release.yml` fires on a push to `main` or on a dispatch, and the flip is
neither.

### 6.4 Step I — dispatch and approve. IRREVERSIBLE

```bash
gh workflow run release.yml --ref main --repo SMK1085/paigasus-core
gh run watch --repo SMK1085/paigasus-core
```

`wheels`, `prebuild` and `proto-dist` build every artifact. Then `approve-release` enters the
`release-approval` environment and **pauses**.

**Confirm it reaches the `waiting` state.** If the run walks straight through without pausing, step
B2 was not performed — **cancel the run immediately**. That pause is the only human gate that
exists.

Approve it. Everything after the approval is irreversible:

- `release` publishes three crates, cuts six tags, creates two GitHub Releases.
- `publish-pypi` uploads three projects.
- `publish-npm` publishes nine packages.

A second release pull request may appear, proposing `0.1.0` again. **Do not merge it.** It resolves
itself once `0.1.0` is on the registry.

---

## 7. Step J — verification, then cleanup

### 7.1 Presence

1. crates.io serves `paigasus-kernel`, `paigasus-proto`, `paigasus-proto-derive` at `0.1.0`.
2. PyPI serves `paigasus-py-bindings`, `paigasus-kernel`, `paigasus-proto` at `0.1.0`.
   `paigasus-py-bindings` carries seven wheels and one sdist.
3. npm serves nine packages at `0.1.0`.
4. Six git tags of the form `<package>-v0.1.0` exist.
5. Exactly **two** GitHub Releases exist for the release commit, one per family head.
6. `moon ci` stays green on `main`. If your shell does not already resolve the
   repository-pinned tools, prefix it with
   `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`.

### 7.2 Installability — inside the 72-hour npm window

Presence is not correctness. `napi prepublish` publishes only the seven platform packages, and the
main package's `optionalDependencies` name them. Wrong ordering leaves `npm install
@paigasus/node-bindings` returning 404 forever while all nine packages read as "served at `0.1.0`".
`paigasus-kernel` on PyPI pins `paigasus-py-bindings==0.1.0` exactly, so an upload-order fault is
likewise invisible to a presence check.

Run all three in a clean environment:

```bash
# PyPI. Installing the FACE pulls the bindings, so this exercises the dependency
# edge, not just one package. The PRN is vector 0 of
# rs/crates/libs/paigasus-kernel-parity/vectors/prn_canonical.json.
python3 -m venv /tmp/v && /tmp/v/bin/pip install paigasus-kernel
/tmp/v/bin/python -c "
import paigasus_kernel as k
p = 'prn:pgs:iam:::organization/0190a1e5-0000-7000-8000-000000000000'
assert k.prn_canonicalize(p) == p, k.prn_canonicalize(p)
assert k.sum_as_string(2, 3) == '5'
print('PyPI face + bindings OK')
"

# npm
mkdir -p /tmp/n && cd /tmp/n && npm init -y >/dev/null && npm i @paigasus/node-bindings
node -e "console.log(Object.keys(require('@paigasus/node-bindings')))"

# crates.io
cargo new /tmp/c && cd /tmp/c && cargo add paigasus-kernel && cargo build
```

### 7.3 Recovery, if a stage failed partway

| State | Recovery |
| --- | --- |
| `release` failed partway | Re-run it. release-plz's `is_published` and existing-tag short-circuits converge |
| crates.io done, PyPI failed | Re-run `publish-pypi`. `skip-existing: true` converges |
| crates.io done, npm main packages failed | Re-run `publish-npm`. The `npmstate` pre-check skips what landed |
| npm platform loop failed partway | Re-run `publish-npm`. `napi prepublish` catches npm's 403 per target and continues the loop |

The last row's guard lives in `@napi-rs/cli`, not in this repository, and it matches on npm's error
**message text**. A napi upgrade or an npm wording change could remove it silently. Nothing gates
that.

### 7.4 Cleanup — only after §7.1 and §7.2 pass

Verify first. A yank is cheap to do late and awkward to undo, and the seeds are the diagnostic
baseline if verification fails.

```bash
cargo yank --version 0.1.0-alpha.1 paigasus-proto-derive
cargo yank --version 0.1.0-alpha.1 paigasus-proto
cargo yank --version 0.1.0-alpha.1 paigasus-kernel
```

Then **revoke the crates.io API token** from §4.1.

---

## 8. The two tracked removals

Both are temporary by decision. **No gate enforces either.**

| Item | Removal condition |
| --- | --- |
| `workflow_dispatch` on `release.yml` | The first release has published and §7 has passed. Remove it in the same pull request that records the outcome |
| `NPM_TOKEN` | Every `@paigasus/*` package exists, so npm Trusted Publishing becomes configurable. **File a follow-up issue**, and set the token's expiry short enough to bound the gap |

---

## 9. What must never happen

- **Never hand-place a `*-vX.Y.Z` tag** to seed release-plz's tracking. Manual tags lack the
  metadata release-plz uses and silently stop all future bumps. The seed in §4 places **no tag**,
  which is what separates it from that failure.
- **Never add `--allow-dirty`** to any command in §4. See the warning there.
- **Never give `release-publish` a reviewer or a wait timer.** See §3.
- **Never add a `pull_request` or `pull_request_target` trigger to `release.yml`.**
- **Never treat an in-workflow check as the authorization boundary for the dispatch trigger.** A
  dispatch runs the definition from the dispatched ref, so every `if:`, `environment:` and
  `github.ref` check in the file is attacker-controlled there. §3 has the real boundary.
