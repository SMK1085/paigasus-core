# SMA-355 — Bootstrap polyglot monorepo structure

**Date:** 2026-05-26
**Linear:** [SMA-355](https://linear.app/smaschek/issue/SMA-355/bootstrap-polyglot-monorepo-structure)
**Status:** Design approved, pending implementation

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
| `.github/workflows/.gitkeep` | Placeholder; CI workflows land in later issues. |
| `.moon/.gitkeep` | Placeholder; Moon config lands in SMA-356. |
| `contracts/README.md` | "Proto source of truth + codegen. Scaffolded in SMA-360." |
| `rs/README.md` | "Cargo workspace (libs/bindings/services). Scaffolded in SMA-357." |
| `py/README.md` | "uv workspace. Scaffolded in SMA-358." |
| `ts/README.md` | "pnpm workspace (packages/apps). Scaffolded in SMA-359." |
| `README.md` | Project overview, planned topology, forward-looking quickstart. |
| `CONTRIBUTING.md` | Branch/PR workflow, CLA notice, internal doc links. |
| `CODEOWNERS` | Root file: `* @SMK1085`. |
| `.gitignore` | Rust, Node, Python, Moon, OS noise. |
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
is fully populated immediately by SMA-356.

### 2. Git workflow — feature branch + PR

Work lands on `feature/sma-355-bootstrap-polyglot-monorepo-structure` (the
Linear-suggested branch name) via a PR to `main`. This establishes the PR habit
from day one even though branch protection and the CLA bot (SMA-362) are not
yet wired up. The AC phrase "initial commit pushed to main" is satisfied when
the PR merges.

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
- **CONTRIBUTING.md** — Branch-and-PR workflow, a CLA notice (the bot lands in
  SMA-362), and links to the internal Development Guidelines, scoping doc, and
  ADR index.
- **CODEOWNERS** — `* @SMK1085`.
- **.gitignore** — `target/`, `node_modules/`, `.next/`, `__pycache__/`,
  `.venv/`, `.moon/cache/`, `.DS_Store`.

## Verification / done criteria

- All paths in the "In scope" table exist and are committed.
- `LICENSE` is unchanged and remains valid Apache 2.0.
- `git status` is clean on the feature branch after commit.
- The six required top-level directories all appear in `git ls-files` output
  (via their README/.gitkeep), confirming git tracks them.
- README renders with no broken/internal links; CONTRIBUTING's internal links
  point to the real Notion pages confirmed during brainstorming.
- A PR is opened from the feature branch to `main`.

## References (internal)

- Polyglot Monorepo Scoping § 1 (directory layout) — Notion
- ADR-0003: One public polyglot monorepo + private cloud repo
- ADR-0007: Apache 2.0 license + CLA + trademark
- Development Guidelines — Notion
