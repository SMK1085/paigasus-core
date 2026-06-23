# SMA-378 — PyPI publishing metadata for `paigasus-proto` & `paigasus-kernel`

**Date:** 2026-06-23
**Linear:** SMA-378 (blocks SMA-407; relates to SMA-358, SMA-357)
**Status:** Approved (design)

## Problem

The Python packages `paigasus-proto` and `paigasus-kernel` were bootstrapped as
stubs in SMA-358 with only `name`, `version`, `requires-python`, and
`dependencies`. They are the two PyPI-bound packages in the `py/` workspace — per
ADR-0006 the open-core boundary is a *published-artifact dependency edge*, so
`paigasus-cloud` will consume these from PyPI. Before their first publish they
need complete, valid project metadata. Both `pyproject.toml`s carry a
`# TODO(SMA-378)` marking exactly this gap.

## Outcome

Both packages carry full PyPI-publishable metadata, the `# TODO(SMA-378)` markers
are removed, and a metadata-only `uv build` of each package succeeds and produces
a wheel whose `METADATA` renders the new fields without conflict.

## Scope

**In scope** — the two packages with the TODO marker:

- `py/packages/paigasus-proto/pyproject.toml`
- `py/packages/paigasus-kernel/pyproject.toml`
- A new `README.md` in each of those two package directories.
- A new `LICENSE` in each of those two package directories (a real copy of the
  root `LICENSE`, not a symlink) so the published wheel embeds the Apache-2.0
  text — see *License completeness* below.
- `.moon/tasks/python-project.yml` — add `README.md` and `LICENSE` to the shared
  `build` task `inputs` so the wheel-build cache tracks them (today it tracks only
  `src/**` + `pyproject.toml`). Harmless for packages lacking those files (the
  glob entries simply match nothing).

**Out of scope:**

- `paigasus-ml` and `paigasus-workflows` — not PyPI-bound, no TODO marker.
- The version floor. Versions stay `0.0.0`; the `0.0.0 → 0.1.0` bump and release
  wiring is SMA-407, which this issue blocks.
