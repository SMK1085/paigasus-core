# SMA-716 — release-plz: group-member changelogs and releases with no shipped change

- Linear: [SMA-716](https://linear.app/smaschek/issue/SMA-716)
- Found on: release PR #306 (`chore: release`, 2026-09-27)
- Tool: release-plz 0.3.158 (`.prototools`), git-cliff-core 2.13.1 (its dependency)
- Status: APPROVED at GATE 1 by Sven, 2026-09-27 (revision 2, Q1–Q4 answered as recommended)
- Markers: READ = read in the pinned source. MEASURED = run on this repo. INFERRED = neither.
  Re-read every READ claim when `.prototools` moves the release-plz pin.

## 1. Problem

### P1. A version-group-only bump gets no CHANGELOG section

`paigasus-proto-derive` moves 0.3.0 → 0.4.0 on PR #306 only because it shares
`version_group = "proto"` with `paigasus-proto`. No commit touched its path after
`paigasus-proto-derive-v0.1.0`, except release commits. Its CHANGELOG has no section for
0.1.1, 0.2.0 or 0.3.0, and PR #306 adds none for 0.4.0. The PR body quotes the 0.1.0 section.

### P2. A commit that changes no shipped behaviour cuts a release

`paigasus-kernel` moves 0.1.0 → 0.1.1 on PR #306. The only commit on its path after
`paigasus-kernel-v0.1.0` is `fdf26773` (SMA-685, `fix(ci):`) (MEASURED). It changed three
comment lines in the kernel's `Cargo.toml`. A merge publishes the kernel and its three bindings
to crates.io, PyPI and npm with no code change.

## 2. Root cause (READ)

Line numbers refer to tag `release-plz-v0.3.158` and to `git-cliff-core` 2.13.1.

- **P1.** `Updater::get_version_groups` (`release_plz_core/src/command/update/updater.rs:161-189`)
  takes the maximum next version over the group. `get_next_version` (796-828) returns that
  maximum for every member, also for a member with an empty diff. The member is then updated
  with `diff.commits = []`. `ChangelogBuilder::build` (`changelog.rs:355-410`) always sets
  `previous.commits = []` (382-391). git-cliff's `Changelog::process_releases`
  (`git-cliff-core/src/changelog.rs:143-175`) drops a release with no commits when the previous
  release also has no commits, unless `render_always` is set (166-168). The inline `[changelog]`
  table does not accept `render_always` (`release_plz/src/changelog_config.rs:9-32`). A full
  git-cliff file through `[workspace] changelog_config` can set it (`release_plz/src/config.rs:168-170`,
  `changelog.rs:252`). It does not help this design: with the §4.2 filter on, release-plz skips
  a member with an empty diff before any render. So release-plz writes nothing. The PR body
  comes from `last_changes()` (`next_ver.rs:432-437`). That function parses the FIRST
  `## [x.y.z]` section of the CHANGELOG file, which is still the old 0.1.0 section.
- **P2.** `are_cargo_toml_equal` (`package_compare.rs:178-187`) compares the local `Cargo.toml`
  byte by byte with the `Cargo.toml.orig` of the published package. A comment edit makes them
  differ. The commit then enters `diff.commits` (`updater.rs:698-702`), and `next_from_diff`
  bumps the patch version. `[changelog] commit_parsers` with `skip = true` affect only the
  rendered changelog, never the version (`next_version` crate).

## 3. Decisions

- **D1 (Sven, 2026-09-27).** A group member without its own commits lists the commits of the
  other group members. Mechanism: the package key `changelog_include`. No new CI code writes
  changelogs. Consequence, accepted at GATE 1 (Q4): because R1 (§4.1) is
  symmetric, the section of EVERY member lists the commits of the whole group. This includes
  the `paigasus-proto` section and its GitHub Release body.
- **D2 (Sven, 2026-09-27).** A crate releases only when at least one of its commits is a
  releasing commit. Mechanism: the workspace key `release_commits`. The releasing scopes
  were set at GATE 1 (Q1) to the set in §4.2.

## 4. Design

### 4.1 `changelog_include`, symmetric in every version group

`changelog_include = [<names>]` (`release_plz/src/config.rs:295-308`) appends the named
packages' commits to this package's `Diff.commits` (`updater.rs:241-257`) (READ). Facts:

- The append runs before the group maximum and before the `release_commits` filter. So it
  affects the changelog, the version and the filter.
- It runs only when the INCLUDING package has a published package: a registry entry, or in
  `git_only` mode a tag (`updater.rs:248-257`, `next_ver.rs:122-163`). Both proto crates meet
  this today.
- It reads a snapshot of each package's OWN commits, taken before any include (`updater.rs:241-244`).
  Includes are therefore not transitive, and R1 must name ALL other members.
- `Diff::add_commits` (`diff.rs:95-101`) appends after the package's own commits, and removes
  duplicates by full equality. `sort_commits = "newest"` does not sort them again
  (`changelog.rs:368-374`). A section can therefore show its own commits first, then the
  included ones.

**Rule R1.** Every member of a version group that release-plz processes sets
`changelog_include` to exactly the list of the other processed members of that group. A crate
that is in no group, or is not processed, sets no `changelog_include`. "Processed" mirrors
`packages_to_process()` (`updater.rs:283-302`): Cargo-publishable (the `cargo metadata`
`publish` field is `null` or a non-empty list), or `git_only`.

Today this touches only the proto group:

```toml
[[package]]
name = "paigasus-proto"
version_group = "proto"
release = true
changelog_include = ["paigasus-proto-derive"]

[[package]]
name = "paigasus-proto-derive"
version_group = "proto"
release = true
git_release_enable = false
changelog_include = ["paigasus-proto"]
```

The kernel group has one processed member (`paigasus-kernel`). The three binding crates are
`publish = false`, so release-plz never processes them (SMA-685). R1 needs no key there.

**Why symmetric.** release-plz computes the group maximum from all diffs. It applies the
`release_commits` filter per package, later (`updater.rs:90-97`). A package that fails the
filter goes to `packages_to_check_for_deps`. In the real graph, `paigasus-proto` depends on
`paigasus-proto-derive` with a version requirement (`rs/crates/libs/paigasus-proto/Cargo.toml:42`).
Without R1, a `feat(rs):` commit only on proto-derive gives proto an empty diff. The filter
skips proto, and the dependency cascade then gives proto a PATCH bump with a synthetic `chore:`
entry (`updater.rs:155-157`, `404-436`). The result is derive 0.5.0 and proto 0.4.1: the
lockstep breaks, and `repo:version-lockstep` reds the release PR. With R1, all members have the
same commit set, so the filter gives the same answer for all of them.

**Adding a new group member.** Before its first publish, the new member has no published
package, so its own includes do not apply (see the second fact above). Add it to the other
members' `changelog_include` in the same PR. The R1 check (§4.4) enforces the config shape.

### 4.2 `release_commits` allowlist

commitlint allows a closed scope list (`ts/packages/commitlint-config/index.cjs:42`):
`rs, py, ts, contracts, ci, docs, deps, release, repo, claude, workspace`. It requires a scope on
every commit (`:43`). The squash-merge subject on `main` is the PR title, and nothing lints it:
`main` has unscoped subjects such as `feat: grant gateway_user … (SMA-676) (#315)`, which is the
only releasing commit on the proto paths since `paigasus-proto-v0.3.0` (MEASURED). So the regex
must handle both a scope and no scope.

Because the scope list is closed, the regex is an ALLOWLIST of scopes, not an exclusion. That
avoids look-ahead, which the Rust `regex` crate does not have. The approved regex (Q1):

```toml
release_commits = '^(feat|fix|perf)(\((rs|py|ts|contracts|deps)(,(rs|py|ts|contracts|deps))*\))?!?:|^[a-z]+(\([^)]*\))?!:|(?m:^BREAKING[ -]CHANGE:)'
```

- Releasing scopes: `rs`, `py`, `ts`, `contracts` (code), and `deps` (a dependency fix, for
  example a RUSTSEC floor raise). No scope also releases, because PR titles are not linted.
- Non-releasing scopes: `ci`, `docs`, `release`, `repo`, `claude`, `workspace`.
- A scope list such as `fix(rs,ci)` releases only if EVERY scope in it is releasing. That is
  the conservative reading.
- Any type with `!`, and a `BREAKING CHANGE:` footer, always release.
- `^` without the `m` flag anchors to the start of the message, that is, the subject line.

`Diff::any_commit_matches` (`diff.rs:104-108`) tests the full message. The filter runs in
`update` and `release-pr` (`args/release_pr.rs:8-29`). `release` does not use it; `release`
publishes every local version that is not yet on the registry.

**Classification table.** Each row is one fixture crate in §4.3. Baseline 0.1.0.

| #  | Commits on crate `a` (oldest first)            | Expected `a` | Why                                      |
| -- | ---------------------------------------------- | ------------ | ---------------------------------------- |
| 1  | `fix: x`                                       | 0.1.1        | allowed type, no scope                   |
| 2  | `fix(rs): x`                                   | 0.1.1        | releasing scope                          |
| 3  | `feat(contracts): x`                           | 0.2.0        | releasing scope                          |
| 4  | `perf(rs): x`                                  | 0.1.1        | allowed type                             |
| 5  | `fix(deps): x`                                 | 0.1.1        | releasing scope                          |
| 6  | `fix(py,ts): x`                                | 0.1.1        | every scope releasing                    |
| 7  | `fix(ci): x`                                   | 0.1.0        | non-releasing scope                      |
| 8  | `feat(ci): x`                                  | 0.1.0        | non-releasing scope                      |
| 9  | `perf(ci): x`                                  | 0.1.0        | non-releasing scope                      |
| 10 | `fix(repo): x`                                 | 0.1.0        | non-releasing scope (Q1)                 |
| 11 | `fix(workspace): x`                            | 0.1.0        | non-releasing scope (Q1)                 |
| 12 | `fix(rs,ci): x`                                | 0.1.0        | one scope non-releasing                  |
| 13 | `fix(cid): x`                                  | 0.1.0        | scope not in the list                    |
| 14 | `chore(rs): x`                                 | 0.1.0        | type not allowed                         |
| 15 | `build(deps): x`                               | 0.1.0        | type not allowed (dependabot's form)     |
| 16 | `refactor(rs): x`                              | 0.1.0        | type not allowed                         |
| 17 | `Revert "feat(rs): x (#1)"`                    | 0.1.0        | GitHub revert form (Q3)                  |
| 18 | `fix(ci)!: x`                                  | 0.2.0        | explicit breaking marker                 |
| 19 | `chore(rs): x` + body `BREAKING CHANGE: y`     | 0.2.0        | breaking footer                          |
| 20 | `refactor(rs): x`, then `fix(rs): y`           | 0.1.1        | carry-forward; section lists both        |
| 21 | `feat(ci): x`, then `fix(rs): y`               | 0.2.0        | a non-releasing `feat` still sets the level |
| 22 | comment-only edit of `a/Cargo.toml`, `fix(ci): x` | 0.1.0     | P2 reproduced, filter holds              |
| 23 | comment-only edit of `a/Cargo.toml`, `fix(rs): x` | 0.1.1     | P2 trigger MEASURED on the fixture       |

**Consequences to record** (in `rs/release-plz.toml`, `.github/CLAUDE.md` and CONTRIBUTING.md):

- **C1.** A change to shipped code with a non-releasing type or scope does not open a release.
  It is not lost: the diff walks back to the published state, so the next releasing commit on
  that crate carries it into the release and its section (row 20).
- **C2.** The filter decides only WHETHER a crate releases. The version LEVEL comes from all
  commits in the diff (`version.rs:13-21`). A `feat(ci):` commit on a crate path makes the next
  release a minor bump, which is incompatible in 0.x (row 21). Rule: for a crate-file change that
  must not ship, use `chore`, `ci`, `build`, `docs`, `refactor` or `test`, never `feat(ci)` or
  `fix(ci)`.
- **C3.** A new publishable crate first releases only after a releasing commit touches it. The
  filter runs before the new-package branch (`updater.rs:91-98` against `115-119`).
- **C4.** The dependency cascade bypasses the filter (`updater.rs:155-157`). A dependent of a
  released crate still gets a patch bump.
- **C5.** A dependency floor change needs a `fix(deps)` commit to release. Dependabot writes
  `build(deps)` (`.github/dependabot.yml:23`), which does not release (row 15).

**Effect on PR #306** (MEASURED inputs, INFERRED result). If this change merges before #306,
release-plz regenerates #306. The kernel then has only `fix(ci):` and drops out. The proto family
stays, because of `feat:` (#315), with a correct section for both crates. Rollout check after the
merge: confirm that #306 was force-updated and no longer lists the kernel. release-plz returns
before it reads open PRs when no package needs an update
(`release_plz_core/src/command/release_pr/mod.rs:152-175`), so a stale release PR can stay
open. Close a stale one by hand; never merge it. Rollback: remove the two keys.

### 4.3 Proof: a release-plz-only fixture suite

The shared `ci/release-parity/cases.tsv` is a cross-tool parity contract. python-semantic-release
and semantic-release have no `release_commits` equivalent. The new cases do NOT go into it, and
the `cases.tsv` fixture does NOT get `release_commits`: its commits are unscoped `fix:`/`feat:`,
and it tests classification, not the filter.

**Location.** A new file `ci/release-parity/ecosystems/release-plz-filter.sh`, called only from
`ecosystems/release-plz.sh` through a new hook `ecosystem::extra_suite`. `run.sh` calls the hook
if the ecosystem defines it; the `python-semantic-release` and `semantic-release` modules do not,
so `repo:release-parity-py` and `-ts` skip the suite.

**Config derivation (F3 kept).** The suite's fixture `release-plz.toml` takes `release_commits`
verbatim from the real `rs/release-plz.toml`. It fails with rc 2 if the key is missing.

**Filter fixture.** One fixture repo with one crate per row of the §4.2 table (crates `r01` … `r23`),
no group and no dependency edges, and a `b` crate that no commit touches. Each crate is tagged at
0.1.0, then gets its row's commits. One `release-plz update` run covers all rows. Each crate sees
only its own commits. The suite asserts each crate's version, and that `b` stays at 0.1.0. Row 20
also asserts that the crate's first `## [` section lists both commits.

**Group fixture.** A second fixture repo with crates `g1` and `g2`, in one `version_group`, with R1's
symmetric `changelog_include`, and `g1` depending on `g2` by path and version (the real proto → derive
shape). Before the 0.1.0 tag, each crate gets a seeded `CHANGELOG.md` with release-plz's header,
`## [Unreleased]` and a `## [0.1.0]` section, the same shape as the real crates. So release-plz takes
the `prepend` path, where P1 happened (`updater.rs:1026-1029`, `changelog.rs:67-89`). Cases:

- **G1.** `fix(rs): x` on `g1` only. Both reach 0.1.1. `g2`'s first `## [` heading is `0.1.1`, and
  the line `- *(rs)* x` is under `### Fixed` in that section (`changelog.rs:494-505`).
- **G2.** `feat(rs): x` on `g2` only (the follower direction). Both reach 0.2.0.
- **G3.** `fix(ci): x` on `g1` only. Both stay at 0.1.0.
- **G4.** One `fix(rs): x` commit that touches both crates. The line appears exactly once in each
  crate's new section.

Measure first: SMA-658 M7 saw a `git_only` hard error with an unpublished workspace dependency
(`.github/CLAUDE.md`). If the path+version dependency errors in `git_only` mode, the plan records
the measurement and drops the edge; G2's prediction then changes as §4.1 describes.

**Negative controls.** Each control mutates the fixture config and must make the named assertion
fail with rc 1. An rc 2 is INCONCLUSIVE and fails the control.

- NC1: no `release_commits`. Row 7 bumps to 0.1.1.
- NC2: no `changelog_include`, `release_commits` kept. G2 red (the lockstep breaks).
- NC3: no `changelog_include` AND no `release_commits` (P1 reproduced exactly). G1's versions pass,
  and `g2`'s first section stays `0.1.0`. The control requires the CHANGELOG half to red while the
  version half passes.

`run.sh --negative-control` today runs one case and exits (`run.sh:68-77`), and
`RELEASE_PARITY_SH_CALL_SITES` pins five of its lines (`ci/affected-graph/ci_targets.py:1119-1125`).
The plan adds an `ecosystem::extra_negative_control` hook next to it, and adds the new control lines
to that pin list and to the README's limitation list, so a deletion reds `repo:affected-smoke`.

**Cost.** Two fixture runs (filter and group) plus three controls, each one `release-plz update`.
The plan measures one run first and records a time budget in the README.

### 4.4 R1 config check in `repo:publish-metadata`

The R1 check needs the Cargo manifests. `repo:release-parity` does not list them as inputs
(`moon.yml:91-95`), so a PR that changes a crate's `publish` value would replay a cached pass. The
check therefore goes into `metadata_checks` in `ci/publish-metadata/run.sh:146-190`:

- That function is already a pure function of the `cargo metadata` JSON and `rs/release-plz.toml`,
  in python3 with `tomllib`.
- It already has `is_publishable` with Cargo's own semantics (`:183-187`).
- `repo:publish-metadata` already lists `rs/crates/**/*` and `rs/release-plz.toml` as inputs
  (`moon.yml:598-611`), and has a fixture-driven negative control.

The check computes the processed set (publishable, or `git_only` in release-plz.toml). For each
`version_group`, it fails if a processed member's `changelog_include` is not exactly the other
processed members. It also fails if any other `[[package]]` sets `changelog_include`. The message
names the group, the package, and the missing, extra or unknown names. Negative-control fixtures:
a missing name, an extra name, a misspelt name, and a `changelog_include` outside a group.

### 4.5 Documentation

- `rs/release-plz.toml`: comments for both keys, with the §2 and §4 citations and the READ marker.
- `.github/CLAUDE.md`: R1, the releasing-commit rule, and C1–C5.
- `CONTRIBUTING.md`: the releasing-commit rule and C2, in the commit-convention section
  (next to the scope list at lines 117-119).
- `ci/release-parity/README.md`: the release-plz-only suite, why it is outside `cases.tsv`, and the
  new pinned control lines.

## 5. Out of scope

- Back-filling the missing 0.1.1, 0.2.0 and 0.3.0 sections of the proto-derive CHANGELOG.
  Follow-up issue, if wanted.
- A commitlint rule on PR titles.
- The merge order of PR #306. §4.2 describes the effect.

## 6. Acceptance (from SMA-716, mapped)

| Acceptance item                                              | Where                     |
| ------------------------------------------------------------ | ------------------------- |
| Every bumped group member gets a section for that version    | §4.1, G1, G2, NC3         |
| The release PR body shows the new section                    | §2 (`last_changes()` reads the first section), G1 |
| Decide and record the policy for no-behaviour changes        | §3 D2, §4.2, §4.5         |
| A test proves the fix on a fixture                           | §4.3, §4.4                |

The SMA-716 text asks that the section "says that the version follows the group head". D1
replaces that wording: the section lists the group's commits. SMA-716 is updated to match.

## 7. Risks

- **Regex mistakes.** One fixture crate per table row, and NC1.
- **A pin bump changes the filter or the include order.** The suites run the real pinned binary,
  and `.prototools` is a task input.
- **A new group member without R1.** The §4.4 check reds it.
- **A new commitlint scope.** It does not release until someone adds it to the regex. Recorded in
  the CONTRIBUTING.md rule next to the scope list.

## 8. Decisions taken at GATE 1 (Sven, 2026-09-27: "go with your recommendations")

- **Q1.** Releasing scopes. Recommended: `rs, py, ts, contracts, deps`, plus no scope. Not
  releasing: `ci, docs, release, repo, claude, workspace`. Note: `fix(repo):` SMA-528 (#145) is in
  the proto-derive 0.1.0 section; it was CI work.
- **Q2.** Hide non-releasing commits from the CHANGELOG with `commit_parsers` `skip = true`?
  Without it, the next kernel section lists `fdf26773` under "Fixed". Recommended: no. The list
  shows what changed in the packaged files, and C1 depends on it.
- **Q3.** Should a GitHub `Revert "…"` of a shipped `feat` or `fix` start a release? Recommended:
  no. The author writes a `fix(rs):` commit when the revert must ship.
- **Q4.** D1 consequence: each member's section, including `paigasus-proto` and its GitHub
  Release body, lists the whole group's commits. Recommended: accept.
