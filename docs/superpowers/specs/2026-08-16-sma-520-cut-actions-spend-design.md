# SMA-520 — Cut GitHub Actions spend

Design document. Revision 2 — incorporates the adversarial spec challenge of 2026-08-16.

## Problem

`paigasus-core` Actions spend reached ~$22/month (~3,750 billed-equivalent minutes),
over the 3,000-minute plan allowance. A 40-run sample (2026-06-15 → 2026-08-15)
attributes 43% of the bill to macOS runners despite their being only 7% of wall-clock —
macOS bills at $0.062/min against Linux's $0.006/min. That arithmetic checks out
exactly: 7% × 10.3 ⇒ 43.8% of the bill.

The source is `prebuild.yml`: a 7-platform napi cross-build matrix firing on **every**
push to `main` with no path filter, across two separate macOS runners. Its own header
comment states it is build-only verification — it publishes nothing and creates no
GitHub release.

Two further facts make this urgent beyond cost:

- `macos-15-intel` is the **last x86_64 macOS image on Actions**, available only until
  **August 2027**. Migrating off it is required regardless of the billing decision.
- The repository is designed to be public — `CLAUDE.md` opens with "Public, Apache-2.0
  polyglot monorepo", `ts/.npmrc` documents itself as belonging to "a public,
  Apache-2.0 repo", and `ci.yml` already carries two comments asserting public
  visibility. The flip has simply never happened.

### Lever ordering

The three workstreams are **not** comparable in impact. Stated plainly so review and
follow-up are not misdirected:

| Lever | Share of the bill it removes |
| -- | -- |
| W1 — go public | ~100% |
| W3 — path filter | ~10–20% of remaining runs, if W1 does not land |
| W2 — merge darwin legs | ~5–10% (~$1–2.50/month) |

**W2 is justified by the August 2027 retirement, not by cost.** An earlier revision of
this document implied the merge was a major cost lever; it is not. The saving is one
duplicated toolchain setup, partially offset by moving the x86_64 compile into the
surviving job. It is worth doing because the current runner ceases to exist.

## Scope

One PR touching three files, plus one operator action that a PR cannot perform.

| Deliverable | Owner | Artifact |
| -- | -- | -- |
| W2 — migrate `darwin-x64` off `macos-15-intel` | this PR | `.github/workflows/prebuild.yml` |
| W3 — triggers | this PR | `.github/workflows/prebuild.yml` |
| Ignore agent scratch dirs before publication | this PR | `.gitignore` |
| W1 — go public | Sven, post-merge | `docs/ops/RUNBOOK-go-public.md` |

Acceptance criterion 1 ("spend is $0/month") is satisfiable **only** by the visibility
flip, which is a GitHub settings change. The PR carries the runbook and the evidence;
the flip itself stays a human action.

### Verified assumption — macOS is free on public repos

AC-1 rests entirely on this. Confirmed verbatim from the linked pricing page: *"Standard
GitHub-hosted or self-hosted runner usage on public repositories will remain free."* The
rule is stated uniformly across standard runner types with **no macOS carve-out**. Every
runner label in the matrix (`ubuntu-latest`, `ubuntu-24.04-arm`, `windows-latest`,
`macos-latest`) is standard-class.

## Workstream 2 — merge the darwin legs

### Decision

Delete the `macos-15-intel` matrix entry and build **both** darwin targets in the
single `macos-latest` (arm64) job.

This is the vendor-recommended path, not an improvisation. napi-rs's cross-build
documentation states that on a macOS host no cross flag is needed to build for a
different architecture, and that napi's own generated CI uses `macos-latest` for **both**
`x86_64-apple-darwin` and `aarch64-apple-darwin`. `-x` / `cargo-zigbuild` is required
only when cross-building darwin *from Linux*.

Two in-repo facts de-risk it further:

