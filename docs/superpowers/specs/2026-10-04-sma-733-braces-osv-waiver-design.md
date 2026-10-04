# SMA-733 — waive GHSA-vfj7-8cjw-p6xm (braces 3.0.3) with a production-path guard

Status: draft, 2026-10-04. Linear: SMA-733. Related: SMA-693 (PR 363, blocked by this).

## 1. Problem

The advisory GHSA-vfj7-8cjw-p6xm (CVE-2026-93687, severity 8.7) affects `braces@3.0.3` in
`ts/pnpm-lock.yaml`. Its text: "braces vulnerable to stack-exhaustion denial of service
through deeply nested patterns". `repo:osv` fails on it. Thus every PR that runs the gate
fails, and the daily `osv advisory scan` job on `main` fails too.

## 2. Measured facts

All measured on 2026-10-04 in this worktree at `020a6a33`, with osv-scanner 2.5.0.

| # | Fact | Evidence |
|---|---|---|
| F1 | No fixed version exists. osv.dev records `introduced=0`, `last_affected=3.0.3`. The newest `braces` on npm is 3.0.3. | Linear issue; `pnpm view braces version` |
| F2 | One `braces` version is in the lock file. Only `micromatch@4.0.8` reaches it. | `pnpm why braces -r`; lock lines 10726-10728 |
| F3 | Only the root importer `.` (`@paigasus/workspace`) reaches `micromatch`, and only through `devDependencies`: `@next/eslint-plugin-next@16.3.6 -> fast-glob@3.3.1`, `semantic-release@25.0.9`, `@semantic-release/commit-analyzer@13.0.1`. | `pnpm why braces -r` |
| F4 | No production path exists. | `pnpm why braces -r --prod` prints nothing |
| F5 | No upgrade removes the path. The newest `micromatch` (4.0.8), `fast-glob` (3.3.3), `@next/eslint-plugin-next` (16.3.8, pins `fast-glob` 3.3.1), `semantic-release` (25.0.9) and `@semantic-release/commit-analyzer` (13.0.1) all still reach `braces`. | `pnpm view <pkg> version dependencies` |
| F6 | The console images do not contain `braces`. `ts/Dockerfile` installs with `--filter "@paigasus/${APP}..."`, which does not include the root importer. It ships only `.next/standalone`. The built standalone trees on disk contain no `braces`, `micromatch` or `fast-glob` directory. `next@16.3.6` has no dependency on any of the three. | `ts/Dockerfile`; `find ts/apps/*/.next/standalone` |
| F7 | `py/uv.lock` contains no `braces`. | `grep -c braces py/uv.lock` = 0 |
| F8 | `ignoreUntil` must be a bare TOML date. `ignoreUntil = 2027-01-04` filters the finding (rc 0). `ignoreUntil = 2026-01-01` (past) does not filter it (rc 1, "unused ignores"). `ignoreUntil = "2027-01-04"` (quoted) is a config error (rc 127). | scratch probe |
| F9 | osv-scanner 2.5.0 gives no dev or production marker for a pnpm v9 lock file, also with `--all-packages --format json`. | scratch probe |
| F10 | The `osv advisory scan` job (`.github/workflows/security-scan.yml`) installs only osv-scanner. It has no node, pnpm or uv. It runs `ci/osv/run.sh` and has the runner's `python3`. | workflow lines 46-72 |
| F11 | The repo does not rely on a system PyYAML: `ci/helm-render` and `ci/workflow-credentials` get PyYAML from their own uv projects. Not measured on the runner. | `ci/*/pyproject.toml` |

Not measured: the advisory's vulnerable function in detail, and the Linux runner's Python
modules.

## 3. Decision

Waive the advisory, and add a guard that keeps the waiver's reason true.

- Removing the path (issue option 1) is not possible (F5). A pnpm override to a
  braces-free `micromatch` replacement would change the glob behaviour of three tools and
  add a new supply-chain dependency. We reject it.
- A waiver applies to the advisory ID for every path. Without a guard, a later production
  path to `braces` would stay silent until the waiver expires. The guard closes that gap.
