# GO-PUBLIC RUNBOOK — flipping `paigasus-core` to public visibility (SMA-520)

Operator-facing procedure for making `SMK1085/paigasus-core` a public repository. The motive is
cost: GitHub Actions spend reached ~$22/month against a 3,000-minute allowance, and the 2026
pricing terms state plainly that *"Standard GitHub-hosted or self-hosted runner usage on public
repositories will remain free."* That sentence is uniform across runner classes — there is **no
macOS carve-out**, which is the clause that matters here, because macOS is 43% of this repo's bill
on 7% of its wall-clock. Every runner label in `prebuild.yml` (`ubuntu-latest`,
`ubuntu-24.04-arm`, `windows-latest`, `macos-latest`) is standard-class.

The design rationale — why the bill looks the way it does, why `prebuild.yml` was the cause, and
what SMA-520 changed in it — lives in
[`docs/superpowers/specs/2026-08-16-sma-520-cut-actions-spend-design.md`](../superpowers/specs/2026-08-16-sma-520-cut-actions-spend-design.md).
This runbook does not repeat it. It covers only the flip.

**Read this before anything else below**: the workflow changes shipped in SMA-520 (path filtering
and the merged darwin job) reduce the bill by well under 15%. **The flip is the fix.** Everything
else is preparation for it, or insurance against it not happening.

---

## 1. This is one-way with respect to disclosure

Visibility can be toggled back in the UI. That does **not** undo publication:

- Anything cloned, forked, or scraped during the public window stays out. There is no recall.
- Reverting to private **detaches existing forks into a separate network** — they do not
  disappear, and they retain the code.
- Commits reachable only by SHA (see §2) remain served for as long as they were public.

Treat every check below as a gate, not a formality. There is no rollback for a leaked credential —
only rotation.

---

## 2. Pre-flight A — credential scan

**Status: executed 2026-08-16 — CLEAN.** Two passes, both with zero findings:
>
> | Pass | Scope | Result |
> | -- | -- | -- |
> | 1 | 777 commits, standard refs only | 0 findings |
> | 2 | **1,824 commits incl. `refs/pull/*`** (2.06 GB, 24m43s) | **0 findings — `no leaks found`** |
>
> Pass 2 is the one that counts; §2's `refs/pull/*` note explains why pass 1 alone would have
> left most of the disclosure surface unscanned.

Scan the whole history, not the working tree: a credential that was ever committed is exposed even
if a later commit removed it.

**The repository is still private at this point**, so the HTTPS clone needs credentials — an
unauthenticated `git clone` here fails with a 403 that reads like a missing repository. Set Git up
to use your `gh` token and prove access before the long clone:

```bash
gh auth setup-git
git ls-remote https://github.com/SMK1085/paigasus-core.git HEAD >/dev/null   # preflight
git clone --mirror https://github.com/SMK1085/paigasus-core.git core-mirror.git
docker run --rm -v "$PWD/core-mirror.git:/repo:ro" -v "$PWD:/out" \
  ghcr.io/gitleaks/gitleaks:latest detect \
  --source=/repo --report-path=/out/gitleaks.json --report-format=json --redact
```

Expect this to take ~30 minutes. It is CPU-bound on the large lockfiles (`ts/pnpm-lock.yaml`,
`rs/Cargo.lock`), whose integrity hashes exercise the entropy rules hard. A silent 20 minutes is
normal, not a hang — confirm with `docker top` if unsure.

**`git clone --mirror` does NOT fetch `refs/pull/*`.** GitHub does not advertise those refs, but
every PR head and merge commit remains reachable by SHA and through the API the moment the repo is
public. Fetch them explicitly and rescan:

```bash
git --git-dir=core-mirror.git remote add gh https://github.com/SMK1085/paigasus-core.git
git --git-dir=core-mirror.git fetch gh '+refs/pull/*:refs/pull/*'
```

On this repo that step more than doubled the reachable history — **777 → 1,845 commits** — so
skipping it would have left the majority of commits unscanned. Budget ~25 minutes for the second
pass; the 2026-08-16 run scanned 2.06 GB in 24m43s.

**A clean scan does not prove absence.** Objects orphaned by force-push and rebase cannot be
enumerated locally; GitHub retains them and serves them by SHA on public repos. Therefore:

> Any credential *suspected* of ever having been committed must be **rotated**, not scanned-and-
> cleared. Record that as an explicit decision here, with a date — do not infer safety from a green
> gitleaks run.

Rotation decisions:

| Date | Credential | Decision |
| -- | -- | -- |
| 2026-08-16 | — | None suspected. Repo has never referenced `secrets.*` in any workflow; the only token in use is `${{ github.token }}`. |

---

## 3. Pre-flight B — content review

Distinct from §2, and gitleaks finds none of it. Going public publishes far more than code:

- **71 tracked files carry internal references** — 87 `linear.app`, 72 `.internal`, 11
  `notion.so`. **59 of them are in `docs/superpowers/specs/` and `docs/superpowers/plans/`**: the
  full internal design history and forward roadmap, including SMA-520's own billing figures.
- **All historical Actions run logs and uploaded artifacts become world-readable**, not just
  future ones.
- Agent scratch state (`.claude/`, `.entire/`) was not ignored until SMA-520 and was one
  `git add -A` from publication. That is fixed; `.claude/scheduled_tasks.lock` was also untracked
  in the same commit.

