# SMA-580 — Release activation E: pre-flight, the crates.io bootstrap, and the flip

Fifth and final increment of SMA-407. Input spec: §10 and §13 of
`docs/superpowers/specs/2026-08-22-sma-407-release-activation-design.md` (the umbrella).

**Status:** design, **revision 3**. Revision 1 was reviewed adversarially and returned NEEDS
REWORK with two blocking findings; both are **measured**, not argued (§11). Revision 3 records two
owner decisions: the owner performs the flip, and `release.yml` gains a **temporary**
`workflow_dispatch` trigger. §12 records what changed. Nothing here is implemented yet.

---

## 0. What this issue does

SMA-579 shipped the complete release path in `.github/workflows/release.yml`. The path is inert
until `vars.PAIGASUS_RELEASE_ENABLED` is `'true'`. This issue flips that variable.

The issue therefore builds no workflow machinery. It does four things:

1. It runs the pre-flight. §1 records the result of every item.
2. It settles the crates.io bootstrap. §2.
3. It ships three committed artifacts, one of which is a draft applied to Notion. §6.
4. It executes the activation sequence. §3.

**One finding changes the shape of the issue.** crates.io cannot pre-register a Trusted Publisher
for a crate that does not exist. The `release` job holds no other crates.io credential. So the
release path as designed cannot perform the first publish. §2 settles the bootstrap.

---

## 1. Pre-flight — measured 2026-08-29

Every row below is measured. The method is stated for each.

### 1.1 Registry names — all free

| Registry | Names checked | Result |
| --- | --- | --- |
| crates.io | all **13** workspace members | 13/13 return HTTP 404 — **free** |
| PyPI | `paigasus-kernel`, `paigasus-proto`, `paigasus-py-bindings`, `paigasus-ml`, `paigasus-workflows`, `paigasus` | 6/6 return HTTP 404 — **free** |
| npm | `@paigasus/{kernel,proto,node-bindings,wasm,sdk,ui,console}` | 7/7 return HTTP 404 — **free** |

Method: `GET https://crates.io/api/v1/crates/<name>`, `GET https://pypi.org/pypi/<name>/json`,
`GET https://registry.npmjs.org/<url-encoded-name>`.

**Two limits on the npm row, both real.** It checked the wrong set. Three of those names
(`sdk`, `ui`, `console`) never publish, and two (`kernel`, `proto`) are `private: true` at `0.0.0`
(§6.4 item 4). Only two of the nine names that **do** publish were checked: `@paigasus/node-bindings`
and `@paigasus/wasm`. The other **seven** — the platform packages derived from `napi.targets` — were
not. The risk is nil once the scope is owned, because a scope owner
holds every name under it. **The row is evidence about names, not about the scope.** §1.4 is the
row that matters.

**The crates.io row is an exact-name check.** crates.io treats `-` and `_` as colliding, so a
squatted `paigasus_kernel` would not appear under the queried name. §1.2's prefix search covers
that case.

### 1.2 Squatting — none

`GET https://crates.io/api/v1/crates?q=paigasus&per_page=50` returns 20 crates. All 20 carry the
`paigasus-helikon-` prefix and belong to the author's own earlier project. **No name collides with
a workspace member,** under either separator. This closes the hazard §10 of the umbrella spec
named: with `git_only` unset, release-plz performs a registry lookup for every member name, so a
squatted name would silently become the version-comparison baseline.

### 1.3 Repository state

| Item | Measured value |
| --- | --- |
| Visibility | `PUBLIC` |
| GitHub environments | **0** — neither `release-approval` nor `release-publish` exists |
| Repository variables | **0** — `PAIGASUS_RELEASE_ENABLED` is unset |
| Repository secrets | `PAIGASUS_BOT_APP_ID`, `PAIGASUS_BOT_PRIVATE_KEY`. **`NPM_TOKEN` is absent** |
| Release PR 170 | **OPEN**, `chore: release v0.1.0`, branch `release-plz-2026-08-28T12-14-48Z` |
| ADR-0011 amendment | **Not landed.** The newest amendment is 2026-06-04 (SMA-406) |
| "Which App" comment | **Absent.** Both mint steps say why a token is minted, never which App |
| Rulesets | **One**, `Protect main`, `target = branch`. Rules: `deletion`, `non_fast_forward`, `required_status_checks` (`strict = true`, requiring `moon ci`). **No `pull_request` rule.** Bypass: `RepositoryRole` at `always` |
| Tag protection | **None.** No ruleset targets `refs/tags/**`, and `GET /repos/{o}/{r}/tags/protection` returns 404 |

The last two rows close a hazard revision 1 did not check. `release-plz release` pushes six
`<package>-v0.1.0` tags from inside the `release` job, **after** the crates.io upload. A tag
ruleset would have blocked that push and split the release permanently. Nothing blocks it.

**Residual:** that the App installation can in fact create a tag and a GitHub Release on `main` is
**not** proven by the absence of protection. §3 step B3 proves it with a throwaway tag.

### 1.4 npm scope ownership — NOT established

`https://www.npmjs.com/org/paigasus` returns HTTP 403. The registry org API returns HTTP 404.
Neither result proves ownership, and neither proves the scope is free. A package 404 says nothing
about the scope. **The owner must confirm this from the npm account.** §5.2.

### 1.5 Items that need no further work

- The repository is public, so published `repository` and `homepage` URLs resolve.
- The App credentials work. PR 170 exists, which proves the `release-pr` job mints its token.
- SMA-596 is closed (`ffcaab5`), so the `repo:release-parity*` gates run locally again.

---

## 2. The crates.io bootstrap

### 2.1 The finding

RFC 3691, the normative source for crates.io Trusted Publishing, states:

> A *Trusted Publisher Configuration* can only be created after an initial manual publishing of a
> crate.

The RFC lists a `PENDING` state under **Future possibilities**. It is not implemented. The
crates.io development update of 2026-01-21 adds GitLab support, an enforcement option and a block
on two GitHub triggers. It adds no pending publishers.

The `release` job authenticates only with `rust-lang/crates-io-auth-action`. All three publishable
crates are unpublished. **The first publish therefore cannot succeed as designed.**

