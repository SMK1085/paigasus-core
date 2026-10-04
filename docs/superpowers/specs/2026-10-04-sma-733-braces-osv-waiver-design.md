# SMA-733 — waive GHSA-vfj7-8cjw-p6xm (braces 3.0.3) with a shipped-path guard

Status: revised after the spec challenge, 2026-10-04. Linear: SMA-733. Related: SMA-693
(PR 363, blocked by this).

## 1. Problem

The advisory GHSA-vfj7-8cjw-p6xm (CVE-2026-93687, severity 8.7) affects `braces@3.0.3` in
`ts/pnpm-lock.yaml`. Its text: "braces vulnerable to stack-exhaustion denial of service
through deeply nested patterns". `repo:osv` fails on it. Thus every PR that runs the gate
fails, and the daily `osv advisory scan` job on `main` fails too.

## 2. Measured facts

All measured on 2026-10-04 in this worktree at `020a6a33`, with osv-scanner 2.5.0. F12 to
F15 come from the spec challenger's code reading. The plan re-checks them.

| # | Fact | Evidence |
|---|---|---|
| F1 | No fixed version exists. osv.dev records `introduced=0`, `last_affected=3.0.3`. The newest `braces` on npm is 3.0.3. | Linear issue; `pnpm view braces version` |
| F2 | One `braces` version is in the lock file. Only `micromatch@4.0.8` reaches it. | `pnpm why braces -r`; lock lines 10726-10728 |
| F3 | Only the root importer `.` (`@paigasus/workspace`) reaches `micromatch`, and only through `devDependencies`: `@next/eslint-plugin-next@16.3.6 -> fast-glob@3.3.1`, `semantic-release@25.0.9`, `@semantic-release/commit-analyzer@13.0.1`. | `pnpm why braces -r` |
| F4 | `pnpm` finds no production path. | `pnpm why braces -r --prod` prints nothing |
| F5 | No upgrade removes the path. The newest `micromatch` (4.0.8), `fast-glob` (3.3.3), `@next/eslint-plugin-next` (16.3.8, pins `fast-glob` 3.3.1), `semantic-release` (25.0.9) and `@semantic-release/commit-analyzer` (13.0.1) all still reach `braces`. | `pnpm view <pkg> version dependencies` |
| F6 | `ts/Dockerfile` installs with `--filter "@paigasus/${APP}..."`. This installs the dev and production dependencies of the app and of its workspace dependencies, but not of the root importer. The image ships only `.next/standalone` and `.next/static`. The standalone trees on disk have no `braces`, `micromatch` or `fast-glob` directory. `next@16.3.6` depends on none of the three. A bundler can inline code without a directory, so the plan also greps the build output for `braces` module IDs. | `ts/Dockerfile:43`; `find ts/apps/*/.next/standalone` |
| F7 | `py/uv.lock` contains no `braces`. | `grep -c braces py/uv.lock` = 0 |
| F8 | `ignoreUntil` must be a bare TOML date. `ignoreUntil = 2027-01-04` filters the finding (rc 0). `ignoreUntil = 2026-01-01` (past) does not filter it (rc 1, "unused ignores"). `ignoreUntil = "2027-01-04"` (quoted) is a config error (rc 127). | scratch probe |
| F9 | osv-scanner 2.5.0 gives no dev or production marker for a pnpm v9 lock file, also with `--all-packages --format json`. | scratch probe |
| F10 | The `osv advisory scan` job installs only osv-scanner. It has no node, pnpm or uv. It runs `ci/osv/run.sh` with the runner's `python3`. | `.github/workflows/security-scan.yml:46-72` |
| F11 | The repo does not rely on a system PyYAML. `ci/helm-render` and `ci/workflow-credentials` get PyYAML from their own uv projects. | `ci/helm-render/pyproject.toml:11`, `ci/workflow-credentials/pyproject.toml:13` |
| F12 | `micromatch` 4.0.8 calls `braces` only in `parse`, `braces` and `braceExpand`. `fast-glob` expands only the patterns it gets. The Next ESLint plugin globs `settings.next.rootDir`, which the repo's own ESLint config sets. | challenger; `micromatch/index.js:427,456,465`, `fast-glob/out/utils/pattern.js:137`, `@next/eslint-plugin-next` `get-root-dirs.js:15` |
| F13 | No workflow runs semantic-release. Pull-request workflows have no write credentials. | challenger; `.github/workflows/` |
| F14 | `run.sh` sets `-uo pipefail` and not `-e`. An unexpected exit code from a sub-command does not stop it. | `ci/osv/run.sh:29` |
| F15 | PR 363 changes `ci/osv/run.sh`, but only the `LOCKFILES` array (it adds `ci/wasm-lockstep/uv.lock`). PR 363 does not change `ts/pnpm-lock.yaml`. | `gh pr diff 363` |
| F16 | The local `python3` is Homebrew 3.14.7. `/usr/bin/python3` is 3.9.6. | `python3 --version` |

