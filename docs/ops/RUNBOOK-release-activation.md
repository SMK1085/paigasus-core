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
| **B1** | ~~Create the three environments (§3)~~ **done** | — | owner | yes |
| **B2** | ~~Add the required reviewer to `release-approval`~~ **done** | — | owner | yes |
| **B3** | App tag/Release capability — evidenced, see §2 | — | owner | yes |
| **C** | ~~npm scope, `NPM_TOKEN`, `PYPI_API_TOKEN`~~ **done** — the bootstrap tokens. SMA-602 replaces them; see C1–C4 below | — | owner | yes |
| **D** | ~~Publish the three `0.1.0-alpha.1` seeds~~ **done 2026-08-29** | — | owner | **NO** |
| **E** | Configure crates.io Trusted Publishing for the three crates | — | owner | yes |
| **F** | Merge this issue's PR. **OBSERVATION GATE** | the merge in F | owner | yes |
| **G** | Merge the release PR. Still gated, so nothing publishes | the merge in G | owner | yes |
| **H** | **THE FLIP** — set `PAIGASUS_RELEASE_ENABLED` to `true` | — | owner | yes, until I |
| **I** | Dispatch `release.yml`. Approve when it pauses | the dispatch | owner | **NO** |
| **J** | Verify §7. **Then** yank the seeds | — | owner | — |

**SMA-602's owner steps — the move to trusted publishing.** These are the ONLY ordering of them;
the design spec is not an operations document. Nothing here is done until the owner confirms it.
The order is the safety property: registering early is free, merging early breaks the next release.

| Step | Action | When | Owner | Status |
| --- | --- | --- | --- | --- |
| **C1** | Register **twelve** trusted publishers — nine on npm (§5.1), three on PyPI (§5.2). Confirm the `environment` field on every one | **before** the SMA-602 PR merges | owner | **required before merge** |
| **C2** | Verify the nine npm configurations with `npm trust list` (§5.1.2). It uploads nothing. PyPI has no read-back | **before** the SMA-602 PR merges | owner | **required before merge** |
| **C3** | Confirm both token VALUES are stored outside GitHub, record where (§8), **then** delete `PYPI_API_TOKEN` and `NPM_TOKEN` from `release-publish` | **after** the SMA-602 PR merges | owner | **required after merge** |
| **C4** | Revoke the PyPI token on PyPI | **only after** a release has actually published through OIDC | owner | **required after the first OIDC release** |

C1 must precede the merge. C3 must follow it — the workflow stops reading the secrets at merge, so
deleting them earlier breaks a release that starts in between. C4 must follow a proven OIDC
release: the token is the only credential that can publish to PyPI by hand, and PyPI has no
documented hand-recovery procedure (§8).

Steps C, D and E use this runbook **from the PR branch**, before step F merges it.

---

## 2. Steps A and B — already done when you read this