- The existing `darwin-x64` leg **already** passes `--target x86_64-apple-darwin`
  explicitly, and the `linux-x64-musl` leg already builds a non-host target on
  `ubuntu-latest` and uploads `paigasus-node-bindings.linux-x64-musl.node`. So napi
  derives the platform suffix from `--target`, not from the host — proven by the repo's
  own workflow, not assumed. The two darwin builds emit different filenames and cannot
  overwrite each other.
- `rs/.cargo/config.toml` already declares the `-undefined dynamic_lookup` link flags
  for **both** apple triples, which the napi cdylib requires on macOS. Had it been
  arm64-only this would have been a hard blocker. Any future tidy-up of that file must
  not drop the `x86_64-apple-darwin` block.

### Shape

The `darwin-arm64` entry gains two optional fields; the other five entries are
untouched and the fields expand to nothing for them:

```yaml
- { platform: darwin-arm64, target: aarch64-apple-darwin, runner: macos-latest, zig: false,
    extra_platform: darwin-x64, extra_target: x86_64-apple-darwin }
```

**Five** step-level changes, each inert on non-darwin legs. Step order is load-bearing
and fixed as follows:

1. `rustup target add ${{ matrix.target }} ${{ matrix.extra_target }}` — rustup accepts
   multiple triples in one invocation; an empty `extra_target` leaves the existing
   single-target command.
2. Build `matrix.target` (unchanged).
3. Build `matrix.extra_target`, gated `if: ${{ matrix.extra_target }}`. An **exact copy**
   of step 2 with only `--target` changed — same `working-directory:
   ts/packages/paigasus-kernel` and same `--cwd ../../../rs/crates/bindings/paigasus-node-bindings`.
   That `--cwd` is load-bearing: `rs/.cargo/config.toml` is discovered by walking up from
   cargo's working directory, and losing it loses the darwin link flags above.
4. Architecture assertion, gated `if: runner.os == 'macOS'` (see below).
5. Upload `matrix.platform` (unchanged), then upload `matrix.extra_platform`, gated
   `if: ${{ matrix.extra_platform }}`. Two separate uploads rather than one glob, to
   preserve the `prebuild-<platform>` artifact naming that `assemble`'s
   `pattern: prebuild-*` download depends on.

Both builds complete before either upload, so a failure in step 3 loses both darwin
artifacts from that run. This is deliberate and stated accurately here — an earlier
revision asserted the consequence without fixing the step order that determines it.

The job name renders `build darwin-arm64 + darwin-x64` for that leg only, via a
`format()` fallback. Safe to rename: the `Protect main` ruleset requires only the
`moon ci` check, and declares no `required_workflows`.

### Architecture assertion

`prebuild.yml` never executes a macOS binary — the only runtime check in the whole
workflow is `linux-x64-gnu` resolution in `assemble`, on ubuntu. So "still produces
darwin-x64" is otherwise unproven for a cross-built artifact.

The assertion must use **exact equality**, not a substring match:

```bash
set -euo pipefail
[ "$(lipo -archs …paigasus-node-bindings.darwin-x64.node)"   = "x86_64" ]
[ "$(lipo -archs …paigasus-node-bindings.darwin-arm64.node)" = "arm64"  ]
```

`lipo -archs` on a *universal* binary prints `x86_64 arm64`, so a `grep -q x86_64`
assertion would pass for a fat file — vacuously green in exactly the case worth
catching. This repo has been burned by that class of assertion before (the Prometheus
`# TYPE` line).

The step is gated `if: runner.os == 'macOS'`; `lipo` does not exist on ubuntu or
windows and an ungated step would red the other five legs.

Additionally record the minimum-OS stamp with
`otool -l …darwin-x64.node | grep -A3 LC_BUILD_VERSION`. Rust pins the deployment
target per *target* rather than per host, so the cross-build should carry the same
`minos` the `macos-15-intel` job produced — but napi's own generated CI explicitly sets
`MACOSX_DEPLOYMENT_TARGET=10.13`, which this workflow never has. Recording the value
proves it rather than assuming it. If it diverges from the last known-good
`macos-15-intel` artifact, set `MACOSX_DEPLOYMENT_TARGET` explicitly on the darwin job.