Not measured: the Linux runner's Python version and modules.

## 3. Decision

Waive the advisory, and add a guard that keeps the waiver's reason true.

- Removing the path (issue option 1) is not possible (F5). A pnpm override to a
  braces-free `micromatch` replacement would change the glob behaviour of three tools and
  add a new supply-chain dependency. We reject it.
- A waiver applies to the advisory ID for every path. Without a guard, a later shipped path
  to `braces` would stay silent until the waiver expires. The guard closes that gap.
- Sven chose the expiry (90 days) and the guard on 2026-10-04.

Approaches for the guard:

- **A (chosen): a Python lock-file walker that uses only the standard library**, in
  `ci/osv/`, called from `run.sh`. It works in both places where the gate runs (F10). It
  needs no new tool.
- B: the same walker with PyYAML, through a uv project in `ci/osv/`. The parse is more
  reliable, but the scan job must install uv, and osv must scan another lock file.
  Rejected for that cost.
- C: `pnpm why --prod`. It needs `node_modules`, and the scan job has no node (F10).
  Rejected.

## 4. Design

### 4.1 The waiver: `osv-scanner.toml`

Add one entry:

```toml
[[IgnoredVulns]]
id = "GHSA-vfj7-8cjw-p6xm"
ignoreUntil = 2027-01-02
reason = "<see below>"
```

- `ignoreUntil` is a bare TOML date (F8). 2027-01-02 is 90 days after 2026-10-04.
- Put a comment block above the entry, in the style of `rs/deny.toml`'s RUSTSEC-2023-0071
  entry. It gives:
  - the CVE alias and the advisory text;
  - F1 (no fix) and F5 (no upgrade removes the path);
  - F3 and F6 (only root dev tooling reaches it; the images do not contain it);
  - the threat model, from F12 and F13. The tools expand only glob patterns that are
    written in the repo's own config. A fork PR can change those patterns. Then the worst
    result is a stack exhaustion that stops that PR's own CI job, which has no write
    credentials. No release job runs `braces` on untrusted input;
  - the guard's name, `ci/osv/shipped_reachability.py`, and the key SMA-733.
- `reason` is one line, because osv-scanner prints it in its output. Text:
  "No fixed braces release. Only root dev tooling (ESLint Next plugin, semantic-release)
  reaches it, on repo-authored glob patterns; no shipped package does, and
  ci/osv/shipped_reachability.py enforces that (SMA-733)."
- Change the header line "There are currently no waivers. Every finding to date has been
  fixed by upgrading." so that it names this waiver as the first one.
- Add one rule to the header: when a waiver's reason depends on "no shipped package
  reaches it", the guard's `SHIPPED_FREE_WAIVERS` table must name that advisory.

When the waiver expires, `repo:osv` fails again (F8). This affects the daily scan and every
PR that changes a lock file. The person who triages it checks for a fixed `braces` or a
braces-free path. They then remove the waiver, or renew it with a new date.

### 4.2 The guard: `ci/osv/shipped_reachability.py`

One file, Python 3 standard library only, with the SPDX header. It must run on Python 3.9
or later (F16). At start, it checks `sys.version_info >= (3, 9)`, else it exits 2.

**Table.** A module constant maps each such waiver to the npm package it waives:

```python
SHIPPED_FREE_WAIVERS = {"GHSA-vfj7-8cjw-p6xm": "braces"}
```

**Inputs.** `--lockfile PATH` (default `ts/pnpm-lock.yaml`) and `--config PATH` (default
`osv-scanner.toml`).

