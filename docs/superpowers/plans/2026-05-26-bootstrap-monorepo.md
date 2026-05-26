# Bootstrap Polyglot Monorepo — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Create the `paigasus-core` repository shell — six top-level workspace directories plus baseline docs (README, CONTRIBUTING, CODEOWNERS, .gitignore, PR template) — and open a PR to `main`.

**Architecture:** Pure scaffolding. No build system or source code yet; those land in the issues this one blocks (SMA-356 through SMA-361). Empty workspace dirs are made git-trackable via a self-documenting `README.md` (workspace roots) or `.gitkeep` (pure-config dirs). All work happens on `feature/sma-355-bootstrap-polyglot-monorepo-structure` and merges via PR.

**Tech Stack:** Git, GitHub, Markdown. (Moon/Cargo/uv/pnpm/buf are referenced in docs but deliberately not configured here.)

**Spec:** [`docs/superpowers/specs/2026-05-26-bootstrap-monorepo-design.md`](../specs/2026-05-26-bootstrap-monorepo-design.md)

---

## Preconditions

- Current branch is `feature/sma-355-bootstrap-polyglot-monorepo-structure` (already created).
- `LICENSE` (Apache 2.0) already exists at repo root — **do not modify it**.
- The spec docs already live under `docs/superpowers/`.

Verify before starting:

```bash
git branch --show-current   # → feature/sma-355-bootstrap-polyglot-monorepo-structure
test -f LICENSE && echo "LICENSE present"
```

## File structure (what this plan creates)

| File | Responsibility |
|------|----------------|
| `contracts/README.md` | Describe the proto/codegen workspace; mark scaffolded in SMA-360. |
| `rs/README.md` | Describe the Cargo workspace (libs/bindings/services); SMA-357. |
| `py/README.md` | Describe the uv workspace; SMA-358. |
| `ts/README.md` | Describe the pnpm workspace (packages/apps); SMA-359. |
| `.moon/.gitkeep` | Make `.moon/` trackable; deleted by SMA-356. |
| `.github/workflows/.gitkeep` | Make `.github/workflows/` trackable; CI lands in SMA-361. |
| `.gitignore` | Ignore build output, env/secret files, OS/editor noise across all stacks. |
| `CODEOWNERS` | Default ownership `* @SMK1085`. |
| `.github/pull_request_template.md` | PR summary + AC checklist + convention reminders. |
| `README.md` | Public, self-contained project overview + topology + forward-looking quickstart. |
| `CONTRIBUTING.md` | Canonical contributor guidance; holds the internal Notion links. |

---

## Task 1: Workspace directory skeleton

Creates the four workspace-root READMEs and the two `.gitkeep` placeholders, making all six required top-level directories exist and be tracked by git.

**Files:**
- Create: `contracts/README.md`
- Create: `rs/README.md`
- Create: `py/README.md`
- Create: `ts/README.md`
- Create: `.moon/.gitkeep`
- Create: `.github/workflows/.gitkeep`

- [ ] **Step 1: Create `contracts/README.md`**

```markdown
# contracts/

Protobuf source of truth and code generation for paigasus-core. Holds the
`.proto` definitions under `proto/paigasus/<context>/<version>/` and the
[buf](https://buf.build) configuration that generates Rust, Python, and
TypeScript bindings.

**Status:** scaffolded in SMA-360. Empty until the buf workspace lands.
```

- [ ] **Step 2: Create `rs/README.md`**

```markdown
# rs/

Rust workspace for paigasus-core — a single [Cargo](https://doc.rust-lang.org/cargo/)
workspace with three crate groups:

- `crates/libs/` — reusable libraries (e.g. `paigasus-kernel`, `paigasus-proto`)
- `crates/bindings/` — FFI wrappers (PyO3, napi-rs, wasm-bindgen)
- `crates/services/` — service binaries

**Status:** scaffolded in SMA-357. Empty until the Cargo workspace lands.
```

- [ ] **Step 3: Create `py/README.md`**

```markdown
# py/

Python workspace for paigasus-core, managed with [uv](https://docs.astral.sh/uv/).
Packages live under `packages/` (e.g. `paigasus-proto`, `paigasus-kernel`,
`paigasus-ml`).

**Status:** scaffolded in SMA-358. Empty until the uv workspace lands.
```

- [ ] **Step 4: Create `ts/README.md`**

```markdown
# ts/

TypeScript workspace for paigasus-core, managed with [pnpm](https://pnpm.io/).
Publishable libraries live under `packages/`; deployable apps under `apps/`.

**Status:** scaffolded in SMA-359. Empty until the pnpm workspace lands.
```

- [ ] **Step 5: Create the two `.gitkeep` placeholders**

Both files are empty. Create the parent directories as needed.

