# SMA-371 — Local git hooks: commit-msg (Conventional Commits) + branch-name validation

**Status:** Approved (design)
**Date:** 2026-05-29
**Linear:** [SMA-371](https://linear.app/smaschek/issue/SMA-371/local-git-hooks-commit-msg-conventional-commits-branch-name-validation)
**ADR:** [ADR-0010 — lefthook + commitlint over convco](https://www.notion.so/36c830e8fbaa8110bbaee37475ad57c8)

## Goal

Make Conventional Commits and branch naming a **hard local requirement** — fast
feedback at `git commit` and `git push` time, not first discovered at CI. This is
the developer-machine layer; SMA-361 is the server-side counterpart.

The toolchain is already settled by ADR-0010: **lefthook** (hook manager) +
**commitlint** (commit-msg validator) with a shared `@paigasus/commitlint-config`
package, a `pre-push` hook for branch-name enforcement, and Moon's native
`vcs.hooks` left empty so lefthook is the sole owner of `.git/hooks/*`. This spec
covers the concrete *implementation shape* the ADR/AC left open, not the toolchain
choice.

## Scope boundary with SMA-361

SMA-371 ships the **artifacts + docs**: the `@paigasus/commitlint-config` package,
`lefthook.yml`, `commitlint.config.cjs`, the install wiring, and CONTRIBUTING
updates. **SMA-361** (CI workflow) consumes the exact pinned config in CI and adds
the `release-plz` / semantic-release dry-run parity smoke job. The CI-parity
*invariant* (same commitlint binary + same `@paigasus/commitlint-config` version,
both pinned via `pnpm-lock.yaml`) is documented here but enforced by SMA-361.

## Key decisions (this brainstorm)

Four implementation decisions the ADR/AC did not fully pin down:

1. **Install trigger.** Moon cannot auto-run a project task during `moon sync` —
   its only native "act on sync" path for hooks is `vcs.hooks`, which we
   deliberately disable. So install is triggered two ways:
   - a `prepare` script in `ts/package.json` runs `lefthook install` on
     `pnpm install` (covers everyone who touches `ts/`);
   - `moon run repo:install-hooks` documented as the explicit one-time onboarding
     step (covers pure-Rust / pure-Python contributors who never run `pnpm install`).

   > **Correction to the AC.** The AC says install is "triggered automatically by
   > Moon `sync`". With `vcs.hooks` empty (which the same AC mandates), that is not
   > achievable — `moon sync` runs `SyncWorkspace` (codeowners, VCS hooks), not
   > project tasks. The `prepare` + documented-task pair is the ADR-faithful
   > substitute. Re-enabling `vcs.hooks` as the installer was considered and
   > rejected: it makes Moon **and** lefthook both write `.git/hooks/*`, and the
   > last writer wins (the exact failure mode ADR-0010 guards against).

2. **`repo` Moon project home.** A root-level `repo` project registered via Moon's
   combined `projects` form (`globs: [...]` + `sources: { repo: '.' }`) with a root
   `moon.yml` (`id: repo`, `layer: configuration`). This puts the `install-hooks`
   task where `lefthook.yml` lives and matches the AC's `repo:` target naming. A
   `repo/` subdirectory project was the alternative; rejected to avoid a near-empty
   directory whose only purpose is hosting one task.

3. **Tool resolution** (the pnpm-rooted-at-`ts/` vs. hooks-at-git-root seam). The
   two tools are acquired differently by nature:
   - **lefthook** is pinned in `.prototools` (proto), on `$PATH` after
     `proto install` — exactly like `moon` and `buf`. This sidesteps lefthook's
     known monorepo failure mode: its npm shim looks for the binary in
     `<git-root>/node_modules/lefthook`, which does not exist here (node_modules is
     in `ts/`), triggering a slow `npx` fallback / version drift
     ([lefthook#510](https://github.com/evilmartians/lefthook/issues/510),
     [#443](https://github.com/evilmartians/lefthook/issues/443)).
   - **commitlint** is a pnpm dependency inside `ts/`; lefthook invokes it via
     `pnpm -C ts exec commitlint --edit {1}` (`{1}` is the absolute commit-msg file
     path, so it is cwd-independent).

4. **post-checkout warn hook: skipped.** `pre-push` is the enforcement point (git
   ignores `post-checkout` exit codes). The warn-only hook is a second script to
   maintain for marginal zero-latency feedback; trivial to add later if wanted.

## Architecture

```
git event ──> .git/hooks/{commit-msg,pre-push}   (shims written by `lefthook install`)
                     │
                     └─> lefthook (from $PATH, pinned via proto)
                           reads lefthook.yml (repo root)
                           │
              commit-msg ──┼─> pnpm -C ts exec commitlint --edit {1}
                           │       └─> ts/commitlint.config.cjs
                           │             └─ extends @paigasus/commitlint-config
                           │                  └─ extends @commitlint/config-conventional
              pre-push   ──┘─> .lefthook/pre-push/check-branch.sh  (reads stdin refs)
```

- **Single owner of `.git/hooks/*`:** lefthook. Moon `vcs.hooks` stays empty.
- **lefthook binary:** proto-pinned, `$PATH`. No git-root `node_modules` dependency.
- **commitlint:** pnpm tool in `ts/`, reached via `pnpm -C ts exec`.

## Files

### New

| File | Purpose |
|---|---|
| `moon.yml` (repo root) | `id: repo`, `layer: configuration`; task `install-hooks` → `lefthook install`. Runs as a system command (no language toolchain). |
| `lefthook.yml` (repo root) | `commit-msg` (commitlint) + `pre-push` (branch-name) hooks; global `skip: [merge, rebase]`; bot-email guard per command. |
| `.lefthook/pre-push/check-branch.sh` | Branch-name validator as a committed, testable script (not a YAML-embedded blob). |
| `ts/commitlint.config.cjs` | `module.exports = { extends: ['@paigasus/commitlint-config'] }` — no per-repo overrides (ADR-0010). |
| `ts/packages/commitlint-config/package.json` | `@paigasus/commitlint-config`; `private: true` (matches current repo state); `main: index.cjs`; deps `@commitlint/config-conventional`. |
| `ts/packages/commitlint-config/index.cjs` | The canonical ruleset (types, scopes, lengths). |
| `ts/packages/commitlint-config/moon.yml` | `id: commitlint-config-ts`, `layer: library`, `language: typescript` (per `-ts` suffix convention, SMA-380). |

### Modified

| File | Change |
|---|---|
| `.prototools` | Add the lefthook pin. |
| `.moon/workspace.yml` | Convert `projects` list → `{ globs: [...existing...], sources: { repo: '.' } }`. `vcs.hooks` stays unset. |
| `ts/package.json` | Add `prepare: "lefthook install"`; devDep `@commitlint/cli` (catalog); `@paigasus/commitlint-config: "workspace:*"`. |
| `ts/pnpm-workspace.yaml` | Catalog entries: `@commitlint/cli`, `@commitlint/config-conventional`. |
| `CONTRIBUTING.md` | New "Local development setup" subsection; `git commit --no-verify` escape hatch; scope-list maintenance rule. |
| `pnpm-lock.yaml` | Regenerated. |

## Commit-msg rule set (`@paigasus/commitlint-config`)

`index.cjs` extends `@commitlint/config-conventional`, then sets:

- `type-enum`: `feat`, `fix`, `docs`, `chore`, `refactor`, `test`, `ci`, `build`,
  `perf`, `style`, `revert`
- `scope-enum`: `rs`, `py`, `ts`, `contracts`, `ci`, `docs`, `deps`, `release`,
  `repo`, `claude`, `workspace`
- `header-max-length: 100`
- `body-max-line-length: 100`
- `footer-leading-blank: [2, 'always']`
- `subject-empty: [2, 'never']`
- `scope-enum` / `type-enum` set at error level (`2`)

The repo's `ts/commitlint.config.cjs` extends this package **only**. Any rule
change is a config-file edit (no code change) and, if it touches the scope list,
must be mirrored in `CONTRIBUTING.md` (maintenance rule below).

## Branch-name rule (`pre-push` → `check-branch.sh`)

- Reads pushed refs from **stdin** (git's pre-push contract:
  `<local-ref> <local-sha> <remote-ref> <remote-sha>` per line).
- For each `refs/heads/<name>`, validate `<name>` against `^feature/[a-z0-9._-]+$`.
- Allow-list: `main` and any branch matching `^dependabot/`.
- On mismatch: print the rule + a rename hint (`git branch -m feature/<slug>`) and
  exit non-zero.

> The regex is intentionally looser than the `feature/sma-NNN-<slug>` house style so
> external contributors without a Linear key can use `feature/<slug>` (consistent
> with CONTRIBUTING).

## Bot exemption + replay safety

- **Bot guard:** each command begins with an inline check — if
  `git config user.email` matches `*[bot]@*`, exit 0. Robust and portable
  regardless of lefthook's skip-condition surface.
- **Replay safety:** global `skip: [merge, rebase]` so interactive rebases do not
  re-validate every replayed commit.

## Error handling

- `commitlint` or `pnpm` missing on `$PATH` → print an install hint, exit non-zero.
  Never silently pass.
- `git commit --no-verify` is the documented escape hatch (CI is the authoritative
  gate and still catches bypassed commits).

## CI-parity invariant (documented here, enforced by SMA-361)

- CI runs the **same** commitlint binary against the **same**
  `@paigasus/commitlint-config` version as the local hook — both pinned via
  `pnpm-lock.yaml`.
- A CI smoke job runs `release-plz` (and the Python / TS release tools when
  available) in dry-run mode against synthetic Conventional-Commit examples and
  asserts the semver classifications match expectation (catches `feat!:` vs
  `BREAKING CHANGE:` footer drift).
- **Maintenance rule (added to CONTRIBUTING):** the scope allowlist in
  `@paigasus/commitlint-config` and the scope list in `CONTRIBUTING.md` are updated
  together.

## Testing / verification (AC Section F)

**Manual matrix** (run after `moon run repo:install-hooks`):

| Input | Expected |
|---|---|
| commit `wip` | rejected (`subject`/`type` rules) |
| commit `feat(rs): add kernel` | passes |
| commit `feat(unknown-scope): something` | rejected by `scope-enum` |
| push from `sven/foo` | rejected by `pre-push` |
| push from `feature/sma-371-local-git-hooks` | passes |
| `git commit --no-verify` | bypasses all hooks (documented) |
| commit as `dependabot[bot]@users.noreply.github.com` | not validated locally |

**One unit test:** a `bats`/sh test for `check-branch.sh` (the only piece of real
logic we author) covering: conforming `feature/...`, non-conforming `sven/...`,
`main` allow-list, `dependabot/...` allow-list.

## SPDX headers

`lefthook.yml`, `moon.yml`, `.prototools`, and `package.json` are config → no SPDX
header (per CONTRIBUTING). `commitlint.config.cjs` and `index.cjs` are treated as
**config** (no header), consistent with how `*.yaml`/`*.json`/dotfiles are handled;
`check-branch.sh` is a hand-written script and carries
`# SPDX-License-Identifier: Apache-2.0`. Confirm the `.cjs` call in PR review.

## Open items (resolve at implementation)

1. **proto lefthook backend syntax.** Prefer the npm backend (prebuilt binary) over
   the cargo backend (compiles from source, slow first install). If the proto pin
   proves awkward in 2.2.5 / current proto, fall back to a documented
   `brew install lefthook` / `mise` / `cargo install lefthook` and a plain
   `lefthook install` task. Verify before committing the pin.
2. **Root `repo` project with `source: '.'`.** Watch for Moon file-ownership
   overlap warnings against the nested projects; explicit task `inputs` keep caching
   scoped. Verify `moon run repo:install-hooks` resolves and `moon ci :build` /
   `moon sync` stay clean.
3. **`pnpm -C ts exec` config resolution.** Confirm commitlint resolves
   `ts/commitlint.config.cjs` when invoked with `-C ts` from a git-root cwd.
4. **lefthook skip-condition surface.** If lefthook supports an email-pattern
   `skip` cleanly, prefer it over the inline guard; otherwise keep the inline guard.

## Out of scope

- The GitHub server-side branch-name ruleset (separate concern; mirrors this rule).
- CI workflow wiring and the release-tool parity smoke job (SMA-361).
- Publishing `@paigasus/commitlint-config` to a registry (kept `private` until the
  repo's broader publish story lands; consumed locally via `workspace:*`).