**Shipped roots.** A shipped importer is an importer whose key starts with `apps/`, plus
every importer that such an importer reaches through a `link:` entry, transitively. The
root importer `.` is never shipped. For each shipped importer, the roots are its
`dependencies`, `optionalDependencies` and `devDependencies`. Dev dependencies count,
because the Dockerfile installs them (F6) and a bundler can inline anything that app code
imports.

**Parse rules.** A line parser for pnpm lock file format 9.0.

- A top-level section starts with a key at indent 0 and ends at the next key at indent 0.
  The parser reads only `lockfileVersion`, `importers:` and `snapshots:`. It skips every
  other section, which includes `settings:`, `catalogs:`, `overrides:` and `packages:`.
- Remove one pair of `'` or `"` quotes from every key and every value.
- `importers:`: an importer key is at indent 2. `dependencies:`, `optionalDependencies:`
  and `devDependencies:` are at indent 4. A package name is at indent 6. Its `version:` is
  at indent 8. The parser ignores `specifier:` and any other key at indent 8.
- `snapshots:`: a snapshot key is at indent 2, with a trailing `:`. An inline `{}` means no
  edges. Under a key, only the blocks `dependencies:` and `optionalDependencies:` at
  indent 4 give edges, with `name: version` at indent 6. Any other block at indent 4 ends
  the edge block: for example `transitivePeerDependencies:` (a list of `- name` items) or
  `optional: true`.
- Map an entry `name: version` to a node key:
  - A `link:` value goes to the importer at that path, resolved relative to the current
    importer. It is not a snapshot.
  - A `file:` value maps to the snapshot key `name@file:...`.
  - A value is an alias when `@` appears after index 0 in the part before the first `(`
    AND before any `://`. A URL value such as `git+ssh://git@host/...` is not an alias.
    The node key of an alias is the value itself (`realname@version...`).
  - Else the node key is `name@version`, with the full value, peer suffixes included.
- The package name of a snapshot key is the part before the first `(`, cut at the first `@`
  after index 0.

**Walks.** The guard does two breadth-first walks over the snapshot edges. It records the
parent of each node, so that a hit can print its full path.

1. The shipped walk: from the shipped roots.
2. The control walk: from the roots of every importer, `.` included, with all three
   dependency kinds.

**Checks, in this order.**

1. Infrastructure checks. Any failure exits 2 with a reason token in the first output
   line:
   - `python-too-old`, `no-lockfile`, `no-config`;
   - `lockfile-version`: `lockfileVersion` is not `9.0`;
   - `empty-section`: zero importers or zero snapshots;
   - `no-shipped-importer`: no importer key starts with `apps/`;
   - `dangling-edge`: an edge or a root points to a snapshot key or an importer that does
     not exist;
   - `table-stale`: a table ID has no `id = "<ID>"` line in the config. The guard reads
     the config as text; it does not need a TOML parser.
2. Edge control. For each table entry, the control walk must reach the package through a
   path of at least two edges. If it does not, exit 2 with `edge-control`. This proves that
   the walk follows snapshot edges, quoted keys and peer suffixes. A direct root does not
   prove that. It also fails when the waived package leaves the lock file. Then the waiver
   is stale, and the message says to remove it.
3. Shipped check. If the shipped walk reaches a waived package, exit 3 with `shipped-path`.
   Print the GHSA ID, the package and the full path, for example
   `apps/iam-console > x@1.0.0 > micromatch@4.0.8 > braces@3.0.3`. Then print this
   instruction: remove the waiver and fix the path, or rewrite the waiver's reason.
4. Otherwise exit 0. Print one line per table entry, for example
   `osv shipped-guard: braces (GHSA-vfj7-8cjw-p6xm) reached only from dev tooling, not from a shipped importer`.

A Python traceback exits 1. Because "reachable" is 3 and not 1, a crash can never read as a
finding or as a pass. `run.sh` maps the code (section 4.3).

**Self-test.** `--self-test` runs the fixtures in `ci/osv/fixtures/`. Each fixture names
its expected exit code and, for 2 and 3, its expected reason token. `--self-test` exits 0
when every fixture matches. Else it exits 2 and names each fixture that did not match.

