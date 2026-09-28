# SMA-716 Group Changelogs and Release Filter Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give every bumped version-group member its own CHANGELOG section (`changelog_include`, rule R1), and stop a crate from releasing when it has no releasing commit (`release_commits`). Prove both on release-plz fixtures, and check R1 in CI.

**Architecture:** Two keys go into `rs/release-plz.toml`. `repo:publish-metadata` gets a new Check 5 that enforces R1 against `cargo metadata`. `repo:release-parity` gets two optional hooks in `run.sh` (`ecosystem::extra_suite`, `ecosystem::extra_negative_control`). Only `ecosystems/release-plz.sh` defines them. It delegates to a new file `ecosystems/release-plz-filter.sh` with a filter fixture (one crate per classification row), a group fixture (four version groups, G1–G4), and three negative controls (NC1–NC3). `ci/affected-graph` pins the new `run.sh` lines and the new `source` edge.

**Tech Stack:** bash (3.2-compatible), release-plz 0.3.158 (proto-pinned), python3 `tomllib` (inline heredoc in `ci/publish-metadata/run.sh`), Moon 2.5.3, git.

**Spec:** `docs/superpowers/specs/2026-09-27-sma-716-release-plz-changelog-design.md` (APPROVED, Q1–Q4 as recommended). Read it before you start a task.

## Global Constraints

- Every new source file opens with `#!/usr/bin/env bash` then `# SPDX-License-Identifier: Apache-2.0`.
- Conventional commits with a scope from: `rs, py, ts, contracts, ci, docs, deps, release, repo, claude, workspace`. End every commit message with a blank line and `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Do not put a line that starts with `#` or looks like `token: value` in a commit body (commitlint `footer-leading-blank`).
- Never `git commit --amend`. Never `git reset HEAD~1`. Never `--no-verify` or `--no-gpg-sign`. If signing fails with `failed to fill whole buffer`, stop and report: 1Password is locked.
- Run everything in the FOREGROUND. Do not use `run_in_background`, `&`, or `nohup`.
- Do not install host software (no `brew install`, no global `cargo install`, no `pip install`).
- Put the proto tools on PATH in every shell: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`.
- `ci/release-parity/run.sh` exports `PROTO_REPORTER=text` at its top. Keep that line. Any new script that captures `proto` or shim output must export it too.
- New bash must run under `/bin/bash` 3.2: no `mapfile`, no `declare -A`, no `${x,,}`. Do not use a here-string (`<<<`) and do not write a heredoc over 400 bytes: this Mac can have a 512-byte pipe, and Homebrew bash 5.3.15 then deadlocks. Write files with `printf`.
- Do not pipe into a reader that can exit early (`grep -q`, `grep -m`, `head`, `awk … exit`, `sed … q`). Read a FILE instead: `grep -qF -- "$x" "$file"`. `repo:actionlint` check 13 bans the pipe form in every tracked `*.sh`.
- `run.sh` calls the hooks as `hook "$REAL_TOML" || rc=$?`. Bash switches errexit OFF inside a function called that way. So every `rpf::` step must check its own status (`|| return 2`). Do not rely on `set -e` in the new module.
- In a function, do not end with `[ … ] && cmd`: when the test is false, the function returns 1. Use `if … fi`.
- Return codes for every suite and control function: `0` pass, `1` an assertion failed, `2` infrastructure. A negative control must see rc `1`. An rc `2` is INCONCLUSIVE and fails the control (spec §4.3).
- Every fixture git repo sets `maintenance.auto false` and `gc.auto 0` directly after `git init` (SMA-708, `ci/CLAUDE.md`).
- Do not write a second release-plz or cargo call site in `release-plz-filter.sh`. Use `ecosystem::run_update` from `release-plz.sh`. `repo:affected-smoke` A10 waives only that one call by its exact text (`ci/affected-graph/cargo_moon_parity.py:461-475`). Do not put the word `cargo` followed by a subcommand in any quoted string in the new file.
- Do not write the name of the moon CI report JSON file in any new doc or plan: `repo:actionlint` check 12 then requires a marker.
- A measured result that disagrees with a predicted value in this plan: STOP, do not edit the expected value, and report the measurement to the controller.
- Bash per gate on this Mac: `repo:release-parity*` has no bash-4 builtins, so run it with `/bin/bash`. `repo:publish-metadata` needs bash 4+ (`declare -A` at `run.sh:662`): use `/opt/homebrew/bin/bash`. `repo:affected-smoke` needs `/bin/bash` 3.2. `repo:ruff-ci` needs bash 4+. A gate that sits at 0% CPU is a host pipe hang, not a verdict.
- Do not touch anything under `rs/crates/`.
- The regex is fixed by the spec (Q1). Copy it exactly:
  `release_commits = '^(feat|fix|perf)(\((rs|py|ts|contracts|deps)(, ?(rs|py|ts|contracts|deps))*\))?!?:|^[a-z]+(\([^)]*\))?!:|(?m:^BREAKING[ -]CHANGE:)'`

## Review Focus

1. **A squash merge whose PR title does not release, but whose body lists a releasing sub-commit** (`fix(ci): x` with body `fix(rs): y`). A person expects the title to decide, because `^` has no `m` flag. Pinned by filter row `r24` (expect 0.1.0) in Task 2.
2. **The hyphen footer `BREAKING-CHANGE: y` on a non-releasing type.** A person expects it to release like the space form (row 19). Pinned by row `r25` (expect 0.2.0) in Task 2.
3. **A PR title with a space after the comma in the scope list** (`fix(rs, py): x`). PR titles are not linted. DECIDED (Sven, 2026-09-27): allow one optional space after each comma. The regex uses `, ?`, and row `r26` expects 0.1.1. Task 5 documents that the space is allowed.
4. **A group member that is Cargo `publish = false` but `git_only = true`.** release-plz processes it, so R1 must count it as a member. Pinned by two Check 5 fixture rows in Task 1.
5. **`changelog_include` written as a string, not a list.** A person expects a clear rc-1 message, not a Python traceback. Pinned by a Check 5 fixture row in Task 1.

---

### Task 1: R1 check (Check 5) in `repo:publish-metadata`, and `changelog_include` in the real config

**Files:**
- Modify: `ci/publish-metadata/run.sh` (header list at lines 5-45; `metadata_checks` Python at lines 149-325, insert before `if errors:` at about line 313; `negative_control` at lines 1056-1654)
- Modify: `rs/release-plz.toml:134-143` (the proto family)

**Interfaces:**
- Consumes: `metadata_checks $1 metadata.json $2 release-plz.toml $3 expected-csv $4 snapshot` (existing). `is_publishable(pkg)` (existing, Python). `_expect_rc want label cmd…` and `_meta` (existing, inside `negative_control`).
- Produces: Check 5 inside `metadata_checks` (no new function name outside Python). A new helper `_meta_many out json…` inside `negative_control`. The real `rs/release-plz.toml` gets `changelog_include` on `paigasus-proto` and `paigasus-proto-derive`.

- [ ] **Step 1: Add the failing fixture rows to `negative_control`**

Insert this block directly above the line `# Positive control: a clean fixture must pass, or every "red" above is meaningless.` (about line 1643):

```bash
  # --- Check 5 fixtures: rule R1, symmetric changelog_include (SMA-716) ------------
  # A multi-package metadata file. _meta writes exactly one package; R1 needs a group.
  _meta_many() { # $1 out-file, rest = one JSON object per package
    local out="$1"; shift
    python3 - "$out" "$@" <<'PY'
import json, sys
with open(sys.argv[1], "w", encoding="utf-8") as fh:
    json.dump({"packages": [json.loads(a) for a in sys.argv[2:]]}, fh)
PY
  }
  # The real repo shape: kernel group (one processed member plus a publish = false binding)
  # and the proto group (two processed members).
  _r1_toml() { # $1 out  $2 proto lines  $3 derive lines  $4 kernel lines  $5 binding lines
    printf '[workspace]\n\n[[package]]\nname = "paigasus-kernel"\nversion_group = "kernel"\n%s\n\n[[package]]\nname = "paigasus-py-bindings"\nversion_group = "kernel"\npublish = false\n%s\n\n[[package]]\nname = "paigasus-proto"\nversion_group = "proto"\n%s\n\n[[package]]\nname = "paigasus-proto-derive"\nversion_group = "proto"\n%s\n' \
      "$4" "$5" "$2" "$3" >"$1"
  }
  local r1_base r1_proto r1_derive r1_bind r1_derive_np
  r1_base="$(printf '%s' "$base" | sed 's/"version":"0.0.0"/"version":"0.1.0"/')"
  r1_proto="$(printf '%s' "$r1_base" | sed 's/"paigasus-kernel"/"paigasus-proto"/')"
  r1_derive="$(printf '%s' "$r1_base" | sed 's/"paigasus-kernel"/"paigasus-proto-derive"/')"
  r1_bind="$(printf '%s' "$r1_base" | sed 's/"paigasus-kernel"/"paigasus-py-bindings"/; s/"publish":null/"publish":[]/')"
  r1_derive_np="$(printf '%s' "$r1_derive" | sed 's/"publish":null/"publish":[]/')"
  _meta_many "$tmp/r1.json" "$r1_base" "$r1_proto" "$r1_derive" "$r1_bind"
  _meta_many "$tmp/r1-np.json" "$r1_base" "$r1_proto" "$r1_derive_np" "$r1_bind"
  local r1_csv="paigasus-kernel,paigasus-proto,paigasus-proto-derive"
  local r1_csv_np="paigasus-kernel,paigasus-proto"
  local inc_p='changelog_include = ["paigasus-proto-derive"]'
  local inc_d='changelog_include = ["paigasus-proto"]'

  _r1_toml "$tmp/r1-ok.toml" "$inc_p" "$inc_d" '' ''
  _expect_rc 0 "Check 5 (R1 symmetric proto group passes)" \
    metadata_checks "$tmp/r1.json" "$tmp/r1-ok.toml" "$r1_csv" "$fix_snap"

  _r1_toml "$tmp/r1-missing.toml" 'changelog_include = []' "$inc_d" '' ''
  _expect_rc 1 "Check 5 (R1 member missing from changelog_include)" \
    metadata_checks "$tmp/r1.json" "$tmp/r1-missing.toml" "$r1_csv" "$fix_snap"

  _r1_toml "$tmp/r1-absent.toml" '' "$inc_d" '' ''
  _expect_rc 1 "Check 5 (R1 changelog_include absent on one member)" \
    metadata_checks "$tmp/r1.json" "$tmp/r1-absent.toml" "$r1_csv" "$fix_snap"

  _r1_toml "$tmp/r1-extra.toml" 'changelog_include = ["paigasus-proto-derive", "paigasus-kernel"]' "$inc_d" '' ''
  _expect_rc 1 "Check 5 (R1 extra name from another group)" \
    metadata_checks "$tmp/r1.json" "$tmp/r1-extra.toml" "$r1_csv" "$fix_snap"

  _r1_toml "$tmp/r1-typo.toml" 'changelog_include = ["paigasus-proto-derivee"]' "$inc_d" '' ''
  _expect_rc 1 "Check 5 (R1 misspelt name)" \
    metadata_checks "$tmp/r1.json" "$tmp/r1-typo.toml" "$r1_csv" "$fix_snap"

  _r1_toml "$tmp/r1-string.toml" 'changelog_include = "paigasus-proto-derive"' "$inc_d" '' ''
  _expect_rc 1 "Check 5 (R1 changelog_include is a string, not a list)" \
    metadata_checks "$tmp/r1.json" "$tmp/r1-string.toml" "$r1_csv" "$fix_snap"

  _r1_toml "$tmp/r1-unprocessed.toml" "$inc_p" "$inc_d" '' 'changelog_include = ["paigasus-kernel"]'
  _expect_rc 1 "Check 5 (R1 key on a group member release-plz does not process)" \
    metadata_checks "$tmp/r1.json" "$tmp/r1-unprocessed.toml" "$r1_csv" "$fix_snap"

  printf '[workspace]\n\n[[package]]\nname = "paigasus-kernel"\nchangelog_include = ["paigasus-proto"]\n\n[[package]]\nname = "paigasus-proto"\nversion_group = "proto"\n%s\n\n[[package]]\nname = "paigasus-proto-derive"\nversion_group = "proto"\n%s\n' \
    "$inc_p" "$inc_d" >"$tmp/r1-nogroup.toml"
  _expect_rc 1 "Check 5 (R1 key on a crate in no version group)" \
    metadata_checks "$tmp/r1.json" "$tmp/r1-nogroup.toml" "$r1_csv" "$fix_snap"

  # Review Focus 4: a Cargo publish = false member with git_only = true IS processed.
  _r1_toml "$tmp/r1-gitonly-ok.toml" "$inc_p" $'git_only = true\nchangelog_include = ["paigasus-proto"]' '' ''
  _expect_rc 0 "Check 5 (R1 counts a git_only publish = false member)" \
    metadata_checks "$tmp/r1-np.json" "$tmp/r1-gitonly-ok.toml" "$r1_csv_np" "$fix_snap"

  _r1_toml "$tmp/r1-gitonly-bad.toml" 'changelog_include = []' $'git_only = true\nchangelog_include = ["paigasus-proto"]' '' ''
  _expect_rc 1 "Check 5 (R1 git_only member missing from the other member)" \
    metadata_checks "$tmp/r1-np.json" "$tmp/r1-gitonly-bad.toml" "$r1_csv_np" "$fix_snap"
```