### Cache — the target dir must be re-keyed

The current key is
`prebuild-rust-${{ runner.os }}-${{ matrix.target }}-${{ hashFiles('rs/rust-toolchain.toml') }}-${{ hashFiles('rs/Cargo.lock') }}`.

The merged job keeps `matrix.target: aarch64-apple-darwin`, and this PR changes neither
hashed file — so the **exact primary key already exists** in `main`'s cache scope,
written by the pre-change arm64-only job. `actions/cache` does not run its post-job save
when the primary key hits. The consequence: `rs/target/x86_64-apple-darwin/` would be
restored-never and saved-never, recompiling the entire dependency tree from scratch on
**every** run, permanently, on the most expensive runner class in the repo — until
`rs/Cargo.lock` or `rs/rust-toolchain.toml` happens to move.

A verification dispatch would not reveal this: feature branches read the base branch's
cache scope, hit the same key, and the cold compile reads as ordinary first-run cost.

**Fix:** add a literal discriminator to both `key:` and `restore-keys:`, e.g.
`prebuild-rust-${{ runner.os }}-${{ matrix.target }}-dual-…`. There is direct repo
precedent — `ci.yml:91` carries a `-line-tables-only-` segment added for exactly this
"semantics changed, invalidate the old entry" reason.

The plan must record the darwin job's wall-clock before and after, so a cache regression
is visible rather than inferred.

### Other consequences

- Jobs 7 → 6; macOS jobs 2 → 1. All 7 platform artifacts still produced (AC 2).
- `timeout-minutes` rises from 30 to 45 for the darwin leg only
  (`${{ matrix.extra_target && 45 || 30 }}`). The merged job does two full `--release`
  builds; a timeout is a hard red on `main` and is the one failure the dispatch
  verification may not reproduce once cache scopes diverge.
- The isolation loss is smaller than it appears: `assemble` declares `needs: build`, so
  *any* leg failure already skips assemble today.
- The pre-change `prebuild-rust-macOS-x86_64-apple-darwin-*` cache entry is never read
  again and consumes repo cache quota until LRU eviction. Runbook item: `gh cache delete`
  it after the first post-merge run.

## Workstream 3 — triggers

### Two triggers, asymmetric paths

The single biggest defect in revision 1: `.github/dependabot.yml` produces a grouped
`npm-minor-patch` PR **every Monday**, which by construction touches
`ts/pnpm-lock.yaml`. Under revision 1's allowlist that fired the full matrix weekly —
a purely-`ts` merge triggering prebuild, directly violating AC-3, and defeating most of
W3's intended saving.

The fix separates the two concerns onto the two triggers:

```yaml
on:
  workflow_dispatch:
  push:
    branches: [main]
    paths:                                  # post-merge verification of Rust changes
      - 'rs/**'
      - '.github/workflows/prebuild.yml'
      - '.prototools'
      - '.moon/**'
  pull_request:
    branches: [main]
    paths:                                  # pre-merge verification of build inputs
      - '.github/workflows/prebuild.yml'
      - '.prototools'
      - '.moon/**'
      - 'ts/pnpm-lock.yaml'
      - 'ts/pnpm-workspace.yaml'
      - 'ts/packages/paigasus-kernel/package.json'
      - 'ts/.npmrc'
```

Why this shape:

- **AC-3 is satisfied literally.** No `ts/` path appears in the `push` allowlist, so a
  docs-only or `ts`-only merge — including the weekly Dependabot npm PR — does not
  trigger the matrix on `main`.
- **napi CLI bumps are verified *before* they merge.** `@napi-rs/cli` is `catalog:` in
  `ts/packages/paigasus-kernel/package.json`, resolved from `ts/pnpm-workspace.yaml:108`
  and pinned in `ts/pnpm-lock.yaml`; all three are on the `pull_request` list. This is
  strictly better than revision 1, which only caught such a bump after it hit `main`.