| Fixture | Shape it covers | Expected |
|---|---|---|
| `clean.yaml` | `braces` only under `.`'s dev dependencies, through two edges | 0 |
| `shipped-prod.yaml` | an app's production dependency reaches `braces` through two edges | 3 `shipped-path` |
| `shipped-dev.yaml` | an app's dev dependency reaches `braces` | 3 `shipped-path` |
| `shipped-link.yaml` | an app links a workspace package, whose production dependency reaches `braces` | 3 `shipped-path` |
| `shipped-optional.yaml` | the only path uses an `optionalDependencies` block in a snapshot | 3 `shipped-path` |
| `shipped-alias.yaml` | the only path uses an alias value (`name: realname@1.0.0`) | 3 `shipped-path` |
| `shipped-url.yaml` | the only path uses a URL value that contains `@` (`git+ssh://git@host/...`) | 3 `shipped-path` |
| `block-end.yaml` | a `transitivePeerDependencies:` list with quoted items, and an `optional: true` snapshot, sit between edge blocks; `braces` must not be reached through them | 0 |
| `catalogs.yaml` | a `catalogs:` block with importer-like `version:` lines that name `braces` | 0 |
| `file-dep.yaml` | a `file:` dependency on the path | 3 `shipped-path` |
| `bad-version.yaml` | `lockfileVersion: '6.0'` | 2 `lockfile-version` |
| `dangling.yaml` | an edge to a missing snapshot key | 2 `dangling-edge` |
| `no-edges.yaml` | `braces` is a direct root dependency of `.` only | 2 `edge-control` |
| `table-stale` case | the fixture config has no `id` line for the table ID | 2 `table-stale` |

Every fixture uses the real names, quoting and peer suffixes of `ts/pnpm-lock.yaml`. Each
has a scoped `@` name and a `(peer)` suffix, so that the fixtures do not test only the easy
format. Each fixture has its own small config file, or uses one shared fixture config.

### 4.3 Wiring: `ci/osv/run.sh`

- After the package-count control, run the guard twice: first with `--self-test`, then on
  the real files. Store each exit code.
- Map each code. 0 means pass. 3 means a guard finding (`guard=1`). Any other code (1, 2,
  126, 127, a signal) means an infrastructure error (`guard=2`). So a missing `python3`
  (127) or a traceback (1) can never read as clean or as a finding.
- Always run the findings block, also when the guard failed, so that a new, unrelated osv
  finding still prints.
- Exit with this precedence: 2 if the guard is 2. Else 1 if osv found something or the
  guard is 1. Else 0. The script prints a one-line summary of both results before it
  exits.
- Update the header comment to describe the guard and the code map.
- `moon.yml`'s `repo:osv` inputs already contain `ci/osv/**/*` and `ts/pnpm-lock.yaml`. No
  change. The scan job already runs `run.sh` (F10). No workflow change.
- PR 363 changes only the `LOCKFILES` array of this file (F15). This change is in other
  lines, so the merge into PR 363 is expected to be clean. The plan checks it.

### 4.4 Documents

- `ci/CLAUDE.md` line 59 (the osv policy): add that a waiver whose reason depends on "no
  shipped package reaches it" needs a `SHIPPED_FREE_WAIVERS` entry.
- No `ci/osv/README.md` exists. The detail stays in the `run.sh` header and the script's
  docstring, as for the rest of this gate today.

### 4.5 Known residual

Nothing reds if someone deletes the guard call from `run.sh` (mutation M1 shows this).
Some gates pin their own call sites through `repo:affected-smoke`. We do not do that here:
it changes a second gate, which needs bash 3.2 locally, for a guard that lives only as long
as the waiver. The `run.sh` header and the waiver comment name this residual. If Sven wants
the pin, it is a separate issue.

## 5. Verification

1. Prototype first. Before any test is written, run a first version of the guard on the
   real lock file. It must give 0. This checks the claim that the guard passes today; F4
   used `pnpm why`, not the guard.
2. `bash ci/osv/run.sh` exits 0. It prints the four package counts, the guard line and "no
   known vulnerabilities".
3. Remove the waiver entry temporarily. `run.sh` exits 1 and names GHSA-vfj7-8cjw-p6xm. The
   guard then exits 2 with `table-stale`, so `run.sh` exits 2. Both results are printed.
4. `python3 ci/osv/shipped_reachability.py --self-test` exits 0, under the Homebrew
   `python3` and under `/usr/bin/python3` 3.9.6.
