# SMA-683 — wasm-bindgen cannot move through dependabot: record the freeze and the lockstep bump

**Linear:** SMA-683. **Related:** SMA-634 (the committed wasm glue), SMA-680 (`dependencies_update =
false`). **Status:** revision 2, after the adversarial challenge and measurement M0 (2026-09-26).
Revision 1 proposed a separate dependabot group. M0 showed that the group would never get a PR, and
Sven chose this design on 2026-09-26.

## 1. Problem

The five wasm artifacts under `rs/crates/bindings/paigasus-wasm/` are committed (SMA-634).
`paigasus-kernel-ts:generate-wasm` is their only writer, and it has `runInCI: false`.
`ts/packages/paigasus-kernel/tests/committed-wasm.test.ts` compares them with a fresh build. A move
of `wasm-bindgen` changes the `__wbg_Error_*` import hash and fails checks 1 and 2 (SMA-680 M1).

The issue assumed that a dependabot cargo PR moves `wasm-bindgen`. M0 (section 3) shows that
dependabot cannot move it by itself. So the real risk after SMA-680 is different: **nothing moves
`wasm-bindgen` any more, and nothing reports that.** Before SMA-680, the release PR's full
`cargo update` moved it (v0.1.0, v0.2.0). Since SMA-680 (`dependencies_update = false`), no
automated path moves it.

## 2. Acceptance, and how this design meets it

The issue: "A dependabot cargo PR that moves `wasm-bindgen` can merge with no manual regeneration,
or it arrives separately from the rest of the cargo group with a documented one-step fix."

- **A direct `wasm-bindgen` bump:** dependabot cannot make one (M0). The case does not occur.
- **The `reqwest` case (4.3):** a cargo PR (usually `cargo-minor-patch`) can move `wasm-bindgen`
  through a new `reqwest`. That PR does NOT arrive separately, so it does not meet the
  acceptance. It gets the documented fix in 4.2. Sven accepted this remaining risk on
  2026-09-26 when he chose this design.
- **The freeze:** a person moves `wasm-bindgen` on purpose with the runbook in 4.2, in a normal
  feature PR. The follow-up (section 7) automates it.

## 3. Facts and measurements

| # | Fact | Source |
|---|------|--------|
| F1 | `wasm-bindgen = "0.2"` is in `[workspace.dependencies]` (`rs/Cargo.toml:152`). dependabot-core's cargo parser reads `workspace.dependencies` as direct dependencies, so `wasm-bindgen` is an update candidate. Being a candidate does not mean dependabot can resolve a newer version (M0). | `rs/Cargo.toml`; dependabot-core `cargo/lib/dependabot/cargo/file_parser.rb` (READ) |
| F2 | By default, dependabot version updates cover direct dependencies only. | docs.github.com, "Controlling dependencies updated" (READ) |
| F3 | Three crates in the lock pin `wasm-bindgen` exactly: `js-sys` 0.3.105 (`wasm-bindgen = "=0.2.128"`), `web-sys` 0.3.105 (`js-sys = "=0.3.105"`, `wasm-bindgen = "=0.2.128"`), `wasm-bindgen-futures` 0.4.78 (the same two pins). `wasm-bindgen` 0.2.128 pins `-macro` and `-shared` exactly. | registry manifests `js-sys-0.3.105/Cargo.toml:73-74`, `web-sys-0.3.105/Cargo.toml:2821-2826`, `wasm-bindgen-futures-0.4.78/Cargo.toml:54-59`, `wasm-bindgen-0.2.128/Cargo.toml:78-82` (READ by the challenger) |
| F4 | Crates in the lock with a CARET requirement on a family crate: `chrono`, `getrandom` 0.2 and 0.4, `iana-time-zone`, `jsonwebtoken`, `reqwest`, `rust_decimal`, `uuid`, `wasm-streams`, `web-time`, `quanta` (`web-sys = "0.3"`). Only `reqwest` depends on all three pinning crates of F3. | `rs/Cargo.lock` (READ 2026-09-26; `reqwest` at `rs/Cargo.lock:4156-4195`) |
| F5 | Only the wasm-bindgen version changes the glue. The `paigasus-wasm` wasm32 build graph holds `wasm-bindgen`, and `paigasus-kernel` with `uuid` (no `js` feature) and `thiserror`. `js-sys`, `web-sys` and `wasm-bindgen-futures` are not in it, so they affect only resolution. | challenger READ of the build graph |
| F6 | After a person pushes a commit to a dependabot branch, dependabot stops its rebases. `[dependabot skip]` in a commit message lets it force-push over that commit. `gh pr update-branch --rebase` and `@dependabot recreate` also replace the branch. | docs.github.com, "Managing pull requests for dependency updates"; memory `paigasus-main-strict-checks-dependabot` (READ) |
| F7 | `rs/Cargo.toml:149` points to a "dependency-bump runbook". No file holds it. | `grep` (READ by the challenger) |
| F8 | `repo:workflow-credentials` reads only `.github/workflows/*.y*ml`. `repo:actionlint` lints workflows only. No CI gate reads `.github/dependabot.yml`. | `moon.yml:835-836` (READ by the challenger) |

