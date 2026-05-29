# Review — SMA-371 local git hooks design

**Reviews:** [`2026-05-29-sma-371-local-git-hooks-design.md`](./2026-05-29-sma-371-local-git-hooks-design.md)
**Reviewer perspective:** staff engineer
**Date:** 2026-05-29
**Sources cross-referenced:** Linear SMA-371 (+ relations SMA-359/356/361), [ADR-0010](https://www.notion.so/36c830e8fbaa8110bbaee37475ad57c8), and the live `paigasus-core` tree (`.moon/workspace.yml`, `.prototools`, `ts/`, `CONTRIBUTING.md`).

## Verdict

The toolchain and most of the structural decisions are right, and the spec does the hard, valuable work of correcting an impossible AC (Moon `sync` cannot auto-run a project task). It is implementable. But the design rests on two PATH/cwd assumptions that are very likely wrong in practice, and those are exactly the failure modes that make git-hook setups infamous: they pass in the author's terminal and fail silently — or reject every commit — in other environments.

The single highest-value thing to do before implementing is to **actually run the commit-msg chain end-to-end** (`git commit` from a clean shell *and* from a GUI client), because findings H1 and M1 will not show up in a YAML review — only in execution.

The findings are ordered by how badly they bite and how late they surface.

## What the spec gets right (calibration)

- **It corrects the AC's "triggered automatically by Moon `sync`" (decision #1).** `moon sync` runs `SyncWorkspace` (codeowners, VCS hooks), not project tasks; with `vcs.hooks` deliberately empty, sync-triggered install is unachievable. The `prepare` + documented-task substitute is the right shape. (This is the same class of AC-correction the SMA-360 spec did well.)
- **Proto-pinning the lefthook binary (decision #3) sidesteps a real monorepo bug.** lefthook's npm shim looks for the binary in `<git-root>/node_modules`, which doesn't exist here (node_modules lives in `ts/`). Pinning via `.prototools` like `moon`/`buf` is well-researched (issues #510/#443 cited) and correct.
- **Committing `check-branch.sh` as a testable script** rather than a YAML-embedded blob is the right maintainability call.
- **`skip: [merge, rebase]`** replay safety and the **inline `*[bot]@*` email guard** are portable and correct.
- **Conventions are matched:** `-ts` id suffix (SMA-380), the `tool`/`configuration` `layer` values (CONTRIBUTING now sanctions `tool`), and the config-file SPDX carve-out (SMA-383).

## Findings summary

| # | Severity | Finding | Bites when |
|---|----------|---------|-----------|
| H1 | High | `commitlint --edit {1}` under `pnpm -C ts exec`: `{1}` is a worktree-relative path, but `-C ts` changes cwd — file likely not found | First commit, every commit |
| H2 | High | `prepare: "lefthook install"` needs lefthook on `$PATH`, but lefthook is proto-acquired (decision #3), not a pnpm dep — `pnpm install` before `proto install` hard-fails | First clone / JS-only contributor |
| M1 | Medium | Whole `git → lefthook → pnpm → commitlint` chain depends on `$PATH`; GUI git clients strip it → "exit non-zero" turns into rejecting every commit | Committing from VS Code/IntelliJ/etc. |
| M2 | Medium | `repo` project `source: '.'` + `install-hooks` has no `runInCI: false` → may run `lefthook install` in CI; the rejected `repo/` subdir avoids the overlap | When SMA-361 runs `moon ci` |
| M3 | Medium | CONTRIBUTING already lists 7 types vs the config's 11, and has **no** scope allowlist — the "keep lists in sync" maintenance rule syncs against nothing | Immediately; drift already exists |
| M4 | Medium | `check-branch.sh` test matrix omits real pre-push cases: delete (zero-sha), tags, multi-ref, detached HEAD | First non-trivial push pattern |
| M5 | Medium | "Hard local requirement" overstated: non-ts contributors get no auto-install; ADR-0010/AC "wired into sync" is unachievable | Rust/Python-only contributors |
| L1 | Low | `scope-empty` unset → scopeless commits pass locally despite all-scoped CONTRIBUTING examples | First `feat: x` with no scope |
| L2 | Low | `.cjs` SPDX treatment punted to PR review; `.cjs` is executable JS, and the (eventually published) config wants a header | PR review / publish |
| L3 | Low | `@paigasus/commitlint-config` kept `private` with no publish-tracking — defers ADR-0010's headline rationale | When cross-repo reuse is wanted |

## High-severity

### H1 — `pnpm -C ts exec commitlint --edit {1}` will likely fail to find the commit-msg file

This is the load-bearing line of the whole commit-msg hook, and the spec's justification is the part I'd bet against. The spec (decision #3) states: *"`{1}` is the absolute commit-msg file path, so it is cwd-independent."* Git's `commit-msg` contract passes the message file as a path **relative to the worktree root** — typically `.git/COMMIT_EDITMSG`, not an absolute path. lefthook's `{1}` expands to that argument as-is.

So the effective invocation is `pnpm -C ts exec commitlint --edit .git/COMMIT_EDITMSG` **with cwd changed to `ts/`** by `-C ts`. commitlint then resolves `.git/COMMIT_EDITMSG` against `ts/` → `ts/.git/COMMIT_EDITMSG`, which does not exist → the hook errors on a path lookup, not on the commit content. The cwd change needed to locate the *binary* (in `ts/node_modules`) is the same cwd change that breaks the *file* path.

Open item #3 flags "confirm commitlint resolves its **config** under `-C ts`" — but the more dangerous half is the **commit-msg file path**, which the spec doesn't question. Recommendation: don't trust `{1}` to be absolute. Either absolutize it at the hook (`--edit "$(git rev-parse --absolute-git-dir)/COMMIT_EDITMSG"`), or invoke from the git root with an explicit `--config ts/commitlint.config.cjs` and a `node_modules/.bin` path that doesn't require `-C ts`. Verify with a real commit before committing the YAML.

### H2 — `prepare: "lefthook install"` and the proto-pinned binary are in tension

Decision #1 uses a `prepare` script in `ts/package.json` so that "everyone who touches `ts/`" gets hooks installed on `pnpm install`. Decision #3 says lefthook is **not** a node_modules dependency — it comes from proto/`$PATH`. These two decisions collide:

- `prepare` runs during `pnpm install`. If lefthook is not yet on `$PATH` (i.e. `proto install` hasn't run), `lefthook install` fails, and a failing `prepare` script makes **`pnpm install` itself exit non-zero**. A JS-leaning contributor who clones and runs `pnpm install` in `ts/` before `proto install` gets a hard install failure with a confusing message.
- The convenience the `prepare` hook was supposed to buy ("without thinking about it") only materializes if proto already ran — at which point the contributor could as easily run the documented `moon run repo:install-hooks`. The auto-install path quietly depends on an ordering the spec doesn't state.

Recommendation: (a) make `prepare` resilient — `command -v lefthook >/dev/null && lefthook install || echo "run 'proto install' to enable git hooks"` — so a missing binary never breaks `pnpm install`; and (b) document the required order (`proto install` → workspace installs) in the new CONTRIBUTING "Local development setup" subsection. Without (a), the spec's own auto-install convenience becomes an onboarding footgun.

## Medium-severity

### M1 — the hook chain depends on `$PATH`, which GUI git clients routinely strip

Every link — the `.git/hooks/*` shim finding `lefthook`, lefthook finding `pnpm`, pnpm finding `node`/`commitlint` — assumes a populated `$PATH`. Terminal git inherits the shell's rc-built PATH; **GUI clients (VS Code, IntelliJ, GitKraken, Tower) commonly launch with a minimal PATH** that lacks proto shims and node. The spec's error handling ("commitlint or pnpm missing on `$PATH` → exit non-zero, never silently pass") then converts a fully-set-up contributor's GUI commit into a **hard rejection with an install hint they don't need**. The spec aims partly at external contributors (it loosens the branch regex for them), so GUI usage is in-scope. Recommendation: verify the chain under a stripped PATH; consider having the generated hook source the proto env or resolve absolute binary paths, and decide whether a missing-toolchain hook should *warn-and-pass* locally (CI is the authoritative gate per the spec's own framing) rather than block.

### M2 — `repo` with `source: '.'` and an un-guarded `install-hooks` task

Decision #2 puts a root project at `source: '.'` to host `install-hooks`, rejecting a `repo/` subdir "to avoid a near-empty directory." That trades a cosmetic concern for two operational ones: (1) Moon may emit project-root overlap warnings for a `.`-rooted project sitting above every nested project (open item #2 acknowledges this but only says "watch for" it); (2) more concretely, the `install-hooks` task has no `options.runInCI: false` / `local: true`, so when SMA-361 wires `moon ci`, a `repo`-affected run could execute `lefthook install` in CI — pointless at best, and it writes into a CI checkout's `.git/hooks`. Recommendation: mark `install-hooks` `local`/`runInCI: false` and scope its `inputs` to `lefthook.yml` + `.lefthook/**`; and reconsider the `repo/` subdir — it eliminates the overlap question entirely for the cost of one directory, which is the cheaper trade for a foundational config file.

### M3 — the scope/type lists already drift, and the maintenance rule syncs against a list that doesn't exist

CONTRIBUTING's "Commit messages" section currently states *"Common types: `feat`, `fix`, `docs`, `chore`, `refactor`, `test`, `ci`"* — **seven** types. The spec's `@paigasus/commitlint-config` `type-enum` has **eleven** (adds `build`, `perf`, `style`, `revert`). And CONTRIBUTING contains **no enumerated scope allowlist** at all (only prose: "a scope naming the workspace or area"). So the spec's headline maintenance rule — "the scope allowlist in the config and the scope list in `CONTRIBUTING.md` are updated together" — references a CONTRIBUTING list that doesn't exist, and it covers scopes but not types, which is the dimension already drifted.

A "keep two hand-maintained lists in sync" rule is a known smell; it has already failed here before the feature ships. Recommendation: make `@paigasus/commitlint-config` the single source of truth for both lists and have CONTRIBUTING *link* to it (or to a generated snippet) rather than restating them. If the lists must appear in CONTRIBUTING, the spec should explicitly add both the type and scope allowlists there in this PR, and the maintenance rule should cover types too.

### M4 — the one piece of custom logic is under-tested

`check-branch.sh` is correctly called out as "the only piece of real logic we author," but the `bats` matrix covers only conforming `feature/...`, non-conforming `sven/...`, `main`, and `dependabot/...`. The git pre-push stdin contract produces several shapes the script must survive: **branch deletion** (`<local-ref>` is `(delete)`, local-sha all-zeros), **tag pushes** (`refs/tags/...`, must be ignored, not rejected), **multiple refs in one push** (`git push --all`), **detached-HEAD / SHA pushes**, and the local-vs-remote ref column (branch-name enforcement cares about the *local* ref). Recommendation: extend the matrix to cover delete/tag/multi-ref/zero-sha; these are the cases that turn a branch-name guard into "can't delete a branch" bug reports.

### M5 — "hard local requirement" is overstated for non-ts contributors

The goal states hooks are a "hard local requirement." In practice, only contributors who run `pnpm install` get the `prepare`-driven install; pure-Rust/pure-Python contributors must remember the manual `moon run repo:install-hooks`. For them, a wrong scope or branch name still surfaces first at CI — the exact outcome the goal says to prevent. This is an accepted consequence of the (correct) Moon-sync correction, but it should be stated plainly as a residual coverage hole, and ADR-0010's "hook installation wired into the Moon `sync` step" language (and AC item A) should be amended, since the spec proves it isn't achievable. (Same source-of-truth-drift pattern as the SMA-360 review: the spec fixes it locally but leaves the ADR asserting the impossible.)

## Low-severity / hygiene

- **L1 — `scope-empty` not set.** The config sets `subject-empty: never` but leaves `scope-empty` at the `config-conventional` default (empty allowed). So `feat: something` (no scope) passes locally, despite every CONTRIBUTING example carrying a scope. Decide whether scope is mandatory; if so, add `scope-empty: [2, 'never']`.
- **L2 — `.cjs` SPDX punted.** The spec treats `commitlint.config.cjs` and `index.cjs` as config (no header) and says "confirm in PR review." `.cjs` is executable JavaScript, not declarative config like the yaml/json/toml the SMA-383 carve-out enumerated; the (eventually publishable) `@paigasus/commitlint-config` is Apache-2.0 source that arguably wants a header. Resolve against the convention now rather than deferring — a punted convention question re-litigates on every future `.cjs`/`.js` file.
- **L3 — config package shipped `private` with no publish tracking.** ADR-0010's headline rationale for commitlint-over-convco is "shared config as a published package" consumable cross-repo (helikon). Shipping it `private` (consumed via `workspace:*`) is fine for now, but with no tracked issue to flip it, the main differentiator is silently deferred — the same pattern as the SMA-360 proto crate's untracked `publish` TODO. Mirror the kernel's `TODO(SMA-NNN)` and file the flip.

## Suggested additions to the acceptance criteria / follow-ups

1. **Execute the chain before merge:** real `git commit` (good + bad message) from a terminal *and* a GUI client; confirm the commit-msg file path resolves under `-C ts` (H1) and behavior under stripped PATH (M1).
2. Make `prepare` resilient to a missing lefthook and document the `proto install` → workspace-install ordering (H2).
3. Add `options.runInCI: false` (or `local: true`) + scoped `inputs` to `install-hooks`; re-decide `source: '.'` vs a `repo/` subdir (M2).
4. Add the type **and** scope allowlists to CONTRIBUTING (or link to the config as the single source) and reconcile the existing 7-vs-11 type drift (M3).
5. Extend the `check-branch.sh` test matrix to delete/tag/multi-ref/zero-sha cases (M4).
6. Amend ADR-0010 + AC item A to drop the unachievable "wired into Moon sync" language; state the non-ts auto-install gap (M5).
7. File a tracking issue for flipping `@paigasus/commitlint-config` to published (L3).

## Sources

- Spec under review: `docs/superpowers/specs/2026-05-29-sma-371-local-git-hooks-design.md`
- [Linear SMA-371 — Local git hooks](https://linear.app/smaschek/issue/SMA-371/local-git-hooks-commit-msg-conventional-commits-branch-name-validation) (AC A–F; blocked by SMA-359/356; related SMA-361/335)
- [Notion — ADR-0010: lefthook + commitlint over convco](https://www.notion.so/36c830e8fbaa8110bbaee37475ad57c8)
- Repo: `.moon/workspace.yml` (`projects` list form, `vcs.hooks` unset), `.prototools` (`buf`/`moon`, no lefthook yet), `ts/package.json` (`@paigasus/workspace`, no `prepare`), `ts/pnpm-workspace.yaml` (catalog, no commitlint entries), `ts/packages/*`, `CONTRIBUTING.md` (commit/branch policy, 7-type list, no scope allowlist)