The failure is safe. The OIDC exchange fails before `cargo publish` runs. But the flip cannot
proceed until a bootstrap path exists.

PyPI is not affected. Its pending-publisher form covers projects that do not exist.

### 2.2 The decision — a throwaway pre-release seed

Publish `0.1.0-alpha.1` of the three crates by hand. This creates the crates, which makes Trusted
Publishing configurable. The real `0.1.0` then publishes through the automated path.

#### 2.2.1 The scratch tree

**The tree must not be a git repository, and must not sit inside one.** Cargo walks *upward* to
find a repository, so extracting under a directory that is itself tracked reintroduces the fault.
§2.5 gives the measured reason. Build it with `git archive` — not `git worktree add`, not a clone:

```
rm -rf /tmp/seed && mkdir -p /tmp/seed        # rm -rf: a stale tree from a prior run would ship
git archive HEAD | tar -x -C /tmp/seed
git -C /tmp/seed rev-parse --show-toplevel   # MUST fail: "not a git repository"
```

#### 2.2.2 The six edits

Revision 1 listed four. **That set does not resolve** — measured, §11.1. Six are required.

| # | File | Change |
| --- | --- | --- |
| 1 | `rs/crates/libs/paigasus-kernel/Cargo.toml` | `version = "0.1.0-alpha.1"` |
| 2 | `rs/crates/libs/paigasus-proto-derive/Cargo.toml` | `version = "0.1.0-alpha.1"` |
| 3 | `rs/crates/libs/paigasus-proto/Cargo.toml` | `version = "0.1.0-alpha.1"` |
| 4 | `rs/Cargo.toml:140` | `paigasus-proto-derive = { path = …, version = "=0.1.0-alpha.1" }` |
| 5 | `rs/Cargo.toml:143` | `paigasus-kernel = { path = …, version = "=0.1.0-alpha.1" }` |
| 6 | `rs/Cargo.toml:146` | `paigasus-proto = { path = …, version = "=0.1.0-alpha.1" }` |

Edits 5 and 6 are the ones revision 1 missed. Ten workspace members consume `paigasus-kernel` or
`paigasus-proto` through `[workspace.dependencies]`. A pre-release never satisfies a
non-pre-release requirement, so leaving those pins at `"0.1.0"` makes **every** cargo command in
`rs/` fail — including the derive publish, which does not touch either crate.

Cross-check the list against `SITES` in `ci/version-lockstep/run.sh`, which already enumerates
every version-carrying site, including the three `cargo-wsdep` rows.

`rs/Cargo.lock` is rewritten by the first cargo command. That is harmless in a throwaway tree.
**Do not pass `--locked`.**

#### 2.2.3 The pre-upload assertion

Run this for each of the three crates **before any upload**. It converts an irreversible discovery
into a free local red:

```
cargo package -p <crate>
tar tzf target/package/<crate>-0.1.0-alpha.1.crate | grep -c cargo_vcs_info   # MUST be 0
```

#### 2.2.4 The publish

**Do not pass `--allow-dirty`.** In the intended non-git tree the flag is inert. In the failure
case it is exactly what converts cargo's hard error into a silent success that embeds the SHA1 —
measured, §11.2. The flag removes the only automatic guard against §2.5's fault.

Publish in this order, with an index poll between the pair:

```
cargo publish -p paigasus-proto-derive
until cargo info paigasus-proto-derive@0.1.0-alpha.1 >/dev/null 2>&1; do sleep 10; done
cargo publish -p paigasus-proto
cargo publish -p paigasus-kernel
```

Only the proto pair is ordered. `paigasus-kernel` declares no in-tree dependency (measured: its
`[dependencies]` holds `uuid` and `thiserror` only).

**Why sequential rather than combined.** `cargo publish -p paigasus-proto-derive -p paigasus-proto`
also works (measured, §11.3) and resolves the order itself from the locally staged tarball, so it
needs no poll. It is the more robust form, and it is what `repo:publish-metadata` Check 2 already
runs. The sequential form is chosen anyway, because it is the **only rehearsal** the live
derive→proto path gets before step I — CLAUDE.md records that `release-plz release` has never
published this pair, and that the first live release is the first genuine test of it. The poll
closes the index-lag gap the sequential form would otherwise carry.

**Fallback.** If the poll does not converge within a few minutes, use the combined form and record
that the rehearsal was given up.

### 2.3 Why this shape

Three properties decide it.

**`release.yml` stays untouched.** The alternative that keeps release-plz in charge of the first
`0.1.0` tag is a temporary `CARGO_REGISTRY_TOKEN` secret plus a conditional around the OIDC step.
That puts a second credential path on the single irreversible job, and stores a long-lived token
that the OIDC design exists to avoid. No gate would force its later removal.

**The real `0.1.0` goes through the full path.** release-plz cuts the six tags and the two GitHub
Releases, and PyPI and npm publish from the same run. This is a real end-to-end test of the
machinery, on the version users install.

The rejected alternative — publish the real `0.1.0` by hand — loses that. release-plz skips a
version already on the registry, so it would cut no `0.1.0` tag and no GitHub Release, and its
`released` output would stay empty. `publish-pypi` and `publish-npm` read that output, so neither
would fire. The whole `0.1.0` release would become manual across three registries, and the
automated path would stay untested until `0.2.0`.

**The seed is honest.** It is the `0.1.0` code with a different version string. Cargo never
resolves a pre-release for a `0.1` requirement, so no consumer sees it.

### 2.4 The premise — measured in the source

The design assumed release-plz proposes `0.1.0` over a `0.1.0-alpha.1` registry baseline. **The
premise holds.** Read in the release-plz source at `release_plz_core` 0.36.14, the version this
repository pins through CLI 0.3.158.

**1. The registry query includes pre-releases.** `clone/mod.rs::query_latest_package_summary`
calls `Crate::new(name, None)`. A `None` version becomes `OptVersionReq::Any` in cargo 0.96.0,
whose `matches` arm returns `true` unconditionally. So the baseline is `0.1.0-alpha.1`, not
"unpublished".

**2. release-plz proposes `0.1.0`, unchanged.** `update/updater.rs::get_package_diff` holds an
explicit already-bumped branch:

```rust
if package.version > registry_package.package.version && diff.is_version_published {
    diff.set_version_unpublished(registry_package.package.version.clone());
}
```