5. Mutations. Each must give the stated result. Restore each one by reverting the marked
   change only, not with `git checkout --` (memory: a mutation restore discards the fix).
   - M1 (the call site): point `run.sh`'s guard call at `fixtures/shipped-prod.yaml`.
     `run.sh` exits 1. Then delete the call. `run.sh` exits 0. This records the residual of
     section 4.5.
   - M2: skip `devDependencies` of shipped importers. `shipped-dev.yaml` fails.
   - M3: drop every snapshot edge. The real run exits 2 with `edge-control`.
   - M4: remove the dangling-edge check. `dangling.yaml` fails.
   - M5: ignore `optionalDependencies` blocks. `shipped-optional.yaml` fails.
   - M6: treat an alias value as `name@value`. `shipped-alias.yaml` fails.
   - M7: run `run.sh` with a `PATH` that has no `python3`. `run.sh` exits 2.
   - M8: make the guard raise an exception. `run.sh` exits 2, not 1 and not 0.
6. Run `ci/osv/run.sh` under system `/bin/bash` 3.2 and Homebrew bash 5. The script uses no
   `mapfile` or `declare -A`.
7. The full `moon ci` target list from the root `CLAUDE.md`, with `--base origin/main`.
8. Build both console images, or use the existing `.next` output, and grep
   `.next/standalone` and `.next/static` for `braces`. Record the result in the PR body.
9. On the PR: `CI` and `osv advisory scan` pass. The PR touches `osv-scanner.toml`, so the
   scan job runs.

## 6. Acceptance criteria, mapped

| Linear criterion | Where |
|---|---|
| `repo:osv` and `osv advisory scan` pass on `main` | §5 steps 2, 7, 9; §7 dispatch run after merge |
| The waiver names the GHSA ID, has `ignoreUntil`, and gives a reason | §4.1 |
| The PR states that no shipped artifact contains `braces` | the PR body lists each artifact: the two console images (F6, §5 step 8); `@paigasus/node-bindings`, its platform packages and `@paigasus/wasm`, which have no npm dependencies (the plan re-checks this); the Python wheels and `py/uv.lock` (F7); the Rust crates and images, which have no npm content |
| Merge `main` into PR 363 after this lands | §7 |

## 7. Follow-up after merge

- Start the `security-scan` workflow on `main` with `workflow_dispatch`. It must pass.
- Merge `main` into PR 363 (SMA-693), so that its CI can go green. Before that, run the
  guard against PR 363's lock file. PR 363 does not change it (F15), so this is a check.
- Make a Linear issue with a due date of 2026-12-19, two weeks before the expiry: "Re-check
  GHSA-vfj7-8cjw-p6xm (braces) before the osv waiver expires on 2027-01-02".

## 8. Out of scope

- A guard for pip waivers. No pip waiver exists, and `uv.lock` has a different format.
- A check that each `[[IgnoredVulns]]` entry has an `ignoreUntil`.
- A pin of the guard's call site (section 4.5).
- A replacement for `micromatch` or `fast-glob`.

## 9. Spec challenge record

The challenger's verdict was APPROVE WITH CHANGES.

Applied:

- B1, exit codes: the guard uses 3 for a finding, and `run.sh` maps every other non-zero
  code to 2 (§4.2, §4.3).
- B2, the positive control did not test edges: it is replaced by the edge control, which
  needs a path of at least two edges (§4.2 check 2).
- M, dev dependencies can ship: the roots are now the shipped importers' three dependency
  kinds (§4.2).
- M, fixture coverage: the fixture table now has alias, optional, block-end, catalogs,
  `file:` and stale-table cases, and each checks a reason token (§4.2).
- M, a guard red hid osv findings: the findings block always runs (§4.3).
- M, an unmeasured claim in the reason: F12 and F13 and the threat model (§4.1).
- Minor items: the Python 3.9 floor, explicit parse rules, the stale-table check, the
  prototype step, the artifact list, the dispatch run, and the expiry reminder.

Not applied:

- M, pin the call site: recorded as a residual instead (§4.5). Reason: it changes a second
  gate for a guard with a 90-day life. Sven can ask for the pin.
- Q, PR 363's lock file: answered by F15. No design change.

Found later:

- The PR review (CodeRabbit) found that a URL-valued version with `@` was read as an alias,
  and that the package name was cut at the last `@`. Both are fixed (§4.2), and
  `shipped-url.yaml` covers them.