- **`rs/**` is deliberately absent from the `pull_request` list.** Most PRs in this repo
  touch `rs/**`; adding a 6-job matrix including macOS to every one of them would
  *increase* the bill until the flip lands — the opposite of this issue's purpose. Rust
  changes keep today's post-merge-only verification.
- **prebuild gains a visible PR status** for the changes that carry it, which is the
  only observability this workflow has ever had. It does not become blocking: the
  `Protect main` ruleset requires only `moon ci`.
- **In-repo precedent:** `security-scan.yml` already uses exactly this
  `pull_request: branches: [main]` + `paths:` shape.

`ts/.npmrc` is on the list because it pins `registry=https://registry.npmjs.org/` and is
a direct input to the `pnpm --dir ts install --frozen-lockfile` step (prebuild.yml:83,
and again at :116 in `assemble`). Revision 1 audited the `ts/` inputs and still missed
it. `.moon/**` replaces the narrower `.moon/toolchains.yml` because `moon setup` also
requires `.moon/workspace.yml`. `rs/Cargo.lock` from the issue is already inside `rs/**`.

`workflow_dispatch` remains unfiltered — the manual escape hatch always runs the full
matrix.

### Concurrency — revision 1 was wrong, keep the current behaviour

Revision 1 proposed `cancel-in-progress: true` on the grounds that "`prebuild` warms no
caches and carries no status". **That is false.** prebuild.yml:54-63 *is* an
`actions/cache` step, and since prebuild has no PR trigger today, push-to-`main` is the
only event that ever writes to `main`'s cache scope. Combined with the re-keying fix
above, cancelling push runs is precisely the thing that would stop the newly-keyed cache
from ever being populated.

So: **never cancel a push run, cancel every other kind**, and fix the group collision too:

```yaml
group: prebuild-${{ github.workflow }}-${{ github.ref }}-${{ github.event_name }}
cancel-in-progress: ${{ github.event_name != 'push' }}
```