SemVer precedence makes `0.1.0 > 0.1.0-alpha.1` true, so `is_version_published` becomes `false`.
`Diff::should_update_version` then returns `false`, and `version.rs::next_from_diff` returns the
manifest version unchanged. The pre-release increment branch (`VersionIncrement::Prerelease`) is
never reached, because `should_update_version` short-circuits before any increment runs.

**3. The content diff does not cause a skip.** `package_compare.rs::are_packages_equal` compares
the local `Cargo.toml` against the registry tarball's `Cargo.toml.orig` **first**, by hashing whole
file bytes. The version field is part of that file. `0.1.0` and `0.1.0-alpha.1` differ, so the
comparison fails immediately and release-plz never concludes "no changes".

**4. `release-plz release` publishes on an exact-version registry lookup.**
`command/release.rs::release_package_if_needed` first short-circuits on an existing git tag, then
calls `is_published`, which runs `cargo info <name>@<version>` — an exact-version query. With only
`0.1.0-alpha.1` present, `cargo info paigasus-kernel@0.1.0` reports missing, and `cargo publish`
proceeds.

**One claim is NOT cited, and §3.1 is built to survive it being wrong.** Revision 1 asserted that a
package with no version change still appears in the release PR, logged as *"updating changelog for
version 0.1.0"*. Findings 1–4 do not establish that. If it is wrong, release-plz proposes nothing
and PR 170 is simply not refreshed — which an operator could misread as "confirmed, proceed".
§3.1's decision table keys on the job's JSON output precisely so the vacuous case is
distinguishable from a pass.

### 2.5 Why the scratch tree must not be a git repository

**This is the finding revision 1 of this design missed.** It is now measured — §11.2.

With no git tags present, release-plz's commit walk has two possible stopping boundaries
(`update/updater.rs`, via `is_commit_too_old`): a tag commit, or
`registry_package.published_at_sha1()`. The second reads `.cargo_vcs_info.json` from the downloaded
registry tarball.

A seed published from a git checkout records the current HEAD in that file. release-plz would then
bound the `0.1.0` changelog at that SHA1, so the changelog would cover **only commits made after
the seed** — close to empty. That silently replaces PR 170's full-history first-release changelog
with a near-empty one.

Publishing from a non-git tree leaves `published_at_sha1()` at `None`. §11.2 measures all four
cases and confirms the mitigation.

**Confidence:** the boundary mechanism is read from the deciding code. The "covers full history"
outcome is inferred from that mechanism, not observed on this repository. Step F observes it.

### 2.6 Cleanup

Yank the three seeds **after** the §7 verification passes, not before — §3 step J. A yank is cheap
to do late and awkward to undo, and the alphas are the diagnostic baseline if verification fails.

```
cargo yank --version 0.1.0-alpha.1 paigasus-proto-derive
cargo yank --version 0.1.0-alpha.1 paigasus-proto
cargo yank --version 0.1.0-alpha.1 paigasus-kernel
```

A yank changes no resolution here, because cargo already ignores a pre-release for a `0.1`
requirement. It records the intent on the crates.io page.

**Open, see §10:** whether release-plz's registry query skips yanked versions. That decides what
the baseline is at `0.2.0`.

### 2.7 A consequence to record

`repo:publish-metadata` Check 2 runs one `cargo publish --dry-run` per publish group. The proto
group needs the combined `-p paigasus-proto-derive -p paigasus-proto` form.

**The seed does not change this,** because `paigasus-proto` requires `paigasus-proto-derive` at
`0.1.0` and the alpha does not satisfy that requirement — measured, §11.3.

**This reason expires at step I.** Once the real `0.1.0` derive is published, a per-package
dry-run of `paigasus-proto` would resolve. The grouping rule must stay anyway: it is what makes
Check 2 correct for the *next* unpublished sibling. Keep the rule; do not keep this reason for it.

### 2.8 Check 2 does not red on `main` after the release

A concern raised in review, and closed by measurement. `cargo publish --dry-run` of a name and
version already on crates.io **succeeds** — it never consults the registry for its own version, and
the "already exists" error comes from the server at upload, which a dry run skips. Measured with
`ripgrep@14.1.0`, §11.5.

So `main` sitting at `0.1.0` after the release does not red `repo:publish-metadata` on every
subsequent pull request.

---

## 3. The activation order

`release.yml` triggers on `push: branches: [main]` only. **This issue adds a temporary
`workflow_dispatch` trigger** (§6.3), so step I is an explicit dispatch rather than a push.

**Steps F, G and I depend on a workflow trigger. The rest are manual actions with no trigger.**

| Step | Action | Trigger | Owner | Reversible |
| --- | --- | --- | --- | --- |
| **A** | Write this issue's PR — runbook, `release.yml` comment + dispatch trigger, ADR amendment | — | agent | yes |
| **B1** | Create both environments with the **full** settings in §4 | — | agent | yes |
| **B2** | Add the required reviewer to `release-approval` | — | agent | yes |
| **B3** | Prove the App can push a tag and cut a Release (throwaway tag, then delete) | — | agent | yes |
| **C** | `@paigasus` npm scope, `NPM_TOKEN`, three PyPI pending publishers | — | owner | yes |
| **D** | Publish the three `0.1.0-alpha.1` seeds (§2.2) | — | owner | **no** |
| **E** | Configure crates.io Trusted Publishing for the three crates (§5.3) | — | owner | yes |
| **F** | Merge this issue's PR. **OBSERVATION GATE**, §3.1 | the merge in F | owner | yes |
| **G** | Merge PR 170. The path is still gated, so nothing publishes | the merge in G | owner | yes |
| **H** | **THE FLIP** — set `PAIGASUS_RELEASE_ENABLED` to `true` | — | **owner** | yes, until I |
| **I** | **Dispatch `release.yml` on `main`.** `approve-release` pauses. The owner approves | the dispatch | owner | **no** |
| **J** | Verify §7. **Then** yank the seeds and remove the dispatch trigger | — | owner | — |

**Steps B1–B3 need `administration` scope**, which this session's token does not carry (`repo`
only). If the agent cannot create the environments, they move to the owner; §4 states the full
settings either way. **Step H is the owner's, by decision, not by capability.**

