# SMA-683 — a dependabot `wasm-bindgen` bump must not stop the cargo group

**Linear:** SMA-683. **Related:** SMA-634 (the committed wasm glue), SMA-680 (`dependencies_update =
false`). **Status:** design approved in chat on 2026-09-26; written spec awaiting review.

## 1. Problem

The five wasm artifacts under `rs/crates/bindings/paigasus-wasm/` are committed (SMA-634).
`paigasus-kernel-ts:generate-wasm` is their only writer, and it has `runInCI: false`.
`ts/packages/paigasus-kernel/tests/committed-wasm.test.ts` compares them with a fresh build.

A dependabot cargo PR that moves `wasm-bindgen` fails check 1 and check 2 of that test. SMA-680
measured this: 0.2.127 to 0.2.128 changes the `__wbg_Error_*` import hash. A person must then run
`generate-wasm` and push a commit to the dependabot branch.

Since SMA-680, the weekly `cargo-minor-patch` group is the only path for third-party Rust updates.
A `wasm-bindgen` bump in that group therefore stops every other Rust update in the group.

## 2. Acceptance (from the issue)

A dependabot cargo PR that moves `wasm-bindgen` can merge with no manual regeneration, **or** it
arrives separately from the rest of the cargo group with a documented one-step fix.

This spec takes the second branch. Section 7 opens a follow-up issue for the first branch.

## 3. Facts

| # | Fact | Source |
|---|------|--------|
| F1 | `wasm-bindgen = "0.2"` is in `[workspace.dependencies]` of `rs/Cargo.toml:151`. dependabot-core's cargo parser reads `workspace.dependencies` as direct dependencies. So dependabot proposes `wasm-bindgen` bumps itself. | `rs/Cargo.toml`; dependabot-core `cargo/lib/dependabot/cargo/file_parser.rb` (READ, not measured) |
| F2 | By default, dependabot version updates cover direct dependencies only. | docs.github.com, "Controlling dependencies updated" |
| F3 | The lock holds `wasm-bindgen`, `wasm-bindgen-macro`, `wasm-bindgen-macro-support`, `wasm-bindgen-shared` at 0.2.128, `js-sys` and `web-sys` at 0.3.105, and `wasm-bindgen-futures` 0.4.78. `js-sys` and `web-sys` require an exact `wasm-bindgen` version. | `rs/Cargo.lock` (READ 2026-09-26) |
| F4 | These packages depend on `wasm-bindgen` or `js-sys` in the lock: `chrono`, `getrandom` (0.2 and 0.4), `iana-time-zone`, `jsonwebtoken`, `reqwest`, `rust_decimal`, `uuid`, `wasm-streams`, `web-time`, `wasm-bindgen-futures`, `web-sys`. | `rs/Cargo.lock` (READ 2026-09-26) |
| F5 | Dependabot has not yet moved `wasm-bindgen`. Cargo group PRs 284 and 292 changed only `rs/Cargo.lock` and did not touch the `wasm-bindgen` family. Only release PRs moved it (v0.1.0, v0.2.0). | `gh pr diff 284`, `gh pr diff 292`, `git log -S` |
| F6 | When a dependency matches more than one group, it goes into the first group it matches. `exclude-patterns` takes `*` globs. A group can combine `patterns` and `update-types`. | docs.github.com, "Dependabot options reference", `groups` |
| F7 | Open bug dependabot-core#14202: when `patterns` matches, `update-types` can be ignored, so a major update can be suppressed. Open bug #7939: a dependency can appear in two group PRs. | GitHub issues (READ, not measured here) |
| F8 | A lockfile update of a lead dependency writes every transitive move the resolver needs, whatever group the transitive package belongs to. | INFERRED from dependabot-core's group flow and #7939; no primary statement found |
| F9 | After a person pushes a commit to a dependabot branch, dependabot stops its rebases. A commit message with `[dependabot skip]` lets dependabot force-push over that commit. | docs.github.com, "Managing pull requests for dependency updates" |
| F10 | `main` requires strict up-to-date branches. | memory `paigasus-main-strict-checks-dependabot` |

## 4. Design

### 4.1 Dependabot routing — `.github/dependabot.yml`, the `/rs` cargo entry

Add a group `cargo-wasm-bindgen` **before** `cargo-minor-patch`:

```yaml
    groups:
      cargo-wasm-bindgen:
        applies-to: version-updates
        patterns:
          - "wasm-bindgen*"
          - "js-sys"
          - "web-sys"
        update-types:
          - minor
          - patch
      cargo-minor-patch:
        applies-to: version-updates
        update-types:
          - minor
          - patch
        exclude-patterns:
          - "wasm-bindgen*"
          - "js-sys"
          - "web-sys"
```

