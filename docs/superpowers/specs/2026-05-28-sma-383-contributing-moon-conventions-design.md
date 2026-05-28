# SMA-383 — Document `moon.yml` field-order convention and SPDX carve-out for config files

**Status:** Designed (brainstorming complete; review pass applied 2026-05-28 — see
[review doc](../reviews/2026-05-28-sma-383-contributing-moon-conventions-design-review.md))
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

At the same time, close the consistency gap surfaced by review: the `python` and `typescript`
scaffold templates omit an `id:` line, which means `moon generate` produces projects that violate
the SMA-380 `-py`/`-ts` suffix convention from the moment they're scaffolded. Fixing them here
(one line per template) keeps the documented convention and the generators self-consistent on day
one.

## Decision

Edit `CONTRIBUTING.md` (docs) and two scaffold templates (alignment fix). Two commits:

- `docs(repo):` — `CONTRIBUTING.md` edits.
- `fix(repo):` — `id:` lines added to `.moon/templates/python/moon.yml` and
  `.moon/templates/typescript/moon.yml`.

Inside `CONTRIBUTING.md`'s `## Code conventions` section:

- **Restructure the SPDX guidance** into the rule + three exemption rules (hand-written config,
  generated files, markdown docs), with examples kept non-exhaustive.
- **Append a new `### Moon project files` subsection** documenting the field order with explicit
  positions for `tasks` and `options:`, the `layer:` vs `type:` reminder, and an enumeration of
  the `layer:` values actually used in the repo today.

## Changes

### Change A — SPDX guidance restructure (replaces the existing first bullet of `## Code conventions`)

Replace the single SPDX bullet with four bullets that separate the rule from its exemptions and
state *why* each exemption applies. Examples are explicitly non-exhaustive so the next contributor
adding a new file type (Dockerfile, Makefile, shell script, etc.) reads the rule rather than
re-litigating the list:

```markdown
- Every source file starts with an SPDX license header, using the language's
  comment syntax:
  - Rust / TypeScript / Protobuf: `// SPDX-License-Identifier: Apache-2.0`
  - Python: `# SPDX-License-Identifier: Apache-2.0`
- Hand-written config carries no SPDX header. Examples in this repo:
  `moon.yml`, `*.toml`, `*.yaml` / `*.yml`, `*.json`, and dotfiles like
  `.gitignore` / `.editorconfig`. If you're unsure for a new file type, ask in
  the PR — it's almost always config.
- Generated files (lockfiles such as `Cargo.lock` / `uv.lock` /
  `pnpm-lock.yaml`, plus codegen output) carry whatever header the generator
  emits. Don't hand-edit a generated file's header.
- Markdown docs (`README.md`, `CONTRIBUTING.md`, ADRs, design specs) and the
  `LICENSE` file itself carry no SPDX header.
```

Rationale for the four-bullet split:

- **Config vs generated** are exempt for different reasons. Lockfiles aren't "config" in any
  natural sense — they're machine-output. Splitting these makes the reason legible and lets
  upcoming codegen output (e.g. buf-generated Rust files in SMA-360) inherit the right rule.
- **Markdown docs** weren't covered at all by the previous bullet. The repo already contains
  `README.md`, `CONTRIBUTING.md`, and the spec/plan files under `docs/superpowers/`, none of which
  carry SPDX.
- **Non-exhaustive examples** (`Examples in this repo:` plus "If you're unsure, ask") close the
  "is the list closed?" loop without forcing every future file type into a closed enumeration.

### Change B — New subsection `### Moon project files` (appended to `## Code conventions`)

Add immediately after the SPDX bullets and the per-language formatting bullet, as a sibling
subsection:

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
7. `tasks`
8. `options`
9. Any remaining fields (alphabetical)