Steps C, D and E use the runbook from the PR branch, before F merges it.

### 3.0 Why B is three steps

**GitHub auto-creates a referenced environment on first use, with no protection rules.** So
skipping step B and performing it wrongly produce the *identical* outcome: `approve-release` walks
straight through, and the entire irreversible stage runs unattended.

SMA-579 §4.5 records the same trap as its own table row — *"the environment alone does nothing"*.
The reviewer is the substance; the environment is not. Splitting the step is what stops a runbook
reader treating "environment exists" as done.

### 3.1 Step F — the observation gate

Merging this issue's PR is a push to `main`, so it re-runs `release-pr` automatically. That job only
opens or updates a pull request. It publishes nothing, so the observation is free.

**Read the `release-pr` job's `--output json` line, which `release.yml` already echoes to the log.
Do not judge by the visual state of PR 170,** and do not identify the release PR by number —
release-plz opens a new one if 170 is ever closed. Use `.prs[0].number`.

| Observation | Meaning | Action |
| --- | --- | --- |
| `.prs[0]` proposes `0.1.0` for all three crates | §2.4 confirmed | **Proceed** |
| `.prs` is `[]`, or no PR is refreshed | §2.4's uncited claim is wrong. The gate is **vacuous**, not passed | **Stop.** Diagnose before flipping |
| Any other version proposed | §2.4 is wrong | **Stop. Do not flip** |

Check the changelog too. §2.5 predicts it still covers the full history. A near-empty changelog
means the seed embedded `.cargo_vcs_info.json`.

If the gate stops the sequence, the loss is three burned pre-release versions.

**Cost warning.** §2.4 finding 3 has an unstated consequence that lands here. Because
`are_packages_equal` never matches, the commit walk has no stopping boundary at all, and must
compare the package at each commit across `main`'s full history. Today, with no registry package,
there is nothing to compare and the walk is cheap. After the seed it is not. `release-pr` carries
`timeout-minutes: 20`. A timeout at F reads as "the gate broke" and blocks the sequence with three
alphas already burned. **Record the F run's wall-clock as acceptance evidence**, and raise the
timeout before step D if the margin looks thin. The cost is self-limiting: tags exist after step I.

### 3.2 Why G runs before H

Step G merges PR 170 while the path is still gated, so the changelogs reach `main` safely and
nothing publishes.

The reverse order carries a window. If the flip came first, any other push to `main` before the
PR 170 merge would fire the release. The cost is bounded but real: the publish itself would still
be the correct `0.1.0` code, but the two GitHub Releases would carry thin notes, because the
`CHANGELOG.md` files PR 170 creates would not yet exist on `main`.

### 3.3 Step I — an explicit dispatch

**Owner decision.** Revision 2 proposed re-running G's workflow run. That rested on two premises
this design never measured: that a re-run re-reads repository variables, and that "re-run all jobs"
re-executes jobs that previously **skipped** rather than only those that ran. A temporary
`workflow_dispatch` trigger removes both.

So step I is: flip at H, then dispatch `release.yml` against `main`. The commit released is
`main`'s head, which after G is PR 170's merge — the same commit the re-run would have used, chosen
explicitly instead of inferred.

**What the trigger costs, and where the boundary actually is.** Revision 3 first claimed the flag
and `approve-release` bound this trigger. **That claim was wrong**, and a local review caught it
before the branch was pushed.

A dispatch runs the workflow definition **from the dispatched ref**. Anyone with write access can
push a branch carrying an edited `release.yml` — flag check deleted, `environment:` keys deleted —
and dispatch that ref. Every in-workflow control is attacker-controlled there. So no `if:`, no
`environment:` key, and no `github.ref` check inside the file can bound this trigger.

**The boundary is enforced outside the repository, and it closes in both directions:**

- The edited copy **keeps** `environment: release-publish` → that environment's deployment branch
  policy is **`main` only**, so a job entering it from any other ref fails.
- The edited copy **removes** it → the OIDC token carries no environment claim, and the crates.io
  and PyPI trusted publishers are configured to **require** `release-publish`, so both registries
  reject it. `NPM_TOKEN` is an environment secret on the same environment, so npm loses its
  credential too.

This is why §4.2's branch policy is `main`-only rather than "permits `main`", and why §5.1 and
§5.3 make the environment field mandatory rather than recommended. **Those three settings are the
authorization boundary.** Relaxing any one of them re-opens the hole.

The trigger is also **temporary**. §6.3 states the removal condition and §9.1 tracks it.

`release.yml` must still never gain `pull_request` or `pull_request_target`. `workflow_dispatch`
carries no such prohibition, and no gate in `ci/` asserts anything about this file's trigger set —
checked, not assumed.

### 3.3.1 What the boundary does NOT cover — `release-pr`, an open decision

Found in the PR review, after §3.3's boundary was written. **§3.3 covers only the jobs that enter
`release-publish`.** `release-pr` enters no environment at all.

It mints an App token with `contents: write` and `pull-requests: write` from **repository**
secrets, and a repository secret is readable by any run of the workflow regardless of ref. So a
dispatched ref can still reach those credentials — by adding a step, or by editing a script the
job already runs, such as `ci/version-lockstep/run.sh`.

**How much this escalates is genuinely limited.** `create-github-app-token` defaults `repositories`
to the current repository, so the minted token is repo-scoped and grants roughly what a
write-access holder already has. It is masked and revoked in the action's post-step. The real loss
is auditability: a dispatch runs without a pull request and without `moon ci`.

**What was done:** `release-pr` now carries `if: github.event_name == 'push'`. This closes the
**accidental** case only — a dispatch never mints the token by mistake. It is **not** a boundary:
an edited copy of `release.yml` on the dispatched ref simply deletes the line. The spec says so,
and so does the workflow comment, because overstating this is exactly the error §3.3 was written
to correct.

**The proper fix, and why it is not in this branch.** Move `PAIGASUS_BOT_APP_ID` and
`PAIGASUS_BOT_PRIVATE_KEY` from repository secrets to **environment** secrets on a `main`-only
environment, and give `release-pr` that environment. Then the same both-directions property holds:
keeping the environment hits the branch policy, removing it loses the secrets.