Run:
```bash
mkdir -p .moon .github/workflows
touch .moon/.gitkeep .github/workflows/.gitkeep
```

- [ ] **Step 6: Verify all six top-level directories are now staged-trackable**

Run:
```bash
git add -A
git status --porcelain
```
Expected: lists exactly these new files (order may vary):
```
A  .github/workflows/.gitkeep
A  .moon/.gitkeep
A  contracts/README.md
A  py/README.md
A  rs/README.md
A  ts/README.md
```
Confirm the six required dirs (`contracts/ rs/ py/ ts/ .github/ .moon/`) are all represented. If any is missing, the directory was empty and git skipped it — add its placeholder.

- [ ] **Step 7: Commit**

```bash
git commit -m "chore: scaffold top-level workspace directories"
```

---

## Task 2: `.gitignore`

A comprehensive, template-derived `.gitignore` (Rust + Node + Python + Moon + macOS/Windows/Linux + editors), with explicit secret-file coverage. The secret entries are the load-bearing part — their absence is what lets a `.env` get committed in week two.

**Files:**
- Create: `.gitignore`

- [ ] **Step 1: Create `.gitignore`**

```gitignore
# ─── Secrets / environment ───────────────────────────────────────────────
.env
.env.*
!.env.example
*.pem
*.key
*.p12

# ─── Rust ────────────────────────────────────────────────────────────────
target/
**/*.rs.bk
*.pdb

# ─── Node / TypeScript ───────────────────────────────────────────────────
node_modules/
.next/
out/
.pnpm-store/
*.tsbuildinfo
npm-debug.log*
yarn-debug.log*
yarn-error.log*
pnpm-debug.log*

# ─── Python ──────────────────────────────────────────────────────────────
__pycache__/
*.py[cod]
*$py.class
.venv/
venv/
.mypy_cache/
.ruff_cache/
.pytest_cache/
*.egg-info/

# ─── Build output (shared) ───────────────────────────────────────────────
dist/
build/

# ─── Moon ────────────────────────────────────────────────────────────────
.moon/cache/
.moon/docker/

# ─── Editors / IDE ───────────────────────────────────────────────────────
.idea/
.vscode/*
!.vscode/extensions.json
!.vscode/settings.json
*.swp
.history/

# ─── OS noise ────────────────────────────────────────────────────────────
.DS_Store
Thumbs.db
.directory
```

- [ ] **Step 2: Verify secret files are ignored**

Run:
```bash
git add .gitignore
printf 'SECRET=x\n' > .env
printf 'SECRET=x\n' > .env.local
git check-ignore .env .env.local && git status --porcelain | grep -E '\.env' || echo "OK: .env files ignored, not shown in status"
```
Expected: `git check-ignore` prints `.env` and `.env.local` (exit 0); the `grep` finds nothing in `git status`, so the `|| echo` branch prints `OK: .env files ignored, not shown in status`.

- [ ] **Step 3: Verify the example file is NOT ignored**

Run:
```bash
printf 'SECRET=\n' > .env.example
git check-ignore .env.example; echo "exit=$?"
```
Expected: no path printed and `exit=1` (the `!.env.example` negation un-ignores it).

- [ ] **Step 4: Clean up the temp files**

Run:
```bash
rm -f .env .env.local .env.example
```

- [ ] **Step 5: Commit**

```bash
git add .gitignore
git commit -m "chore: add comprehensive .gitignore with secret-file coverage"
```

---

## Task 3: Repo metadata — CODEOWNERS + PR template

**Files:**
- Create: `CODEOWNERS`
- Create: `.github/pull_request_template.md`

- [ ] **Step 1: Create `CODEOWNERS`**

```
# Default owner for everything in the repo.
# Moon will later manage this file via codeowners.syncOnRun (SMA-356);
# this static entry is the interim default.
* @SMK1085
```

- [ ] **Step 2: Create `.github/pull_request_template.md`**

```markdown
## Summary

<!-- What does this PR do and why? Link the Linear issue, e.g. SMA-NNN. -->

## Acceptance criteria

<!-- Copy the issue's acceptance criteria and check them off as you satisfy them. -->

- [ ]
- [ ]

## Checklist

- [ ] Branch is named `feature/sma-NNN-<slug>`
- [ ] Commits follow Conventional Commits (`type(scope): subject`)
- [ ] `moon ci` passes locally (once workspace tooling is available)
- [ ] Docs updated if setup or behavior changed
```

- [ ] **Step 3: Verify both files exist and CODEOWNERS has the default rule**

Run:
```bash
test -f CODEOWNERS && test -f .github/pull_request_template.md && grep -q '^\* @SMK1085' CODEOWNERS && echo "OK"
```
Expected: prints `OK`.

- [ ] **Step 4: Commit**

