# SMA-520 — Cut GitHub Actions spend

Design document. Status: approved 2026-08-16.

## Problem

`paigasus-core` Actions spend reached ~$22/month (~3,750 billed-equivalent minutes),
over the 3,000-minute plan allowance. A 40-run sample (2026-06-15 → 2026-08-15)
attributes 43% of the bill to macOS runners despite their being only 7% of wall-clock —
macOS bills at $0.062/min against Linux's $0.006/min.

The source is `prebuild.yml`: a 7-platform napi cross-build matrix firing on **every**
push to `main` with no path filter, across two separate macOS runners. Its own header
comment states it is build-only verification — it publishes nothing and creates no
GitHub release. `ci.yml` is a single well-cached ubuntu job and is not a target.

Two further facts make this urgent beyond cost:

- `macos-15-intel` is the **last x86_64 macOS image on Actions**, available only until
  **August 2027**. Migrating off it is required regardless of the billing decision.
- The repository is designed to be public — `CLAUDE.md` opens with "Public, Apache-2.0
  polyglot monorepo", `ts/.npmrc` documents itself as belonging to "a public,
  Apache-2.0 repo", and `ci.yml` already carries two comments asserting public
  visibility. The flip has simply never happened.

## Scope

One PR touching two files, plus one operator action that a PR cannot perform.

| Deliverable | Owner | Artifact |
| -- | -- | -- |
| W2 — migrate `darwin-x64` off `macos-15-intel` | this PR | `.github/workflows/prebuild.yml` |
| W3 — path-filter + concurrency | this PR | `.github/workflows/prebuild.yml` |
| W1 — go public | Sven, post-merge | `docs/ops/RUNBOOK-go-public.md` |

Acceptance criterion 1 ("spend is $0/month") is satisfiable **only** by the visibility
flip, which is a GitHub settings change. The PR carries the runbook and the evidence;
the flip itself stays a human action.

## Workstream 2 — merge the darwin legs

### Decision

Delete the `macos-15-intel` matrix entry and build **both** darwin targets in the
single `macos-latest` (arm64) job, rather than repointing `darwin-x64` at a second
`macos-latest` job.

The issue claims removing `macos-15-intel` "halves the macOS runner count per run".
That claim is **false for the naive repoint** — replacing the runner label leaves two
macOS jobs, each paying a full proto/Moon/pnpm toolchain setup, which dominates job
wall-clock. Merging the legs is what actually delivers the halving.

Cross-compiling `x86_64-apple-darwin` on Apple Silicon is a first-class path: the macOS
SDK ships both slices and the Apple linker cross-links via `-arch`. No emulation, no
extra toolchain.

### Shape

The `darwin-arm64` entry gains two optional fields; the other five entries are
untouched and the fields expand to nothing for them:

```yaml
- { platform: darwin-arm64, target: aarch64-apple-darwin, runner: macos-latest, zig: false,
    extra_platform: darwin-x64, extra_target: x86_64-apple-darwin }
```

Four step-level changes, each inert on non-darwin legs:

1. `rustup target add ${{ matrix.target }} ${{ matrix.extra_target }}` — rustup accepts
   multiple triples in one invocation; an empty `extra_target` leaves the existing
   single-target command.
2. A second `napi build` step gated on `if: ${{ matrix.extra_target }}`.
3. A second upload step gated on `if: ${{ matrix.extra_platform }}`. Two separate
   uploads (rather than one glob) preserve the `prebuild-<platform>` artifact naming
   that the `assemble` job's `pattern: prebuild-*` download depends on.
4. The job name renders `build darwin-arm64 + darwin-x64` for that leg only, via a
   `format()` fallback. Without this the job would advertise one platform while
   building two.

### Byte-validity verification

`prebuild.yml` never executes a macOS binary — the only runtime check in the whole
workflow is `linux-x64-gnu` resolution in `assemble`, on ubuntu. So "still produces
darwin-x64" is otherwise unproven for a cross-built artifact.

A new step asserts the Mach-O architecture of each darwin `.node` with `lipo -archs`:
`x86_64` for `darwin-x64`, `arm64` for `darwin-arm64`. This closes the gap the issue
identifies ("Confirm the produced `.node` artifact is byte-valid for darwin-x64")
without specifying a mechanism.

### Consequences

- Jobs 7 → 6; macOS jobs 2 → 1. All 7 platform artifacts still produced (AC 2).
- Cache key stays on `matrix.target`. Both triples write to distinct
  `rs/target/<triple>/` subdirectories, so a single cache entry holds both without
  collision. A pre-change arm64-only cache restores harmlessly — the x64 build is
  simply cold once.
- `fail-fast: false` isolation is lost *between the two darwin targets only*: an x64
  build failure now also loses the arm64 artifact from that run. Accepted — this is a
  build-only workflow, and a failing run is investigated as a whole regardless. The
  other five legs keep full isolation.

## Workstream 3 — triggers

### Path filter

```yaml
push:
  branches: [main]
  paths:
    - 'rs/**'
    - '.github/workflows/prebuild.yml'
    - '.prototools'
    - '.moon/toolchains.yml'
    - 'ts/pnpm-lock.yaml'
    - 'ts/pnpm-workspace.yaml'
    - 'ts/packages/paigasus-kernel/package.json'
```

The issue specifies `rs/**`, `Cargo.lock`, and the workflow file. Two corrections:

- `rs/Cargo.lock` is already inside `rs/**`; listing it separately is redundant.
- The literal list is **insufficient**. `prebuild.yml` runs `pnpm --dir ts install
  --frozen-lockfile` and invokes `napi` from `ts/packages/paigasus-kernel`, so the napi
  CLI version is resolved from the `ts/` catalog and lockfile. A Dependabot
  `@napi-rs/cli` bump — the single change most likely to break a cross-build — would
  merge to `main` with the matrix never running.

