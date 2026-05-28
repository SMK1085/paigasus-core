# SMA-383 — Document `moon.yml` field-order convention and SPDX carve-out for config files

**Status:** Designed (brainstorming complete)
**Date:** 2026-05-28
**Linear:** [SMA-383](https://linear.app/smaschek/issue/SMA-383/document-moonyml-field-order-config-file-spdx-carve-out-in)
**Branch:** `feature/sma-383-document-moonyml-field-order-config-file-spdx-carve-out-in`
**References:** SMA-381 (established `layer:` field + field ordering across rust + py and all three
scaffold templates), SMA-380 (introduced the `-rs`/`-py`/`-ts` Moon id suffix convention),
SMA-359 (ts workspace, upcoming), SMA-360 (contracts workspace, upcoming).

## Goal

Codify in `CONTRIBUTING.md` two `moon.yml` / config-file conventions that already hold in the rust +
py workspaces, so they carry forward unambiguously when the `contracts/` (SMA-360) and `ts/`
(SMA-359) workspaces are scaffolded — and so they don't get re-litigated on every config-file PR:

1. The top-level field order used inside `moon.yml` files.
2. An explicit SPDX-header exemption for config files (the current rule says "every source file",
   which is technically correct but invites argument about whether `moon.yml` counts).

Pure documentation change. No code, no template changes, no behavior impact.

## Decision

Edit `CONTRIBUTING.md` only. Inside the existing `## Code conventions` section make two changes:

- **Rewrite the SPDX bullet** to enumerate the config-file carve-out instead of leaving it implicit.
- **Append a new `### Moon project files` subsection** documenting the 7-step field order.

Scope is deliberately doc-only. The `.moon/templates/python/moon.yml` and
`.moon/templates/typescript/moon.yml` files currently omit an explicit `id:` line, which is a
real inconsistency with the rust template (and with the `-py`/`-ts` suffix policy in SMA-380),
but addressing it belongs with the workspace bootstrap issues that exercise those templates
(SMA-359 / SMA-360), not here. Surfaced as a follow-up; not fixed in this PR.

## Changes

### Change A — SPDX bullet rewrite (existing first bullet of `## Code conventions`)

Replace the current bullet with one that keeps the existing intent (SPDX on every source file in
the supported languages) and adds an enumerated list of exempt config-file types. The enumeration
matches what's actually in the repo today (and what's coming with SMA-359 / SMA-360):

```markdown
- Every source file starts with an SPDX license header, using the language's
  comment syntax:
  - Rust / TypeScript / Protobuf: `// SPDX-License-Identifier: Apache-2.0`
  - Python: `# SPDX-License-Identifier: Apache-2.0`

  Config files carry no SPDX header. This includes `moon.yml`, `*.toml`,
  `*.yaml` / `*.yml`, `*.json`, lockfiles (`Cargo.lock`, `uv.lock`,
  `pnpm-lock.yaml`), and dotfiles like `.gitignore` / `.editorconfig`.
```

The enumerated list was chosen over a general "config files are exempt" sentence because the
general form invites edge-case re-litigation (Dockerfiles? Makefiles? shell scripts?), which is
the specific failure mode this issue is trying to prevent.

### Change B — New subsection `### Moon project files` (appended to `## Code conventions`)

Add immediately after the existing two bullets, as a sibling subsection:

```markdown
### Moon project files

Hand-written `moon.yml` files use a fixed top-level field order so diffs
across workspaces stay readable and so generated/scaffolded files line up
with hand-written ones:

1. `$schema`
2. `id` (when present)
3. `layer`
4. `language`
5. `dependsOn`
6. `fileGroups`
7. `tasks`, `options`, and any remaining fields

Use `layer:`, not the pre-2.x `type:` — Moon 2.2.5's parser rejects `type:`.
The three scaffold templates under `.moon/templates/{rust,python,typescript}/`
emit this same order, so `moon generate` output is consistent with
hand-written projects (SMA-381).
```

#### Design notes for the field-order list

- **`id (when present)`** — phrased as positional, not prescriptive. The convention defines *where*
  `id:` goes in the ordering, not *whether* it's required. (When `id:` is required and what suffix
  it takes is governed by SMA-380's `-rs`/`-py`/`-ts` convention, which is recorded elsewhere; the
  workspace-root `py/moon.yml` legitimately omits `id:` today, so a "required" phrasing here would
  be misleading.)
- **`fileGroups` between `dependsOn` and `tasks`** — mirrors the actual layout in `py/moon.yml`,
  the only hand-written file in the repo that currently uses `fileGroups`. Templates don't emit
  `fileGroups`; this slot only matters for hand-written overrides.
- **`layer:` vs `type:` reminder** — kept in this subsection (rather than as a separate item)
  because anyone reaching for the field-order rule is most likely to need this reminder at the
  same time. Also recorded in [SMA-381's design doc](./2026-05-27-sma-381-rust-scaffold-type-design.md).

## Out of scope / follow-up

- **Python/TypeScript template `id:` gap.** Both templates omit an explicit `id:` line, which
  means `moon generate` produces projects without the SMA-380 stack suffix unless the contributor
  adds it manually. Real bug, but it only bites when those templates are actually used, which
  happens in SMA-359 (ts) and SMA-360 (contracts). Surface as a note in the PR description; don't
  fix here.
- **No ADR.** Per CLAUDE.md, ADRs are for significant choices; this codifies existing practice.

## Verification

1. `CONTRIBUTING.md` renders cleanly (preview locally; check the new bullet's nested
   indentation and the new subsection's numbering).
2. The enumerated SPDX carve-out matches the file types actually present in the repo
   (`moon.yml`, `*.toml`, `*.yaml`, `*.json`, lockfiles, dotfiles). Spot-check with `find` /
   `git ls-files` that no listed type currently carries an SPDX header.
3. The documented field order matches every existing hand-written `moon.yml` (8 files plus the
   three template files). Spot-check by reading each file's first ~6 lines.
4. `moon ci :build :test` stays green (no Moon config touched, but run anyway as a sanity check).

## Acceptance criteria

- [ ] `CONTRIBUTING.md` "Code conventions" SPDX bullet enumerates exempt config file types
      (`moon.yml`, `*.toml`, `*.yaml` / `*.yml`, `*.json`, lockfiles, and dotfiles like
      `.gitignore` / `.editorconfig`).
- [ ] `CONTRIBUTING.md` has a `### Moon project files` subsection inside "Code conventions"
      documenting the 7-step field order (`$schema` → `id` → `layer` → `language` →
      `dependsOn` → `fileGroups` → `tasks`/other), with the `layer:` vs `type:` note.
- [ ] No other files modified.
- [ ] Single commit with `docs(repo):` scope referencing SMA-383.
- [ ] PR description surfaces the python/typescript template `id:` gap as a follow-up
      observation (not fixed here).