**A** prepared the pull request carrying this file, the `release.yml` comment naming the Paigasus
bot App, and the `workflow_dispatch` trigger (permanent — see release.yml's own comment, SMA-603).
**That pull request is merged at step F, not here** — you read this runbook from its branch
while you work through steps C, D and E.

**B1 and B2 are done — measured 2026-08-29:**

| Environment | Branches | Reviewers | Secrets |
| --- | --- | --- | --- |
| `release-pr` | `main` only | none | `PAIGASUS_BOT_APP_ID`, `PAIGASUS_BOT_PRIVATE_KEY` |
| `release-approval` | `main` only | `SMK1085`, self-review allowed | — |
| `release-publish` | `main` only | none | `PAIGASUS_BOT_APP_ID`, `PAIGASUS_BOT_PRIVATE_KEY` |

**PENDING, not done (SMA-602, step C3).** The table above shows the state **after** step C3 runs.
Until the owner confirms C3, `release-publish` also holds `NPM_TOKEN` and `PYPI_API_TOKEN` — the
SMA-580 bootstrap tokens. An earlier revision of this section stated their removal as accomplished
fact; it was not, and there is no way for a reader to check it from the repository. Read the live
state instead:

```bash
gh api repos/SMK1085/paigasus-core/environments/release-publish/secrets \
  --jq '.secrets[].name'
```

Expected after C3: `PAIGASUS_BOT_APP_ID` and `PAIGASUS_BOT_PRIVATE_KEY`, nothing else.

No wait timers. **The two REPOSITORY secrets are deleted** — a separate, completed SMA-580 step
(`gh api …/actions/secrets` returns an empty list), so the App credentials live only on the two
environments that need them, and §3.1's migration is live. That is about the `PAIGASUS_BOT_*`
pair; it says nothing about the two publish tokens, which are ENVIRONMENT secrets and are removed
by C3.

**B3 — the App's tag and Release capability — is satisfied by evidence already in hand, with one
stated gap.** The App force-updates the release-plz branch on every push to `main` (that is how
PR 170 stays current), which proves its `contents: write` works. Tag creation uses the same
permission, and §1.3 measured that no ruleset targets `refs/tags/**` and that tag protection returns
404. **The gap:** a branch push is not literally a tag push. The inference is small and the two
measurements bracket it, but it is an inference. The first real proof is step I.

If any of B1–B3 looks different from the table above, stop and fix it. §3 has the settings.

---

## 3. The three GitHub environments

**GitHub auto-creates a referenced environment on first use, with no protection rules.** So a
missing environment and a misconfigured one produce the same outcome: `approve-release` walks
straight through and the whole irreversible stage runs unattended. Verify both, do not assume.

### `release-pr` — the credential boundary for the release-PR job

**Configured 2026-08-29.** Branch policy `main`, both App secrets present, no reviewers, no wait
timer.

| Setting | Value | Why |
| --- | --- | --- |
| Required reviewers | **none** | It runs on every push to `main`; a reviewer would block every merge |
| Wait timer | **0** | Nothing to delay |
| Deployment branch policy | **`main` only** | The boundary — a dispatched ref fails here |
| Secrets | `PAIGASUS_BOT_APP_ID`, `PAIGASUS_BOT_PRIVATE_KEY` | **Environment** secrets, so dropping the `environment:` key loses them |

A job may declare only **one** environment. That is why `release-pr` gets its own rather than
joining `release-publish`: the latter is bound by the crates.io and PyPI trusted publishers and
must stay on the `release` job.

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
| Deployment branch policy | **`main` only** | This is the authorization boundary for the `workflow_dispatch` trigger, which is permanent (SMA-603). See below |

**The `main`-only branch policy is load-bearing, not hygiene.** A dispatch runs the workflow
definition from the dispatched ref, so anyone with write access can dispatch an edited copy of
`release.yml`. Every in-workflow control is attacker-controlled on such a ref. Two things outside
this repository close that, and they hold in both directions:

- If the edited copy **keeps** `environment: release-publish`, this branch policy fails the job on
  any ref but `main`.
- If it **removes** the environment, the OIDC token carries no environment claim, and the npm,
  crates.io and PyPI trusted publishers — which §5.1, §5.2 and §5.3 tell you to configure to
  require `release-publish` — reject it. **Corrected 2026-08-30 (SMA-602):** this used to name
  `NPM_TOKEN` as the npm-side control; once step C3 deletes that secret, npm uses the same OIDC
  trusted-publishing rejection as PyPI and crates.io.

**Do not relax the branch policy, and do not leave the environment field blank on any of the twelve
registry configurations.** Together they are the boundary.

**READ THE SECOND HALF LITERALLY — it rests on twelve external settings, not on this repository.**
The environment field is **optional** on both registries. npm defines it with `default: null` and
no `required: true` (`trust/github.js:35-40` in npm 11.13.0), and PyPI's form accepts an empty
environment. A publisher registered without it accepts an exchange carrying **no** environment
claim: that package then publishes from any ref the workflow name matches, this repository stays
green, and nothing here detects it. `npm trust list` (§5.1.2) reads back nine of the twelve; PyPI
offers no read-back at all. Verify all twelve at step C1, re-check the nine npm ones periodically
per §5.1.2, and treat this as the one boundary with no automated control behind it.

**`release-pr` is now covered too, by the same shape.** It was the one gap: it entered no
environment and minted its App token from **repository** secrets, which any run reaches regardless
of ref. It now declares `environment: release-pr` (above), whose branch policy is `main`-only and
whose App secrets are **environment** secrets. Both directions close, exactly as for
`release-publish`.

**This only holds once the REPOSITORY secrets are deleted.** An environment secret does not hide a
repository secret of the same name — the repository one still resolves. Until they are gone the job
works either way and the migration has proved nothing. See §3.1.

GitHub pauses **each** job that enters an environment, and three enter this one: `release`,
`publish-pypi` and `publish-npm`. A reviewer here would stop the run again between crates.io and
PyPI. A rejected or timed-out second approval leaves crates.io published and PyPI empty — the split
state the job order exists to prevent.

The environment must still exist. Both PyPI's and crates.io's OIDC claims bind to it.

### 3.1 Finishing the App-secret migration — the step that makes it real

1. ~~Confirm you still hold the App private key~~ — done.
2. ~~**Delete the repository secrets**~~ — **done 2026-08-29.** `gh api …/actions/secrets` returns
   an empty list, so an environment secret is now the only source.
3. **Still outstanding.** Push to `main` and confirm `release-pr` **ran** — that it reached
   release-plz and refreshed the release PR.

**Step F is that push.** Merging this issue's pull request is the next push to `main`, so it doubles
as the migration proof. Check it there before going near step H — the observation gate and this
check read the same run.

**Step 3 is the only proof, and "the run was green" is not it.** `release-pr`'s preflight makes the
whole job skip **green** when `PAIGASUS_BOT_APP_ID` is unreadable, so a botched migration looks
identical to a healthy run in the checks list. Open the run and confirm the job executed.

---

### 5.4 Credential cross-check — measured 2026-08-29

Every `secrets.X` in `release.yml` resolves on the environment of the job that reads it. There are
no repository secrets left, so a job sees only its own environment's.

| Job | Environment | Secrets needed | |
| --- | --- | --- | --- |
| `release-pr` | `release-pr` | `PAIGASUS_BOT_APP_ID`, `PAIGASUS_BOT_PRIVATE_KEY` | OK |
| `release` | `release-publish` | `PAIGASUS_BOT_APP_ID`, `PAIGASUS_BOT_PRIVATE_KEY` | OK |
| `publish-pypi` | `release-publish` | none — OIDC trusted publishing | OK |
| `publish-npm` | `release-publish` | none — OIDC trusted publishing | OK |

The "Secrets needed" column reads the **workflow file**, which stops referencing the two publish
tokens on this PR. It does **not** say the two secrets are gone from `release-publish`; that is
step C3, and §2 has the command to check the live state.

Worth re-running after any credential change. A miss is invisible at runtime for `release-pr`,
whose preflight skips **green**.

---

## 4. Step D — the crates.io seed. IRREVERSIBLE — **EXECUTED 2026-08-29**

> **Done. Do not repeat.** All three crates are published at `0.1.0-alpha.1` and verified against
> the crates.io API:
>
> | Crate | Versions | `max_stable_version` |
> | --- | --- | --- |
> | `paigasus-proto-derive` | `0.1.0-alpha.1` | `None` |
> | `paigasus-proto` | `0.1.0-alpha.1` | `None` |
> | `paigasus-kernel` | `0.1.0-alpha.1` | `None` |
>
> `max_stable_version` is `None` on all three — only the pre-release exists, which is the baseline
> §2.4 of the spec analyses. The three names are now permanently claimed, which also closes the
> squatting hazard for good. The seeds are yanked at step J, **after** verification.
>
> The procedure below is kept as the record of what was run, and for anyone repeating it for a
> future crate.



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

### 5.1 npm — the SMA-602 steady state. STEP C1, PENDING

**REQUIRED BEFORE THE SMA-602 PR MERGES — this is step C1, and it is not done until you do it.**
All nine `@paigasus/*` packages exist (published 2026-08-29 by the bootstrap token — history
below), so npm Trusted Publishing is registrable. Register each of the nine packages with owner
`SMK1085`, repository `paigasus-core`, workflow `release.yml` — with the extension — and
environment `release-publish`.

**Registration needs an OTP-capable login.** The npm trust commands wrap their registry call in
`otplease`, and 2FA-bypass granular tokens already lost the ability to change trusted-publishing
configuration (§5.1.1). The Automation token cannot do this. Use an interactive login or the web
UI.

`npm trust github <pkg> --file release.yml --repository SMK1085/paigasus-core \
  --environment release-publish` registers one package. It also honours `--dry-run`.

**`--environment` is OPTIONAL to npm, and mandatory to us.** npm defines it with `default: null`
and no `required: true` (`trust/github.js:35-40`) — only `--file` is required. Omit it and the
publisher accepts an exchange with no environment claim, which removes the §3 boundary for that
package silently. Pass it on all nine, then read all nine back with `npm trust list`.

**Which npm you run this from matters.** The flags above were measured against **npm 11.13.0**,
the npm bundled with the repo-pinned Node 24.16.0. Use **npm >= 11.5.1** on your machine — that is
the floor for trusted publishing, and `npm trust` does not exist below it. Newer npm documents
`--allow-publish` and `--allow-stage-publish`; 11.13.0 has neither (`grep -r allow-publish` over
its `lib/` returns nothing), so a publisher it creates carries whatever the registry defaults to.
**If your npm offers those flags, check whether publish is default-on** before relying on the
result — a publisher that cannot publish would fail the release at the same irreversible point a
missing one would. Check your version first:

```bash
npm --version   # must be >= 11.5.1
```

**Do NOT enable npm's "Require two-factor authentication and disallow tokens" on these nine
packages** (final review, Important 4). npm's Publishing-access settings offer it per package, and
npm's own documentation recommends it after a trusted publisher is configured. It is real
hardening and this repository deliberately declines it for now: enabling it revokes the Automation
token's ability to publish these packages, which kills §7.4 — the ONLY npm hand-recovery path this
repository has — immediately, rather than at its January 2027 expiry. `publish-npm` has never
completed a real publish, so §7.4 is the live fallback, not a theoretical one. Revisit once a
release has published through OIDC and a token-free recovery path exists.

`NPM_TOKEN` is deleted from `release-publish` at **step C3, after the merge**. §8 tracks that
removal, and its precondition: confirm the token value is stored outside GitHub first.

#### 5.1.1 History — the bootstrap token, step C, done 2026-08-29

**Already confirmed (2026-08-29):** `npm org ls paigasus` reports `smaschek - owner`, and
`npm access list packages @paigasus` was empty before the first release.

The first release created the **nine** packages under the scope: `@paigasus/node-bindings`, seven
platform packages, and `@paigasus/wasm`. Trusted Publishing could not be configured before they
existed, so the first release used a token instead:

| Property | Value |
| --- | --- |
| Type | **Automation** |
| Scope | the `@paigasus` scope, read and write — **not** "only select packages", since nine packages did not exist yet |
| Stored as | an **environment secret on `release-publish`**, not a repository secret |

The type was not a free choice: this account reports `two-factor auth: auth-and-writes` (measured
2026-08-29), so 2FA **is** enforced for writes and a classic publish token fails with *"2FA
required for publishing"*.

**Why a token and not OIDC, at that time.** npm Trusted Publishing has the same first-publish
constraint crates.io does: `npm trust` requires that *"the package you're configuring must already
exist on the npm registry"*, and `npm/cli#8544`, the request to allow an initial OIDC publish, was
still open. That constraint is now moot for this repository — the packages exist — but it still
applies to any future `@paigasus/*` package that does not yet exist.

**The token's underlying Automation credential is kept, deliberately, outside CI.** §7.4 depends
on it for hand recovery. GitHub's changelog of 2026-07-31 restricts npm 2FA-bypass granular access
tokens: they have **already** lost the ability to change package access, maintainers and
trusted-publishing configuration, and they lose **direct publish entirely in January 2027**. After
that date §7.4's recovery path stops working and this repository has no npm hand-recovery path at
all.

#### 5.1.2 Read the nine configurations back — step C2, and a PERIODIC re-check

`npm trust list <pkg> --json` reads the configuration back — an authenticated
`GET /-/package/<pkg>/trust` that needs **no** GitHub Actions context, so it verifies all nine from
a laptop. Confirm three fields on every package: `repository` is `SMK1085/paigasus-core`,
`workflow_ref.file` is `release.yml`, and **`environment` is `release-publish`**.

**Run this before the merge (step C2), and again on a schedule.** It is the only read-back this
repository has, and it covers nine of the twelve configurations. Nothing in CI can see any of them:
a registration that omitted `environment`, or a later web-UI edit that cleared it, leaves every
check in this repository green while that package's boundary is gone. Put a calendar reminder on
it — quarterly, and after any change to the npm scope, the workflow filename or the environment
name. The three PyPI publishers cannot be read back at all (§5.2); their only proof is a green
`publish-pypi`.

### 5.2 PyPI — the SMA-602 steady state. STEP C1, PENDING

**REQUIRED BEFORE THE SMA-602 PR MERGES — this is the PyPI half of step C1, and it is not done
until you do it.** Register three normal trusted publishers, one for each of
`paigasus-py-bindings`, `paigasus-kernel` and `paigasus-proto` — owner `SMK1085`, repository
`paigasus-core`, workflow `release.yml` (with the extension), environment `release-publish` on
each. **Set the environment on all three:** the field is optional on PyPI's form too, and PyPI
offers **no read-back**, so this is the one configuration nobody can verify afterwards. Its only
proof is a green `publish-pypi` on the next release.

`PYPI_API_TOKEN` is deleted from `release-publish` at **step C3, after the merge**, and revoked on
PyPI at **step C4, only after a release has published through OIDC**. §8 tracks both.

The steps below are kept as the record of the token-era bootstrap (step C, done 2026-08-29) that
made the three projects exist in the first place — a normal trusted publisher cannot be registered
before its project does.

**A token first, then trusted publishing. Pending publishers cannot work here.**

PyPI allows only **one pending** trusted publisher per `(owner, repo, workflow, environment)`
tuple. All three projects share that tuple exactly — same repository, same `release.yml`, same
`release-publish` environment — so registering the second fails with *"A pending trusted publisher
matching this configuration has already been registered for a different project name"*. That is
[pypi/warehouse#16920](https://github.com/pypi/warehouse/issues/16920), open since October 2024. It
is the documented monorepo limitation, not a misconfiguration.

The constraint binds **pending** publishers only. Normal publishers may share a tuple — that is the
ordinary monorepo case. So:

1. **Delete any pending publisher you already registered**, so nothing is half-configured.
2. Create a PyPI **API token** at <https://pypi.org/manage/account/token/>. Scope it to *"Entire
   account"* — the three projects do not exist yet, so there is nothing narrower to scope to.
3. Add it to the **`release-publish` environment** as `PYPI_API_TOKEN`. **Environment secret, not a
   repository secret** — a repository secret resolves for any run regardless of ref and would
   defeat §3's boundary.
4. The first release creates all three projects with it.
5. **Afterwards**, add a *normal* trusted publisher to each of the three projects, then delete
   `PYPI_API_TOKEN` and re-scope. **This is SMA-602's steps C1, C3 and C4 — see the summary above.
   Not done until the owner confirms each one.**

`publish-pypi` keeps `id-token: write`: it is what the normal publishers will use at step 5, and
removing it now would only have to be added back.

**Normal publisher fields, for step 5** (and for reference — they are what the pending form would
have taken):

| Field | Value |
| --- | --- |
| Owner | `SMK1085` |
| Repository name | `paigasus-core` |
| Workflow name | `release.yml` — **with the extension** |
| Environment name | `release-publish` |

The three project names are `paigasus-py-bindings`, `paigasus-kernel` and `paigasus-proto`, taken
from the `[project] name` in each `pyproject.toml`, not from the wheel filenames — the wheels use
underscores, PyPI normalizes.

**There is no slot limit to check.** PyPI's documented limit is a rate limit of 100 trusted
publishers per user or IP per 24 hours. An earlier revision of this runbook told you to confirm
three free slots; that was wrong.

### 5.3 crates.io Trusted Publishing — step E — **done 2026-08-29, UNVERIFIED FROM OUTSIDE**

**This is the one step in the sequence that cannot be checked programmatically.**
`GET /api/v1/trusted_publishing/github_configs?crate=<name>` returns HTTP 403
(*"this action requires authentication"*), so there is no unauthenticated way to confirm the three
configurations exist or that their fields are right.

**Check them by eye** at `https://crates.io/crates/<name>/settings` for each of the three, and
confirm the **Environment** field reads exactly `release-publish` — not blank, not
`release-approval`.

**The failure mode, so the risk is clear.** `release`'s first step is
`rust-lang/crates-io-auth-action`. A wrong configuration fails **there**, before `cargo publish`
runs — so it cannot half-publish. But it fails *after* the 12-leg matrix has built and *after* the
human approval, so the cost is a wasted run and a re-approval, not a split registry state.



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

**That is guaranteed here because step I is a `workflow_dispatch`.** Since SMA-603, `release.yml`
carries a `plan` job (`ci/release-plan/`) that decides whether anything is releasable, and it
always builds on a `workflow_dispatch` — a dispatch is a deliberate act meaning "release now". The
same guarantee does **not** hold for an ordinary push to `main` once the release path is live: a
push with nothing new to release makes `plan` skip the whole matrix, `approve-release` included,
so no human is asked to approve anything. See the `plan` job's comment in `release.yml`.

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
4. **Three** git tags: `paigasus-kernel-v0.1.0`, `paigasus-proto-v0.1.0`,
   `paigasus-proto-derive-v0.1.0`. Not six — `release-plz release` tags only what it
   publishes, so the three `publish = false` binding crates get none (measured).
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
| npm platform loop failed partway | Re-run `publish-npm` **in place**. `napi prepublish` skips each already-published platform package. **A fresh dispatch does NOT work** — see §7.4 |
| npm published nothing and the release is already tagged | Publish by hand from the run's artifacts — **§7.4**. A re-dispatch would go green having published nothing |

The last row's guard lives in `@napi-rs/cli`, not in this repository, and it matches on npm's error
**message text**. A napi upgrade or an npm wording change could remove it silently. Nothing gates
that.

### 7.4 Recovering the npm half by hand — EXECUTED 2026-08-29, and its two traps

The first live release published crates.io and PyPI, then `publish-npm` died on
`pnpm: command not found`. This is the recovery that was actually run. Keep it: the same shape
applies to any future partial npm failure.

**Why a re-dispatch cannot fix it.** `relinfo` reads `needs.release.outputs.released`. Once the
release commit is tagged, `release-plz release` finds the tags, returns an empty `releases` array,
and `kernel_release` becomes `false` — so every npm step **skips and the job goes green having
published nothing**. Re-running the job *within the original run* does work, because that run's
`released` output is still populated; a fresh dispatch does not.

**The procedure.** The artifacts CI built are already asserted by the workflow, so reuse them
rather than rebuilding:

```bash
gh run download <RUN_ID> --repo <owner/repo> -n npm-dirs -D npm-dirs
gh run download <RUN_ID> --repo <owner/repo> -n wasm-dist -D wasm-dist
cp -R npm-dirs rs/crates/bindings/paigasus-node-bindings/npm
```

**TRAP 1 — you cannot use your logged-in npm session.** `napi prepublish` shells out with
`execSync(..., {stdio: 'pipe'})`, so npm's one-time-password prompt has nowhere to go. On a 2FA
account (this one is `auth-and-writes`) it dies with `npm error code EOTP` on the **first** package.
It needs a credential that bypasses 2FA — the **Automation** token. SMA-602 removes the
`NPM_TOKEN` secret from `release-publish` (step C3), but the underlying npm Automation token is
deliberately KEPT on the registry precisely so this recovery stays possible.

**WHERE THE VALUE LIVES — fill this in at step C3, and do not delete the secret until you have.**
A GitHub secret is write-only: deleting `NPM_TOKEN` does not reveal its value, and no one can read
it back afterwards. "Kept outside CI" is true of the token on npm's side and says nothing about
the value. If the only copy of the value was the GitHub secret, this whole procedure is dead the
moment C3 runs.

> **npm Automation token value:** _record the password-manager vault and entry name here at step
> C3._ If it is not stored anywhere, mint a replacement Automation token on npm, store that, and
> only then delete the secret.

**That token class loses direct publish in January 2027**, after which this procedure stops
working and this repository has no npm hand-recovery path at all. Enabling npm's *"disallow
tokens"* setting on the nine packages ends it sooner — §5.1 says not to.
Point npm at a throwaway rc so your login survives:

```bash
printf '//registry.npmjs.org/:_authToken=%s\n' "$NPM_AUTOMATION_TOKEN" > /tmp/npmrc-publish
export npm_config_userconfig=/tmp/npmrc-publish
# ... publish ...
rm -f /tmp/npmrc-publish
```

**TRAP 2 — never `npm publish` the main package directly from the committed manifest.** It carries
**no** `optionalDependencies`; `napi prepublish` injects them. Publishing it as-is ships a package
that resolves no platform binary and fails at `require()`. Always run `napi prepublish` first, and
in this order:

```bash
cd ts/packages/paigasus-kernel
pnpm exec napi prepublish --no-gh-release --npm-dir npm \
  --cwd ../../../rs/crates/bindings/paigasus-node-bindings   # 7 platform packages + injection

cd ../../../rs/crates/bindings/paigasus-node-bindings
npm publish --access public                                   # main package, AFTER the seven

cd <wasm-dist>
npm publish --access public
```

Omit `--provenance` locally — it needs an OIDC context only CI has.

**Afterwards, revert `rs/crates/bindings/paigasus-node-bindings/package.json`.** napi rewrites it
in place (injected `optionalDependencies`, reflowed arrays, stripped trailing newline). That edit
must never be committed — the committed manifest deliberately carries none, and the reflow fails
`ts:fmt`.

**Verifying: retry before you believe a MISSING.** npm's read replicas lag. Immediately after a
successful publish (`exit 0`, `info ok` in `~/.npm/_logs`), both `npm view` and the registry
endpoint reported eight of nine packages missing for several minutes. Trust the publish exit
status; re-check the registry after a pause.

### 7.5 Cleanup — only after §7.1 and §7.2 pass

Verify first. A yank is cheap to do late and awkward to undo, and the seeds are the diagnostic
baseline if verification fails.

```bash
cargo yank --version 0.1.0-alpha.1 paigasus-proto-derive
cargo yank --version 0.1.0-alpha.1 paigasus-proto
cargo yank --version 0.1.0-alpha.1 paigasus-kernel
```

Then **revoke the crates.io API token** from §4.1.

---

## 8. The tracked removals

**A gate now enforces both, and here is exactly what it covers.**
`ci/actionlint/release_guard.py`'s V10 reds `release.yml` on four rules:

1. **a strict-equality allowlist of secret NAMES.** `release.yml` may reference
   `PAIGASUS_BOT_APP_ID` and `PAIGASUS_BOT_PRIVATE_KEY` and nothing else. A NEW secret name — the
   fresh project-scoped PyPI token the rollback plan would mint, for instance — reds until someone
   adds it to `EXPECTED_RELEASE_SECRETS` on purpose.
2. **any `password:`** inside the `with:` of a `pypa/gh-action-pypi-publish` step, whatever the
   value is. Under Trusted Publishing there is nothing legitimate to put there.
3. **three names on a denylist:** `PYPI_API_TOKEN`, `NPM_TOKEN`, `NODE_AUTH_TOKEN`.
4. **an npm `_authToken` or `_auth`** written anywhere the parsed document reaches. `_auth` is a
   live npm credential — `getCredentialsByURI` honours it exactly as it honours `_authToken`.

It reads the PARSED YAML, so it sees no comment. It cannot see a credential fetched at run time (a
vault call, a decoded blob), and rule 1 covers the `secrets` context only, not `vars`.
`PAIGASUS_BOT_*` must keep working, and `ci/workflow-credentials/run.sh:84` separately asserts that
release.yml still reads a secret.

**Before deleting either secret (step C3), confirm the token VALUE is stored outside GitHub and
record where.** GitHub secrets are write-only. Deleting one does not show you its value and nothing
can read it back. The two rows below say the credentials stay usable by hand; that is true of the
tokens on the REGISTRY side and false of their values, if the only copy was the GitHub secret.
Record the npm value's location in §7.4 and the PyPI value's location in the row below.

**WITHDRAWN (SMA-603): removing the `workflow_dispatch` trigger.** An earlier version of this
runbook tracked removing that trigger once the first release published. That instruction is
withdrawn — the trigger is now permanent. See its own comment in `release.yml` for the reason: a
dispatch is the "build anyway" lever for the state where release-plz has already cut tags but a
registry is still missing an artifact, and `ci/release-plan/` deliberately always builds on a
dispatch. Do not remove it.

| Item | Status |
| --- | --- |
| `PYPI_API_TOKEN` | **Superseded by SMA-602; the owner steps are PENDING.** `release.yml` stops reading it on this PR. C1 registers three normal trusted publishers **before** the merge; C3 deletes the environment secret **after** it; C4 revokes the token on PyPI **only after** a release has published through OIDC — it is the only credential that can publish to PyPI by hand, and PyPI has no documented hand-recovery procedure. Record the value's storage location before C3: _fill in at step C3_ |
| `NPM_TOKEN` | **Superseded by SMA-602; the owner steps are PENDING.** `release.yml` stops reading it on this PR. C1 registers nine trusted publishers **before** the merge; C3 deletes the environment secret **after** it. The underlying Automation token stays alive on npm as §7.4's recovery credential, until its January 2027 expiry — but only if its VALUE is stored somewhere. Record that location in §7.4 before C3 |

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
