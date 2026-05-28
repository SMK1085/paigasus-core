# SMA-383 design review — staff-eng pass

**Reviewer:** Claude, 2026-05-28
**Spec under review:** `paigasus-core/docs/superpowers/specs/2026-05-28-sma-383-contributing-moon-conventions-design.md`
**Cross-checked against:** Linear SMA-383; current `CONTRIBUTING.md`; all 8 hand-written `moon.yml` files (3 rs, 5 py); all 3 templates (`rust`/`python`/`typescript`); related SMA-380 / SMA-381 specs and reviews.

---

## TL;DR

Trivial doc-only PR: two edits to one file. The convention being documented is real (already followed by every hand-written `moon.yml` in the repo) and the formulation is sound. SMA-381's `layer:` field-name correction surfaces here for the first time as a public-facing rule.

No blockers. The biggest critique is consistency: **the spec documents a convention while explicitly leaving two templates that violate it unfixed**. If you're codifying "every project gets a stack-suffixed `id:`," the python and typescript templates omitting `id:` is precisely the thing the convention is meant to prevent — and it's a one-line fix per template. Documenting without fixing creates a brand-new spec/code divergence on day one of the convention's life.

Three significant items, three smaller. None require redesigning.

---

## Significant concerns

### S1. The python and typescript templates violate the convention this PR documents — and the fix is one line each

On disk today:

```yaml
# .moon/templates/rust/moon.yml  (correct)
$schema: 'https://moonrepo.dev/schemas/project.json'
id: '{{ name }}-rs'                     # ← present
layer: '{% if archetype == "service" %}application{% else %}library{% endif %}'
language: 'rust'

# .moon/templates/python/moon.yml  (gap)
$schema: 'https://moonrepo.dev/schemas/project.json'
layer: '{% if archetype == "service" %}application{% else %}library{% endif %}'
language: 'python'
# ← no id: line

# .moon/templates/typescript/moon.yml  (gap)
$schema: 'https://moonrepo.dev/schemas/project.json'
layer: '{% if archetype == "app" %}application{% else %}library{% endif %}'
language: 'typescript'
# ← no id: line
```

After SMA-383 merges, CONTRIBUTING.md will say "field order is `$schema` → `id` (when present) → `layer` → `language` → ..." while two of three templates emit no `id:` at all — meaning any contributor running `moon generate python` or `moon generate typescript` produces a project that violates the SMA-380 `-py`/`-ts` suffix convention from the moment it's generated.

The spec acknowledges this:

> "Python/TypeScript template `id:` gap. Both templates omit an explicit `id:` line, which means `moon generate` produces projects without the SMA-380 stack suffix unless the contributor adds it manually. Real bug, but it only bites when those templates are actually used, which happens in SMA-359 (ts) and SMA-360 (contracts). Surface as a note in the PR description; don't fix here."

The "only bites when used" argument is weaker than it looks. Three counter-points:

- **Cost of the fix is one line per template.** Adding `id: '{{ name }}-py'` and `id: '{{ name }}-ts'` is mechanical and matches what the rust template already does. The diff is smaller than the spec describing why it's out of scope.
- **The SMA-383 PR is literally about Moon convention consistency.** Shipping the docs while leaving two templates inconsistent invites a "wait, didn't this PR fix that?" review comment for the rest of the templates' life.
- **"Surface as a note in the PR description"** is a TODO buried in a PR description for an issue that closes — it's the lowest-visibility tracking option. If SMA-359 lands six months from now without the maintainer remembering, the first generated ts project ships with a bare id and the convention is silently broken.

**Recommendation, pick one:**
- **(a) Preferred:** Expand scope to include the two template fixes. Adds two lines to two files. PR remains doc + template, scope creeps by ~10 lines, but the PR is self-consistent.
- **(b)** Keep doc-only and file the template fix as its own tracked Linear issue *before* opening the SMA-383 PR, so the follow-up exists as a Linear ticket and not just a PR-description footnote.

The argument the spec implicitly makes for keeping it doc-only ("doesn't bite until SMA-359/SMA-360 use them") is the same as saying "the convention can be silently violated until then." If the convention is worth documenting, it's worth enforcing in the templates that exist today.

### S2. The convention says where `layer:` goes but not what values are valid

The field-order list mentions `layer:` at position 3 and the reminder "Use `layer:`, not the pre-2.x `type:`." Good. But it never says what values `layer:` accepts. From on-disk usage today:

| File | `layer:` value |
|---|---|
| `rs/crates/libs/paigasus-kernel/moon.yml` | `library` |
| `rs/crates/bindings/paigasus-py-bindings/moon.yml` | `library` |
| `rs/crates/services/paigasus-gateway/moon.yml` | `application` |
| `py/packages/*/moon.yml` (×4) | `library` |
| `py/moon.yml` | `configuration` |

Three distinct values are in active use: `library`, `application`, `configuration`. Moon likely supports more (the documentation typically lists `automation` and `scaffolding` as well). The reader of CONTRIBUTING.md learns:

- That a `layer:` field exists.
- Where it sits in the file.
- What it *isn't* called (not `type:`).

But not what to put in it. The first contributor adding a new project will either guess, read the existing files, or check Moon docs — none of which is hostile, but a one-line enumeration in CONTRIBUTING.md would close the gap:

> "Common values: `library` (importable code), `application` (runnable binary or app), `configuration` (workspace-root project that aggregates child projects). Full list in [Moon's project config docs](https://moonrepo.dev/docs/config/project)."

The `configuration` value especially deserves a name — it's only used in one file (`py/moon.yml`) and isn't obvious. The SMA-381 review I wrote previously didn't even mention it as a third option because the spec didn't surface it; SMA-383 is the right place to make all three visible.

### S3. The SPDX enumeration trades one re-litigation surface for another

The spec's own rationale for the enumerated list over a general clause:

> "The enumerated list was chosen over a general 'config files are exempt' sentence because the general form invites edge-case re-litigation (Dockerfiles? Makefiles? shell scripts?), which is the specific failure mode this issue is trying to prevent."

That's the right concern but the enumeration doesn't actually solve it — it just narrows the surface. The first time someone adds a `Dockerfile`, `Makefile`, `*.sh`, `*.bats`, or `*.dhall` to the repo, the question recurs: "is this in the exempt list? It's not listed. Is it config or source?" The new bullet's list has:

- `moon.yml`, `*.toml`, `*.yaml`/`*.yml`, `*.json`
- Lockfiles (`Cargo.lock`, `uv.lock`, `pnpm-lock.yaml`)
- Dotfiles (`.gitignore`, `.editorconfig`)

What's missing (and will plausibly land):

- **Dockerfile / .dockerignore** — service deployment will likely add these.
- **Makefile / justfile** — already-precedented for paigasus (the helikon repo uses `just`).
- **Shell scripts** (`*.sh`, `*.bash`) — `scripts/` directories tend to exist.
- **GitHub Actions workflows** (`.github/workflows/*.yml`) — covered by `*.yml` but worth naming since they're a different mental category.
- **PR / issue templates** (`.github/*.md`) — markdown, not source code, no SPDX expected.
- **`README.md`, `CONTRIBUTING.md`, `LICENSE`** — obviously no SPDX, not mentioned.

The cleanest fix is to **state the rule plus give examples**, rather than treating the examples as exhaustive:

> "Config files, lockfiles, dotfiles, and markdown docs carry no SPDX header. Examples in this repo today: `moon.yml`, `*.toml`, `*.yaml`/`*.yml`, `*.json`, lockfiles (`Cargo.lock`, `uv.lock`, `pnpm-lock.yaml`), dotfiles (`.gitignore`, `.editorconfig`), and markdown (`README.md`, `CONTRIBUTING.md`, etc.). If unsure for a new file type, ask in the PR — it's almost always config."

This keeps the enumeration as concrete examples but doesn't pretend they're a closed set.

There's also a meaningful distinction the spec collapses: **lockfiles and codegen output are exempt because they're generated**, not because they're config. The reason matters when someone adds (say) `paigasus-proto`'s generated Rust files post-SMA-360 — those *are* source code, but they're owned by the generator, which sets the header. Worth separating in the bullet:

```markdown
- Hand-written config carries no SPDX: moon.yml, *.toml, *.yaml, *.json, dotfiles.
- Generated files (lockfiles, codegen output) carry whatever header the generator emits.
  Don't hand-edit a generated file's header.
- Markdown docs and the LICENSE itself carry no SPDX.
```

Three rules instead of one bullet, but each has a clean reason behind it.

---

## Smaller concerns / nice-to-fix (N1–N3)

### N1. `(repo)` is not an established commit scope in the existing CONTRIBUTING.md examples

The acceptance criterion says "Single commit with `docs(repo):` scope." But CONTRIBUTING.md's commit-message section gives examples using `(rs)`, `(contracts)`, `(py)` — workspace-level scopes. `(repo)` isn't shown anywhere. It's a sensible scope name for "applies to the whole repo" but the spec is establishing a new scope value without saying so.