It is deliberately not done blind. `release-pr`'s preflight step makes the whole job **skip green**
when `PAIGASUS_BOT_APP_ID` is absent, so a botched secret migration is invisible — it looks
identical to "not configured yet". This is the only job in the release path that currently works,
and the migration cannot be verified before merge. **Owner decision — §10.2.**

### 3.4 The branch ruleset does not obstruct the sequence

`strict = true` (§1.3) means a pull request must be up to date with `main` before it merges. Step
F's merge therefore leaves PR 170 behind `main`. This resolves itself: `release-pr` runs on that
same push and **force-updates PR 170's branch**, which is the refresh step F reads. So PR 170 is
mergeable at step G without manual intervention.

### 3.5 What step I actually does

The run starts `wheels`, `prebuild` and `proto-dist`. They build every artifact. Then
`approve-release` enters the `release-approval` environment and pauses. **Everything after the
approval is irreversible.**

`release` publishes the three crates, cuts six tags and creates two GitHub Releases.
`publish-pypi` uploads three projects. `publish-npm` publishes nine packages.

`release-pr` is ungated, so it also runs on G's merge. It may open a
second release pull request proposing `0.1.0` again over the alpha baseline. **The operator must
not merge it.** The runbook says so explicitly.

---

## 4. The two GitHub environments

Revision 1 specified reviewers only. Every setting below matters, because each one can split the
release by a different mechanism.

### 4.1 `release-approval`

| Setting | Value | Reason |
| --- | --- | --- |
| Required reviewers | the repository owner | The one place a human can stop the run. `approve-release` is its only consumer |
| Prevent self-review | **OFF** | The owner dispatches the step-I run *and* must approve it |
| Wait timer | 0 | Nothing to delay |
| Deployment branch policy | must permit `main` | A policy excluding `main` fails the job |

### 4.2 `release-publish` — no reviewers, deliberately

| Setting | Value | Reason |
| --- | --- | --- |
| Required reviewers | **none** | See below |
| Wait timer | **0** | A wait timer delays *each* of the three jobs independently — the same split-state fault by a different mechanism |
| Deployment branch policy | must permit `main` | Excluding `main` fails `release` **after** the approval was given |

GitHub pauses **each** job that enters an environment. Three jobs enter this one: `release`,
`publish-pypi` and `publish-npm`.

A reviewer here would stop the run again between crates.io and PyPI. A rejected or timed-out second
approval then leaves crates.io published and PyPI empty. That is the split state the job order
exists to prevent. `release.yml` states this rule and names this issue.

The environment must still exist, because both PyPI's and crates.io's OIDC claims bind to it.

---

## 5. Credentials

### 5.1 PyPI — three pending publishers

Projects: `paigasus-py-bindings`, `paigasus-kernel`, `paigasus-proto`.

| Field | Value |
| --- | --- |
| PyPI project name | one per project, above |
| Repository URL / owner + name | this repository |
| Workflow filename | `release.yml` — **with the extension** |
| Environment name | `release-publish` |

`publish-pypi` runs with `id-token: write` and no token.

**VERIFY before step D.** PyPI caps pending publishers per account, and three are needed. Confirm
that **three slots are free**, not merely that a cap exists, and confirm the field labels against
the live form. A wrong field fails *after* crates.io has published.

### 5.2 npm — an Automation token, as an environment secret

**The token type is not free choice.** SMA-579 §4.5 measured it: `NPM_TOKEN` must be an
**Automation** token, because a classic publish token fails with *"2FA required for publishing"* on
an account with 2FA enforced. Revision 1 of this spec said "granular" with no reason. That was a
silent regression against a measured decision, and it is reverted.

| Property | Value |
| --- | --- |
| Type | **Automation** |
| Scope | the `@paigasus` scope, read and write — not "only select packages", since nine packages do not exist yet |
| Stored as | an **environment secret on `release-publish`**, not a repository secret |

`publish-npm` already declares `environment: release-publish`, so `secrets.NPM_TOKEN` resolves from
an environment secret with **zero workflow change**. A repository secret is readable by every
workflow in the repository; this is the single long-lived registry write credential here, so it gets
the narrower scope. `repo:workflow-credentials` asserts only that no `pull_request` workflow
*declares* a credential — its README's non-goals say it says nothing about whether one could be
obtained.

Trusted Publishing cannot be configured before a package's first publish, the same constraint
crates.io imposes. The token becomes unnecessary after the first release — §9 names the follow-up.

The first release creates **nine** packages under the scope: `@paigasus/node-bindings`, seven
platform packages named for the `napi.targets` entries, and `@paigasus/wasm`.

Scope ownership is unconfirmed (§1.4). Confirm or create it at step C.

### 5.3 crates.io — Trusted Publishing

Three configurations, created at step E after the seed. Same treatment as §5.1, because the form
takes the same shape:

| Field | Value |
| --- | --- |
| Repository owner + name | this repository |
| Workflow filename | `release.yml` — with the extension |
| Environment | `release-publish` |

The environment field matters. `release` runs under `environment: release-publish`. Leaving it
blank makes the configuration broader than intended. Filling it with `release-approval` makes the
OIDC exchange fail — after the human approval and after the full matrix build.

### 5.4 The local crates.io API token — the exception §2.3 does not otherwise acknowledge

§5.3 says no token is stored, and §2.3 rejects a bootstrap `CARGO_REGISTRY_TOKEN` partly because
"no gate would force its later removal". Both are true of *CI*. The seed still needs a crates.io
API token on the operator's machine, for `cargo publish` at step D and `cargo yank` at step J.

| Property | Value |
| --- | --- |
| Scope | `publish-new` and `yank` only. Not `publish-update`, not `change-owners` |
| Lifetime | created at step D, **revoked at step J** |
| Location | the operator's machine only. Never a repository or environment secret |

This is a real exception to the design's credential argument. It is bounded by being local,
narrowly scoped and explicitly revoked.

### 5.5 The App installation

The mint steps request `contents: write` and `pull-requests: write` explicitly.
`actions/create-github-app-token` errors when it asks for a permission the App does not hold.
So an under-granted App fails at mint time, before anything publishes. PR 170 proves the grant is
sufficient for `release-pr`. The `release` job mints a second token, because tokens are per job.

