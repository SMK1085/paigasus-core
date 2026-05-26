# Review: SMA-355 — Bootstrap polyglot monorepo structure

**Date:** 2026-05-26
**Reviewer:** Claude (Cowork)
**Status:** Approve with required changes
**Spec under review:** [`2026-05-26-bootstrap-monorepo-design.md`](./2026-05-26-bootstrap-monorepo-design.md)
**Linear:** [SMA-355](https://linear.app/smaschek/issue/SMA-355/bootstrap-polyglot-monorepo-structure)

## Verdict

The design is scope-disciplined, with conscious deviations from the issue's
acceptance criteria flagged transparently. Ready to implement once the four
items in *Required* are addressed. *Suggested* items are polish that can land
in this PR or a follow-up.

## Required (address before merging the implementation PR)

### R1 — Document `feature/` as the branch-naming standard

Decision (confirmed 2026-05-26): **`feature/sma-NNN-<slug>` is the branch-naming
standard for `paigasus-core` going forward.** This is a deliberate change from
the `sven/sma-NNN-<slug>` form used in `paigasus-backend-rust`. The design
adopts Linear's default `gitBranchName` (which produces `feature/...`); this
review confirms that as intentional.

**Action:**

- Add the decision to the design (§2 "Git workflow") as a one-line statement
  noting the change from the previous repo's convention.
- Add a one-line convention statement to `CONTRIBUTING.md` so future
  contributors don't drift back to `sven/...`.

### R2 — Establish conventional commits in `CONTRIBUTING.md`

`paigasus-backend-rust` uses `feat(scope):`, `fix(scope):`, `docs(scope):`
consistently (visible in `git log`). Any future changelog automation
(release-please, etc., expected post-MVP) is load-bearing on the convention
being followed from day one. The design's `CONTRIBUTING.md` content intent
does not currently mention it.

**Action:** Add a paragraph to `CONTRIBUTING.md` establishing conventional
commits with a couple of concrete examples (one `feat(scope):`, one
`fix(scope):`).

### R3 — Replace the enumerated `.gitignore` with a template-based start

The currently-listed entries (`target/`, `node_modules/`, `.next/`,
`__pycache__/`, `.venv/`, `.moon/cache/`, `.DS_Store`) are correct, but the
absence of `.env`, `.env.local`, `.env.*.local` is specifically risky — that's
the gap that lets a secret commit happen in week two. Also missing: `*.log`,
`dist/`, `build/`, IDE noise (`.idea/`, `.vscode/*.log`, `.history/`), and
Windows/Linux equivalents to `.DS_Store` (`Thumbs.db`, `.directory`).

**Action:** Start from a [gitignore.io](https://gitignore.io/) template for
`rust,node,python,macos,windows,linux,visualstudiocode,jetbrains` and append
`.moon/cache/` and `.next/` if not already covered. The opinionated
enumeration grows linearly with what you forget; the template grows with what
the community already learned.

### R4 — Flesh out `CONTRIBUTING.md` content intent

The current intent — "branch/PR workflow, CLA notice, internal Notion links" —
is too sparse for `CONTRIBUTING.md` to function as canonical guidance.
Contributors won't trust a stub and may file issues in the wrong place or
commit without conventions.

**Action:** Expand the `CONTRIBUTING.md` content intent (and the resulting
file) to also include:

- Branch naming convention (per R1)
- Conventional commits convention (per R2)
- Issue filing: **Linear, not GitHub Issues**
- CI runs on every PR (forward-reference SMA-361)
- Local dev setup pointer → "see README quickstart"

## Suggested (nice-to-have; can land in this PR or a follow-up)

### S1 — Note that `.moon/.gitkeep` is removed by SMA-356

SMA-356 immediately populates `.moon/workspace.yml`, `toolchain.yml`, and
`tasks.yml`. The `.gitkeep` placeholder becomes orphan cruft at that point.

**Action:** Add a one-line note in §1 of the design (or in SMA-356's design
when written): "`.moon/.gitkeep` is deleted by SMA-356 once real Moon config
lands."

### S2 — Tighten the "initial commit" AC wording

The Linear AC reads "Initial commit pushed to `main` branch" but the design
correctly routes through a feature-branch PR. Either tweak the AC wording to
"Initial commit lands on `main` via PR from feature branch," or add a one-line
note in §2 making explicit that the AC is satisfied at PR merge.

### S3 — Consider a PR template

Add `.github/pull_request_template.md` with a conventional-commit reminder and
an AC-checklist prompt. Sets a good pattern from day one. Can be deferred to a
separate small follow-up issue if it's out of scope here.

### S4 — License-header convention for source files

Not relevant for SMA-355 (no source files yet), but worth establishing now as
a convention that SMA-357 / SMA-358 / SMA-359 follow: source files start with
`// SPDX-License-Identifier: Apache-2.0` (or language-appropriate equivalent).
Add to `CONTRIBUTING.md` under a "Code conventions" section.

## Affirmed (strong calls — keep as designed)

- **Scope discipline (in-scope / out-of-scope tables).** Each future file is
  explicitly deferred to its owning issue. Respects the dependency graph and
  prevents scope creep.
- **Conscious AC deviation in §3.** Moving internal Notion links from
  `README.md` to `CONTRIBUTING.md` is the right call (public README + internal-
  only links don't mix), and the discipline of flagging it as a deliberate
  deviation is exactly right.
- **Per-dir README pattern (§1).** Workspace-root READMEs are self-documenting
  and survive as real config lands; the `.gitkeep`-only choice for pure-config
  dirs is the right contrast.
- **CODEOWNERS interim strategy (§4).** Static `* @SMK1085` with the knowledge
  that Moon adopts it cleanly via `codeowners.syncOnRun` is the correct
  interim.
- **Forward-looking honest quickstart.** Refusing to print fake `moon ci`
  instructions for tooling that doesn't exist yet is the right move.

## Verification performed by the reviewer

- `LICENSE` confirmed as Apache 2.0 (header line: `Apache License, Version 2.0`).
- Feature branch `feature/sma-355-bootstrap-polyglot-monorepo-structure`
  exists; design spec committed (`baada8d docs: design spec for SMA-355
  monorepo bootstrap`).
- Pre-implementation state is `LICENSE` + `docs/` only; clean slate for the
  bootstrap.

## References

- Design spec: [`2026-05-26-bootstrap-monorepo-design.md`](./2026-05-26-bootstrap-monorepo-design.md)
- Linear: [SMA-355](https://linear.app/smaschek/issue/SMA-355/bootstrap-polyglot-monorepo-structure)
- ADR-0007 (Apache 2.0 + CLA + trademark) — Notion
- Polyglot Monorepo Scoping § 1 — Notion