**Decision (Sven, 2026-08-16): publish `docs/superpowers/**` as-is.** The design history is part
of what an open Apache-2.0 monorepo offers. The `linear.app` / `notion.so` URLs resolve to a
private workspace and simply 404 for outsiders — they disclose issue titles and workflow
structure, not data. Recorded here so the flip does not silently imply it.

---

## 4. Flip sequence

Order matters. Do not reorder — §4.1 and §4.4 exist because of a specific gap.

**4.1 — Disable Actions.** Settings → Actions → General → *Disable actions*.

Both `ci.yml` and `security-scan.yml` trigger on `pull_request`, and `moon ci` runs arbitrary
build scripts and starts testcontainers. Between the flip and the fork-approval policy being set,
a fork PR could execute on your runners. Disabling Actions closes that window entirely, because
the approval control in §4.4 **cannot be set while the repo is private** — the outside-collaborator
setting is public-repository-only.

**4.2 — Set an Actions spending limit.** Settings → Billing → spending limit.

Insurance in case the flip does not zero macOS the way §0 says it will. A capped surprise beats an
invoiced one.

**4.3 — Flip visibility.**

```bash
gh repo edit SMK1085/paigasus-core --visibility public --accept-visibility-change-consequences
```

**4.4 — Set fork PR workflow approval.** Settings → Actions → General → Fork pull request
workflows → **"Require approval for all outside collaborators"**.

Chosen deliberately over GitHub's newly-public default of *"first-time contributors"*. `moon ci`
executes arbitrary build scripts and spins up Postgres/Redis/Keycloak testcontainers, so the blast
radius of an unreviewed fork PR is a full build environment, not a lint run.

**4.5 — Re-enable Actions.** Settings → Actions → General → *Allow all actions*.

**4.6 — Enable secret scanning and push protection.** Settings → Code security.

Both are free on public repositories. **Push protection is the one that matters**: it blocks a
credential at push time, rather than reporting it once it is already in history and therefore
already subject to §1. This is why SMA-520 added no third-party scanning gate to CI — the native
control is strictly better placed.

**4.7 — Confirm nothing was lost in the transition.**

A name-only check is not enough — a ruleset can survive the transition by name while sitting at
`disabled`/`evaluate` enforcement, or no longer scoped to the default branch. Assert the substance:

```bash
gh api repos/SMK1085/paigasus-core --jq '{visibility, private}'      # expect: public / false

id=$(gh api repos/SMK1085/paigasus-core/rulesets --jq '.[] | select(.name=="Protect main") | .id')
gh api "repos/SMK1085/paigasus-core/rulesets/$id" --jq '{
  enforcement,                                    # expect: active
  branches: .conditions.ref_name.include,         # expect: ["~DEFAULT_BRANCH"]
  checks: [.rules[] | select(.type=="required_status_checks")
           | .parameters.required_status_checks[].context],   # expect: ["moon ci"]
  rules: [.rules[].type] | sort                   # expect: deletion, non_fast_forward, required_status_checks
}'
```

Also confirm `.github/dependabot.yml` still shows scheduled updates in the Insights tab.

---

## 5. Post-flip cleanup

**5.1 — Remove `ci.yml`'s authenticated `main` fetch.** `ci.yml` lines 53-63.

That step exists *only* because a private repo cannot fetch `main` anonymously. After the flip it
is dead weight, and its neighbouring comments are already self-contradictory — lines 9 and 50
assert "public repo" while line 58 asserts "private repo".

> **Do not do this before the flip.** Merging it while the repo is private breaks the `main`-ref
> fetch on every PR run.
>
> **And the coupling runs both ways.** Once this commit lands, reverting visibility to private
> *silently* breaks every PR run. A visibility revert must revert this commit in lockstep.

**5.2 — Delete the orphaned cache entry.**

SMA-520 stopped building on the Intel macOS runner, so its cache entry is never read again and
consumes repo cache quota until LRU eviction:

```bash
gh cache list --key prebuild-rust-macOS-x86_64-apple-darwin
gh cache delete <id>
```

Run this after the first post-merge `prebuild` run, not before.

---

## 6. Verifying AC-1

**Owner: Sven. When: start of the next billing cycle.** Check Settings → Billing.

Minutes already accrued in the current cycle **still invoice** — only post-flip usage is free. Do
not read a non-zero current-cycle figure as a failed flip.

---

## 7. If the flip does not happen

AC-1 ("spend is $0/month") is then permanently unmet, and SMA-520's workflow changes deliver under
~15% on their own. Fallback plan:

1. Keep the path filter and the merged darwin job — both are worth having regardless, and the
   darwin merge is **required** before August 2027 when the x86_64 macOS image is withdrawn.
2. Accept roughly $14/month.
3. **Reopen matrix tiering** — `linux-x64-gnu` smoke on main pushes, full matrix on
   tags/nightly/dispatch. SMA-520 rejected it *purely* on the assumption the flip lands and makes
   runners free. That assumption is what fails in this branch.

---

## 8. Note — scheduled workflows stop on quiet repos

On public repositories GitHub **disables scheduled workflows after 60 days of repository
inactivity**. `security-scan.yml` runs a daily `17 7 * * *` cron whose entire purpose is to fire
on days when nothing changes (SMA-518). A quiet stretch is precisely when it silently stops.

A disabled workflow rejects **every** trigger, `workflow_dispatch` included — so dispatching it is
not a way around the disabled state. Re-enable first, then dispatch:

```bash
gh workflow enable security-scan.yml
gh workflow run    security-scan.yml
```