`!= 'push'` rather than `== 'workflow_dispatch'`: this revision also adds a `pull_request`
trigger, and the narrower expression would leave superseded PR runs to run to completion for no
benefit — their cache scope is the PR's own and is discarded on merge. `!= 'push'` states the
actual rule (protect the only writer to `main`'s cache scope, cancel everything else) and matches
`ci.yml:16`.

Without `event_name` in the group, a manually dispatched run (which evaluates
`cancel-in-progress` to `true`) would cancel a running push-to-`main` job, since both
resolve to `refs/heads/main`. With it, push cancels push, dispatch cancels dispatch, and
neither cancels the other. This strictly dominates revision 1's "accepted side effect".

### Rejected — matrix tiering

The issue floats tiering the matrix (`linux-x64-gnu` smoke on main pushes, full 7 on
tags/nightly/dispatch). Rejected under YAGNI **conditional on W1 landing**: once the
runners are free, tiering trades conditional complexity for nothing measurable. If the
flip is refused or deferred indefinitely, tiering should be reconsidered — see Rollback.

## Workstream 1 — go public

Delivered as `docs/ops/RUNBOOK-go-public.md`, matching the existing
`docs/ops/RUNBOOK-nats.md` / `RUNBOOK-observability.md` convention.

### Pre-flight A — credential scan

- `gitleaks` over the full history via a mirror clone — not just the working tree, since
  a credential ever committed is exposed even if later removed. **Executed 2026-08-16:
  0 findings across 777 commits on all standard refs.**
- **`refs/pull/*` must be fetched explicitly.** GitHub does not advertise these refs, so
  `git clone --mirror` does not fetch them, yet every PR head and merge commit remains
  reachable by SHA and via the API once the repo is public. Run
  `git fetch origin '+refs/pull/*:refs/pull/*'` into the mirror before scanning.
- **Objects orphaned by force-push cannot be enumerated locally.** GitHub retains them
  and serves them by SHA on public repos. A green gitleaks run therefore does not prove
  absence. Any *suspected* historical credential must be **rotated**, recorded as an
  explicit checklist decision rather than inferred from a clean scan.

A visibility flip is irreversible with respect to disclosure: a blob a scraper has
fetched cannot be un-published, and reverting to private does not remediate it.

### Pre-flight B — content review (distinct from the credential scan)

gitleaks finds credentials; it finds none of the following, all of which become
world-readable at the flip:

- **71 tracked files** contain internal references — 87 `linear.app`, 72 `.internal`,
  11 `notion.so` — and **59 of them live in `docs/superpowers/specs/` and
  `docs/superpowers/plans/`**, i.e. the full internal design history and roadmap. This
  very document, including the billing figures above, is among them.
- All historical Actions **run logs and uploaded artifacts** become world-readable.
- `.gitignore` covers `.env`, `*.pem`, `*.key`, but **not `.claude/` or `.entire/`**,
  both of which exist untracked in the working tree today and are one `git add -A` from
  publication. This PR adds both.

**Decision (Sven, 2026-08-16): publish `docs/superpowers/**` as-is.** The design history
is a deliberate part of what an open Apache-2.0 monorepo offers. The `linear.app` and
`notion.so` URLs resolve to a private workspace and simply 404 for outsiders — they
disclose issue titles and workflow structure, not data. Recorded here so the flip does
not silently imply it; no scrubbing pass is required and future specs need no special
discipline beyond the credential rules that already apply.

### Sequence

Revision 1 opened with "enable approval-required for fork PR workflows, before
flipping". That is likely not performable: the outside-collaborator approval control is
a *public*-repository Actions setting, while a private repo exposes a different control.
Reordered so no window exists in which fork-authored code can run:

1. **Disable Actions** (Settings → Actions → Disable).
2. Set an **Actions spending limit**, so a surprise is capped rather than invoiced.
3. Flip: `gh repo edit SMK1085/paigasus-core --visibility public
   --accept-visibility-change-consequences`.
4. Set the fork-approval policy to **"Require approval for all outside collaborators"**
   (Settings → Actions → General → Fork pull request workflows). Chosen deliberately over
   GitHub's newly-public default of "first-time contributors": `moon ci` executes
   arbitrary build scripts and starts testcontainers, so the blast radius of an
   unreviewed fork PR is a full build environment, not a lint run.
5. **Re-enable Actions.**
6. Enable GitHub secret scanning **and push protection** — both free on public
   repositories. Push protection blocks a credential at push time rather than reporting
   it once it is already in history, which is why no third-party CI scanning gate is
   added.
7. Confirm the `Protect main` ruleset and Dependabot configuration survived.

Step ordering matters because both `ci.yml` (`moon ci` — arbitrary build scripts,
testcontainers) and `security-scan.yml` (`ci/osv/run.sh`) have `pull_request` triggers,
so fork-authored code would otherwise execute on GitHub's runners before the approval
policy is in place.

### Post-flip follow-up (explicitly NOT in this PR)

`ci.yml`'s "Materialize main ref" step authenticates its `main` fetch solely because a
private repo requires it. The surrounding comments already contradict each other —
lines 9 and 50 assert "public repo", line 58 asserts "private repo". After the flip the
authenticated fetch is dead weight and the comments are wrong.

This must **not** ship in this PR: merging it while the repo is still private breaks the
`main`-ref fetch on every PR run. It is a runbook step executed after the flip. The
coupling runs both ways — **once that cleanup commit lands, reverting visibility to
private silently breaks every PR run**, so a visibility revert must revert that commit
in lockstep.

### Rollback / if the flip does not happen

- **Visibility revert is not clean.** Existing forks are detached into a new network,
  and anything cloned during the public window stays cloned. Treat the flip as one-way
  for disclosure purposes.
- **If the flip is deferred or refused,** AC-1 is permanently unmet and W2+W3 deliver
  under ~15% of the bill. The fallback plan is: keep the path filter, accept roughly
  $14/month, and reopen matrix tiering — which is rejected above *purely* on the
  assumption that the flip lands.
- **AC-1 verification:** Sven, on the GitHub billing page at the start of the next
  billing cycle. Minutes already accrued in the current cycle still invoice; only
  post-flip usage is free.

### Scheduled-workflow note

`security-scan.yml` runs daily on `ubuntu-latest` and postdates the 2026-06-15 → 08-15
sample, so it is absent from the cost table. On public repositories GitHub disables
scheduled workflows after **60 days of repository inactivity** — worth knowing for a
job whose whole purpose is to fire on quiet days.

## Verification

`prebuild.yml` runs only on push-to-`main`, `workflow_dispatch`, and (after this PR) a
narrowly-filtered `pull_request`. A mistake in the `push` path is invisible on the PR
and reds `main` after merge — the failure class CLAUDE.md calls out explicitly
(SMA-448).

**Positive test, free:** this PR's own merge commit touches
`.github/workflows/prebuild.yml`, which is on the `push` allowlist. So **prebuild must
run on this PR's merge commit** — that is a named acceptance step, not an observation.
If it does not run, the filter is malformed and the change is reverted immediately.
This matters because the negative case ("a docs-only merge shows no run") is
indistinguishable from "the filter is broken and prebuild will never run again".

**Pre-merge proof of W2:** `workflow_dispatch` accepts an arbitrary `--ref`, and
`prebuild.yml` already exists on the default branch:

```
gh workflow run prebuild.yml --ref feature/sma-520-cut-actions-spend
```

A green run proves the merged darwin job, both `lipo` assertions, the `otool` minos
record, all 7 artifacts, and the unchanged `assemble` job. Cost is roughly **$0.75–1.00**
for a full 6-job matrix — revision 1's "$0.10–0.15" was wrong by an order of magnitude.
The conclusion is unchanged: still worth it against a post-merge red `main`.

Local checks before push: YAML parse, and `actionlint` via Docker.

**Known gap:** no repo gate lints workflow YAML. `moon.yml:123` scopes `affected-smoke`
to `ci.yml` only, and no Moon task takes `.github/workflows/prebuild.yml` as an input.
A `repo:actionlint` Moon task added to the `moon ci` target array would close this;
tracked as a follow-up rather than expanded into this PR, because the merge-commit
positive test covers *this* change.

## Acceptance criteria

| AC | Met by | Verified by |
| -- | -- | -- |
| Spend is $0/month | W1 flip (Sven) | Billing page, next cycle |
| No `macos-15-intel`; all 7 artifacts | W2 (this PR) | `workflow_dispatch` run on the feature branch |
| Docs/`ts`-only merge does not trigger | W3 (this PR) | `push` allowlist contains no `ts/` path; plus the merge-commit positive test |
| Artifact storage negligible | no action | 20 MB / 100 artifacts, not a cost factor |

## Out of scope

- Removing `ci.yml`'s authenticated `main` fetch — post-flip only, and coupled to any
  visibility revert.
- A `repo:actionlint` gate — follow-up.
- Tiering the build matrix — rejected above, conditional on W1 landing.
- `paigasus-helikon` (already public) and `aws-policy-generator` (6 runs, noise).
- `docs/superpowers/specs/2026-06-17-sma-428-napi-prebuild-matrix-design.md:73,85-93`
  still documents `darwin-x64 → macos-15-intel` and calls the arm64 cross-build a
  "fallback if `macos-15-intel` is constrained". That fallback is now the design. A
  one-line supersession note pointing at SMA-520 is added; the rest of the historical
  spec is deliberately left frozen.

## References

- [GitHub-hosted runners reference](https://docs.github.com/en/actions/reference/runners/github-hosted-runners)
- [2026 pricing changes for GitHub Actions](https://github.com/resources/insights/2026-pricing-changes-for-github-actions)
- [macOS x64 runner retirement — actions/runner-images#13045](https://github.com/actions/runner-images/issues/13045)
- [napi-rs cross-build documentation](https://napi.rs/docs/cross-build)