- Sven chose the expiry (90 days) and the guard on 2026-10-04.

Approaches for the guard:

- **A (chosen): a Python lock-file walker that uses only the standard library**, in
  `ci/osv/`, called from `run.sh`. It works in both places where the gate runs (F10). It
  needs no new tool.
- B: the same walker with PyYAML, through a uv project in `ci/osv/`. The parse is more
  reliable, but the scan job must install uv, and osv must scan a fifth lock file. Rejected
  for that cost.
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
  entry. It gives the CVE alias, F1, F3, F4, F6, the guard's name, and the SMA-733 key.
- `reason` is one line, because osv-scanner prints it in its output. Text:
  "braces 3.0.3 has no fixed release. Only root dev tooling reaches it (ESLint Next plugin,
  semantic-release), and it expands repo-authored glob patterns only. No production path
  exists; ci/osv/prod_reachability.py enforces this (SMA-733)."
- Change the header line "There are currently no waivers. Every finding to date has been
  fixed by upgrading." so that it names this waiver as the first one.
- Add one rule to the header: a waiver whose reason says "dev only" must have an entry in
  the guard's table.

When the waiver expires, `repo:osv` fails again (F8). The person who triages it checks for
a fixed `braces` or a braces-free path. They then remove the waiver, or renew it with a new
date and the same guard entry.

### 4.2 The guard: `ci/osv/prod_reachability.py`

One file, Python 3 standard library only, with the SPDX header.

**Table.** A module constant maps each dev-only waiver to the npm package it waives:

```python
DEV_ONLY_WAIVERS = {"GHSA-vfj7-8cjw-p6xm": "braces"}
```

**Input.** A path to a pnpm lock file. The default is `ts/pnpm-lock.yaml`.

**Parse.** A line parser for pnpm lock file format 9.0. It reads two sections:

- `importers:`. Each importer is a key at indent 2. Under it, `dependencies:`,
  `optionalDependencies:` and `devDependencies:` are at indent 4, each package name is at
  indent 6, and its `version:` is at indent 8.
- `snapshots:`. Each snapshot key is at indent 2 (`name@version(peers):`, quoted when the
  name starts with `@`). Its `dependencies:` and `optionalDependencies:` are at indent 4,
  with `name: version` entries at indent 6. A key with `{}` has no dependencies.

**Walk.**

- Roots: every importer's `dependencies` and `optionalDependencies`. Never
  `devDependencies`.
- This over-approximates "shipped": a workspace package that is used only as a dev
  dependency still counts as a root. That is the safe direction. F4 shows that it passes
  today.
- A `link:` version is a workspace package. Skip it, because that importer is a root
  already.
- Map an entry `name: version` to the snapshot key `name@version`. An alias value that is
  itself `realname@version` maps to that key directly.
- Follow edges breadth-first. Record the parent of each node, so that a hit can print its
  full path.

**Exit codes.** These follow `run.sh`'s convention.

- **0**: no waived package is reachable from a root. Print one line per table entry, for
  example `osv prod-guard: braces (GHSA-vfj7-8cjw-p6xm) has no production path`.
- **1**: a waived package is reachable. Print the GHSA ID, the package and the path
  (`apps/iam-console > x@1 > micromatch@4.0.8 > braces@3.0.3`). Then print this
  instruction: remove the waiver and fix the path, or rewrite the waiver's reason.
- **2**: infrastructure error, which fails closed. The cases:
  - The file is missing or unreadable.
  - `lockfileVersion` is not `'9.0'`.
  - The parser finds zero importers or zero snapshots.
  - An edge points to a snapshot key that does not exist.
  - The positive control fails: the package `next` is not reachable from a root.

The positive control proves that the walk reaches real production packages. Without it, a
parser that reads nothing passes in silence.

**Self-test.** With `--self-test`, the script runs on fixtures in
`ci/osv/fixtures/`:

- `dev-only.yaml`: `braces` is reachable only through `devDependencies`. Expect 0.
- `prod-path.yaml`: a production dependency reaches `braces` through two levels. Expect 1.
- `bad-version.yaml`: `lockfileVersion: '6.0'`. Expect 2.
- `dangling.yaml`: an edge points to a missing snapshot. Expect 2.

Each fixture has a `next` production dependency, so that the positive control passes
where the fixture expects 0 or 1. The names, quoting and peer suffixes in the fixtures
copy the real lock file. This includes a scoped `@` name and a `(peer)` suffix, so that
the fixtures do not only test the easy format (memory: fixtures inherit the implementer's
assumption). `--self-test` exits 0 only if every fixture gives its expected code.

### 4.3 Wiring: `ci/osv/run.sh`

- Run `python3 ci/osv/prod_reachability.py --self-test`, then
  `python3 ci/osv/prod_reachability.py ts/pnpm-lock.yaml`. Do this after the package-count
  control and before the findings block.
- A guard result of 1 or 2 makes `run.sh` exit with the same code, before the findings
  block. So a broken guard can never read as a clean scan.
- Update the header comment to describe the guard.
- `moon.yml`'s `repo:osv` inputs already contain `ci/osv/**/*` and `ts/pnpm-lock.yaml`. No
  change.
- The scan job already runs `run.sh` (F10). No workflow change.

### 4.4 Documents

- `ci/CLAUDE.md`: in the osv policy line (line 59), add that a dev-only waiver needs a
  `DEV_ONLY_WAIVERS` entry in `ci/osv/prod_reachability.py`.
- No `ci/osv/README.md` exists. The detail stays in the `run.sh` header and the script's
  docstring, as for the rest of this gate today.

## 5. Verification

1. `bash ci/osv/run.sh` exits 0. It prints the four package counts, the guard line and "no
   known vulnerabilities".
2. Remove the waiver entry temporarily. `run.sh` exits 1 and names GHSA-vfj7-8cjw-p6xm. This
   proves that the waiver, and not something else, makes the gate pass.
3. `python3 ci/osv/prod_reachability.py --self-test` exits 0.
4. Prove the guard bites (memory: red-first is not proof, delete the feature). Each
   mutation must give a red result:
   - M1 (the call site): point `run.sh`'s guard call at `fixtures/prod-path.yaml`.
     `run.sh` must exit 1. Then also delete the call. `run.sh` must now exit 0. This
     proves that the call site in `run.sh` produces the red, not only the script in
     isolation (memory: guard the guard's production call site).
   - M2: make the walk include `devDependencies`. The `dev-only` fixture must fail.
   - M3: make the walk return no edges. The positive control must fail with 2.
   - M4: remove the dangling-key check. The `dangling` fixture must fail.
   Restore by reverting the marked change only, not with `git checkout --` (memory: a
   mutation restore discards the fix).
5. Run `ci/osv/run.sh` under system `/bin/bash` 3.2 and Homebrew bash 5. The script uses
   no `mapfile` or `declare -A`. If the pipe preflight shows a small pipe, record this.
6. The full `moon ci` target list from the root `CLAUDE.md`, with `--base origin/main`.
   The change touches only `ci/osv/**`, `osv-scanner.toml`, `ci/CLAUDE.md` and this spec,
   so `repo:osv` is the main affected task.
7. On the PR: `CI` and `osv advisory scan` pass. The PR touches `osv-scanner.toml`, so the
   scan job runs.

## 6. Out of scope

- A guard for pip waivers. No pip waiver exists, and `uv.lock` has a different format.
- A check that each `[[IgnoredVulns]]` entry has an `ignoreUntil`. This is useful, but it is
  a separate gate change. File it as a follow-up if Sven wants it.
- A replacement for `micromatch` or `fast-glob`.

## 7. Follow-up after merge

- Merge `main` into PR 363 (SMA-693), so that its CI can go green. This is an acceptance
  criterion of SMA-733.
- On or before 2027-01-02, check for a fixed `braces`. The expiry makes the gate fail at
  that date in any case.