Either:
- Use a scope already shown in CONTRIBUTING (`(docs)`? That doesn't fit because the commit *is* `docs:` already, so `(docs)` is tautological. Maybe `(workspace)` or `(monorepo)`?)
- Establish `(repo)` as the canonical scope for repo-wide docs/config changes by updating CONTRIBUTING.md's commit-message examples in *this PR* to include one — closes the convention loop in the same edit.

Trivial fix; just worth being intentional. Otherwise the next "applies to the whole repo" commit will pick a different scope and there'll be drift between `(repo)`, `(workspace)`, `(monorepo)`, `(docs)`, etc.

### N2. The field-order list mentions `options` but doesn't position it cleanly

Position 7 reads "`tasks`, `options`, and any remaining fields." That bundles project-level `options:` (Moon supports it for things like cache/affected-files behavior) with "remaining fields" without saying which order they go in. Today none of the hand-written files use project-level `options:`, so it's hypothetical — but the moment one needs to override task caching at the project level, "where does `options:` go?" recurs.

Cheap fix: explicit position-7 = `tasks`, position-8 = `options`, position-9 = "any remaining (`metadata:`, etc.)." Or leave 7 generic but say "alphabetical for any fields beyond the documented six."

### N3. The internal Notion references don't propagate the new convention

The Polyglot Monorepo Scoping doc § 1 still shows the original `workspace.yml` and per-project `moon.yml` examples — without the field-order convention, without `layer:` (still says `type:`), without `-rs`/`-py`/`-ts` suffixes. These reviews have flagged that scoping-doc drift twice already (SMA-356, SMA-357). SMA-383 documenting the convention in CONTRIBUTING.md will hold for *external* contributors and people reading the repo first. For maintainers/contributors who go to Notion first (per CONTRIBUTING's "Internal references" section), the canonical-looking source still shows the old form.

Not this PR's job to fix the Notion docs, but the spec should at least name the divergence:

> "Out of scope: updating Polyglot Monorepo Scoping § 1 in Notion to match the documented conventions. Tracked as SMA-???."

Otherwise the doc drift compounds with every PR like this one.

---

## What the spec gets right

- **Timing.** Documenting the convention *after* every hand-written `moon.yml` already follows it (all 8 files) is the right order. Documenting an aspirational rule with code that doesn't comply is the worse failure mode this avoids.
- **Carrying forward the `layer:` rename.** SMA-381's update to use Moon 2.2.5's `layer:` (rejecting `type:`) propagates here as a "Use `layer:`, not the pre-2.x `type:`" note in CONTRIBUTING. Future readers won't dig through specs/plans to learn the field name changed.
- **The "no ADR" call.** Per CLAUDE.md, ADRs are for significant choices; this codifies existing practice. Correct.
- **Honest scope.** The spec names the python/ts template `id:` gap explicitly rather than letting it lurk. The disagreement is about where the fix should land (see S1), not whether the gap exists.
- **Self-aware enumeration choice.** The spec's argument for enumerating exempt file types vs a general "config files are exempt" sentence is sound (you avoid the Dockerfile/Makefile edge-case loop). The execution falls short of the rationale (S3), but the reasoning is right.
- **Verification step 3 is concrete and tractable.** "Spot-check the field order in 8 hand-written files + 3 templates" is small enough to actually do during review.

---

## Suggested action list, prioritized

1. **[This PR, preferred]** Resolve S1 by expanding scope to add `id:` to the python and typescript templates. Two lines, two files. Makes this PR self-consistent.
2. **[This PR, light edit]** Enumerate valid `layer:` values in the new subsection (S2). Even a one-liner naming `library` / `application` / `configuration` closes the gap.
3. **[This PR, doc edit]** Rework the SPDX bullet to "rule plus examples" instead of "exhaustive enumeration" (S3), and split the lockfile/codegen-generated rationale from the hand-written-config one.
4. **[This PR, single line]** Decide and use a deliberate commit scope (N1) — either commit to `(repo)` and add it to CONTRIBUTING's examples, or pick from the existing scope vocabulary.
5. **[This PR, single line]** Position `options:` explicitly in the field order (N2).
6. **[Open follow-up]** Notion scoping-doc drift (N3). Cumulative with SMA-356/357/380/381's reviews; deserves its own tracked issue.

This is a small PR and should land small. The S1 expansion is the only one that meaningfully grows the diff, and it grows it by ~2 lines. Everything else is wordsmithing the CONTRIBUTING.md edit while it's already open.