```bash
git add CODEOWNERS .github/pull_request_template.md
git commit -m "chore: add CODEOWNERS and pull request template"
```

---

## Task 4: README.md (root)

Public, self-contained project overview. **No internal links** — external readers must not hit pages they can't open. The quickstart is forward-looking and honest about tooling not existing yet.

**Files:**
- Create: `README.md`

- [ ] **Step 1: Create `README.md`**

````markdown
# paigasus-core

Public, Apache-2.0 polyglot monorepo for **Paigasus** — the open core of the
platform. It houses the shared proto contracts, the Rust behavioral kernel and
its language bindings, and the Python and TypeScript workspaces built on top of
them.

## Repository layout

```
paigasus-core/
├── contracts/   # Protobuf source of truth + codegen (buf)
├── rs/          # Rust: Cargo workspace — libs, FFI bindings, services
├── py/          # Python: uv workspace
├── ts/          # TypeScript: pnpm workspace — packages + apps
├── .moon/       # Moon task-runner configuration
└── .github/     # CI workflows and repo automation
```

Each workspace has its own `README.md` with more detail.

## Status

Bootstrapping. The directory shell and baseline docs are in place; the
individual workspaces are being scaffolded issue-by-issue — Moon configuration,
the Cargo / uv / pnpm workspaces, and the proto toolchain. Until those land
there is no unified build yet.

## Quickstart