Use `layer:`, not the pre-2.x `type:` — Moon 2.2.5's parser rejects `type:`.
The values in active use are `library` (importable code, e.g. the rust
crates in `rs/crates/libs/` and the py packages in `py/packages/`),
`application` (runnable binary, e.g. `paigasus-gateway-rs`), and
`configuration` (workspace-root project that aggregates child projects,
e.g. `py/moon.yml`). Moon's full set of seven values is documented in its
[project config docs](https://moonrepo.dev/docs/config/project) — pick
`library` if unsure.

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
- **`tasks` and `options` as separate slots (7, 8)** — `options:` is a project-level Moon field
  (cache/affected-files behaviour). No file uses it today, but giving it an explicit slot prevents
  "where does `options:` go?" coming up the first time someone needs to override task caching.
- **`layer:` vs `type:` reminder** — kept in this subsection (rather than as a separate item)
  because anyone reaching for the field-order rule is most likely to need this reminder at the
  same time. Also recorded in [SMA-381's design doc](./2026-05-27-sma-381-rust-scaffold-type-design.md).
- **`layer:` values enumerated** — three values are in active use across the 8 hand-written
  projects (`library` ×7, `application` ×1, `configuration` ×1). Naming them in CONTRIBUTING.md
  saves the next contributor a doc dive; pointing to Moon's docs covers the remaining four
  (`automation`, `scaffolding`, `tool`, `unknown`).

### Change C — Template alignment (`fix(repo):` commit)

Bring the `python` and `typescript` scaffold templates into line with the documented field order
and SMA-380's suffix convention by adding an `id:` line modelled on the `rust` template:

| File | Change |
|------|--------|
| `.moon/templates/python/moon.yml` | Insert `id: '{{ name }}-py'` between `$schema` and `layer`. |
| `.moon/templates/typescript/moon.yml` | Insert `id: '{{ name }}-ts'` between `$schema` and `layer`. |

After this change, every template emits the documented field order, and `moon generate` produces
projects that already satisfy the SMA-380 suffix convention without the contributor remembering to
patch the generated file. Mirrors the `id: '{{ name }}-rs'` line already present in
`.moon/templates/rust/moon.yml`.

## Out of scope / follow-up

- **No ADR.** Per CLAUDE.md, ADRs are for significant choices; this codifies existing practice.
- **Notion scoping-doc drift.** The "Polyglot Monorepo Scoping" Notion doc § 1 still shows
  pre-SMA-381 `moon.yml` examples (`type:` instead of `layer:`, no `-rs`/`-py`/`-ts` suffixes, no
  field-order convention). Out of scope here — Notion edits aren't on a feature branch — but worth
  reconciling once a maintainer is in Notion next. Flagged across SMA-356/357/380/381 reviews.

## Verification

1. `CONTRIBUTING.md` renders cleanly (preview locally; check the new bullet's nested
   indentation and the new subsection's numbering).
2. The SPDX exemption examples match the file types actually present in the repo
   (`moon.yml`, `*.toml`, `*.yaml`, `*.json`, lockfiles, dotfiles, markdown docs). Spot-check
   with `git ls-files` that no listed type currently carries an SPDX header.
3. The documented field order matches every existing hand-written `moon.yml` (8 files plus the
   three template files post-Change-C). Spot-check by reading each file's first ~6 lines.
4. **Template generation check.** Render the `python` and `typescript` templates with a throwaway
   name (e.g. `moon generate python --name=throwaway --archetype=library --dry-run` or by
   inspecting the rendered output), confirm the generated `moon.yml` parses and carries
   `id: 'throwaway-py'` / `id: 'throwaway-ts'`. Discard the throwaway. Mirrors SMA-381's template
   verification.
5. `moon ci :build :test` stays green (no Moon project config changed, but run as a sanity check).

## Acceptance criteria

- [ ] `CONTRIBUTING.md` "Code conventions" SPDX guidance is split into the rule plus three
      exemption rules (hand-written config, generated files, markdown docs), with examples kept
      non-exhaustive.
- [ ] `CONTRIBUTING.md` has a `### Moon project files` subsection inside "Code conventions"
      documenting the 9-position field order, the `layer:` vs `type:` note, and the three
      `layer:` values in active use (`library` / `application` / `configuration`).
- [ ] `.moon/templates/python/moon.yml` emits `id: '{{ name }}-py'` between `$schema` and `layer`.
- [ ] `.moon/templates/typescript/moon.yml` emits `id: '{{ name }}-ts'` between `$schema` and
      `layer`.
- [ ] All three templates render to a `moon.yml` that parses (no `type:` regression; `id:` matches
      the SMA-380 suffix convention).
- [ ] Two commits on the feature branch: `docs(repo): …` for the CONTRIBUTING.md edits and
      `fix(repo): …` for the template changes; both reference SMA-383.