- [ ] **Step 2: Run the control and see the new rows fail**

Run: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"; /opt/homebrew/bin/bash ci/publish-metadata/run.sh --negative-control; echo "rc=$?"`
Expected: `rc=1` and exactly eight lines `NEGATIVE CONTROL FAILED: Check 5 (…) — expected rc 1, got rc 0` (missing, absent, extra, misspelt, string, unprocessed, no group, git_only-bad). The two rc-0 rows print `ok`. The last line is `negative control: 8 check(s) failed to bite`. If the process sits at 0% CPU, it is the host pipe hang (Global Constraints); report it.

- [ ] **Step 3: Implement Check 5 in `metadata_checks`**

Insert this Python directly above the line `if errors:` (after the Check 3 block, about line 313):

```python
# --- Check 5: rule R1, symmetric changelog_include in each version group (SMA-716) ---
# release-plz 0.3.158 appends a changelog_include package's OWN commits to this package's
# diff (updater.rs:241-257, READ). The include is not transitive, so each processed member
# of a version group must list EVERY other processed member. Then all members see the same
# commits, the release_commits filter gives the same answer for all of them, and a member
# with no own commits still gets a CHANGELOG section. "Processed" mirrors
# packages_to_process() (updater.rs:283-302, READ): Cargo-publishable, or git_only.
# Re-read both citations when .prototools moves the release-plz pin.
try:
    with open(rp_path, "rb") as fh:
        r1_config = tomllib.load(fh)
except FileNotFoundError:
    r1_config = {}  # no config: no group and no include, so R1 holds; Check 3 owns this case
except Exception as exc:
    print(f"FATAL: cannot parse {rp_path}: {exc}", file=sys.stderr)
    sys.exit(2)

all_pkgs = {p["name"]: p for p in meta.get("packages", []) if "name" in p}
r1_workspace = r1_config.get("workspace") or {}
r1_entries = [
    e for e in (r1_config.get("package") or [])
    if isinstance(e, dict) and isinstance(e.get("name"), str)
]


def r1_processed(entry):
    pkg = all_pkgs.get(entry["name"])
    if pkg is None:
        return False
    git_only = entry.get("git_only")
    if git_only is None:
        git_only = r1_workspace.get("git_only")
    return is_publishable(pkg) or git_only is True


r1_groups = {}
for entry in r1_entries:
    group = entry.get("version_group")
    if group is not None and r1_processed(entry):
        r1_groups.setdefault(group, []).append(entry["name"])

for entry in r1_entries:
    name = entry["name"]
    raw = entry.get("changelog_include")
    group = entry.get("version_group")
    if raw is None:
        include = []
    elif isinstance(raw, list) and all(isinstance(x, str) for x in raw):
        include = raw
    else:
        errors.append(
            f"{name}: `changelog_include` must be a list of package names, got {raw!r} "
            "(rule R1, SMA-716)"
        )
        continue
    if group is None or not r1_processed(entry):
        if include:
            errors.append(
                f"{name}: sets `changelog_include` {include}, but it is not a processed "
                "member of a version group. Rule R1 (SMA-716) allows the key only on a group "
                "member that release-plz processes (Cargo-publishable, or git_only)."
            )
        continue
    want = sorted(m for m in r1_groups[group] if m != name)
    got = set(include)
    missing = sorted(set(want) - got)
    unknown = sorted(x for x in got if x not in all_pkgs)
    extra = sorted(x for x in got - set(want) if x in all_pkgs)
    problems = []
    if missing:
        problems.append(f"missing {missing}")
    if extra:
        problems.append(f"extra {extra}")
    if unknown:
        problems.append(f"unknown {unknown}")
    if len(include) != len(got):
        problems.append("duplicate names")
    if problems:
        errors.append(
            f"{name}: version group {group!r}: `changelog_include` must be exactly the other "
            f"processed members {want}; " + ", ".join(problems) + ". Rule R1 (SMA-716): "
            "every member lists every other member, so all members get the same commits."
        )
```

Add this entry to the header list, after the `Check 4` entry (line 45):

```bash
#   Check 5  rule R1 (SMA-716): in each version group, each member that release-plz
#            processes (Cargo-publishable, or git_only) sets `changelog_include` to exactly
#            the other processed members. No other [[package]] sets the key. It lives here,
#            not in repo:release-parity, because it needs the Cargo manifests, and only this
#            task lists them as inputs (moon.yml, repo:publish-metadata).
```

- [ ] **Step 4: Run the control again**

Run: `/opt/homebrew/bin/bash ci/publish-metadata/run.sh --negative-control; echo "rc=$?"`
Expected: `rc=0`, last line `negative control: every check reports red on a broken fixture`, and ten `ok — Check 5 (…)` lines.

- [ ] **Step 5: Run the real gate and see R1 fail on the real config**

Run: `/opt/homebrew/bin/bash ci/publish-metadata/run.sh; echo "rc=$?"`
Expected: `rc=1` with two rows, `paigasus-proto: version group 'proto': … missing ['paigasus-proto-derive']` and `paigasus-proto-derive: … missing ['paigasus-proto']`. This proves Check 5 runs on the real files.

- [ ] **Step 6: Add R1 to `rs/release-plz.toml`**

Replace lines 134-143 (the two proto `[[package]]` blocks) with:

```toml
# CHANGELOG_INCLUDE (SMA-716, rule R1). A version group gives every member the group's highest
# next version, also a member with no own commits (updater.rs:161-189 and 796-828, READ). Such a
# member got NO CHANGELOG section: git-cliff drops a release with no commits (git-cliff-core
# changelog.rs:143-175), and the release PR body then quoted the member's OLD section
# (next_ver.rs:432-437). This was P1 on release PR #306 for paigasus-proto-derive.
# `changelog_include` appends the named packages' own commits to this package's diff
# (updater.rs:241-257, READ). The include is not transitive, so each member names EVERY other
# member (rule R1). It also feeds the version and the `release_commits` filter above, so all
# members see the same commits and stay in lockstep. Consequence (accepted at GATE 1, Q4): the
# section of EACH member, and the paigasus-proto GitHub Release body, lists the whole group's
# commits. repo:publish-metadata Check 5 enforces R1. A new member: add it to the other members'
# lists in the same PR; its own list takes effect only after its first tag or publish.
# READ in release-plz 0.3.158. Read the source again when .prototools moves the pin.
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

Keep the comment lines above line 134 (the proto family and GITHUB RELEASES notes) as they are; put the new comment between them and the first `[[package]]`.

- [ ] **Step 7: Run the real gate again**

Run: `/opt/homebrew/bin/bash ci/publish-metadata/run.sh; echo "rc=$?"`
Expected: `rc=0`, last line `publish-metadata: all checks passed`.

- [ ] **Step 8: Commit**

```bash
git add ci/publish-metadata/run.sh rs/release-plz.toml
git commit -m "feat(ci): check symmetric changelog_include in version groups (SMA-716)" \
  -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: `release_commits`, the hook plumbing, and the filter fixture suite