- `[project.urls]` and `keywords` — deferred to the release-activation work
  (decided: keep this change to exactly the AC's five fields).

## Design

### Metadata fields (added to each `[project]` table)

| Field | `paigasus-proto` | `paigasus-kernel` |
|---|---|---|
| `description` | `Generated protobuf message types and gRPC stubs for Paigasus.` | `Python bindings for the Paigasus behavioral kernel.` |
| `readme` | `"README.md"` | `"README.md"` |
| `license` | `"Apache-2.0"` | `"Apache-2.0"` |
| `authors` | `[{ name = "Paigasus contributors" }]` | `[{ name = "Paigasus contributors" }]` |
| `classifiers` | shared list (below) | shared list (below) |

Shared `classifiers` — **no Development Status, no License classifier, no
per-minor Python version**:

```toml
classifiers = [
  "Programming Language :: Python :: 3",
  "Programming Language :: Python :: 3 :: Only",
  "Intended Audience :: Developers",
  "Operating System :: OS Independent",
  "Topic :: Software Development :: Libraries",
  "Typing :: Typed",
]
```

The specific `Programming Language :: Python :: 3.12` classifier is **dropped**:
`requires-python = ">=3.12"` is open-ended (3.13+ allowed), so advertising only
3.12 contradicts it and would read as stale. `requires-python` is the
authoritative version gate; the classifiers stay at the major-version level.

The `# TODO(SMA-378): ...` comment block is removed from both files.

### Key decisions & rationale

1. **`license` as a PEP 639 SPDX expression string** (`license = "Apache-2.0"`),
   matching the AC's literal text and the Rust workspace's `license = "Apache-2.0"`.
   **Consequence:** the `License :: OSI Approved :: Apache Software License`
   classifier is intentionally omitted — modern PyPI/twine reject a distribution
   that declares both an SPDX `license` expression and a `License ::` classifier.

2. **`authors = [{ name = "Paigasus contributors" }]`** mirrors the Rust workspace
   convention (`authors = ["Paigasus contributors"]` in `rs/Cargo.toml`). No email,
   matching the Rust side.

3. **`readme` requires a file packaged inside the distribution.** No per-package
   README exists today, and a path outside the package root (e.g.
   `../../README.md`) is not included in the sdist/wheel by `uv_build`. Therefore a
   minimal real `README.md` is created in each package directory. It renders as the
   PyPI project description. Content: an H1 title, a one-line purpose, and a short
   license line. Plain Markdown with **no SPDX comment** — consistent with the
   existing `py/README.md` and root `README.md`, which carry none (the SPDX-header
   convention applies to *source* files, not docs).

4. **`Typing :: Typed`** is accurate — both packages ship a `py.typed` marker.

5. **Development Status classifier omitted** (decided) — the packages are `0.0.0`
   pre-release stubs; a maturity classifier is deferred to the 0.1.0 release work.

6. **`Operating System :: OS Independent` is kept for both**, including
   `paigasus-kernel`. The kernel wheel is itself pure Python (a typed re-export);
   it is built as `py3-none-any`. The platform-specific code lives in its runtime
   dependency `paigasus-py-bindings`, which ships its own platform wheels — so
   OS-independence is an accurate claim about *this* distribution.

### License completeness (challenger-surfaced)

Declaring `license = "Apache-2.0"` without an embedded license file would produce
a wheel with `License-Expression: Apache-2.0` but **no license text** —
`uv_build`'s default `license-files` glob is package-root-relative and the repo's
only `LICENSE` lives at the repo root, outside each package. A permissive-licensed
artifact published to PyPI should carry its license text. Fix: place a real copy
of the root `LICENSE` in each package directory (`py/packages/paigasus-proto/LICENSE`
and `py/packages/paigasus-kernel/LICENSE`). `uv_build` then embeds it as
`License-File: LICENSE`. A real copy (not a symlink) is used — symlinks interact
badly with sdist tarballs and cross-platform file collection. The text is the
standard Apache-2.0 license and effectively never changes, so the duplication
carries no meaningful sync burden.

### README content (per package)

`paigasus-proto/README.md`:

```markdown
# paigasus-proto

Generated protobuf message types and gRPC stubs for Paigasus, compiled from the
`contracts/` protobuf source of truth (betterproto2).

Licensed under the Apache License, Version 2.0.
```

`paigasus-kernel/README.md`:

```markdown
# paigasus-kernel

Python bindings for the Paigasus behavioral kernel — a thin, typed re-export over
the PyO3 binding (`paigasus-py-bindings`).

Licensed under the Apache License, Version 2.0.
```

## Verification

Use the **same invocation Moon's `build` task uses** — bare `uv build` from the
package directory (`.moon/tasks/python-project.yml` defines `command: 'uv build'`;
the workspace root has no `[project]` table, so a root build is invalid). Run for
each package:

```bash
cd py/packages/paigasus-proto  && uv build   # then paigasus-kernel
```

Or equivalently `moon run paigasus-proto-py:build` / `paigasus-kernel-py:build`.

1. **Both builds succeed and emit an sdist + wheel.** In particular,
   `paigasus-kernel` builds **without a Rust toolchain**: `paigasus-py-bindings` is
   a runtime `Requires-Dist`, not a build dependency, so `uv build` neither
   resolves nor builds it — only the `uv_build` backend is fetched. (If a build
   ever demands cargo, that is a regression to investigate, not expected.)
2. **Inspect each wheel's `METADATA`** (`unzip -p dist/*.whl '*/METADATA'`) and
   confirm all of: `Metadata-Version: 2.4` (forced by the SPDX expression),
   `Summary`, `Description-Content-Type: text/markdown` (inferred from the `.md`
   extension), `License-Expression: Apache-2.0`, `License-File: LICENSE`, `Author`,
   every `Classifier:` line, and the `Description` (README body) — with **no**
   License-File/classifier conflict and no build error.
3. **Inspect each wheel's contents** (`unzip -l dist/*.whl`) and confirm `py.typed`
   is present (so `Typing :: Typed` is honest), `LICENSE` is present, and — for
   `paigasus-proto` — the `generated/` tree shipped.
4. `moon run py:lint`, `py:format`, `py:typecheck`, `py:test` stay green (TOML +
   README/LICENSE additions should not perturb them; confirm rather than assume).
5. The Prettier whole-tree gate (`ts:fmt`) is **not** affected: it runs
   `prettier --check .` from `ts/`, so `py/**` files are outside its scope
   (verified). No `ts` task needs running for this change.

## Forward notes for SMA-407 (publish activation — out of scope here)

- The SPDX `license` expression forces **`Metadata-Version: 2.4`**. The upload
  tooling SMA-407 wires must accept it: `twine >= 5.1` or `uv publish`. Older
  twine rejects 2.4. Flagging now because this spec is what introduces the 2.4
  requirement.
- No `[project.urls]` and no `authors` email are added (per the scope decision).
  This means the very first published artifact has no contact/source link;
  SMA-407 should add at least a `Homepage`/`Source` URL when it activates publish.

## Considered and not done

- **`[project.urls]` + `keywords`** — decided out (keep to the AC's five fields);
  deferred to SMA-407.
- **A maturity `Development Status` classifier** — decided out while at `0.0.0`.
- **`authors` email** — omitted to mirror the Rust `authors = ["Paigasus
  contributors"]` (name only); contact link deferred to SMA-407 with URLs.

## Definition of done

- Both `pyproject.toml`s carry `description`, `readme`, `license`, `authors`,
  `classifiers`; the `# TODO(SMA-378)` block is gone from both.
- A `README.md` and a `LICENSE` (copy of root) exist in each of the two package
  directories.
- `.moon/tasks/python-project.yml` `build.inputs` includes `README.md` + `LICENSE`.
- Both packages build cleanly via the Moon build invocation; their `METADATA`
  renders the new fields (incl. `License-File: LICENSE`,
  `Description-Content-Type: text/markdown`) and the wheels embed `py.typed` +
  `LICENSE`.
- The `py:*` Moon gates pass.
