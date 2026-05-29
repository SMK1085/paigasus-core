# SMA-371 — Local git hooks: commit-msg (Conventional Commits) + branch-name validation

**Status:** Approved (design)
**Date:** 2026-05-29
**Linear:** [SMA-371](https://linear.app/smaschek/issue/SMA-371/local-git-hooks-commit-msg-conventional-commits-branch-name-validation)
**ADR:** [ADR-0010 — lefthook + commitlint over convco](https://www.notion.so/36c830e8fbaa8110bbaee37475ad57c8)

## Goal

Make Conventional Commits and branch naming a **hard local requirement** — fast
feedback at `git commit` and `git push` time, not first discovered at CI. This is
the developer-machine layer; SMA-361 is the server-side counterpart.

> **Residual coverage gap.** "Hard local requirement" holds only for contributors
> who run `pnpm install` (auto-install via `prepare`) or who run the documented
> `moon run repo:install-hooks`. Pure-Rust / pure-Python contributors who do
> neither still hit violations first at CI. This is an accepted consequence of the
> Moon-sync correction (decision #1) — not a bug, but a known hole. ADR-0010 and AC
> item A have been amended to stop claiming sync-time auto-install.

The toolchain is settled by ADR-0010: **lefthook** (hook manager) + **commitlint**
(commit-msg validator) with a shared `@paigasus/commitlint-config` package, a
`pre-push` hook for branch-name enforcement, and Moon's native `vcs.hooks` left
empty so lefthook is the sole owner of `.git/hooks/*`. This spec covers the
concrete *implementation shape* the ADR/AC left open.

## Scope boundary with SMA-361

SMA-371 ships the **artifacts + docs**: the `@paigasus/commitlint-config` package,
`lefthook.yml`, `commitlint.config.cjs`, the install wiring, and CONTRIBUTING
updates. **SMA-361** (CI workflow) consumes the exact pinned config in CI and adds
the `release-plz` / semantic-release dry-run parity smoke job. The CI-parity
*invariant* (same commitlint binary + same `@paigasus/commitlint-config` version,
both pinned via `pnpm-lock.yaml`) is documented here but enforced by SMA-361.

## Key decisions

Implementation decisions the ADR/AC did not fully pin down.

### 1. Install trigger (with a resilient `prepare`)

Moon cannot auto-run a project task during `moon sync` — its only native "act on
sync" path for hooks is `vcs.hooks`, which we deliberately disable. Install is
triggered two ways:

- a **`prepare` script** in `ts/package.json` runs `lefthook install` on
  `pnpm install` (covers everyone who touches `ts/`);
- **`moon run repo:install-hooks`** documented as the explicit one-time onboarding
  step (covers pure-Rust / pure-Python contributors).

The `prepare` script **must be resilient** so it never breaks `pnpm install` when
lefthook isn't on `$PATH` yet (e.g. `pnpm install` run before `proto install`):

```jsonc
// ts/package.json
"scripts": {
  "prepare": "command -v lefthook >/dev/null 2>&1 && lefthook install || echo \"[hooks] run 'proto install' then 'moon run repo:install-hooks' to enable git hooks\""
}
```

CONTRIBUTING documents the required order: **`proto install` → workspace installs**.

> **Correction to the AC.** The AC says install is "triggered automatically by Moon
> `sync`". With `vcs.hooks` empty (which the same AC mandates), that is not
> achievable — `moon sync` runs `SyncWorkspace` (codeowners, VCS hooks), not project
> tasks. Re-enabling `vcs.hooks` as the installer was considered and rejected: it
> makes Moon **and** lefthook both write `.git/hooks/*`, last-writer-wins — the exact
> failure mode ADR-0010 guards against.

### 2. `repo` Moon project home (+ `local` task)

A root-level `repo` project registered via Moon's combined `projects` form
(`globs: [...]` + `sources: { repo: '.' }`) with a root `moon.yml` (`id: repo`,
`layer: configuration`). The `install-hooks` task is marked **`local: true`** (and
its `inputs` scoped to `lefthook.yml` + `.lefthook/**`) so a future `moon ci` run
never executes `lefthook install` into a CI checkout's `.git/hooks`.

A `repo/` subdirectory project was the alternative (avoids any root-overlap
warnings). We keep the root-level project — it puts the task where `lefthook.yml`
lives and matches the AC's `repo:` naming; the `local: true` guard resolves the
concrete CI concern, and the overlap is a watch-item (open item below).

### 3. Tool resolution + the corrected commitlint invocation

- **lefthook** is pinned in `.prototools` (proto), on `$PATH` after `proto install`
  — like `moon` / `buf`. This sidesteps lefthook's monorepo failure mode: its npm
  shim looks for the binary in `<git-root>/node_modules/lefthook`, which does not
  exist here (node_modules is in `ts/`)
  ([lefthook#510](https://github.com/evilmartians/lefthook/issues/510),
  [#443](https://github.com/evilmartians/lefthook/issues/443)).
- **commitlint** is a pnpm dependency inside `ts/`. The invocation runs from the
  git-root cwd (lefthook's default) using an explicit binary + config path —
  **not** `pnpm -C ts exec`:

  ```yaml
  # lefthook.yml (commit-msg)
  run: ts/node_modules/.bin/commitlint --edit {1} --config ts/commitlint.config.cjs
  ```

  **Why this and not `pnpm -C ts exec`:** lefthook's `{1}` expands to
  `.git/COMMIT_EDITMSG` **relative to the repo root**, and `pnpm -C ts exec` changes
  the child cwd to `ts/` ([pnpm#5068](https://github.com/pnpm/pnpm/issues/5068)) —
  so `--edit {1}` would resolve against `ts/.git/COMMIT_EDITMSG` and fail on a path
  lookup. Keeping cwd at the git root makes `{1}` resolve correctly; the explicit
  bin path avoids needing `-C`; and commitlint resolves the `extends`
  (`@paigasus/commitlint-config`) relative to the config file's own location
  (`ts/`), so module resolution still finds `ts/node_modules`
  ([commitlint shareable-config](https://commitlint.js.org/concepts/shareable-config.html)).

  > **Must be executed before merge:** a real `git commit` with a good and a bad
  > message, from a terminal *and* a GUI client.

### 4. Branch-name validation: current branch, not stdin

`pre-push` validates the **current checked-out branch** via
`git symbolic-ref --short HEAD`, not the pushed refs from stdin.

**Why:** lefthook's forwarding of pre-push **stdin** to commands is historically
unreliable ([lefthook#147](https://github.com/evilmartians/lefthook/issues/147)), so
a stdin-parsing script may not receive its input. Validating HEAD removes the stdin
dependency and naturally sidesteps the awkward pre-push edge cases (branch deletes
with zero-SHAs, tag pushes, multi-ref `--all`) — you cannot be "on" a tag, and
`git push --delete` does not change HEAD.

**Tradeoff:** it does not catch exotic remaps like
`git push origin local:refs/heads/other`. The server-side GitHub ruleset (separate
concern) is the authoritative ref-level gate for those.

### 5. post-checkout warn hook: skipped

`pre-push` is the enforcement point (git ignores `post-checkout` exit codes). The
warn-only hook is a second script to maintain for marginal benefit; trivial to add
later.

## Architecture

```
git event ──> .git/hooks/{commit-msg,pre-push}   (shims written by `lefthook install`)
                     │
                     └─> lefthook (from $PATH, pinned via proto)
                           reads lefthook.yml (repo root); cwd = git root
                           │
              commit-msg ──┼─> ts/node_modules/.bin/commitlint --edit {1} \
                           │        --config ts/commitlint.config.cjs
                           │     └─ extends @paigasus/commitlint-config
                           │          └─ extends @commitlint/config-conventional
              pre-push   ──┘─> .lefthook/pre-push/check-branch.sh
                                  └─ git symbolic-ref --short HEAD vs regex
```

- **Single owner of `.git/hooks/*`:** lefthook. Moon `vcs.hooks` stays empty.
- **lefthook binary:** proto-pinned, `$PATH`. No git-root `node_modules` dependency.
- **commitlint:** explicit `ts/node_modules/.bin` path, cwd at git root.

## Files

### New

| File | Purpose |
|---|---|
| `moon.yml` (repo root) | `id: repo`, `layer: configuration`; task `install-hooks` → `lefthook install`, `options.local: true`, `inputs: [lefthook.yml, .lefthook/**]`. System command (no language toolchain). |
| `lefthook.yml` (repo root) | `commit-msg` (commitlint, explicit bin path) + `pre-push` (branch-name); global `skip: [merge, rebase]`; bot-email guard per command. |
| `.lefthook/pre-push/check-branch.sh` | Branch-name validator (current branch via `git symbolic-ref`). Carries SPDX header (hand-written script). |
| `ts/commitlint.config.cjs` | `module.exports = { extends: ['@paigasus/commitlint-config'] }` — no per-repo overrides (ADR-0010). |
| `ts/packages/commitlint-config/package.json` | `@paigasus/commitlint-config`; `private: true` (see follow-up below); `main: index.cjs`; deps `@commitlint/config-conventional`. |
| `ts/packages/commitlint-config/index.cjs` | The canonical ruleset (types, scopes, lengths, scope-empty). |
| `ts/packages/commitlint-config/moon.yml` | `id: commitlint-config-ts`, `layer: library`, `language: typescript` (`-ts` suffix, SMA-380). |

### Modified

| File | Change |
|---|---|
| `.prototools` | Add the lefthook pin (proto npm backend preferred — see open items). |
| `.moon/workspace.yml` | Convert `projects` list → `{ globs: [...existing...], sources: { repo: '.' } }`. `vcs.hooks` stays unset. |
| `ts/package.json` | Add resilient `prepare` (above); devDep `@commitlint/cli` (catalog); `@paigasus/commitlint-config: "workspace:*"`. |
| `ts/pnpm-workspace.yaml` | Catalog entries: `@commitlint/cli`, `@commitlint/config-conventional`. |
| `CONTRIBUTING.md` | New "Local development setup" subsection (install order, GUI-PATH note, `--no-verify`); **full type + scope allowlists**; `.cjs`/`.js` config added to the no-SPDX list; maintenance rule covering **both** lists. |
| `pnpm-lock.yaml` | Regenerated. |

## Commit-msg rule set (`@paigasus/commitlint-config`)

`index.cjs` extends `@commitlint/config-conventional`, then sets:

- `type-enum` (error): `feat`, `fix`, `docs`, `chore`, `refactor`, `test`, `ci`,
  `build`, `perf`, `style`, `revert`
- `scope-enum` (error): `rs`, `py`, `ts`, `contracts`, `ci`, `docs`, `deps`,
  `release`, `repo`, `claude`, `workspace`
- `scope-empty: [2, 'never']` — **scope is mandatory**; repo-wide changes use the
  `repo` / `workspace` scopes
- `subject-empty: [2, 'never']`
- `header-max-length: 100`
- `body-max-line-length: 100`
- `footer-leading-blank: [2, 'always']`

`ts/commitlint.config.cjs` extends this package **only**. Any rule change is a
config-file edit (no code change); the config package is the machine source of
truth, and CONTRIBUTING mirrors the type + scope lists (maintenance rule below).

## Branch-name rule (`pre-push` → `check-branch.sh`)

- Resolve the current branch: `branch="$(git symbolic-ref --short HEAD 2>/dev/null)"`.
  (Detached HEAD → no branch → nothing to validate; allow.)
- Allow-list: `main`, and any branch matching `^dependabot/`.
- Otherwise require `^feature/[a-z0-9._-]+$`.
- On mismatch: print the rule + a rename hint (`git branch -m feature/<slug>`) and
  exit non-zero.

> The regex is intentionally looser than the `feature/sma-NNN-<slug>` house style so
> external contributors without a Linear key can use `feature/<slug>` (consistent
> with CONTRIBUTING).

## Bot exemption + replay safety

- **Bot guard:** each command begins with an inline check — if
  `git config user.email` matches `*[bot]@*`, exit 0. Portable regardless of
  lefthook's skip-condition surface.
- **Replay safety:** global `skip: [merge, rebase]` so interactive rebases do not
  re-validate replayed commits.

## Error handling

- `commitlint` / `pnpm` / `node` missing on `$PATH` → print an install hint, exit
  non-zero. **Never silently pass** (AC item B). Warn-and-pass was rejected: it
  violates AC-B and guts the local gate.
- **GUI-client `$PATH` mitigation:** GUI git clients (VS Code, IntelliJ, etc.) often
  launch with a stripped `$PATH` lacking proto shims / node, which would turn a
  fully-set-up contributor's commit into a hard rejection. Mitigate by (a) a
  CONTRIBUTING note on adding the proto shim dir to the GUI's environment, and
  (b) resolving the commitlint binary by absolute path (`ts/node_modules/.bin/...`)
  so only `node` itself needs to be discoverable. Verify the chain under a stripped
  `$PATH` before merge.
- `git commit --no-verify` is the documented escape hatch (CI still catches).

## CI-parity invariant (documented here, enforced by SMA-361)

- CI runs the **same** commitlint binary against the **same**
  `@paigasus/commitlint-config` version as the local hook — both pinned via
  `pnpm-lock.yaml`.
- A CI smoke job runs `release-plz` (and the Python / TS release tools when
  available) in dry-run mode against synthetic Conventional-Commit examples and
  asserts the semver classifications match expectation (catches `feat!:` vs
  `BREAKING CHANGE:` footer drift).
- **Maintenance rule (added to CONTRIBUTING):** the **type and scope allowlists** in
  `@paigasus/commitlint-config` and the lists in `CONTRIBUTING.md` are updated
  together; the config package is the source of truth.

## Testing / verification (AC Section F)

**Manual matrix** (run after `moon run repo:install-hooks`):

| Input | Expected |
|---|---|
| commit `wip` | rejected (`type`/`subject`/`scope` rules) |
| commit `feat: add kernel` (no scope) | rejected by `scope-empty` |
| commit `feat(rs): add kernel` | passes |
| commit `feat(unknown-scope): something` | rejected by `scope-enum` |
| push from `sven/foo` | rejected by `pre-push` |
| push from `feature/sma-371-local-git-hooks` | passes |
| `git commit --no-verify` | bypasses all hooks (documented) |
| commit as `dependabot[bot]@users.noreply.github.com` | not validated locally |
| `git commit` from a GUI client (terminal + GUI) | succeeds with toolchain on PATH |

**Unit test:** a `bats`/sh test for `check-branch.sh` covering: conforming
`feature/...`, non-conforming `sven/...`, `main` allow-list, `dependabot/...`
allow-list, detached HEAD (allow). (The current-branch design removes the
stdin/delete/tag/multi-ref cases from scope.)

## SPDX headers (decided)

- `lefthook.yml`, `moon.yml`, `.prototools`, `package.json`, `pnpm-workspace.yaml`
  → config, **no** SPDX header.
- `commitlint.config.cjs` and `index.cjs` → treated as **config** (declarative rule
  objects), **no** SPDX header. CONTRIBUTING's no-header list is extended to name
  `.cjs` / `.js` *config* files so this does not re-litigate per file.
- `.lefthook/pre-push/check-branch.sh` → hand-written script, carries
  `# SPDX-License-Identifier: Apache-2.0`.

## Open items (resolve at implementation)

1. **proto lefthook backend syntax.** Prefer the npm backend (prebuilt binary) over
   the cargo backend (compiles from source). If the proto pin proves awkward in the
   current proto, fall back to a documented `brew` / `mise` / `cargo install
   lefthook` and a plain `lefthook install` task. Verify before committing the pin.
2. **Root `repo` project with `source: '.'`.** Watch for Moon project-root overlap
   warnings vs. the nested projects; the `local: true` + scoped `inputs` already
   keep caching/CI scoped. Verify `moon run repo:install-hooks` resolves and
   `moon ci :build` / `moon sync` stay clean.

## Follow-ups / amendments

- **ADR-0010 + AC item A amended:** dropped the "wired into Moon `sync`" language;
  state the `prepare` + documented-task reality and the non-ts auto-install gap.
  Done as part of this work (user-approved); AC amendment also recorded as a comment
  on SMA-371.
- **Publish-tracking for `@paigasus/commitlint-config`:** ADR-0010's headline
  rationale is cross-repo reuse of a *published* config. It ships `private` for now;
  **SMA-390** tracks flipping `private: false` when the repo's publish story lands.
  The package's `package.json` carries a `TODO(SMA-390)` next to `private: true`,
  mirroring the kernel pattern.

## Implementation notes (as-built)

Deviations discovered during implementation (the design intent held; these are
mechanism corrections):

- **lefthook pinned via a vendored proto TOML plugin** (`.proto/plugins/lefthook.toml`,
  mirroring `buf.toml`), not proto's npm backend — the npm backend requires
  proto-managed Node, but this repo manages Node via nvm. Version pinned: **2.1.8**.
- **`options.runInCI: false`** on the `install-hooks` task — Moon 2.2.5 rejects the
  `local: true` shorthand. Same effect (never runs in CI).
- **`install-hooks` uses `script:`, not `command:`** — Moon 2.2.5 rejects shell
  operators (`&&`) in `command`. The task is `script: 'lefthook validate && lefthook
  install'`; the prepended `lefthook validate` is a guard against invalid hook config.
- **lefthook 2.x requires `skip` per-hook, not top-level** — a top-level
  `skip: [merge, rebase]` is silently dropped (the plan was drafted against lefthook
  1.x). `skip: [merge, rebase]` is nested under both `commit-msg` and `pre-push`; the
  `lefthook validate` gate above catches this class of error.
- **commitlint pinned `^21.0.1`** (`@commitlint/cli` + `@commitlint/config-conventional`).
- Root `repo` project (`source: '.'`) registered with no Moon overlap warning
  (open item #2 resolved favorably).

## Out of scope

- The GitHub server-side branch-name ruleset (separate concern; mirrors this rule
  and is the authoritative ref-level gate).
- CI workflow wiring and the release-tool parity smoke job (SMA-361).
- Publishing `@paigasus/commitlint-config` to a registry (tracked follow-up above).