**Files:**
- Modify: `rs/release-plz.toml:15-17` (after `features_always_increment_minor`) and lines 1-5, 19-21 (the "harness derives" sentences)
- Create: `ci/release-parity/ecosystems/release-plz-filter.sh`
- Modify: `ci/release-parity/ecosystems/release-plz.sh` (append after `ecosystem::version`, line 166)
- Modify: `ci/release-parity/run.sh` (after the `source` at line 29; after `done 3<"$CASES"` at line 91)
- Modify: `ci/affected-graph/ci_targets.py` (`RELEASE_PARITY_SH_CALL_SITES` at 1119-1125 and its comment above; `wired_release_parity` at 2684-2694; the report text at about 3918)
- Modify: `ci/affected-graph/cargo_moon_parity.py:1094-1100` (`REQUIRED_SOURCED_SCRIPTS`)
- Modify: `ci/release-parity/README.md` (new section)

**Interfaces:**
- Consumes: `ecosystem::_derive_config real out` (returns 1 on a missing key), `ecosystem::run_update dir` (returns 1 on failure), both in `release-plz.sh`. `$REAL_TOML` in `run.sh:44`.
- Produces (later tasks rely on these exact names):
  - `rpf::filter_suite REAL_TOML MODE` with MODE `real|no-release-commits` → 0/1/2. FAIL lines on stderr start with `FAIL  <id> ` (id padded to 14 chars by `%-14s`, then a space).
  - `rpf::suites REAL_TOML` → 0/1/2.
  - `ecosystem::extra_suite REAL_TOML` (in `release-plz.sh`) → `rpf::suites`.
  - Helpers for Task 3: `rpf::_write_config REAL OUT WITH_RC`, `rpf::_git_init DIR`, `rpf::_write_crate DIR NAME [DEP_LINE]`, `rpf::_seed_and_tag DIR CRATE…`, `rpf::_commit DIR CRATE KIND SUBJECT [BODY]`, `rpf::_version CARGO_TOML`, `rpf::_first_release_section CHANGELOG OUT`, `rpf::_heading_of SECTION LINE`, `rpf::_check_versions ID DIR WANT CRATE…`, `rpf::_check_section ID DIR CRATE VERSION HEADING LINE…` (HEADING `-` = presence only).

- [ ] **Step 1: Write the new module (the failing suite)**

Create `ci/release-parity/ecosystems/release-plz-filter.sh`:

```bash
#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# SMA-716: release-plz-only suites for `release_commits` and `changelog_include`.
#
# Sourced by ecosystems/release-plz.sh only. run.sh reaches this code through the hooks
# ecosystem::extra_suite and ecosystem::extra_negative_control. The python-semantic-release and
# semantic-release modules define neither hook, so repo:release-parity-py and -ts skip it.
# Not in cases.tsv: that file is the cross-tool parity contract, and the two other tools have no
# release_commits equivalent (spec section 4.3).
#
# Return codes of every rpf:: suite and check: 0 pass, 1 an assertion failed, 2 infrastructure.
# run.sh calls the hooks as `hook || rc=$?`. Bash turns errexit OFF inside a function that is
# called that way, so each step below checks its own status. Do not rely on `set -e` here.
#
# Uses ecosystem::_derive_config and ecosystem::run_update from release-plz.sh. Do not add a
# second release-plz call site in this file: repo:affected-smoke A10 waives only the one in
# ecosystem::run_update, by its exact text.
set -euo pipefail

# --- shared helpers -------------------------------------------------------------------------

# F3 for the new key: copy `release_commits` VERBATIM from the real config. A missing or a
# duplicated key is rc 2, so the suite cannot test a stale regex. The mutation modes still
# read the key first, so a control cannot pass because the real key is gone.
rpf::_release_commits_line() { # real_toml -> the key line on stdout
  local real="$1" n line
  n="$(grep -cE '^[[:space:]]*release_commits[[:space:]]*=' "$real" || true)"
  if [ "$n" != 1 ]; then
    echo "FATAL: rs/release-plz.toml has ${n:-0} release_commits lines, expected 1 (SMA-716 F3)" >&2
    return 2
  fi
  line="$(grep -E '^[[:space:]]*release_commits[[:space:]]*=' "$real")" || return 2
  printf '%s\n' "${line#"${line%%[![:space:]]*}"}"
}

rpf::_write_config() { # real_toml out_toml with_release_commits(0|1)
  local real="$1" out="$2" with_rc="$3" rc_line
  ecosystem::_derive_config "$real" "$out" || return 2
  rc_line="$(rpf::_release_commits_line "$real")" || return 2
  if [ "$with_rc" = 1 ]; then
    printf '%s\n' "$rc_line" >>"$out" || return 2
  fi
}

rpf::_git_init() { # dir
  ( cd "$1" &&
    git init -q &&
    git config maintenance.auto false &&   # SMA-708: no background maintenance in a fixture
    git config gc.auto 0 &&
    git config user.email "parity@example.com" &&
    git config user.name "parity" &&
    git config commit.gpgsign false &&
    git config tag.gpgsign false ) || return 2
}

rpf::_write_workspace() { # dir
  printf '[workspace]\nresolver = "3"\nmembers = ["crates/*"]\n' >"$1/Cargo.toml" || return 2
}

rpf::_write_crate() { # dir name [dependency line]
  local cdir="$1/crates/$2"
  mkdir -p "$cdir/src" || return 2
  printf '[package]\nname = "%s"\nversion = "0.1.0"\nedition = "2024"\npublish = false\n' "$2" \
    >"$cdir/Cargo.toml" || return 2
  if [ -n "${3-}" ]; then
    printf '\n[dependencies]\n%s\n' "$3" >>"$cdir/Cargo.toml" || return 2
  fi
  printf '// seed\n' >"$cdir/src/lib.rs" || return 2
}

rpf::_seed_and_tag() { # dir crate...
  local dir="$1" c
  shift
  ( cd "$dir" && git add -A && git commit -qm "chore: seed fixture" ) || return 2
  for c in "$@"; do
    ( cd "$dir" && git tag "$c-v0.1.0" ) || return 2
  done
}

# kind src: append a comment to src/lib.rs. kind toml: append a comment to Cargo.toml (the P2
# shape: a comment-only manifest edit, spec section 1).
rpf::_commit() { # dir crate kind subject [body]
  local dir="$1" crate="$2" kind="$3" subject="$4" body="${5-}"
  case "$kind" in
    src) printf '// change for: %s\n' "$subject" >>"$dir/crates/$crate/src/lib.rs" || return 2 ;;
    toml) printf '# comment-only edit for: %s\n' "$subject" >>"$dir/crates/$crate/Cargo.toml" || return 2 ;;
    *) echo "FATAL: rpf::_commit: bad kind $kind" >&2; return 2 ;;
  esac
  ( cd "$dir" && git add -A ) || return 2
  if [ -n "$body" ]; then
    ( cd "$dir" && git commit -qm "$subject" -m "$body" ) || return 2
  else
    ( cd "$dir" && git commit -qm "$subject" ) || return 2
  fi
}

rpf::_version() { # Cargo.toml -> the first `version =` value
  awk -F'"' '/^version[[:space:]]*=/ && v == "" { v = $2 } END { print v }' "$1"
}

# The first RELEASE section: from the first `## [<digit>` heading up to the next `## [`.
# `## [Unreleased]` is skipped. release-plz's last_changes() reads the same section for the
# release PR body (next_ver.rs:432-437, READ). A missing file gives an empty section.
rpf::_first_release_section() { # changelog out_file
  if [ ! -f "$1" ]; then
    : >"$2"
    return 0
  fi
  awk '/^## \[/ { if (started) done = 1; else if ($0 ~ /^## \[[0-9]/) started = 1 }
       started && !done { print }' "$1" >"$2"
}

rpf::_heading_of() { # section_file line -> the last `### ` heading above the first exact match
  awk -v want="$2" '/^### / { h = $0 } $0 == want && !seen { seen = 1; found = h } END { print found }' "$1"
}

rpf::_check_versions() { # id dir want crate...
  local id="$1" dir="$2" want="$3" c got bad="" all=""
  shift 3
  for c in "$@"; do
    got="$(rpf::_version "$dir/crates/$c/Cargo.toml")"
    all="$all $c=$got"
    if [ "$got" != "$want" ]; then bad=1; fi
  done
  if [ -z "$bad" ]; then
    printf 'PASS  %-14s%s\n' "$id" "$all"
    return 0
  fi
  printf 'FAIL  %-14s exp=%s got:%s\n' "$id" "$want" "$all" >&2
  return 1
}

# HEADING `-` means: the line only has to be in the section.
rpf::_check_section() { # id dir crate version heading line...
  local id="$1" dir="$2" crate="$3" ver="$4" heading="$5" sec first line n under
  shift 5
  sec="$dir/.rpf-section-$crate"
  rpf::_first_release_section "$dir/crates/$crate/CHANGELOG.md" "$sec" || return 1
  first="$(sed -n 1p "$sec")"
  case "$first" in
    "## [$ver]"*) ;;
    *) printf 'FAIL  %-14s %s: first release heading is %s, expected ## [%s]\n' \
         "$id" "$crate" "${first:-<none>}" "$ver" >&2
       return 1 ;;
  esac
  for line in "$@"; do
    n="$(grep -cxF -- "$line" "$sec" || true)"
    if [ "${n:-0}" = 0 ]; then
      printf 'FAIL  %-14s %s: "%s" is not in ## [%s]\n' "$id" "$crate" "$line" "$ver" >&2
      return 1
    fi
    if [ "$heading" != "-" ]; then
      under="$(rpf::_heading_of "$sec" "$line")"
      if [ "$under" != "$heading" ]; then
        printf 'FAIL  %-14s %s: "%s" is under %s, expected %s\n' \
          "$id" "$crate" "$line" "${under:-<no heading>}" "$heading" >&2
        return 1
      fi
    fi
  done
  printf 'PASS  %-14s %s: ## [%s] has %s line(s)\n' "$id" "$crate" "$ver" "$#"
}

# --- the filter fixture (spec section 4.2 table, one crate per row) -------------------------

# Rows r01-r23 are the spec's classification table. r24-r26 are the plan's Review Focus rows.
RPF_ROWS="r01 r02 r03 r04 r05 r06 r07 r08 r09 r10 r11 r12 r13 r14 r15 r16 r17 r18 r19 r20 r21 r22 r23 r24 r25 r26"