Step B3 proves the tag and Release capability separately (§1.3 residual).

---

## 6. Artifacts

### 6.1 `docs/ops/RUNBOOK-release-activation.md` — committed

The operational sequence of §3, with exact commands, exact field values, and the abort points
marked. It records §2.2's six edits and the no-`--allow-dirty` rule, §2.5 so nobody seeds from a
git checkout, §2.7's grouping rule, and §3.5's "do not merge the second release PR".

### 6.2 The `release.yml` comment — committed

`release.yml` carries two mint steps. Neither says **which** App the workflow uses. A grep for
`existing`, `second App` and `Paigasus bot` returns nothing (measured 2026-08-29).

Add one line at **both** mint steps, saying the workflow reuses the existing Paigasus bot GitHub
App. The purpose is to stop a reader creating a second App while debugging a skipped job.

### 6.3 The temporary `workflow_dispatch` trigger — committed

Add to `release.yml`'s `on:` block:

```yaml
on:
  push:
    branches:
      - main
  # TEMPORARY (SMA-580). Step I of the activation sequence dispatches this workflow explicitly
  # rather than re-running a skipped run, which removes two unmeasured premises about re-run
  # semantics. REMOVE once the first release has published — see §9.1.
  #
  # THE AUTHORIZATION BOUNDARY IS NOT IN THIS FILE. A dispatch runs the definition from the
  # DISPATCHED REF, so the `if:` gate and the `environment:` keys are attacker-controlled there.
  # See §3.3 for what actually bounds it, and for the `release-pr` residual it does NOT bound.
  workflow_dispatch:
```

No `inputs:`, so `repo:actionlint`'s branches-filter extractor has nothing to parse and needs no
`BRANCH_SKIP` entry. `release_guard.py` gates on the jobs' `if:`/`needs:` chain, not on triggers,
so V1 is unaffected. `repo:workflow-credentials` applies only to `pull_request` and
`pull_request_target` workflows. **All three checked, not assumed.**

### 6.4 The ADR-0011 amendment — a committed draft, applied to Notion

This repository holds no ADR directory; ADRs live in Notion, as CLAUDE.md states. **The ADR itself
is therefore not in this repository.** What the pull request carries is a committed *draft* —
`docs/superpowers/specs/2026-08-29-sma-580-adr-0011-amendment-draft.md` — so the wording is
reviewable alongside the rest of the change. The owner applies it to the Notion page separately,
and the draft may be deleted afterwards.

The umbrella spec §13 asks for four items. This issue adds a fifth.

1. **S1 clarification.** Proto's lockstep is realized structurally, through the committed generated
   code plus S5 file-path attribution. No contract version is introduced.
2. **S4 activation shape.** `release-pr` is live. `release` is gated behind a repository variable.
   The guard lives in `ci/actionlint/run.sh` and protects the mechanism, not the decision.
3. **Decision G deferred again, with the reason.** semantic-release ejects `@paigasus/sdk` and
   `@paigasus/ui` to `1.0.0` on their first breaking change, while release-plz and
   python-semantic-release stay in 0.x. Both packages are `private: true` at `0.0.0`.
   semantic-release therefore governs no package that publishes, so the premise for the decision
   has not arrived. **The trigger that reopens it: either package dropping `private: true`.**
4. **The temporary S1 exception.** `@paigasus/kernel` and `@paigasus/proto` sit at `0.0.0` while
   their family siblings move to `0.1.0`. They rejoin at the family's *current* version, not at
   `0.1.0`.
5. **NEW — the crates.io bootstrap exception to S3.** S3 says the tool owns every tag. The seed
   publishes three versions the tool did not cut. It places **no tag**, so it does not reproduce
   the SMA-385 failure, which was caused by hand-placed tags carrying no release-plz metadata.
   Record the seed, its reason, and the fact that release-plz still owns every tag including
   `0.1.0`.

---

## 7. Verification

### 7.1 During the run

- The step-I run shows `approve-release` in the **`waiting`** state before anything publishes. If
  it does not, step B2 was not performed and the run must be cancelled.

### 7.2 Presence

1. crates.io serves `paigasus-kernel`, `paigasus-proto` and `paigasus-proto-derive` at `0.1.0`.
2. PyPI serves `paigasus-py-bindings`, `paigasus-kernel` and `paigasus-proto` at `0.1.0`.
   `paigasus-py-bindings` carries seven wheels and one sdist.
3. npm serves nine packages at `0.1.0`.
4. Six git tags of the form `<package>-v0.1.0` exist.
5. Exactly **two** GitHub Releases exist for the release commit, one per family head.
6. `moon ci` stays green on `main`.

### 7.3 Installability — inside the 72-hour npm window

Presence is not correctness. `release.yml` records the specific hazard: `napi prepublish` publishes
only the seven platform packages, and the main package's `optionalDependencies` name packages that
must already exist. Wrong ordering leaves `npm install @paigasus/node-bindings` 404ing forever
while all nine packages are "served at `0.1.0`". `paigasus-kernel` on PyPI pins
`paigasus-py-bindings==0.1.0` exactly, so an upload-order fault is likewise invisible to a presence
check.

Run all three, in a clean environment, **before the 72-hour npm unpublish window closes**:

| Registry | Check |
| --- | --- |
| PyPI | `pip install paigasus-kernel` in a fresh venv, then the `prn_canonicalize` call `wheels.yml` already uses |
| npm | `npm i @paigasus/node-bindings` in an empty directory, then `require()` it |
| crates.io | `cargo add paigasus-kernel` in a scratch crate, then `cargo build` |

### 7.4 Then, and only then

Yank the three seeds (§2.6) and revoke the local crates.io token (§5.4).

---

## 8. Risks