**M0 — can the one-package update that dependabot runs move `wasm-bindgen`?** MEASURED on
2026-09-26 in a detached scratch worktree of `origin/main` (a71e4063), cargo with Rust 1.95, in `rs/`.

| Step | Command | Result |
|---|---|---|
| 1 | `cargo update -p wasm-bindgen@0.2.128` | `Locking 0 packages`. The lock did not change. |
| 1b | `cargo update -p wasm-bindgen --precise 0.2.129` | Error: `failed to select a version for the requirement wasm-bindgen = "=0.2.128"`, `required by package js-sys v0.3.105`. |
| 2 (control) | `cargo update -p wasm-bindgen -p js-sys -p web-sys -p wasm-bindgen-futures` | `Locking 7 packages`: `wasm-bindgen`, `-macro`, `-macro-support`, `-shared` 0.2.128 → 0.2.129; `js-sys`, `web-sys` 0.3.105 → 0.3.106; `wasm-bindgen-futures` 0.4.78 → 0.4.79. The control bites, so step 1 is valid. |
| History | crates.io API; `git show <merge>:rs/Cargo.lock` | 0.2.128 was published 2026-09-04. PRs 284 (2026-09-21) and 292 (2026-09-23) ran while the lock held 0.2.127, and neither proposed 0.2.128. The v0.2.0 release PR moved it (2026-09-24). |

Conclusion: the one-package form dependabot uses (READ in dependabot-core and in the SMA-604 bullet
of `rs/CLAUDE.md`, not measured against dependabot itself) cannot move `wasm-bindgen`. The history
agrees. INFERRED, not measured: a new lead crate that needs a newer `js-sys` is held back by
dependabot with no message, except `reqwest` (4.3).

## 4. Design

### 4.1 No dependabot group; a comment in `.github/dependabot.yml`

No new group and no `ignore` entry. A group would never get a PR, and an `ignore` would only
restate what M0 shows. Add a comment above the `/rs` cargo entry. It states:

- `wasm-bindgen` does not move through this entry (M0, the three `=` pins), so it is frozen after
  SMA-680 until a person moves it;
- the `reqwest` case (4.3);
- a pointer to the runbook in `rs/CLAUDE.md`.

### 4.2 The runbook — `rs/CLAUDE.md`

`rs/CLAUDE.md` owns the Rust dependency rules, so the runbook goes there as a new bullet. It
replaces the dangling reference in `rs/Cargo.toml:149` (F7): that comment then names
`rs/CLAUDE.md` as the location.

**When to run it.** At each `rust-toolchain.toml` or `wasm-pack` bump, when `repo:deny` reports a
family advisory, or when a person wants a newer `wasm-bindgen`. The person who starts the bump
runs it; nothing schedules it until the follow-up lands.

**Before you start.**

