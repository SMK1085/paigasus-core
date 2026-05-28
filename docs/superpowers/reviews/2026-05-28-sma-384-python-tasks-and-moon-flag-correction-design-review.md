# SMA-384 design review — staff-eng pass

**Reviewer:** Claude, 2026-05-28
**Spec under review:** `paigasus-core/docs/superpowers/specs/2026-05-28-sma-384-python-tasks-and-moon-flag-correction-design.md`
**Cross-checked against:** Linear SMA-384; current on-disk state of `CLAUDE.md`, `CONTRIBUTING.md`, `.moon/tasks.yml`, `.moon/tasks/rust.yml`, `py/moon.yml`, `.moon/templates/python/moon.yml`; SMA-383 and SMA-381 specs.

---

## TL;DR

Three substantive changes wrapped in a tight, well-staged spec: wire `.moon/tasks/python.yml`, slim the python template to match the rust template's shape, and correct the wrong `--output-style stream` claim in two docs. The stacking on SMA-383 is the right PR discipline. Commit grouping (`docs(repo)` / `feat(py)` / `chore(py)`) cleanly separates documentation, behavior change, and template change.

One acknowledgment up front: the wrong `--output-style stream` claim originated in *my* SMA-356 review (suggested CONTRIBUTING add a streaming note as an ergonomic escape hatch — I assumed the CLI flag existed without verifying). It then propagated through the SMA-356 plan, into CONTRIBUTING.md and CLAUDE.md, and got re-cited in subsequent plans. The spec catches and fixes it correctly. Worth noting for posterity that the unverified suggestion was load-bearing — exactly the failure mode the "verify against Moon docs" guidance in some of my earlier reviews was meant to catch in the other direction.

No blockers. Three significant items (the `py/moon.yml` deferral is the most consequential), three smaller items. The PR can land essentially as-described.

---

## Significant concerns

### S1. Deferring `py/moon.yml`'s redundant task block is the same shape of fix as Change D — and not doing it leaves the workspace in a knowingly-redundant state

The spec frames the deferral:

> "After python.yml lands, `py/moon.yml`'s own `tasks:` block becomes structurally redundant — it defines the same task names and commands as python.yml. It still functions because its project-local `fileGroups` override re-scopes `sources` to `packages/*/src/**/*`. Removing the redundant tasks block would mean confirming that the inherited tasks correctly resolve against the project-level `fileGroups` override. Worth doing as a cleanup but scope-creeps this PR; defer to a follow-up."

That argument doesn't quite hold up against the spec's own Change D. Change D removes the python template's redundant tasks block on exactly the same reasoning — those tasks are now inherited from python.yml, so the template's copies are duplicative. The risk profile of removing `py/moon.yml`'s tasks block is essentially identical:

- **Both** redundancies come from python.yml landing.
- **Both** require confirming that inherited tasks correctly resolve against the project's own `fileGroups`.
- **The template** removal is verified by `moon generate python --archetype library` (the spec's verification step 2).
- **The py/moon.yml** removal would be verified by `moon project py` showing the inherited tasks resolving correctly against py's `fileGroups` override.

Moon's task-inheritance semantics already do the right thing here: `@group(sources)` and `@group(tests)` in inherited tasks resolve against the consuming project's file groups, not the language-task-file's groups. So py's `fileGroups` override (`packages/*/src/**/*`) takes effect even after the local tasks block is removed. The "confirming that the inherited tasks correctly resolve" work is the same work, run for an additional project.

The cost of deferring:
- The workspace lands in a deliberately-redundant state ("functions, but the tasks block is dead-by-design").
- Anyone editing py/moon.yml's tasks (to add an input, change a command) will silently no-op the inherited file's intent, because the local definition wins.
- The follow-up issue has to be filed and tracked separately; if it doesn't get prioritized, the redundancy persists indefinitely.

**Recommendation:** Roll the py/moon.yml task-block removal into this PR as a fourth bullet under Change D (or its own Change E). Verification adds one step: `moon project py` shows the inherited tasks resolving against py's `fileGroups`. Total diff growth: ~15 lines removed from py/moon.yml, ~5 lines added to verification. The "scope-creep" argument doesn't really apply when the marginal work is the same shape and the same magnitude as work already in the PR.

Counter-argument: keeping the py/moon.yml tasks block as-is means the PR's behavior change is genuinely zero for existing usage (`moon run py:lint` etc. continue running the same commands from the same definitions). Removing the tasks block means existing invocations start resolving through inheritance — same commands, but a different resolution path that should be verified. If you want to keep the blast radius minimal, the deferral is defensible — but then the spec should say *that* (the reason is "keep the diff behavior-stable for existing usage," not just "scope creep").

### S2. Cross-language task-name asymmetry: rust's `fmt` vs python's `format`

The proposed `.moon/tasks/python.yml` defines `format` (matching the current py/moon.yml). The existing `.moon/tasks/rust.yml` defines `fmt`. After SMA-384, the workspace has:

| Task name | Rust crates | Python projects |
|---|---|---|
| `fmt` | ✓ | ✗ |
| `format` | ✗ | ✓ |

Practical consequences:
- `moon ci :fmt` runs across rust crates only; python is silently skipped.
- `moon ci :format` runs across python projects only; rust is silently skipped.
- "Format everything" requires two invocations, or a future Moon `:fmt|format` glob (Moon doesn't support task-name alternation).

This is a real cross-stack consistency hole that the SMA-380 / SMA-381 / SMA-383 thread has been working to close in other dimensions (project id suffixes, `layer:` field, scaffold templates). Task names are part of the same convention surface and deserve the same treatment.

Three resolutions:

- **(a)** Rename python.yml's `format` → `fmt`. Two-character change. Matches the rust convention. Cost: `py/moon.yml`'s `format` task (and `py/README.md`'s task table from the SMA-358 review thread) need the same rename. Anyone with `moon run py:format` muscle memory has to relearn.
- **(b)** Rename rust.yml's `fmt` → `format`. Same shape of change, opposite direction. Matches what most other ecosystems call the task. Cost: rust contributors with `cargo fmt` muscle memory will type `fmt` and get confused.
- **(c)** Define both names as aliased tasks (one calls the other, or both are identical). Wastes a slot in `moon project --list` for each language; loses the "task name = canonical action" property.

(a) is the cheapest; (b) is the most ecosystem-conventional; (c) is a hack. The spec doesn't have to resolve this here — but it should at least name the asymmetry as a known issue with a deferral.

**Recommendation:** Add one line to "Out of scope / follow-up":

> "Cross-language task name `fmt` (rust) vs `format` (python). After this PR, `moon ci :fmt` runs rust-only and `moon ci :format` runs python-only. Resolve by harmonizing both to one name in a follow-up issue."

Or fold the rename into this PR if you have a position on which name wins. The cost is small either way; the asymmetry compounds with every new task that gets a different conventional name across languages.

### S3. The Notion update is in the acceptance criteria but has no in-repo verifiable artifact

The spec puts the Notion update as Change E and acceptance criterion #8. The verification step says: *"Notion 'Polyglot Monorepo Scoping' § 1 examples use `layer:`/`-py`/`-ts` and reference CONTRIBUTING.md for the canonical field order."* That's the right outcome, but the verification has no checkable evidence — it's "trust that the maintainer ticked the box."

This isn't a new failure mode (every prior review has flagged the Notion drift as a recurring problem), but SMA-384 is the first spec that *actively claims* it'll do the Notion update. If the update doesn't happen, or happens partially, there's no signal in the merged PR.

Two ways to make the AC tractable:

- **(a)** Include the Notion-side diff as a screenshot or markdown excerpt in the PR description. Imperfect (no enforcement) but creates a paper trail.
- **(b)** Reverse the source of truth: make CONTRIBUTING.md the canonical reference, and replace the relevant Notion § 1 content with a one-line redirect ("See `CONTRIBUTING.md` in paigasus-core for current conventions"). This is the durable fix — drift can't recur if Notion doesn't try to maintain a competing copy.

(b) is the bigger lift but is the only thing that ends the recurring "Notion drift" review note. (a) is acceptable as an interim. The spec's current "I'll do it before opening the PR" is the least durable option.

**Recommendation:** Either (a) for this PR plus (b) as a tracked follow-up, or just (b) now. Either way, name the strategy explicitly so the next reviewer doesn't see "AC met: yes" with no evidence.

---

## Smaller concerns / nice-to-fix (N1–N4)

### N1. Verification step 6's grep will match historical plan/spec files

```bash
grep -rn '\-\-output-style' --include='*.md' .
```

Current state (from the on-disk check) shows the grep matches:
- `docs/superpowers/plans/2026-05-27-sma-381-rust-scaffold-type.md:325`
- `docs/superpowers/plans/2026-05-28-sma-383-contributing-moon-conventions.md:432`
- `docs/superpowers/plans/2026-05-26-sma-358-py-uv-workspace.md:431`
- `docs/superpowers/plans/2026-05-26-moon-configuration.md:617, 618, 628`
- This very spec and its predecessors

The spec correctly notes historical plan files are intentionally not retro-edited, but the grep doesn't filter them out — running it as-stated will return matches and look like the change wasn't applied. Refine:

```bash
grep -rn '\-\-output-style' --include='*.md' . | grep -vE '^\./docs/superpowers/'
```

(Or exclude via `--exclude-dir=superpowers`, depending on which directory layer you want to filter.)

### N2. `~/.proto/shims/moon` in verification steps assumes a contributor's install path

Verification steps 1, 2, 3, 4 invoke `~/.proto/shims/moon project ...` and `~/.proto/shims/moon generate ...`. That path is correct for the spec author's environment but unnecessary for any contributor whose proto-installed `moon` is on PATH (which is what `proto install` is supposed to set up via shell init). A fresh contributor following CONTRIBUTING.md gets `moon` on PATH and the `~/.proto/shims/` qualifier is just noise.

Trivial: use `moon` in the documented verification steps; assume PATH is set up. If the contributor's PATH isn't set up, that's a CONTRIBUTING.md issue and the explicit shim path papers over it rather than fixing it.

### N3. The "no `start` task in python.yml" decision is correct but worth a one-line explanation

Decision: `start` lives in the template's service archetype, not in python.yml. The spec notes "because the command needs the project's `name` to construct the module path" — correct. Moon's task-file syntax doesn't have access to per-project Tera variables; `$project` resolves to the Moon project ID (which is `<name>-py` post-SMA-380), not the underscored module name.

This is the right factoring (templates are for per-project variability, language tasks are for shared defaults). But the implication isn't named: **python services not generated from the template will have no `start` task.** No such services exist today, so it's theoretical, but worth a one-line note in the python.yml comment header or the spec:

> "`start` is intentionally absent from this file; service-archetype projects get it from the python template. Hand-written service projects (none today) must add `start` to their own `moon.yml`."

### N4. The `inputs: '/py/uv.lock'` workspace-anchor path encodes a path assumption

python.yml's task inputs use `/py/uv.lock` (Moon's workspace-anchor syntax). That's correct today — the py workspace lives at `py/` and `uv.lock` is workspace-shared. But it bakes the path into the language-task file. Two scenarios where it bites:

- If a second uv workspace ever lands (unlikely but possible — e.g., a separate `experiments/py/` workspace with its own lockfile), python.yml's task inputs are wrong for it.
- If the python tooling layout ever changes (e.g., moves to `python/` or splits into multiple workspaces), every reference here needs updating.

Not a current bug; just a reminder that the path is load-bearing. If you want to be defensive, a comment in python.yml:

```yaml
# /py/uv.lock — workspace-anchor path. Assumes single uv workspace at py/.
# Update if the python workspace layout changes.
```

Cheap insurance.

---

## What the spec gets right

- **Stacking on SMA-383.** PR stacks are the right discipline for sequenced dependent changes. The stacking is named explicitly with the retarget plan.
- **Three-commit grouping.** `docs(repo)` (pure prose), `feat(py)` (behavior change for existing projects), `chore(py)` (template change for future projects) is a principled split. Each commit can be reviewed in isolation; bisect lands on the right scope if something breaks.
- **The `--output-style stream` correction is overdue and accurate.** Moon 2.2.5 does treat `outputStyle` as a per-task config option, not a CLI flag. Catching this here closes a doc bug that was propagating through every plan written off CONTRIBUTING.md.
- **Inline CLAUDE.md audit table.** Treating "fix CLAUDE.md, also check for other improvements" as a discrete audit step — and tabling the audit findings — is the right interpretation of an ambiguous instruction. Avoids both "fix nothing more than asked" and "scope-creep into a rewrite."
- **`feat(py)` over `chore(py)` for the python.yml addition.** It's a behavior change for existing python projects (they start inheriting tasks); `feat` is the right type. Not `chore` (which would minimize the visibility).
- **Test discovery patterns tailored to python.** Dropping `*.test.*` and `*.spec.*` (JS conventions) and adding `test_*.py` (Python pytest convention) is the right per-language override. The asymmetry with global `.moon/tasks.yml`'s `tests` group is intentional.
- **Recognizing the Notion drift as load-bearing.** Putting the Notion update in the AC makes it visible. The mechanism for verification (S3) is the weak link, but the recognition is right.

---

## Suggested action list, prioritized

1. **[This PR, decision]** Resolve S1 — either roll py/moon.yml's redundant tasks block into the same PR (small additional diff) or restate the deferral reason as "keep behavior-stable for existing usage" rather than "scope creep."
2. **[This PR, doc edit]** Add the `fmt` vs `format` asymmetry to "Out of scope / follow-up" (S2) — even a one-line acknowledgment beats silent inconsistency.
3. **[This PR, single-line edits]** Refine the grep filter (N1); drop the `~/.proto/shims/` qualifier (N2); add the "no `start` in python.yml" explanation (N3).
4. **[This PR or fast-follow]** Decide the Notion durability strategy (S3) — interim screenshot/excerpt evidence, or the "Notion redirects to CONTRIBUTING.md" durable fix.
5. **[Optional, this PR]** Add a comment in python.yml about the `/py/uv.lock` workspace-anchor path assumption (N4).

This PR is in good shape and ready to ship pending the actions above. The biggest substantive question is S1 (the py/moon.yml deferral) — that's the one item where the spec's reasoning is light enough to be worth re-litigating before merge.