| Risk | Bound |
| --- | --- |
| release-plz mis-proposes over a pre-release baseline | §2.4 measures the source. §3.1's three-way table catches both a wrong version and a vacuous gate, before the flip |
| The seed embeds `.cargo_vcs_info.json` and truncates the `0.1.0` changelog (§2.5) | §2.2.1's non-git tree prevents it, §2.2.3 asserts it locally before upload, §3.1 reads the changelog |
| `--allow-dirty` hides a git seed tree | The flag is removed from the procedure (§2.2.4) |
| The six-edit set is wrong and cargo will not resolve | Measured, §11.1. The operator hits it locally at step D, before any upload |
| **`release-plz release`'s live derive→proto publish fails** | The highest-likelihood failure of the irreversible job: CLAUDE.md records that this path has never run live. §2.2.4's sequential seed rehearses it. Recovery: re-run the `release` job — release-plz's `is_published` and existing-tag short-circuits make it converge |
| **Partial multi-registry failure** | See §8.1 — it has no complete bound today |
| crates.io rate-limits new crates | Three new crates land in one session. crates.io documents a burst of **5** per account, so three fits. **Provenance is weak** — the docs page renders client-side and could not be fetched. If refused, wait and retry; earlier seeds stay valid |
| The `workflow_dispatch` trigger lets a non-owner fire an irreversible release | Bounded **outside** the workflow file: the `release-publish` branch policy is `main`-only and all three registry credentials require that environment (§3.3). Nothing in the file bounds it |
| A dispatched ref reaches `release-pr`'s **repository-level** App secrets (§3.3.1) | **NOT bounded by the above** — `release-pr` enters no environment. Narrowed, not closed. Open decision |
| The dispatch trigger is left in place after the release | §9 names it as a tracked removal with its condition. **Nothing in CI enforces the removal** — this is a real residual |
| A PyPI publisher field is wrong, or no slots are free | Fails after crates.io published. §5.1 verifies at step C, before D |
| The npm scope is not owned, or the token is the wrong type | Fails after crates.io **and** PyPI published. §1.4 and §5.2 confirm at step C |
| A tag ruleset blocks release-plz's six tag pushes | Measured absent, §1.3. Step B3 proves the App can actually push one |
| The App is under-granted | `create-github-app-token` errors at mint time, before any publish |
| `release-pr` times out at the observation gate (§3.1) | Record the F run's wall-clock; raise `timeout-minutes` before step D if thin |
| The three build jobs have never run **as callees of `release.yml`** | Both carry `workflow_call` and have run green standalone. A caller-context failure lands **before** `approve-release`, while everything is still reversible |

### 8.1 Partial-failure recovery — the honest gap

`publish-pypi` and `publish-npm` are parallel siblings on `needs: [.., release]`. Four states are
reachable, and **all four now have a clean recovery** — the fourth only since the §11.4
measurement. Revision 2 recorded it as unbounded.

| State | Recovery |
| --- | --- |
| `release` failed partway | Re-run it. `is_published` and the tag short-circuit converge |
| crates.io done, PyPI failed | Re-run `publish-pypi`. `skip-existing: true` makes it converge |
| crates.io done, npm main packages failed | Re-run `publish-npm`. The `npmstate` pre-check skips what landed |
| npm platform loop failed partway | Re-run `publish-npm`. `napi prepublish` skips each already-published platform package — measured, §11.4 |

`release.yml`'s `npmstate` pre-check queries only `@paigasus/wasm` and `@paigasus/node-bindings`,
so the **seven platform packages** have no guard in the workflow. Revision 2 recorded that as an
unbounded state. **It is bounded, in the publisher rather than the workflow** — measured, §11.4:
`napi prepublish` catches npm's 403 per target and continues the loop.

So `publish-npm` is re-runnable in every partial state, and **no `release.yml` change is needed**.
Two limits worth keeping: the guard is `@napi-rs/cli`'s, not this repository's, so a napi upgrade
could remove it silently; and it matches on the error **message text**, so an npm wording change
would break it. Neither is gated.

**Rollback reality.** crates.io supports `cargo yank` only — never delete, never reuse. PyPI
supports delete but never reuse. npm supports unpublish within 72 hours only. A wrong publish is a
permanent version burn in two of three registries.

---

## 9. Scope and tracked removals

### 9.1 The two tracked removals

Both are temporary by decision. Neither is enforced by any gate, which is why they are named here.

| Item | Removal condition | Owner |
| --- | --- | --- |
| `workflow_dispatch` on `release.yml` (§6.3) | The first release has published and §7 has passed. Remove at **step J**, in the same pull request that records the outcome | owner |
| `NPM_TOKEN` (§5.2) | Every `@paigasus/*` package exists, so npm Trusted Publishing becomes configurable. **Needs a filed follow-up issue.** Set the token's expiry short enough to bound the gap | owner |

### 9.2 Out of scope

- Any change to `release.yml`'s job graph, credentials or gating. The **trigger** change in §6.3 is
  in scope by owner decision; the §8.1 platform-loop guard is the one remaining candidate, and §10
  decides it.
- crates.io Trusted Publishing beyond the three configurations §5.3 creates.
- A CI gate that proves the release path is configured. §1.3 shows the path is silently inert when
  a credential is absent, and nothing in CI says so. That gap is real and is not closed here.

---

## 10. Questions

### 10.1 Settled by the owner

- **Who performs the flip?** The **owner**, by decision. Step H is no longer delegated. Steps
  B1–B3 stay with the agent if its token permits, and move to the owner if not (§3).
- **`workflow_dispatch` or a re-run for step I?** **`workflow_dispatch`, temporarily** (§3.3,
  §6.3). It removes two unmeasured premises. Its removal is tracked in §9.1.

### 10.2 Still open

1. **Do the App secrets move to an environment (§3.3.1)?** `release-pr` reaches repository-level
   `contents: write` credentials from a dispatched ref. The escalation is narrow — a repo-scoped
   token granting about what a write-access holder already has — but it is real, and the `if:`
   added in this branch narrows it without closing it. The migration cannot be verified before
   merge, because a botched one skips green. **Decide before, or together with, step H.**
2. **Does release-plz's registry query skip yanked versions?** If it does not, `0.1.0-alpha.1`
   stays the baseline forever and §2.4's analysis re-applies at `0.2.0`. If it does, the baseline
   silently becomes "unpublished" again after step J. Knowable before step J, either way.
3. **Does one `crates-io-auth-action` OIDC exchange yield a token valid for all three crates?** The
   action runs once; release-plz publishes three; three separate trusted-publisher configs exist.

---

## 11. Measurements