rpf::_row_expected() { # id -> version from the 0.1.0 baseline
  case "$1" in
    r01|r02|r04|r05|r06|r20|r23|r26) echo 0.1.1 ;;
    r03|r18|r19|r21|r25) echo 0.2.0 ;;
    r07|r08|r09|r10|r11|r12|r13|r14|r15|r16|r17|r22|r24) echo 0.1.0 ;;
    *) echo "FATAL: no expected version for row $1" >&2; return 2 ;;
  esac
}

rpf::_apply_row() { # dir id
  local d="$1" c="rpf-$2"
  case "$2" in
    r01) rpf::_commit "$d" "$c" src 'fix: x' ;;
    r02) rpf::_commit "$d" "$c" src 'fix(rs): x' ;;
    r03) rpf::_commit "$d" "$c" src 'feat(contracts): x' ;;
    r04) rpf::_commit "$d" "$c" src 'perf(rs): x' ;;
    r05) rpf::_commit "$d" "$c" src 'fix(deps): x' ;;
    r06) rpf::_commit "$d" "$c" src 'fix(py,ts): x' ;;
    r07) rpf::_commit "$d" "$c" src 'fix(ci): x' ;;
    r08) rpf::_commit "$d" "$c" src 'feat(ci): x' ;;
    r09) rpf::_commit "$d" "$c" src 'perf(ci): x' ;;
    r10) rpf::_commit "$d" "$c" src 'fix(repo): x' ;;
    r11) rpf::_commit "$d" "$c" src 'fix(workspace): x' ;;
    r12) rpf::_commit "$d" "$c" src 'fix(rs,ci): x' ;;
    r13) rpf::_commit "$d" "$c" src 'fix(cid): x' ;;
    r14) rpf::_commit "$d" "$c" src 'chore(rs): x' ;;
    r15) rpf::_commit "$d" "$c" src 'build(deps): x' ;;
    r16) rpf::_commit "$d" "$c" src 'refactor(rs): x' ;;
    r17) rpf::_commit "$d" "$c" src 'Revert "feat(rs): x (#1)"' ;;
    r18) rpf::_commit "$d" "$c" src 'fix(ci)!: x' ;;
    r19) rpf::_commit "$d" "$c" src 'chore(rs): x' 'BREAKING CHANGE: y' ;;
    r20) rpf::_commit "$d" "$c" src 'refactor(rs): x' && rpf::_commit "$d" "$c" src 'fix(rs): y' ;;
    r21) rpf::_commit "$d" "$c" src 'feat(ci): x' && rpf::_commit "$d" "$c" src 'fix(rs): y' ;;
    r22) rpf::_commit "$d" "$c" toml 'fix(ci): x' ;;
    r23) rpf::_commit "$d" "$c" toml 'fix(rs): x' ;;
    r24) rpf::_commit "$d" "$c" src 'fix(ci): x' 'fix(rs): y' ;;
    r25) rpf::_commit "$d" "$c" src 'chore(rs): x' 'BREAKING-CHANGE: y' ;;
    r26) rpf::_commit "$d" "$c" src 'fix(rs, py): x' ;;
    *) echo "FATAL: no commits for row $2" >&2; return 2 ;;
  esac
}

rpf::_build_filter_fixture() { # dir real_toml with_release_commits
  local dir="$1" real="$2" with_rc="$3" id crates="rpf-b"
  rpf::_write_workspace "$dir" || return 2
  rpf::_write_crate "$dir" rpf-b || return 2
  for id in $RPF_ROWS; do
    rpf::_write_crate "$dir" "rpf-$id" || return 2
    crates="$crates rpf-$id"
  done
  rpf::_write_config "$real" "$dir/release-plz.toml" "$with_rc" || return 2
  rpf::_git_init "$dir" || return 2
  # shellcheck disable=SC2086  # the crate list splits into one argument per crate
  rpf::_seed_and_tag "$dir" $crates || return 2
}

rpf::filter_suite() { # real_toml mode(real|no-release-commits) -> 0/1/2
  local real="$1" mode="$2" with_rc dir id want fails=0
  case "$mode" in
    real) with_rc=1 ;;
    no-release-commits) with_rc=0 ;;
    *) echo "FATAL: rpf::filter_suite: bad mode $mode" >&2; return 2 ;;
  esac
  dir="$(mktemp -d)" || return 2
  if ! rpf::_build_filter_fixture "$dir" "$real" "$with_rc"; then
    echo "FATAL: filter fixture build failed" >&2; rm -rf "$dir"; return 2
  fi
  for id in $RPF_ROWS; do
    if ! rpf::_apply_row "$dir" "$id"; then
      echo "FATAL [$id]: commit failed" >&2; rm -rf "$dir"; return 2
    fi
  done
  if ! ecosystem::run_update "$dir"; then
    echo "FATAL: release-plz update failed on the filter fixture" >&2; rm -rf "$dir"; return 2
  fi
  for id in $RPF_ROWS; do
    if ! want="$(rpf::_row_expected "$id")"; then rm -rf "$dir"; return 2; fi
    rpf::_check_versions "$id" "$dir" "$want" "rpf-$id" || fails=$((fails + 1))
  done
  rpf::_check_versions b "$dir" 0.1.0 rpf-b || fails=$((fails + 1))
  # Row 20, C1: a non-releasing commit is carried into the next release and its section.
  rpf::_check_section r20-changelog "$dir" rpf-r20 0.1.1 - '- *(rs)* x' '- *(rs)* y' \
    || fails=$((fails + 1))
  rm -rf "$dir"
  if [ "$fails" != 0 ]; then return 1; fi
}

# --- entry points for the hooks in release-plz.sh -------------------------------------------

rpf::suites() { # real_toml -> 0/1/2
  local real="$1" rc worst=0
  rc=0; rpf::filter_suite "$real" real || rc=$?
  if [ "$rc" = 2 ]; then return 2; fi
  if [ "$rc" != 0 ]; then worst=1; fi
  return "$worst"
}
```

- [ ] **Step 2: Wire the module into `release-plz.sh`**

Append at the end of `ci/release-parity/ecosystems/release-plz.sh` (after `ecosystem::version`):

```bash

# --- SMA-716: the release-plz-only suites ---------------------------------------------------
# run.sh calls ecosystem::extra_suite (and, from the negative control, a second hook) only when
# the module defines it. Only this module does. The suites live in release-plz-filter.sh.
# _RP_DIR uses the BASH_SOURCE idiom on purpose: repo:affected-smoke's source resolver
# (ci/affected-graph/cargo_moon_parity.py, HERE_IDIOM_ASSIGN_RE) follows it to the new file.
_RP_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=ci/release-parity/ecosystems/release-plz-filter.sh
source "$_RP_DIR/release-plz-filter.sh"

ecosystem::extra_suite() { # real_toml -> 0/1/2
  rpf::suites "$1"
}
```

Also change line 4 to `# Interface: ecosystem::build_fixture / apply_commit / run_update / version (+ ecosystem::extra_suite, SMA-716)`.

- [ ] **Step 3: Measure one filter run and see the suite fail on the missing key**

Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
/bin/bash -c 'export PROTO_REPORTER=text; source ci/release-parity/ecosystems/release-plz.sh; time rpf::filter_suite "$PWD/rs/release-plz.toml" real'; echo "rc=$?"
```
Expected: `FATAL: rs/release-plz.toml has 0 release_commits lines, expected 1 (SMA-716 F3)` and `rc=2`. This is the F3 guard.

- [ ] **Step 4: Add `release_commits` to `rs/release-plz.toml`**

Insert directly after `features_always_increment_minor = true` (line 17):

```toml

# --- Which commits open a release (SMA-716, decision D2) -----------------------------------
# A crate releases only when at least one commit in its diff matches this regex
# (Diff::any_commit_matches, diff.rs:104-108; applied per package at updater.rs:90-97, READ).
# The SMA-716 fixture suite (ci/release-parity/ecosystems/release-plz-filter.sh) copies this line
# verbatim and asserts one fixture crate per case of the spec's classification table.
# - Releasing: feat, fix, perf with the scopes rs, py, ts, contracts, deps, or with NO scope
#   (squash subjects on main are PR titles, and nothing lints them). A scope list releases only
#   if EVERY scope in it releases; one optional space after each comma is allowed. Any type with `!`, and
#   a `BREAKING CHANGE:` or `BREAKING-CHANGE:` footer, always release.
# - Not releasing: the scopes ci, docs, release, repo, claude, workspace; every other type; a
#   GitHub `Revert "..."` subject (Q3). `^` has no m flag, so only the SUBJECT line counts.
# Consequences:
#   C1 A non-releasing change to shipped code is not lost. The next releasing commit on that
#      crate carries it into the release and its section.
#   C2 The filter decides only WHETHER a crate releases. The LEVEL comes from all commits in the
#      diff (version.rs:13-21). So `feat(ci)` on a crate path makes the next release a minor
#      bump. For a crate-file change that must not ship, use chore, ci, build, docs, refactor or
#      test, never feat(ci) or fix(ci).
#   C3 A new publishable crate first releases only after a releasing commit touches it.
#   C4 The "dependencies changed" cascade bypasses the filter (updater.rs:155-157). A dependent
#      of a released crate still gets a patch bump.
#   C5 A dependency floor change needs a `fix(deps)` commit to release. Dependabot writes
#      `build(deps)`, which does not release.
# The filter runs in `update` and `release-pr`, not in `release`. The commitlint scope list
# (ts/packages/commitlint-config/index.cjs) is closed; a new scope does not release until it is
# added here. READ in release-plz 0.3.158. Read the source again when .prototools moves the pin.
release_commits = '^(feat|fix|perf)(\((rs|py|ts|contracts|deps)(, ?(rs|py|ts|contracts|deps))*\))?!?:|^[a-z]+(\([^)]*\))?!:|(?m:^BREAKING[ -]CHANGE:)'
```

Update the two sentences that say the harness derives only one key:
- Lines 3-5: `# The SMA-398 parity harness DERIVES its fixture config from the classification keys below` stays; append the sentence `# The SMA-716 suite also copies release_commits verbatim.`
- Lines 20-21: change `ci/release-parity/ecosystems/release-plz.sh derives only \`features_always_increment_minor\`.` to `ci/release-parity/ecosystems/release-plz.sh derives \`features_always_increment_minor\`, and release-plz-filter.sh copies \`release_commits\` (SMA-716).`

- [ ] **Step 5: Run the filter suite and record its duration**