Tooling is orchestrated by [Moon](https://moonrepo.dev). Once the workspaces
are scaffolded, the standard entry point will be:

```bash
# Available after the workspace-setup issues land:
moon ci          # run the affected build / test / lint graph
```

For now, clone the repo and read the per-workspace `README.md` files to see
what each area will hold.

## Contributing

See [CONTRIBUTING.md](./CONTRIBUTING.md).

## License

[Apache License 2.0](./LICENSE).
````

- [ ] **Step 2: Verify the README contains no internal Notion links**

Run:
```bash
grep -i 'notion.so' README.md && echo "FAIL: internal link in public README" || echo "OK: no internal links"
```
Expected: prints `OK: no internal links`.

- [ ] **Step 3: Commit**

```bash
git add README.md
git commit -m "docs: add root README with project overview and topology"
```

---

## Task 5: CONTRIBUTING.md

Canonical contributor guidance: issue filing (GitHub → triaged to Linear), branch naming, conventional commits, CI reference, SPDX header convention, CLA notice, and the internal Notion links. This is the **only** doc that carries internal links.

**Files:**
- Create: `CONTRIBUTING.md`

- [ ] **Step 1: Create `CONTRIBUTING.md`**

````markdown
# Contributing to paigasus-core

Thanks for your interest in contributing. This document is the canonical guide
to how we work.

## Reporting issues

Open a [GitHub Issue](../../issues). The maintainer triages reports into an
internal Linear tracker, so you don't need Linear access to file one. Where you
can, include reproduction steps and name the affected workspace (`contracts`,
`rs`, `py`, or `ts`).

## Development workflow

1. Branch off `main` as `feature/sma-NNN-<slug>`, where `sma-NNN` is the Linear
   issue key and `<slug>` is a short kebab-case description — e.g.
   `feature/sma-357-bootstrap-rs-cargo-workspace`. External contributors without
   a Linear key may use `feature/<slug>`.
2. Make focused changes with conventional commits (see below).
3. Open a pull request against `main`. CI runs `moon ci` on every PR and must
   pass before merge.
4. Fill in the PR template's summary and acceptance-criteria checklist.

> **Branch-naming note:** this repo uses `feature/...`, a deliberate change from
> the `sven/...` form used in earlier Paigasus repos. Stick to `feature/...`.

## Local development

Per-workspace setup lives in each workspace's `README.md`; the overall toolchain
and entry points are summarized in the root [README](./README.md#quickstart).
The unified `moon ci` flow becomes available once the workspace-setup issues
land.

## Commit messages

We follow [Conventional Commits](https://www.conventionalcommits.org). Use a
type plus a scope naming the workspace or area:

```
feat(rs): add PRN parser to paigasus-kernel
fix(contracts): correct pagination field number in common/v1
docs(py): document uv workspace setup
```

Common types: `feat`, `fix`, `docs`, `chore`, `refactor`, `test`, `ci`. Keep
this consistent — changelog automation depends on it.

## Code conventions

- Every source file starts with an SPDX license header, using the language's
  comment syntax:
  - Rust / TypeScript: `// SPDX-License-Identifier: Apache-2.0`
  - Python: `# SPDX-License-Identifier: Apache-2.0`
- Per-language formatting and linting are enforced by each workspace's Moon
  tasks; run the workspace's `lint`/`fmt` tasks before pushing once it's set up.

## Contributor License Agreement

Before your first contribution can be merged you'll be asked to sign a CLA
(automated via a bot — currently being set up). The CLA preserves the project's
ability to relicense and dual-license contributed code; external contributions
can't be merged without it.

## Internal references

For maintainers and contributors with workspace access:

- [Development Guidelines](https://www.notion.so/368830e8fbaa81d297a1f2dacf2f2ff5)
- [Polyglot Monorepo Scoping](https://www.notion.so/368830e8fbaa8101b0ffded7a3de3b53)
- [Architecture Decision Records](https://www.notion.so/368830e8fbaa816cb411c7ee1682c175)
````

- [ ] **Step 2: Verify CONTRIBUTING covers each required topic**

Run:
```bash
for s in "GitHub Issue" "feature/sma-NNN" "Conventional Commits" "Local development" "SPDX" "CLA" "notion.so"; do
  grep -q "$s" CONTRIBUTING.md && echo "OK: $s" || echo "MISSING: $s"
done
```
Expected: every line prints `OK: ...` (seven OKs, no MISSING).

- [ ] **Step 3: Commit**

```bash
git add CONTRIBUTING.md
git commit -m "docs: add CONTRIBUTING with workflow, conventions, and CLA notice"
```

---

## Task 6: Final verification and PR

**Files:** none created — verification and PR only.

- [ ] **Step 1: Confirm the working tree is clean**

Run:
```bash
git status --porcelain
```
Expected: no output (clean tree; the temp `.env*` files from Task 2 were removed).

- [ ] **Step 2: Confirm all six required top-level dirs are tracked**

Run:
```bash
for d in contracts rs py ts .github .moon; do
  git ls-files "$d" | head -1 | grep -q . && echo "OK: $d tracked" || echo "MISSING: $d"
done
```
Expected: six `OK: ... tracked` lines.

- [ ] **Step 3: Confirm LICENSE is unchanged**

Run:
```bash
git log --oneline -- LICENSE | tail -1   # should still be the initial commit
git diff bc9a98f -- LICENSE              # should produce no output
```
Expected: the `git diff` produces no output (LICENSE untouched since the initial commit).

- [ ] **Step 4: Confirm the full deliverable set is present**

Run:
```bash
git ls-files | grep -E '^(README\.md|CONTRIBUTING\.md|CODEOWNERS|\.gitignore|\.github/pull_request_template\.md|\.github/workflows/\.gitkeep|\.moon/\.gitkeep|contracts/README\.md|rs/README\.md|py/README\.md|ts/README\.md)$' | sort
```
Expected: all 11 paths listed.

- [ ] **Step 5: Push the branch**

Run:
```bash
git push -u origin feature/sma-355-bootstrap-polyglot-monorepo-structure
```

- [ ] **Step 6: Open the PR to `main`**

Run:
```bash
gh pr create --base main \
  --title "SMA-355: Bootstrap polyglot monorepo structure" \
  --body "$(cat <<'EOF'
## Summary

Bootstraps the `paigasus-core` repository shell: the six top-level workspace
directories (`contracts/ rs/ py/ ts/ .github/ .moon/`) plus baseline docs
(README, CONTRIBUTING, CODEOWNERS, .gitignore, PR template). Foundational work
that the rest of Phase 1 (SMA-356 → SMA-361) depends on.

Workspace internals (Moon config, Cargo/uv/pnpm/buf workspaces, real CI) are
intentionally out of scope and owned by the issues this one blocks.

## Acceptance criteria

- [x] Top-level dirs created: contracts/, rs/, py/, ts/, .github/, .moon/
- [x] README.md (public, self-contained — scoping-doc link moved to CONTRIBUTING per design §3)
- [x] LICENSE — Apache 2.0 (pre-existing)
- [x] CONTRIBUTING.md — workflow, conventions, CLA notice
- [x] .gitignore — Rust/Node/Python/Moon/OS + secrets
- [x] CODEOWNERS — `* @SMK1085`
- [ ] Merged to main (on merge)

Design: docs/superpowers/specs/2026-05-26-bootstrap-monorepo-design.md

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```
Expected: `gh` prints the new PR URL.

- [ ] **Step 7: Report the PR URL back** for review before merge.

---

## Notes for the implementer

- **Do not** create any `*.yml` Moon files, `Cargo.toml`, `pyproject.toml`,
  `pnpm-workspace.yaml`, `buf.yaml`, deep nested trees, or real CI workflow
  files. Those belong to SMA-356–361 and are explicitly out of scope (spec §"Out of scope").
- The internal Notion links in `CONTRIBUTING.md` are real and were verified
  during brainstorming — keep them exactly as written.
- If `gh` is not authenticated, stop at Step 5 and report; the user can open the
  PR manually or authenticate with `gh auth login`.