Performed 2026-08-29 on this branch, in
`scratchpad/seed`, a `git archive HEAD` extraction. **Nothing was uploaded to any registry.**

### 11.1 Revision 1's four-edit set does not resolve

Applied revision 1's table verbatim, then `cargo metadata`:

```
exit=101
error: failed to select a version for the requirement `paigasus-kernel = "^0.1.0"`
candidate versions found which didn't match: 0.1.0-alpha.1
required by package `paigasus-node-bindings v0.1.0`
```

Adding edits 5 and 6 (§2.2.2) gives `exit=0`. **BLOCKER confirmed and fixed.**

### 11.2 `.cargo_vcs_info.json` — all four cases

| Tree | Flag | Result |
| --- | --- | --- |
| non-git (`git archive`) | none | **12 files, no `.cargo_vcs_info.json`.** Packaged and verified clean |
| git, clean | none | 13 files, `.cargo_vcs_info.json` present with `sha1` |
| git, dirty | none | **hard error** — *"files in the working directory contain changes…"*. This is the guard |
| git, dirty | `--allow-dirty` | **13 files, `.cargo_vcs_info.json` with `sha1` and `"dirty": true`.** Silent success |

The last row is why §2.2.4 removes the flag: it converts the guard into a silent fault.

### 11.3 The proto pair

- `cargo publish --dry-run -p paigasus-proto` alone: **exit 101**, *"no matching package named
  `paigasus-proto-derive` found"*. Confirms §2.7.
- `cargo publish --dry-run -p paigasus-proto-derive -p paigasus-proto`: **succeeds**, cargo
  resolves the order itself and both reach *"aborting upload due to dry run"*.

### 11.4 `napi prepublish` skips an already-published platform package

`@napi-rs/cli` 3.7.2, `dist/index.js:3451-3463`. Inside the per-target loop:

```js
try {
  const output = execSync(`${npmClient} publish`, { cwd: pkgDir, ... })
  process.stdout.write(output)
} catch (e) {
  if (e instanceof Error && e.message.includes("You cannot publish over the previously published versions")) {
    console.info(e.message)
    debug$3.warn(`${pkgDir} has been published, skipping`)
  } else throw e
}
```

It catches that one error, logs, and **continues the loop**. Any other error rethrows. So the
platform loop is idempotent on re-run. Closes revision 2's one unbounded partial-failure state.

### 11.5 A dry run of an already-published version succeeds

A scratch crate declaring `name = "ripgrep"`, `version = "14.1.0"` — definitively on crates.io —
runs `cargo publish --dry-run` to *"aborting upload due to dry run"*. Confirms §2.8.

---

## 12. What changed

Folded in from the adversarial review, after independent measurement where possible.

**Blocking, both measured:** the seed edit set is six edits, not four (§2.2.2, §11.1); and
`--allow-dirty` is removed, because it suppresses the only guard against §2.5's own finding
(§2.2.4, §11.2). A pre-upload assertion was added (§2.2.3).

**Substantive:** the seed publish now polls the index between derive and proto, with the choice of
sequential over combined stated and justified (§2.2.4). §3.1 became a three-way decision table
keyed on JSON output, so a **vacuous** gate is distinguishable from a pass, and §2.4 now marks its
one uncited claim (§2.4, §3.1). Step B became B1/B2/B3, because GitHub auto-creates unprotected
environments (§3.0). Both environments gained wait-timer and branch-policy settings (§4).
`NPM_TOKEN` reverted to an **Automation** token per SMA-579's measurement, moved to an environment
secret, and gained an explicit scope (§5.2). crates.io Trusted Publishing gained its field values
(§5.3). The local crates.io API token is now named, scoped and revoked (§5.4). Verification gained
installability checks and an `approve-release` waiting check (§7). §8 gained rows for the live
derive→proto publish, tag rulesets and the observation-gate timeout, plus §8.1's partial-failure
table and its one unbounded state.

**Closed by measurement:** tag protection is absent (§1.3); `repo:publish-metadata` Check 2 does
not red `main` after the release (§2.8, §11.5).

**Corrected:** §1.1's npm row measured names that mostly never publish; §2.7's reason expires at
step I; step J now verifies before yanking; the release PR is identified by JSON, not by number
170; §6.3 is a Notion artifact, not a committed one; §3's "every step names its own trigger"
contradicted its own table.

**Rejected:** nothing. Every finding was either folded in or converted into an §10 question.

### Revision 3 — two owner decisions

**The owner performs the flip.** Step H is no longer delegated to the agent (§3, §10.1). Steps
B1–B3 stay with the agent only if its token carries `administration` scope; it currently carries
`repo` only, so they may move to the owner as well. §4 states the full settings either way, so the
outcome does not depend on who applies them.

**`release.yml` gains a temporary `workflow_dispatch` trigger** (§6.3), and step I becomes an
explicit dispatch instead of a re-run of a skipped run (§3.3). This removes revision 2's two
unmeasured premises about re-run semantics. It is now a third committed artifact, so §9.2's
"no `release.yml` changes" exclusion is narrowed to the job graph, credentials and gating — the
trigger set is in scope by decision.

The trigger widens who can fire an irreversible release, and **nothing inside the workflow file
bounds it** — a dispatch runs the definition from the dispatched ref. §3.3 states the external
boundary: the `release-publish` deployment branch policy set to `main`-only, plus all three
registry credentials requiring that environment. Trigger removal at step J is operational cleanup,
**not** part of that boundary. §9.1 tracks it and `NPM_TOKEN` together, because **no gate enforces
either removal**. §3.3 also records the one path the boundary does not cover — `release-pr`.

**Closed by measurement:** `@napi-rs/cli` 3.7.2's `prepublish` skips an already-published platform
package rather than aborting (§11.4), so §8.1's one unbounded partial-failure state is bounded and
**no `release.yml` job-graph change is needed**. That was the last blocking unknown.

Checked, not assumed: no gate in `ci/` asserts anything about `release.yml`'s trigger set.
`repo:actionlint`'s branches-filter extractor has nothing to parse in a bare `workflow_dispatch`,
`release_guard.py` gates on the jobs' `if:`/`needs:` chain rather than on triggers, and
`repo:workflow-credentials` applies only to `pull_request` and `pull_request_target` workflows.