Run the Step 3 command again.
Expected: 27 `PASS` lines for `r01`…`r26` and `b`, one `PASS  r20-changelog`, no `FAIL`, and `rc=0`. Write down the `real` time from `time`: it goes into the README in Step 11.
If a row fails: STOP (Global Constraints). Print that crate's `CHANGELOG.md` and the `release-plz update` output in your report. Rows 22, 23 and 25 are the least certain predictions.

- [ ] **Step 6: Delete-the-feature check for the filter**

Temporarily comment out the `release_commits = …` line in `rs/release-plz.toml` with the Edit tool. Run the Step 3 command. Expected: `rc=2` (F3 guard). Then restore the line with the Edit tool (do not use `git checkout`: it would also revert the uncommitted comments). Run Step 3 again: `rc=0`.

- [ ] **Step 7: Add the hook call to `run.sh`**

Insert after line 29 (`source "$HERE/ecosystems/$ECOSYSTEM.sh"`):

```bash
# SMA-716: the release-plz module MUST define its extra-suite hook. Without this line, a deleted
# hook in ecosystems/release-plz.sh would skip the whole release-plz-only suite in silence.
[ "$ECOSYSTEM" != release-plz ] || declare -F ecosystem::extra_suite >/dev/null || { echo "FATAL: release-parity ABORTED: infrastructure error (rc=2): ecosystems/release-plz.sh defines no ecosystem::extra_suite (SMA-716)" >&2; exit 2; }
```

Insert after `done 3<"$CASES"` and before the final `if [ "$rc" = 0 ]; then echo "== all parity cases passed ==" …` line:

```bash

# SMA-716: the release-plz-only suites (ecosystems/release-plz-filter.sh). They are not in
# cases.tsv, because cases.tsv is the cross-tool parity contract. `|| xec=$?` turns errexit off
# inside the hook, so the hook checks each step itself.
if declare -F ecosystem::extra_suite >/dev/null; then
  xec=0; ecosystem::extra_suite "$REAL_TOML" || xec=$?
  case "$xec" in
    0) ;;
    1) echo "== extra suite FAILURES (see above) ==" >&2; rc=1 ;;
    *) echo "== parity ABORTED: infrastructure error in the extra suite (rc=$xec) ==" >&2; exit 2 ;;
  esac
fi
```

- [ ] **Step 8: Run all three ecosystems through `run.sh`**

Run:
```bash
/bin/bash ci/release-parity/run.sh; echo "rc=$?"
/bin/bash ci/release-parity/run.sh --ecosystem python-semantic-release; echo "rc=$?"
/bin/bash ci/release-parity/run.sh --ecosystem semantic-release; echo "rc=$?"
/bin/bash ci/release-parity/run.sh --negative-control; echo "rc=$?"
```
Expected: the first prints the five cases.tsv PASS lines, then the filter PASS lines, then `== all parity cases passed ==`, `rc=0`. The `-py` and `-ts` runs print only their five cases, `rc=0`. The control prints `negative-control OK: harness reported red as expected`, `rc=0`.

- [ ] **Step 9: Pin the new `run.sh` lines in `ci_targets.py`**

In `ci/affected-graph/ci_targets.py`, extend `RELEASE_PARITY_SH_CALL_SITES` (line 1119) so it reads:

```python
RELEASE_PARITY_SH_CALL_SITES = (
    '--negative-control) NEGATIVE=1; shift ;;',
    'if [ "$NEGATIVE" = 1 ]; then',
    'ec=0; check_case "neg-fix-bang" "fix!: deliberately wrong" "-" "0.1.1" || ec=$?',
    '1) echo "negative-control OK: harness reported red as expected"; exit 0 ;;',
    '0) echo "negative-control FAILED: harness accepted a wrong expectation" >&2; exit 1 ;;',
    # SMA-716 — the release-plz-only suite hook. The first line fails the run when the
    # release-plz module no longer defines the hook; the other three are the call and its
    # verdict. Deleting any one lets the suite be skipped or its failures be dropped.
    '[ "$ECOSYSTEM" != release-plz ] || declare -F ecosystem::extra_suite >/dev/null || { echo "FATAL: release-parity ABORTED: infrastructure error (rc=2): ecosystems/release-plz.sh defines no ecosystem::extra_suite (SMA-716)" >&2; exit 2; }',
    'if declare -F ecosystem::extra_suite >/dev/null; then',
    'xec=0; ecosystem::extra_suite "$REAL_TOML" || xec=$?',
    '1) echo "== extra suite FAILURES (see above) ==" >&2; rc=1 ;;',
)
```

Add this paragraph at the end of the comment block directly above the tuple (after the `TRADEOFF` paragraph):

```python
# SMA-716 adds the release-plz-only suite hook (entries 6-9) and, in a later task, the extra
# negative-control hook. The hook BODIES live in ecosystems/release-plz.sh and
# release-plz-filter.sh, which no haystack here reads: a body replaced with `return 0` still
# passes these pins (ci/release-parity/README.md, L6).
```

In `self_test`, change the end of `wired_release_parity` (line 2684-2694) from

```python
        '  esac\n'
        'fi\n'
    )
```

to

```python
        '  esac\n'
        'fi\n'
        # SMA-716 — derived from the registry so the fixture cannot drift from the pins.
        + "".join(f"  {site}\n" for site in RELEASE_PARITY_SH_CALL_SITES[5:])
    )
```

In the report text (about line 3918), change `"    A row prefixed \`ci/release-parity/run.sh:\` means one of the five pinned\n"` and the lines after it so the paragraph starts: `A row prefixed \`ci/release-parity/run.sh:\` means one of the pinned lines is gone from run.sh: a --negative-control line (the flag parse, the NEGATIVE guard, the check_case assertion, or either report arm) or an SMA-716 hook line (a hook guard, a hook call, or its verdict arm).` Keep the rest of that paragraph. Keep the physical-line layout (`"    …\n"` strings under 100 characters each).

- [ ] **Step 10: Update the source-resolver floor**

In `ci/affected-graph/cargo_moon_parity.py:1094-1100`, change `REQUIRED_SOURCED_SCRIPTS` to:

```python
REQUIRED_SOURCED_SCRIPTS = {
    "ci/release-parity/run.sh": (
        "ci/release-parity/ecosystems/python-semantic-release.sh",
        "ci/release-parity/ecosystems/release-plz-filter.sh",
        "ci/release-parity/ecosystems/release-plz.sh",
        "ci/release-parity/ecosystems/semantic-release.sh",
    ),
    # SMA-716: the release-plz module sources the release-plz-only suites.
    "ci/release-parity/ecosystems/release-plz.sh": (
        "ci/release-parity/ecosystems/release-plz-filter.sh",
    ),
}
```

(`run.sh` sources `ecosystems/$ECOSYSTEM.sh`. `ECOSYSTEM` is assigned twice, so the resolver globs `ecosystems/*.sh`, which now includes the new file.)

Run:
```bash
python3 -c 'import sys; sys.path.insert(0, "ci/affected-graph"); import cargo_moon_parity as c; print(c.check_sourced_scripts("."))'
python3 ci/affected-graph/ci_targets.py --self-test; echo "rc=$?"
python3 ci/affected-graph/cargo_moon_parity.py --self-test; echo "rc=$?"
```
Expected: `[]`, then `rc=0` twice. If the first prints a row, copy the `sources (…)` tuple it reports into the floor only if it lists exactly the files above; otherwise STOP and report.

- [ ] **Step 11: README section**

Append to `ci/release-parity/README.md`, before `## Tool resolution policy (SMA-596)`:

```markdown
## The release-plz-only suites (SMA-716)

`ecosystems/release-plz-filter.sh` tests two release-plz keys that the other two tools do not
have: `release_commits` and `changelog_include`. Its cases are NOT in `cases.tsv`, because
`cases.tsv` is the cross-tool parity contract. `run.sh` calls the hook
`ecosystem::extra_suite` after the `cases.tsv` loop, only if the module defines it. Only
`ecosystems/release-plz.sh` defines it, so `repo:release-parity-py` and `-ts` skip the suite.
For the `release-plz` ecosystem, `run.sh` exits 2 if the hook is missing.

**Config (F3).** The fixture config takes `release_commits` verbatim from `rs/release-plz.toml`,
after the keys `ecosystem::_derive_config` copies. A missing or duplicated key is rc 2.

**Filter fixture.** One fixture repo with one crate per row of the spec's classification table
(`rpf-r01` … `rpf-r23`), three Review Focus rows (`rpf-r24` squash body, `rpf-r25`
`BREAKING-CHANGE:` footer, `rpf-r26` space after the comma), and `rpf-b`, which no commit
touches. One `release-plz update` run covers all rows. Row 20 also asserts that the first
release section lists both commits (consequence C1).

**Cost.** One filter fixture run: <MEASURED SECONDS FROM STEP 5> s on the development Mac.
```

Replace `<MEASURED SECONDS FROM STEP 5>` with the number you measured in Step 5 (for example `41`). Do not commit the angle-bracket text.

- [ ] **Step 12: Commit**

```bash
git add rs/release-plz.toml ci/release-parity/ecosystems/release-plz-filter.sh \
  ci/release-parity/ecosystems/release-plz.sh ci/release-parity/run.sh \
  ci/release-parity/README.md ci/affected-graph/ci_targets.py ci/affected-graph/cargo_moon_parity.py
git commit -m "feat(ci): add the release_commits filter and its release-plz fixture suite (SMA-716)" \
  -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: The group fixture suite (G1–G4)

**Files:**
- Modify: `ci/release-parity/ecosystems/release-plz-filter.sh` (add a group section above `# --- entry points`; extend `rpf::suites`)
- Modify: `ci/release-parity/README.md` (the SMA-716 section)

**Interfaces:**
- Consumes (Task 2): `rpf::_write_config`, `rpf::_git_init`, `rpf::_write_workspace`, `rpf::_write_crate`, `rpf::_seed_and_tag`, `rpf::_commit`, `rpf::_version`, `rpf::_first_release_section`, `rpf::_check_versions`, `rpf::_check_section`, `ecosystem::run_update`.
- Produces: `rpf::group_suite REAL_TOML MODE` with MODE `real|no-include|no-include-no-filter` → 0/1/2. Assertion ids: `G1-version`, `G1-changelog`, `G2-version`, `G2-changelog`, `G3-version`, `G4-version`, `G4-changelog`. `RPG_DEP_EDGE` (0 or 1).