- In an agent shell: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`.
- In a fresh worktree: `proto install` and `pnpm -C ts install`, or the `commit-msg` hook fails.
- Network access: `wasm-pack` downloads `wasm-bindgen-cli` for the new version.
- An unlocked 1Password (commit signing).

**The lockstep bump — a normal `feature/sma-NNN-<slug>` PR.**

```bash
( cd rs && cargo update -p wasm-bindgen -p js-sys -p web-sys -p wasm-bindgen-futures )
git diff -- rs/Cargo.lock   # the family entries (seven at M0) plus any new dep, crates.io only
moon run paigasus-kernel-ts:generate-wasm
moon run paigasus-kernel-ts:test   # the drift gate, before the push
git add rs/Cargo.lock rs/crates/bindings/paigasus-wasm/paigasus_wasm*
git commit -m "build(deps): move wasm-bindgen to <version> and regenerate the wasm glue"
```

- The `git diff` check comes before `generate-wasm`. `generate-wasm` compiles the new proc-macro
  and build scripts on the developer's machine, where the `gh` token and the signing agent are
  available. The same risk changed the SMA-680 design.
- Run `generate-wasm` on ONE host (SMA-634 F12). The gate compares only the glue and the interfaces.
- If `generate-wasm` fails because the pinned `wasm-pack` does not support the new 0.2.z, bump
  `wasm-pack` in `.prototools` in the same PR (`rs/Cargo.toml:144-151`, the invariant). The runbook
  quotes the error text when a bump first shows it; it is not measured yet.

**The `reqwest` case on a dependabot branch.** When a cargo PR (usually `cargo-minor-patch`)
moves `wasm-bindgen` (4.3), follow these steps in order:

1. Merge every other open cargo PR.
2. Let dependabot rebase this PR (its head commit changes), or comment `@dependabot rebase`.
3. `gh pr checkout <N>` (it keeps the `dependabot/*` name that the pre-push hook needs).
4. Run `git fetch origin`. Then read `git diff origin/main...HEAD -- rs/Cargo.lock`. Every
   changed entry must have a crates.io source. The wasm family entries must be among the
   changes. A group PR also holds `reqwest` and other bumps. Then run `generate-wasm` and the
   test. Commit and push.
5. If the branch goes stale again: the merge-only `update-branch` call
   (`gh api -X PUT repos/<owner>/<repo>/pulls/<N>/update-branch -f expected_head_sha=<sha>`), then
   `git pull`, then run `generate-wasm` again only if the merge changed the kernel, the wasm
   binding or the five artifacts.
6. For a `Cargo.lock` conflict, do not edit the lock by hand. Comment `@dependabot recreate`.
   Dependabot then writes a new lock on the current `main`. This deletes the glue commit.
   Wait for the new head commit. Then run `gh pr checkout <N> --force` and do step 4 again.
   A hand merge of the lock cannot keep both sides: `main`'s side drops the PR's bumps, and
   the PR's side drops `main`'s changes.

Except in step 6, never use `--rebase`, `@dependabot recreate` or `[dependabot skip]`. Each one
deletes the glue commit (F6).

### 4.3 The `reqwest` case — documented, not fixed

`cargo update -p reqwest` unlocks `reqwest` and its own dependencies. `reqwest` depends on all
three pinning crates (F4), so a new `reqwest` that needs newer wasm crates can move `wasm-bindgen`
inside a cargo PR (usually `cargo-minor-patch`). That PR then fails the drift gate and gets the
fix in 4.2. Today's
`reqwest` 0.12.28 asks only for `js-sys 0.3.77` and `wasm-bindgen 0.2.89`, so the case needs a
future `reqwest` release. INFERRED, not measured.

### 4.4 The test message — `committed-wasm.test.ts`

Add one sentence to the `REGENERATE` constant: when `wasm-bindgen` moved, follow the wasm-bindgen
runbook in `rs/CLAUDE.md`. The test logic, the checks and `SURFACE_CHANGED` do not change.

## 5. Out of scope

- A dependabot group or `ignore` entry for the family (4.1).
- A CI step that detects a `wasm-bindgen` move and prints the fix. Chosen against on 2026-09-26.
- A single `moon` target for the bump. The runbook is two commands before the commit, and the
  follow-up replaces it.
- The file-header claim in `dependabot.yml` that majors arrive separately. For a 0.x crate a
  0.2 → 0.3 move may count as a minor update; this is not checked here.
- The token scope of the release stamp step (SMA-680 follow-up B).

## 6. Verification

The change is a YAML comment, prose and one string.

- **V1:** `.github/dependabot.yml` validates against the SchemaStore Dependabot schema, run in a
  Docker container, not on the host:
  `docker run --rm -v "$PWD:/w" -w /w python:3.13-slim sh -c "pip install -q check-jsonschema && check-jsonschema --builtin-schema vendor.dependabot .github/dependabot.yml"`.
  No CI gate reads this file (F8), and a key error there stops updates for every ecosystem.
- **V2:** `repo:actionlint` stays green. It does not read `dependabot.yml`; the check is that the
  PR does not disturb it.
- **V3:** `paigasus-kernel-ts:test` passes with the new `REGENERATE` text, and `ts:fmt` accepts it.
- **V4:** the full `moon ci` graph from the root `CLAUDE.md` passes in CI on the PR.
- **V5:** M0 is the evidence for the premise. It is recorded in section 3 and in a comment on
  SMA-683.

## 7. Follow-up

A new Linear issue (project Paigasus Polyglot, milestone CI & Tooling, priority Medium): a
scheduled lockstep updater for the wasm-bindgen family.

- Workflow 1, `schedule` (weekly), `permissions: contents: read`: runs the four-package
  `cargo update -p` and `generate-wasm`, and uploads `rs/Cargo.lock` and the five artifacts as an
  artifact. No write-capable token is in its environment.
- Workflow 2, `workflow_run` on workflow 1: holds the App token and runs no build. It pushes a
  branch and opens a PR only when these hold:
  - the artifact holds exactly the six expected paths;
  - the `Cargo.lock` diff touches only the seven family entries, all crates.io sources;
  - the base is still the SHA that workflow 1 built on.
- The five files come from a build that ran third-party code. The path check does not make their
  content safe; the PR's own CI and a human review of the lock diff are the control.
- `repo:workflow-credentials` covers only the `pull_request`, `pull_request_target` and
  `issue_comment` triggers. The follow-up must extend it to `workflow_run`, or state why not.
