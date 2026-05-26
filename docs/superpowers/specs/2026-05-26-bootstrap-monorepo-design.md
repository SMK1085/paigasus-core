# SMA-355 — Bootstrap polyglot monorepo structure

**Date:** 2026-05-26
**Linear:** [SMA-355](https://linear.app/smaschek/issue/SMA-355/bootstrap-polyglot-monorepo-structure)
**Status:** Design approved, review feedback incorporated, pending implementation
**Review:** [`2026-05-26-bootstrap-monorepo-design-review.md`](./2026-05-26-bootstrap-monorepo-design-review.md) — R1–R4 + S1–S4 addressed (R4's issue-filing item resolved in favor of GitHub→Linear; see §"Content intent")

## Goal

Establish the top-level directory layout, workspace files, and baseline
documentation for `paigasus-core`, the public Apache-2.0 polyglot monorepo.
This is the foundational issue — Phase 1 work (SMA-356 through SMA-363) depends
on this skeleton existing.

## Scope

### In scope

Create the repository shell and baseline docs only:

| Path | Purpose |
|------|---------|
| `.github/workflows/.gitkeep` | Placeholder; CI workflows land in SMA-361. |
| `.github/pull_request_template.md` | Conventional-commit reminder + AC-checklist prompt (review S3). |
| `.moon/.gitkeep` | Placeholder; Moon config lands in SMA-356, which deletes this file. |
| `contracts/README.md` | "Proto source of truth + codegen. Scaffolded in SMA-360." |
| `rs/README.md` | "Cargo workspace (libs/bindings/services). Scaffolded in SMA-357." |
| `py/README.md` | "uv workspace. Scaffolded in SMA-358." |
| `ts/README.md` | "pnpm workspace (packages/apps). Scaffolded in SMA-359." |
| `README.md` | Project overview, planned topology, forward-looking quickstart. |
| `CONTRIBUTING.md` | Workflow, conventions, CLA notice, issue-filing policy, internal doc links. |
| `CODEOWNERS` | Root file: `* @SMK1085`. |
| `.gitignore` | Template-based (rust/node/python/macos/windows/linux/editors) + secrets. |
| `LICENSE` | Already present (Apache 2.0) — left as-is. |

### Out of scope (owned by blocked issues)

Deliberately **not** created here, to respect issue boundaries:

- `.moon/workspace.yml`, `toolchain.yml`, `tasks.yml` → **SMA-356**
- `rs/Cargo.toml` + `crates/{libs,bindings,services}/` tree → **SMA-357**
- `py/pyproject.toml`, `uv.lock`, `packages/` tree → **SMA-358**
- `ts/pnpm-workspace.yaml`, `package.json`, `packages/`, `apps/` → **SMA-359**
- `contracts/proto/`, `buf.yaml`, `buf.gen.yaml`, `moon.yml` → **SMA-360**
- Dependabot config + CLA bot → **SMA-362**
- Actual CI workflow files (`ci.yml`, `release.yml`, `codegen-drift.yml`)

## Design decisions

### 1. Empty-directory tracking — per-dir README + `.gitkeep`

Git cannot commit empty directories. The four workspace roots (`contracts/`,
`rs/`, `py/`, `ts/`) each get a short `README.md` that states the directory's
purpose and names the issue that fills it in. These are self-documenting and
survive as real config lands (later issues add files alongside, they don't
replace the README). The two pure-config dirs (`.github/workflows/`, `.moon/`)
get a bare `.gitkeep`, since a prose README there adds no value and `.moon/`
is fully populated immediately by SMA-356. **Note (review S1):** SMA-356 deletes
`.moon/.gitkeep` once real Moon config lands — the placeholder is interim cruft
by design, not a permanent file.

### 2. Git workflow — feature branch + PR, with naming and commit standards

Work lands on `feature/sma-355-bootstrap-polyglot-monorepo-structure` (the
Linear-suggested branch name) via a PR to `main`. This establishes the PR habit
from day one even though branch protection and the CLA bot (SMA-362) are not
yet wired up. **The AC phrase "initial commit pushed to main" is satisfied when
this PR merges** (review S2 — made explicit).

Two conventions are set here for the whole repo, both confirmed against the
prior `paigasus-backend-rust` repo:

- **Branch naming (review R1):** `feature/sma-NNN-<slug>` is the standard going
  forward. This is a deliberate change from the `sven/sma-NNN-<slug>` form used
  in `paigasus-backend-rust`; we adopt Linear's default `gitBranchName`. Stated
  in `CONTRIBUTING.md` so contributors don't drift back to the old form.
- **Conventional commits (review R2):** `feat(scope):`, `fix(scope):`,
  `docs(scope):`, etc., with the scope naming the workspace/area (e.g.
  `feat(rs):`, `fix(contracts):`). `paigasus-backend-rust` follows this and any
  future changelog automation (release-please, post-MVP) is load-bearing on it.
  Documented in `CONTRIBUTING.md` with concrete examples.

### 3. Public-facing doc links — README self-contained, internal links in CONTRIBUTING

This repo is public, but the reference docs (Polyglot Monorepo Scoping,
Development Guidelines, ADRs) are internal Notion pages external readers cannot
open. Therefore:

- **README.md** is fully self-contained — no internal links. It carries the
  project overview and the planned topology inline.
- **CONTRIBUTING.md** holds the internal Notion links (Development Guidelines,
  scoping doc, ADR index), since contributors with access are the audience.

This is a **conscious deviation** from the AC wording "README.md ... link to
the Polyglot Monorepo Scoping doc" — the scoping link moves to CONTRIBUTING
instead. Flagged here so it is a deliberate, reviewable choice rather than an
oversight.

### 4. CODEOWNERS interim ownership

A static root `CODEOWNERS` with `* @SMK1085`. SMA-356 enables Moon's
`codeowners.syncOnRun`, which will later *generate* this file from per-project
ownership; a static root file is the correct interim and Moon adopts it cleanly.

## Content intent

- **README.md** — Identifies `paigasus-core` as the public, Apache-2.0 polyglot
  monorepo for Paigasus. Shows the planned four-workspace topology
  (`contracts/`, `rs/`, `py/`, `ts/`) as a tree. The quickstart is
  forward-looking and honest: it states that per-workspace tooling setup arrives
  in SMA-356–360, rather than printing `moon ci` instructions for tooling that
  does not exist yet.
- **CONTRIBUTING.md** (expanded per review R4) — Functions as canonical
  contributor guidance, covering:
  - Branch-and-PR workflow.
  - Branch-naming convention `feature/sma-NNN-<slug>` (R1).
  - Conventional-commit convention with examples (R2).
  - **Issue filing:** open a **GitHub Issue**; the maintainer triages it into
    Linear internally. (Decision: public-facing intake is GitHub, since external
    contributors cannot access Linear — consistent with §3's public-repo
    principle. Resolves the conflict in review R4, which had said "Linear, not
    GitHub Issues.")
  - CI runs `moon ci` on every PR (forward-reference SMA-361).
  - Local dev setup pointer → "see README quickstart."
  - A **Code conventions** section establishing SPDX headers
    (`SPDX-License-Identifier: Apache-2.0`, language-appropriate comment syntax)
    for source files, so SMA-357/358/359 inherit the rule (review S4).
  - A CLA notice (the bot lands in SMA-362).
  - Links to the internal Development Guidelines, scoping doc, and ADR index
    (internal links live here, not in the public README — see §3).
- **.github/pull_request_template.md** (review S3) — A short template with a
  conventional-commit reminder and an AC-checklist prompt.
- **CODEOWNERS** — `* @SMK1085`.
- **.gitignore** (review R3) — Template-based rather than hand-enumerated. Start
  from a `gitignore.io` set covering `rust, node, python, macos, windows, linux,
  visualstudiocode, jetbrains`, then ensure `.moon/cache/` and `.next/` are
  present. Must include secret-bearing files (`.env`, `.env.local`,
  `.env.*.local`) — their omission is the specific gap that lets a secret get
  committed early. Supersedes the original hand-listed set, which is a subset.

## Verification / done criteria

- All paths in the "In scope" table exist and are committed.
- `LICENSE` is unchanged and remains valid Apache 2.0.
- `git status` is clean on the feature branch after commit.
- The six required top-level directories all appear in `git ls-files` output
  (via their README/.gitkeep), confirming git tracks them.
- README renders with no broken/internal links; CONTRIBUTING's internal links
  point to the real Notion pages confirmed during brainstorming.
- `.gitignore` ignores `.env` / `.env.local` (verify: a staged `.env` does not
  appear in `git status`).
- `CONTRIBUTING.md` covers branch naming, conventional commits, issue filing,
  CI reference, SPDX headers, and CLA — not a stub.
- A PR is opened from the feature branch to `main`.

## References (internal)

- Polyglot Monorepo Scoping § 1 (directory layout) — Notion
- ADR-0003: One public polyglot monorepo + private cloud repo
- ADR-0007: Apache 2.0 license + CLA + trademark
- Development Guidelines — Notion