- The order puts `wasm-bindgen` in `cargo-wasm-bindgen` (F6). The `exclude-patterns` state the same
  rule explicitly, so the result does not depend on file order alone.
- Today only `wasm-bindgen` is a direct dependency (F1, F2). `js-sys` and `web-sys` are in the
  patterns because they move in lockstep with `wasm-bindgen` (F3). If a crate ever declares one of
  them directly, its bump then goes to the right group with no config change.
- A comment block above the group records: why the group exists, the transitive leak (4.4), the
  #14202 caveat (F7), and a pointer to the runbook in `rs/CLAUDE.md`.
- Major updates: `wasm-bindgen = "0.2"` does not allow 0.3, so a 0.3 release is a manifest change.
  It comes as an individual PR, as for every other cargo major. #14202 could suppress it (F7). This
  risk is recorded in the comment; this issue does not fix it.

### 4.2 The one-step fix — runbook in `rs/CLAUDE.md`

`rs/CLAUDE.md` owns the Rust dependency rules, so the runbook goes there. The steps:

```bash
gh pr checkout <N>
moon run paigasus-kernel-ts:generate-wasm
git add rs/crates/bindings/paigasus-wasm/paigasus_wasm*
git commit -m "build(deps): regenerate the committed wasm glue for wasm-bindgen <version>"
git push
```

The runbook also states:

- Run it on ONE host, as the `generate-wasm` comment requires. The binary bytes differ per host
  (SMA-634 F12). The gate compares only the glue and the interfaces.
- After the push, dependabot stops its rebases (F9). Bring the branch up to date with the
  update-branch API, as for any PR on strict `main` (F10).
- Do NOT put `[dependabot skip]` in the commit message. It lets dependabot force-push over the
  commit, and the regenerated glue is then lost.
- If the fix fails with a wasm-bindgen schema mismatch, the pinned `wasm-pack` in `.prototools`
  does not support the new 0.2.z. Bump it in the same PR (`rs/Cargo.toml:144-150`, the invariant).
- The same fix applies to a `cargo-minor-patch` PR when the leak (4.4) moves `wasm-bindgen` there.

### 4.3 The test message — `committed-wasm.test.ts`

Add one sentence to the `REGENERATE` constant: on a dependabot PR, follow the wasm-bindgen runbook
in `rs/CLAUDE.md`. The test logic, the checks and `SURFACE_CHANGED` do not change.

### 4.4 The transitive leak — documented, not fixed

A bump of a lead dependency in `cargo-minor-patch` can need a newer `js-sys` or `wasm-bindgen` (F4).
Then the resolver moves `wasm-bindgen` inside that group PR (F8), and the group PR fails the test.
No group rule can stop this. This issue documents the case in the dependabot.yml comment and in the
runbook. The follow-up (section 7) removes it.

## 5. Out of scope

- Automated regeneration on dependabot PRs (section 7).
- A CI step that detects a `wasm-bindgen` move and prints the fix. Chosen against on 2026-09-26.
- A gate that asserts the dependabot.yml group layout.
- The token scope of the release stamp step (SMA-680 follow-up B).

## 6. Verification

There is no runtime behavior to unit-test. The change is config, prose and one string.

- **V1:** `.github/dependabot.yml` parses as YAML, and the `/rs` entry has the two groups in the
  order of 4.1.
- **V2:** `repo:actionlint` and `repo:workflow-credentials` stay green. `repo:workflow-credentials`
  treats the `.github/` directory as a repository signal, so it must not change its verdict.
- **V3:** `paigasus-kernel-ts:test` passes with the new `REGENERATE` text, and `ts:fmt` accepts it.
- **V4:** the full `moon ci` graph from the root `CLAUDE.md` passes in CI on the PR.
- **Not verifiable before merge:** the dependabot behavior itself. The first Monday run after the
  merge shows it. The follow-up issue records the first `cargo-wasm-bindgen` PR, or its absence, as
  the measurement. GitHub's "Dependabot" insights tab shows a config parse error, if any, after the
  merge.

## 7. Follow-up

A new Linear issue (project Paigasus Polyglot, milestone CI & Tooling, priority Medium): automated
regeneration of the committed wasm glue on dependabot cargo PRs.

- Workflow 1, `pull_request` from dependabot, `permissions: contents: read`: runs `generate-wasm`
  and uploads the five files as an artifact. No write-capable token is in its environment.
- Workflow 2, `workflow_run` on workflow 1: holds the App token, runs no build, checks that the
  artifact holds exactly the five expected paths, then commits and pushes.
- It covers the transitive leak (4.4), so it can remove the `cargo-wasm-bindgen` group.
- It records the first real `cargo-wasm-bindgen` PR as the measurement for section 6.