AC 3 says "a docs-only or `ts`-only merge to `main` does not trigger the prebuild
matrix". This design reads that as *`ts` application-code-only*: a merge touching
`ts/packages/paigasus-proto/**` or any TS source does not trigger, while a napi or
toolchain version change does. Verifying the build inputs is worth the handful of extra
runs, which cost nothing post-flip.

`workflow_dispatch` remains unfiltered — the manual escape hatch always runs the full
matrix.

Path-filtering is safe here because `prebuild.yml` has no `pull_request` trigger. It is
therefore not a required status check and cannot wedge a merge by reporting `skipped`.

### Concurrency

`cancel-in-progress: true`, unconditionally, replacing
`${{ github.event_name == 'workflow_dispatch' }}`.

This workflow publishes nothing and creates no release, so only the latest `main` state
carries meaning. This is deliberately the opposite of `ci.yml`, which lets push-to-main
runs finish because they warm caches and carry commit status — `prebuild` carries
neither.

Accepted side effect: a push to `main` now cancels a concurrent manual dispatch, since
the concurrency group keys on `github.ref` and both resolve to `refs/heads/main`. This
is the lowest-conviction change in the PR and is trivially revertible in isolation.

### Rejected — matrix tiering

The issue floats tiering the matrix (`linux-x64-gnu` smoke on main pushes, full 7 on
tags/nightly/dispatch). Rejected under YAGNI: once the path filter lands, full-matrix
runs are already infrequent, and post-flip the runners are free. Tiering trades
conditional complexity for no measurable gain.

## Workstream 1 — go public

Delivered as `docs/ops/RUNBOOK-go-public.md`, matching the existing
`docs/ops/RUNBOOK-nats.md` / `RUNBOOK-observability.md` convention. Ordered so nothing
breaks mid-flip.

### Pre-flight evidence (gathered during this work)

- `gitleaks` over the **full history** of every ref via a mirror clone — not just the
  working tree, since a credential ever committed is exposed even if later removed.
- No `pull_request_target` workflow exists anywhere in `.github/`.
- Zero `secrets.*` references in any workflow. The only token used is
  `${{ github.token }}`, in one `ci.yml` fetch step.
- No self-hosted runners. Every runner label in the matrix (`ubuntu-latest`,
  `ubuntu-24.04-arm`, `windows-latest`, `macos-latest`) is standard-class, and standard
  runners remain free on public repositories under the 2026 pricing changes.

### Sequence

1. Enable approval-required for fork pull request workflows, **before** flipping.
2. Flip: `gh repo edit SMK1085/paigasus-core --visibility public
   --accept-visibility-change-consequences`.
3. Enable GitHub secret scanning **and push protection** — both free on public
   repositories. Push protection blocks a credential at push time rather than reporting
   it once it is already in history, which is why no third-party CI scanning gate is
   added.
4. Confirm branch protection rules and Dependabot configuration survived the
   transition.

### Post-flip follow-up (explicitly NOT in this PR)

`ci.yml`'s "Materialize main ref" step authenticates its `main` fetch solely because a
private repo requires it. The surrounding comments already contradict each other —
lines 9 and 50 assert "public repo", line 58 asserts "private repo". After the flip the
authenticated fetch is dead weight and the comments are wrong.

This must **not** ship in this PR: merging it while the repo is still private breaks
the `main`-ref fetch on every PR run. It is a runbook step executed after the flip.

### Billing note

Only usage after the flip stops billing. Minutes already accrued in the current cycle
still invoice.

## Verification

`prebuild.yml` runs only on push-to-`main` and `workflow_dispatch`. A mistake is
therefore invisible on the PR and reds `main` after merge — the failure class CLAUDE.md
calls out explicitly (SMA-448).

`workflow_dispatch` accepts an arbitrary `--ref`, and `prebuild.yml` already exists on
the default branch, so the change is provable on the feature branch before merge:

```
gh workflow run prebuild.yml --ref feature/sma-520-cut-actions-spend
```

A green run proves the merged darwin job, both `lipo` assertions, all 7 artifacts, and
the unchanged `assemble` job. It costs one billed run (~$0.10–0.15) — worth it against
a post-merge red `main`.

Local checks before push: YAML parse, and `actionlint` via Docker.

The path filter itself cannot be proven by dispatch (dispatch ignores `paths`). It is
verified by inspection plus the first post-merge docs-only or TS-only merge, which must
show no prebuild run.

## Acceptance criteria

| AC | Met by | Verified by |
| -- | -- | -- |
| Spend is $0/month | W1 flip (Sven) | GitHub billing page next cycle |
| No `macos-15-intel`; all 7 artifacts | W2 (this PR) | `workflow_dispatch` run on the feature branch |
| Docs/`ts`-only merge does not trigger | W3 (this PR) | Inspection + first qualifying merge |
| Artifact storage negligible | no action | 20 MB / 100 artifacts, not a cost factor |

## Out of scope

- Removing `ci.yml`'s authenticated `main` fetch — post-flip only (see above).
- Tiering the build matrix — rejected above.
- `ci.yml` runtime optimisation — already a single well-cached ubuntu job.
- `paigasus-helikon` (already public) and `aws-policy-generator` (6 runs, noise).

## References

- [GitHub-hosted runners reference](https://docs.github.com/en/actions/reference/runners/github-hosted-runners)
- [2026 pricing changes for GitHub Actions](https://github.com/resources/insights/2026-pricing-changes-for-github-actions)
- [macOS x64 runner retirement — actions/runner-images#13045](https://github.com/actions/runner-images/issues/13045)