- [ ] **Step 1: Write the group suite**

Insert above the line `# --- entry points for the hooks in release-plz.sh ---…`:

```bash
# --- the group fixture (spec section 4.3, G1-G4) --------------------------------------------

# Four independent version groups in ONE repo, so one release-plz run covers G1-G4. Group X has
# members rpg-X1 and rpg-X2. With RPG_DEP_EDGE=1, rpg-X1 depends on rpg-X2 by path AND version:
# the real paigasus-proto -> paigasus-proto-derive shape (rs/Cargo.toml:173).
RPG_GROUPS="a b c d"
RPG_DEP_EDGE=1

# The shape of the real crates' CHANGELOG.md: release-plz's header with `## [Unreleased]`, then a
# release section. So release-plz takes the PREPEND path, where P1 happened
# (updater.rs:1026-1029, changelog.rs:67-89, READ).
rpf::_seed_changelog() { # crate_dir
  printf '%s\n' \
    '# Changelog' '' \
    'All notable changes to this project will be documented in this file.' '' \
    'The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),' \
    'and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).' '' \
    '## [Unreleased]' '' \
    '## [0.1.0] - 2026-01-01' '' \
    '### Other' '' \
    '- seed the fixture' >"$1/CHANGELOG.md" || return 2
}

rpf::_group_package() { # toml name group other with_include(0|1)
  printf '\n[[package]]\nname = "%s"\nversion_group = "%s"\n' "$2" "$3" >>"$1" || return 2
  if [ "$5" = 1 ]; then
    printf 'changelog_include = ["%s"]\n' "$4" >>"$1" || return 2
  fi
}

rpf::_commit_pair() { # dir crate1 crate2 subject  (one commit that touches both crates)
  printf '// change for: %s\n' "$4" >>"$1/crates/$2/src/lib.rs" || return 2
  printf '// change for: %s\n' "$4" >>"$1/crates/$3/src/lib.rs" || return 2
  ( cd "$1" && git add -A && git commit -qm "$4" ) || return 2
}

rpf::_build_group_fixture() { # dir real_toml with_release_commits with_include
  local dir="$1" real="$2" with_rc="$3" with_inc="$4" g dep crates=""
  rpf::_write_workspace "$dir" || return 2
  for g in $RPG_GROUPS; do
    dep=""
    if [ "$RPG_DEP_EDGE" = 1 ]; then
      dep="rpg-${g}2 = { path = \"../rpg-${g}2\", version = \"0.1.0\" }"
    fi
    rpf::_write_crate "$dir" "rpg-${g}1" "$dep" || return 2
    rpf::_write_crate "$dir" "rpg-${g}2" || return 2
    rpf::_seed_changelog "$dir/crates/rpg-${g}1" || return 2
    rpf::_seed_changelog "$dir/crates/rpg-${g}2" || return 2
    crates="$crates rpg-${g}1 rpg-${g}2"
  done
  rpf::_write_config "$real" "$dir/release-plz.toml" "$with_rc" || return 2
  for g in $RPG_GROUPS; do
    rpf::_group_package "$dir/release-plz.toml" "rpg-${g}1" "g$g" "rpg-${g}2" "$with_inc" || return 2
    rpf::_group_package "$dir/release-plz.toml" "rpg-${g}2" "g$g" "rpg-${g}1" "$with_inc" || return 2
  done
  rpf::_git_init "$dir" || return 2
  # shellcheck disable=SC2086  # the crate list splits into one argument per crate
  rpf::_seed_and_tag "$dir" $crates || return 2
}

rpf::_check_once() { # id dir line crate...  (the line is in each crate's first section once)
  local id="$1" dir="$2" line="$3" c sec n all=""
  shift 3
  for c in "$@"; do
    sec="$dir/.rpf-section-$c"
    rpf::_first_release_section "$dir/crates/$c/CHANGELOG.md" "$sec" || return 1
    n="$(grep -cxF -- "$line" "$sec" || true)"
    all="$all $c=${n:-0}"
    if [ "${n:-0}" != 1 ]; then
      printf 'FAIL  %-14s "%s" count per first section:%s, expected 1 each\n' "$id" "$line" "$all" >&2
      return 1
    fi
  done
  printf 'PASS  %-14s "%s" once in each first section:%s\n' "$id" "$line" "$all"
}

rpf::group_suite() { # real_toml mode(real|no-include|no-include-no-filter) -> 0/1/2
  local real="$1" mode="$2" with_rc with_inc dir fails=0
  case "$mode" in
    real) with_rc=1; with_inc=1 ;;
    no-include) with_rc=1; with_inc=0 ;;
    no-include-no-filter) with_rc=0; with_inc=0 ;;
    *) echo "FATAL: rpf::group_suite: bad mode $mode" >&2; return 2 ;;
  esac
  dir="$(mktemp -d)" || return 2
  if ! rpf::_build_group_fixture "$dir" "$real" "$with_rc" "$with_inc"; then
    echo "FATAL: group fixture build failed" >&2; rm -rf "$dir"; return 2
  fi
  if ! { rpf::_commit "$dir" rpg-a1 src 'fix(rs): x' &&       # G1: leader only
         rpf::_commit "$dir" rpg-b2 src 'feat(rs): x' &&      # G2: follower only
         rpf::_commit "$dir" rpg-c1 src 'fix(ci): x' &&       # G3: non-releasing
         rpf::_commit_pair "$dir" rpg-d1 rpg-d2 'fix(rs): x'; }; then  # G4: both crates
    echo "FATAL: group fixture commits failed" >&2; rm -rf "$dir"; return 2
  fi
  if ! ecosystem::run_update "$dir"; then
    echo "FATAL: release-plz update failed on the group fixture" >&2; rm -rf "$dir"; return 2
  fi
  rpf::_check_versions G1-version "$dir" 0.1.1 rpg-a1 rpg-a2 || fails=$((fails + 1))
  rpf::_check_section G1-changelog "$dir" rpg-a2 0.1.1 '### Fixed' '- *(rs)* x' || fails=$((fails + 1))
  rpf::_check_versions G2-version "$dir" 0.2.0 rpg-b1 rpg-b2 || fails=$((fails + 1))
  rpf::_check_section G2-changelog "$dir" rpg-b1 0.2.0 '### Added' '- *(rs)* x' || fails=$((fails + 1))
  rpf::_check_versions G3-version "$dir" 0.1.0 rpg-c1 rpg-c2 || fails=$((fails + 1))
  rpf::_check_versions G4-version "$dir" 0.1.1 rpg-d1 rpg-d2 || fails=$((fails + 1))
  rpf::_check_once G4-changelog "$dir" '- *(rs)* x' rpg-d1 rpg-d2 || fails=$((fails + 1))
  rm -rf "$dir"
  if [ "$fails" != 0 ]; then return 1; fi
}
```

Extend `rpf::suites` so it also runs the group suite:

```bash
rpf::suites() { # real_toml -> 0/1/2
  local real="$1" rc worst=0
  rc=0; rpf::filter_suite "$real" real || rc=$?
  if [ "$rc" = 2 ]; then return 2; fi
  if [ "$rc" != 0 ]; then worst=1; fi
  rc=0; rpf::group_suite "$real" real || rc=$?
  if [ "$rc" = 2 ]; then return 2; fi
  if [ "$rc" != 0 ]; then worst=1; fi
  return "$worst"
}
```

- [ ] **Step 2: MEASURE the path+version edge in `git_only` mode first**

SMA-658 M7 saw a `git_only` hard error with an unpublished workspace dependency (`.github/CLAUDE.md`). Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
/bin/bash -c 'export PROTO_REPORTER=text; source ci/release-parity/ecosystems/release-plz.sh; time rpf::group_suite "$PWD/rs/release-plz.toml" real'; echo "rc=$?"
```
- If the output shows `FATAL: release-plz update failed on the group fixture` and the replayed release-plz error names the dependency (`rpg-a2`), a registry, or "not published": this is the M7 error. Record the exact error text. Set `RPG_DEP_EDGE=0` and replace the comment above it with: `# 0: MEASURED (SMA-716) — release-plz 0.3.158 in git_only mode rejects the path+version edge: <exact error text>. The edge is dropped. With R1 all members have the same commits, so G1-G4 do not change; NC2's intermediate value for rpg-b1 changes from 0.1.1 (cascade) to 0.1.0.` Then run the command again.
- If it fails for another reason: STOP and report the output.
- If it does not fail on the update: keep `RPG_DEP_EDGE=1` and replace the comment with `# 1: MEASURED (SMA-716) — git_only accepts the path+version edge on release-plz 0.3.158.`

- [ ] **Step 3: Check the predictions and the duration**

Expected on the run with the final `RPG_DEP_EDGE`: seven PASS lines (`G1-version`, `G1-changelog`, `G2-version`, `G2-changelog`, `G3-version`, `G4-version`, `G4-changelog`), no FAIL, `rc=0`. Write down the `real` time.
If an assertion fails: STOP. Report the failing line and the CHANGELOG.md of the named crate. Do not change an expected value.

- [ ] **Step 4: Delete-the-feature check for `changelog_include`**

Temporarily change `rpf::group_suite`'s `real)` arm to `real) with_rc=1; with_inc=0 ;;` with the Edit tool. Run the Step 2 command. Expected: `rc=1` with at least `FAIL  G2-version` and `FAIL  G1-changelog` or `FAIL  G1-version`. Restore the arm with the Edit tool and run again: `rc=0`. (Task 4 makes this a permanent control.)

- [ ] **Step 5: Run the gate**

Run: `/bin/bash ci/release-parity/run.sh; echo "rc=$?"`
Expected: cases.tsv PASS lines, filter PASS lines, the seven group PASS lines, `== all parity cases passed ==`, `rc=0`.

- [ ] **Step 6: README**

In the SMA-716 section of `ci/release-parity/README.md`, insert before `**Cost.**`:

```markdown
**Group fixture.** A second fixture repo with four independent version groups (`rpg-a*` …
`rpg-d*`), so one `release-plz update` run covers G1–G4. Each group has two members with R1's
symmetric `changelog_include`. Each member has a seeded `CHANGELOG.md` in the real crates' shape
(release-plz's header, `## [Unreleased]`, a `## [0.1.0]` section), so release-plz takes the
prepend path where P1 happened. G1: `fix(rs):` on the leader only; both reach 0.1.1, and the
follower's first release section has `- *(rs)* x` under `### Fixed`. G2: `feat(rs):` on the
follower only; both reach 0.2.0, and the leader gets a section. G3: `fix(ci):` only; both stay at
0.1.0. G4: one `fix(rs):` commit on both crates; the line is in each new section exactly once.
The path+version edge (`rpg-X1` depends on `rpg-X2`, the real proto → derive shape):
<RECORD THE STEP 2 MEASUREMENT: "accepted in git_only mode" or "dropped: <error text>">.
```

Replace the angle-bracket text with the Step 2 result. Change the `**Cost.**` line to: `**Cost.** One filter fixture run: <T2> s. One group fixture run: <T3> s (development Mac).` with both measured numbers.

- [ ] **Step 7: Commit**

```bash
git add ci/release-parity/ecosystems/release-plz-filter.sh ci/release-parity/README.md
git commit -m "test(ci): group changelog fixture for changelog_include (SMA-716)" \
  -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: Negative controls NC1–NC3 and their pins

**Files:**
- Modify: `ci/release-parity/ecosystems/release-plz-filter.sh` (add controls above `# --- entry points`)
- Modify: `ci/release-parity/ecosystems/release-plz.sh` (add the second hook)
- Modify: `ci/release-parity/run.sh` (the guard after `source`; the block inside `if [ "$NEGATIVE" = 1 ]; then`)
- Modify: `ci/affected-graph/ci_targets.py` (`RELEASE_PARITY_SH_CALL_SITES`)
- Modify: `ci/release-parity/README.md` ("What guards it" and a new L6)
- Modify: `ci/affected-graph/README.md:309-311`

**Interfaces:**
- Consumes: `rpf::filter_suite REAL MODE`, `rpf::group_suite REAL MODE` (Tasks 2-3), their `FAIL  <id> ` stderr lines.
- Produces: `rpf::negative_controls REAL_TOML` → 0 (every control went red on its named assertion) / 1 (a control did not) / 2 (infrastructure). `ecosystem::extra_negative_control REAL_TOML`.

- [ ] **Step 1: Write the controls**

Insert above `# --- entry points for the hooks in release-plz.sh ---…`:

```bash
# --- negative controls (spec section 4.3) ---------------------------------------------------

# One control: run a suite on a mutated config. It must return rc 1, the FAIL line of $must
# must be there, and the FAIL line of $mustnot (if given) must NOT be there. rc 0 means the
# suite accepted the mutation; rc 2 is INCONCLUSIVE, and both fail the control.
rpf::_one_control() { # label suite mode must mustnot real_toml -> 0/1/2
  local label="$1" suite="$2" mode="$3" must="$4" mustnot="$5" real="$6" errf rc=0
  errf="$(mktemp)" || return 2
  "$suite" "$real" "$mode" >/dev/null 2>"$errf" || rc=$?
  case "$rc" in
    1) ;;
    0) echo "negative-control FAILED: $label: the suite passed on a mutated config" >&2
       rm -f "$errf"; return 1 ;;
    *) echo "negative-control INCONCLUSIVE: $label: infrastructure error (rc=$rc)" >&2
       cat "$errf" >&2; rm -f "$errf"; return 2 ;;
  esac
  if ! grep -qE "^FAIL  $must " "$errf"; then
    echo "negative-control FAILED: $label: the suite went red, but not on $must" >&2
    cat "$errf" >&2; rm -f "$errf"; return 1
  fi
  if [ -n "$mustnot" ] && grep -qE "^FAIL  $mustnot " "$errf"; then
    echo "negative-control FAILED: $label: $mustnot went red too; this control needs it green" >&2
    cat "$errf" >&2; rm -f "$errf"; return 1
  fi
  rm -f "$errf"
  echo "negative-control OK: $label reported red on $must"
}

rpf::negative_controls() { # real_toml -> 0/1/2
  local real="$1" nc rc worst=0
  for nc in 1 2 3; do
    rc=0
    case "$nc" in
      # NC1: no release_commits. Row 7 (`fix(ci): x`) then bumps to 0.1.1.
      1) rpf::_one_control "NC1 (no release_commits)" rpf::filter_suite no-release-commits \
           r07 "" "$real" || rc=$? ;;
      # NC2: no changelog_include, release_commits kept. G2's lockstep breaks.
      2) rpf::_one_control "NC2 (no changelog_include)" rpf::group_suite no-include \
           G2-version "" "$real" || rc=$? ;;
      # NC3: neither key: P1 exactly. G1's versions pass, and the follower's first section
      # stays 0.1.0. The CHANGELOG half must red while the version half passes.
      3) rpf::_one_control "NC3 (no changelog_include, no release_commits)" rpf::group_suite \
           no-include-no-filter G1-changelog G1-version "$real" || rc=$? ;;
    esac
    if [ "$rc" = 2 ]; then return 2; fi
    if [ "$rc" != 0 ]; then worst=1; fi
  done
  return "$worst"
}
```

Append to the SMA-716 block at the end of `ci/release-parity/ecosystems/release-plz.sh`:

```bash

ecosystem::extra_negative_control() { # real_toml -> 0/1/2
  rpf::negative_controls "$1"
}
```

- [ ] **Step 2: Run the controls directly**

Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
/bin/bash -c 'export PROTO_REPORTER=text; source ci/release-parity/ecosystems/release-plz.sh; time rpf::negative_controls "$PWD/rs/release-plz.toml"'; echo "rc=$?"
```
Expected: three `negative-control OK:` lines (on `r07`, `G2-version`, `G1-changelog`), `rc=0`.
If NC3 reports `G1-version went red too` or `not on G1-changelog`: STOP and report. That means release-plz does not reproduce P1 as the spec predicts.

- [ ] **Step 3: Prove the controls can fail**

Temporarily change the NC1 arm's `r07` to `r01` with the Edit tool (row 1 passes in both modes, so NC1 must now fail). Run Step 2. Expected: `negative-control FAILED: NC1 (no release_commits): the suite went red, but not on r01`, `rc=1`. Restore `r07` with the Edit tool. Then temporarily change the NC3 `must` id `G1-changelog` to `G3-version` (G3 passes in NC3). Expected: `rc=1`. Restore it. Run Step 2 again: `rc=0`.

- [ ] **Step 4: Wire the control into `run.sh`**

Insert directly after the SMA-716 `extra_suite` guard line (Task 2, Step 7):

```bash
[ "$ECOSYSTEM" != release-plz ] || declare -F ecosystem::extra_negative_control >/dev/null || { echo "FATAL: release-parity ABORTED: infrastructure error (rc=2): ecosystems/release-plz.sh defines no ecosystem::extra_negative_control (SMA-716)" >&2; exit 2; }
```

Inside the negative-control block, insert directly after `if [ "$NEGATIVE" = 1 ]; then` and before `echo "== negative control: feeding a deliberately wrong expectation =="`:

```bash
  # SMA-716: the release-plz-only controls NC1-NC3 run FIRST, because the base control below
  # exits on its own verdict. A control that does not go red is rc 1; rc 2 is INCONCLUSIVE.
  if declare -F ecosystem::extra_negative_control >/dev/null; then
    xnc=0; ecosystem::extra_negative_control "$REAL_TOML" || xnc=$?
    case "$xnc" in
      0) echo "negative-control OK: every extra control reported red as expected" ;;
      1) echo "negative-control FAILED: an extra control did not report red" >&2; exit 1 ;;
      *) echo "negative-control INCONCLUSIVE: extra control infrastructure error (rc=$xnc)" >&2; exit 2 ;;
    esac
  fi
```

Run:
```bash
/bin/bash ci/release-parity/run.sh --negative-control; echo "rc=$?"
/bin/bash ci/release-parity/run.sh --ecosystem python-semantic-release --negative-control; echo "rc=$?"
/bin/bash ci/release-parity/run.sh --ecosystem semantic-release --negative-control; echo "rc=$?"
```
Expected: the first prints the three NC OK lines, `negative-control OK: every extra control reported red as expected`, then `negative-control OK: harness reported red as expected`, `rc=0`. The other two print only the base control line, `rc=0`.

- [ ] **Step 5: Pin the new lines**

Append to `RELEASE_PARITY_SH_CALL_SITES` in `ci/affected-graph/ci_targets.py` (after the four Task 2 entries, before the closing `)`):

```python
    # SMA-716 — the extra negative-control hook: its guard, its call and both verdict arms.
    # Deleting the call or the rc-1 arm lets NC1-NC3 stop failing the gate.
    '[ "$ECOSYSTEM" != release-plz ] || declare -F ecosystem::extra_negative_control >/dev/null || { echo "FATAL: release-parity ABORTED: infrastructure error (rc=2): ecosystems/release-plz.sh defines no ecosystem::extra_negative_control (SMA-716)" >&2; exit 2; }',
    'if declare -F ecosystem::extra_negative_control >/dev/null; then',
    'xnc=0; ecosystem::extra_negative_control "$REAL_TOML" || xnc=$?',
    '0) echo "negative-control OK: every extra control reported red as expected" ;;',
    '1) echo "negative-control FAILED: an extra control did not report red" >&2; exit 1 ;;',
```

`wired_release_parity` already derives entries `[5:]` from the tuple (Task 2, Step 9), so it needs no edit.

Run: `python3 ci/affected-graph/ci_targets.py --self-test; echo "rc=$?"`
Expected: `rc=0`.

- [ ] **Step 6: Delete-the-feature check for one pin**

With the Edit tool, delete the line `xnc=0; ecosystem::extra_negative_control "$REAL_TOML" || xnc=$?` from `run.sh`. Run the full affected-graph gate with bash 3.2:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
/bin/bash ci/affected-graph/run.sh; echo "rc=$?"
```
Expected: `rc=1` and a row `ci/release-parity/run.sh: xnc=0; ecosystem::extra_negative_control "$REAL_TOML" || xnc=$?`. Re-insert the line with the Edit tool at the same place. Run again: `rc=0`. If the run aborts with a `proto-shim` or `Permission denied` line, re-run it once (`ci/CLAUDE.md`, the affected-smoke entry).

- [ ] **Step 7: README and affected-graph README**

In `ci/release-parity/README.md`:
- In the SMA-716 section, before `**Cost.**`, add:

```markdown
**Negative controls.** `run.sh --negative-control` calls the hook
`ecosystem::extra_negative_control` BEFORE the base control, because the base control exits on
its own verdict. Each control mutates the fixture config and must make one named assertion fail
with rc 1; rc 2 is INCONCLUSIVE and fails the control. NC1: no `release_commits`, so row 7
bumps (`r07`). NC2: no `changelog_include`, so G2's lockstep breaks (`G2-version`). NC3: neither
key, which is P1 exactly: `G1-changelog` must red while `G1-version` stays green.
```

- Change the `**Cost.**` line to add: `The three controls add one filter run and two group runs: <T4> s in all.` with the Step 2 time.
- In "What guards it", after the sentence that ends `…the `exit 1` on "accepted a wrong expectation"),`, add: `SMA-716 adds nine more whole lines: for each of the two hooks, the guard that fails the release-plz run when the module lost the hook, the call, and the verdict arm (both arms for the control hook).`
- Add a limitation after L5:

```markdown
- **L6 — the SMA-716 hook bodies are not pinned.** `RELEASE_PARITY_SH_CALL_SITES` reads only
  `run.sh`. The hook bodies in `ecosystems/release-plz.sh` and all of
  `ecosystems/release-plz-filter.sh` are in no haystack. A hook body replaced with `return 0`
  passes every pin, and the suite or the controls then assert nothing. The guard lines in
  `run.sh` stop only the DELETION of a hook. A second haystack parameter for
  `check_self_invocation` would close this; it was not added because that function has 65 call
  sites in `ci_targets.py`.
```

In `ci/affected-graph/README.md:309-311`, change `the flag parse, the guard, the assertion and the two report arms (`RELEASE_PARITY_SH_CALL_SITES`, whole-line-matched — SMA-530)` to `the flag parse, the guard, the assertion and the two report arms, plus the SMA-716 hook guards, calls and verdict arms (`RELEASE_PARITY_SH_CALL_SITES`, whole-line-matched — SMA-530, SMA-716)`.

- [ ] **Step 8: Commit**

```bash
git add ci/release-parity/ecosystems/release-plz-filter.sh ci/release-parity/ecosystems/release-plz.sh \
  ci/release-parity/run.sh ci/release-parity/README.md ci/affected-graph/ci_targets.py \
  ci/affected-graph/README.md
git commit -m "test(ci): negative controls for the release-plz-only suites (SMA-716)" \
  -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: Contributor and project-memory documentation

**Files:**
- Modify: `CONTRIBUTING.md:103-130` (the `## Commit messages` section)
- Modify: `.github/CLAUDE.md` (the `## release-plz` section, lines 7-110)

**Interfaces:**
- Consumes: the final key text in `rs/release-plz.toml` (Tasks 1-2). No code interfaces.
- Produces: documentation only.

- [ ] **Step 1: CONTRIBUTING.md**

Insert after the paragraph that ends `…any commit footer (e.g. \`Closes #12\`).` (line 120):

```markdown
**Which commits release a crate (SMA-716).** release-plz opens a release for a
crate only when one of its commits is a releasing commit (`release_commits` in
`rs/release-plz.toml`):

- `feat`, `fix` or `perf` with the scope `rs`, `py`, `ts`, `contracts` or
  `deps`, or with no scope. A scope list such as `fix(rs,py)` releases only if
  every scope in it releases. One space after the comma is allowed:
  `fix(rs, py)` also releases.
- Any type with `!`, or a `BREAKING CHANGE:` footer.

The scopes `ci`, `docs`, `release`, `repo`, `claude` and `workspace` do not
release. The PR title becomes the squash commit on `main`, so the PR title
decides.

**Do not use `feat(ci)` or `fix(ci)` on a file inside a crate.** The filter
decides only whether a crate releases. The version level comes from all commits
since the last release. A `feat(ci)` commit on a crate path makes the next
release of that crate a minor bump. For a crate-file change that must not ship,
use `chore`, `ci`, `build`, `docs`, `refactor` or `test`.
```

Extend the Maintenance rule blockquote (line 126-130) with one sentence at its end: `A new scope does not release until you also add it to \`release_commits\` in \`rs/release-plz.toml\`.`

- [ ] **Step 2: `.github/CLAUDE.md`**

Append these bullets at the end of the `## release-plz` section (before `## Publishing, wheels and version lockstep`):

```markdown
- **Rule R1 (SMA-716): every processed member of a version group sets `changelog_include` to
  exactly the other processed members.** "Processed" is Cargo-publishable or `git_only`. Without
  it, a member that moves only because of the group gets NO CHANGELOG section, and the release PR
  body quotes its old section (P1, release PR #306). The include is not transitive (READ,
  updater.rs:241-257), so name every member. Consequence (Q4): each member's section, and the
  `paigasus-proto` GitHub Release body, lists the whole group's commits. `repo:publish-metadata`
  Check 5 enforces R1. A new member: add it to the other members' lists in the same PR.
- **A crate releases only on a releasing commit (SMA-716, `release_commits`).** Releasing:
  `feat`/`fix`/`perf` with scope `rs`, `py`, `ts`, `contracts`, `deps` or no scope (one optional
  space after a comma in a scope list); any `!`; a `BREAKING CHANGE:` footer. Only the subject line
  counts. Consequences: **C1** a non-releasing change to shipped code rides along with the next
  releasing commit; **C2** the filter decides WHETHER, not the LEVEL, so `feat(ci)` on a crate
  path makes the next release minor; **C3** a new publishable crate first releases only after a
  releasing commit touches it; **C4** the "dependencies changed" cascade bypasses the filter;
  **C5** dependabot's `build(deps)` does not release, so a floor change needs a `fix(deps)`
  commit. The filter runs in `update` and `release-pr`, not in `release`.
- **A stale release PR can stay open (SMA-716, READ).** When no package needs an update,
  release-plz returns before it reads open PRs (`release_pr/mod.rs:152-175`). Close such a PR by
  hand; never merge it. Rollout check for SMA-716: after the merge, confirm that release PR #306
  was force-updated and no longer lists `paigasus-kernel`. Rollback: remove the two keys.
- The fixture proof for both keys is `ci/release-parity/ecosystems/release-plz-filter.sh`
  (`repo:release-parity`); see the SMA-716 section of `ci/release-parity/README.md`.
```

- [ ] **Step 3: Check the markers and the text**

Run:
```bash
git diff --stat
grep -c 'ci-targets:begin' CLAUDE.md
```
Expected: only `CONTRIBUTING.md` and `.github/CLAUDE.md` changed in this task; the count is `1`.

- [ ] **Step 4: Commit**

```bash
git add CONTRIBUTING.md .github/CLAUDE.md
git commit -m "docs(release): record the releasing-commit rule and rule R1 (SMA-716)" \
  -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 6: Full gate run

**Files:** none, unless a gate finds a defect. A fix goes in a NEW commit on the task that owns the file.

**Interfaces:**
- Consumes: everything above.
- Produces: a verdict per gate, for the controller's report and the PR description.

- [ ] **Step 1: The release-parity gates, as Moon runs them**

Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run repo:release-parity repo:release-parity-py repo:release-parity-ts --force; echo "rc=$?"
```
Expected: `rc=0`. Moon uses the PATH `bash` (Homebrew 5.3.15 on this Mac). If a task sits at 0% CPU, it is the host pipe state: run the three scripts with `/bin/bash` directly (Task 4, Step 4 commands plus the three bare runs) and report both results.

- [ ] **Step 2: publish-metadata and ruff (bash 4+)**

Run: `moon run repo:publish-metadata repo:ruff-ci --force; echo "rc=$?"`
Expected: `rc=0`. If `stdout.log` is empty and stderr says `declare: -A: invalid option`, bash 3.2 ran it: re-run with `/opt/homebrew/bin/bash ci/publish-metadata/run.sh --negative-control && /opt/homebrew/bin/bash ci/publish-metadata/run.sh`.

- [ ] **Step 3: affected-smoke (bash 3.2 through a shim directory)**

Run:
```bash
shim="$(mktemp -d)"; ln -s /bin/bash "$shim/bash"
PATH="$shim:$HOME/.proto/shims:$HOME/.proto/bin:$PATH" moon run repo:affected-smoke repo:input-liveness --force; echo "rc=$?"
rm -rf "$shim"
```
Expected: `rc=0`. A sub-3s abort with a `proto-shim … Permission denied` line is the known infrastructure abort (`ci/CLAUDE.md`); capture the output, then re-run once.

- [ ] **Step 4: actionlint**

Run: `/opt/homebrew/bin/bash ci/actionlint/run.sh; echo "rc=$?"`
Read the preflight line first. `pipe capacity 65536 bytes` then `rc=0` is a pass. `rc=2` with a message that the pipe holds only 512 bytes is a host condition, not a verdict: report it, and CI gives the verdict. Check 13 (early-exit readers) and check 12 (the report-file marker) are the checks this change can trip.

- [ ] **Step 5: The full affected graph, as CI runs it**

Run the command between the `ci-targets` markers in the root `CLAUDE.md`, with the proto PATH:
```bash
moon ci :build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking \
  :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free \
  :redis-connect-single-site :iam-docker-policy-single-site :error-code-single-site \
  :http-extractor-envelope :input-liveness :promtool :observability-drift \
  :nats-permissions :release-parity :release-parity-py :release-parity-ts \
  :publish-metadata :version-lockstep :workflow-credentials :pyo3-stub-drift :ruff-ci \
  :next-public-free :helm-render :test-e2e \
  --base origin/main --include-relations; echo "rc=$?"
```
Expected: `rc=0`. One bash cannot run every gate on this Mac. For a gate that fails only because of the bash version (an empty stdout plus `declare`/`mapfile` on stderr, or a here-string hang), take the direct run from Steps 1-4 as its verdict and say so in the report. Before you re-run any other failure, copy `.moon/cache/states/<project>/<task>/` out of the repo (root `CLAUDE.md`, "Diagnosing an unattributed moon ci failure").

- [ ] **Step 6: Report**

Report to the controller: each gate's verdict, the measured durations (filter run, group run, three controls), the `RPG_DEP_EDGE` measurement, and the Review Focus 3 behaviour (`fix(rs, py)` releases at 0.1.1, row r26, as decided by Sven). Put the post-merge rollout check in the PR description: confirm that release PR #306 was force-updated and no longer lists `paigasus-kernel`; close a stale release PR by hand.
