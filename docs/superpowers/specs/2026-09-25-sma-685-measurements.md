# SMA-685 measurements

This file records the measurements for SMA-685 (release-plz `version_group` and the
`publish = false` binding crates). Task 1 covers M1, M1b, M1c, and M2, using one
throwaway full clone at commit `cb772393`. Task 4 records M3 and the mutations, using the
same clone and its tags.

## Host

Command (as run):

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"; export PROTO_REPORTER=text
/opt/homebrew/bin/bash --version | sed -n 1p
python3 - <<'PY'
import os, fcntl
r, w = os.pipe()
fcntl.fcntl(w, fcntl.F_SETFL, os.O_NONBLOCK)
n = 0
try:
    while True:
        n += os.write(w, b"x" * 64)
except BlockingIOError:
    pass
print("pipe capacity", n)
PY
release-plz --version
```

Verbatim output:

```
GNU bash, version 5.3.20(1)-release (aarch64-apple-darwin25.6.0)
pipe capacity 512
release-plz 0.3.158
```

MEASURED: the shell is bash 5.x. This meets the plan's expectation.

MEASURED: the release-plz binary is version 0.3.158. This meets the plan's expectation.

MEASURED: the pipe capacity is 512 bytes. This value is below the plan's 8192-byte floor.
The check ran four times in total. Each run read 512 bytes. The result is stable. This
matches a known host condition. The root `CLAUDE.md` file records it under SMA-612. A new
pipe on this development Mac can hold only 512 bytes. That is not the nominal value the
kernel reports.

**Controller ruling (scope decision, not a measurement):** the 8192-byte pipe floor gates
only runs of bash-5 gate scripts, for example `ci/version-lockstep/run.sh`. Task 1 runs no
such script. M1, M1b, M1c, and M2 each run `release-plz update` once, with small heredocs
well under the 512-byte pipe size. So the controller ruled that the pipe-capacity block
does not apply to Task 1, and Steps 2 through 8 ran on this host.

## Baseline

Command (as run):

```bash
C=$S/sma685/clone
mkdir -p $S/sma685 && git clone -q /Users/smaschek/dev/paigasus/paigasus-core "$C"
cd "$C" && git switch -q -c m1 cb772393
git rev-parse --abbrev-ref --symbolic-full-name '@{upstream}' ; echo "upstream-rc=$?"
git rev-parse HEAD origin/main main
grep -n '^version' rs/crates/libs/paigasus-kernel/Cargo.toml rs/crates/bindings/*/Cargo.toml rs/crates/libs/paigasus-proto*/Cargo.toml
git log --oneline paigasus-kernel-v0.1.0..HEAD -- rs/crates/libs/paigasus-kernel
git log --oneline paigasus-proto-v0.2.0..HEAD -- rs/crates/libs/paigasus-proto
```

Verbatim output:

```
fatal: no upstream configured for branch 'm1'
upstream-rc=128
cb77239361f67660eff5f4366ff447ea78e24a22
cb77239361f67660eff5f4366ff447ea78e24a22
cb77239361f67660eff5f4366ff447ea78e24a22
rs/crates/libs/paigasus-kernel/Cargo.toml:9:version = "0.1.0"
rs/crates/bindings/paigasus-node-bindings/Cargo.toml:3:version = "0.1.0"
rs/crates/bindings/paigasus-py-bindings/Cargo.toml:3:version = "0.1.0"
rs/crates/bindings/paigasus-wasm/Cargo.toml:3:version = "0.1.0"
rs/crates/libs/paigasus-proto-derive/Cargo.toml:8:version = "0.2.0"
rs/crates/libs/paigasus-proto/Cargo.toml:6:version = "0.2.0"
1fd87a15 feat(rs): interactive-user auth on the gateway chat surface, and the console playground (SMA-635)
```

(The kernel log query returned no lines, as expected.)

MEASURED: `upstream-rc` is 128 (non-zero). No upstream is set on the fresh branch. This
meets the expectation.

MEASURED: `HEAD`, `origin/main`, and `main` are all `cb77239361f67660eff5f4366ff447ea78e24a22`.
This meets the expectation.

MEASURED: the kernel and the three bindings read version 0.1.0. Both proto crates read
version 0.2.0. This meets the expectation.

MEASURED: the kernel log is empty. The proto log lists `1fd87a15`. This meets the
expectation: an unreleased `feat(rs)` commit touches `paigasus-proto`.

## M1

Command (as run):

```bash
cd "$C"
printf '\n' >> rs/crates/libs/paigasus-kernel/README.md
git -c commit.gpgsign=false commit -q -am 'fix(rs): scratch readme edit for the sma-685 m1 measurement' --no-verify
git -c tag.gpgsign=false tag m1-scratch && git rev-parse m1-scratch
git status --short
(cd rs && RUST_LOG=release_plz_core=debug,git_cmd=trace GIT_TRACE=$S/sma685/m1.gittrace \
  release-plz update >$S/sma685/m1.log 2>&1); echo "rc=$?"
grep -n '^version' rs/crates/libs/paigasus-kernel/Cargo.toml rs/crates/bindings/*/Cargo.toml rs/crates/libs/paigasus-proto*/Cargo.toml
grep -n 'no upstream configured\|already up to date\|next version' $S/sma685/m1.log
grep -n 'checkout' $S/sma685/m1.gittrace
```

Note on the tag command: the repo's global git config sets `tag.gpgsign=true`. A plain
`git tag <name>` under that setting tries to make a signed tag, and a signed tag needs a
message. The first attempt failed with `fatal: no tag message?`. The fix was
`git -c tag.gpgsign=false tag m1-scratch`. This ran after `release-plz update` had already
run against the correct commit. HEAD had not moved: `release-plz update` only edits files,
it does not commit. This is a host git-config artifact, not a release-plz result.

Verbatim output (key lines):

```
[2m2026-09-25T20:14:09.121277Z[0m [33m WARN[0m no upstream configured for branch m1
[2m2026-09-25T20:14:12.981716Z[0m [34mDEBUG[0m next version calculated starting from commits after `64c96242cab3509b4a6d7cb5ef53b9af648be896`
[2m2026-09-25T20:14:14.394513Z[0m [32m INFO[0m paigasus-kernel: next version is 0.1.1
[2m2026-09-25T20:14:14.396651Z[0m [32m INFO[0m paigasus-proto-derive: next version is 0.3.0
[2m2026-09-25T20:14:14.397354Z[0m [32m INFO[0m paigasus-proto: next version is 0.3.0
rc=0
81:...trace: built-in: git checkout m1
```

Result table:

| Crate | Before | After | Expected |
|---|---|---|---|
| paigasus-kernel | 0.1.0 | 0.1.1 | 0.1.1 |
| paigasus-node-bindings | 0.1.0 | 0.1.0 | 0.1.0 |
| paigasus-py-bindings | 0.1.0 | 0.1.0 | 0.1.0 |
| paigasus-wasm | 0.1.0 | 0.1.0 | 0.1.0 |
| paigasus-proto-derive | 0.2.0 | 0.3.0 | 0.3.0 |
| paigasus-proto | 0.2.0 | 0.3.0 | 0.3.0 |

MEASURED: `release-plz update` exits 0. The kernel moves to 0.1.1. The three bindings stay
at 0.1.0. Both proto crates move to 0.3.0. The log has a `no upstream configured` warning.
The git trace shows a checkout of `m1`. This matches the plan's expectation for the B1
symptom in full.

**Watchdog:** the run finished at once (elapsed_since_progress stayed at 0 seconds on the
first check). The watchdog never triggered.

Scratch commit for Task 4 (as run):

```bash
git add -A && git -c commit.gpgsign=false commit -q -m 'chore: scratch m1 result' --no-verify
git -c tag.gpgsign=false tag m1-result && git rev-parse m1-result
cp rs/Cargo.lock $S/sma685/lock-m1-result
git show cb772393:rs/Cargo.lock > $S/sma685/lock-base
```

MEASURED: tag `m1-scratch` = `b068c8b623aff7e30e4c070068d6a08bb9f58f4a`.
MEASURED: tag `m1-result` = `37cced8b114e85769023451897a899dd57d05b79`.

## M1b

Crates.io check (as run):

```bash
curl -s -A 'sma-685-probe (smaschek@outlook.com)' -o /dev/null -w '%{http_code}\n' https://crates.io/api/v1/crates/paigasus-wasm
```

Verbatim output: `404`. MEASURED. This meets the expectation, so `paigasus-wasm` is the
control crate; no substitution was needed.

Command (as run):

```bash
git reset -q --hard m1-scratch && git clean -fdq -e target -e node_modules && git status --short
sed -i '' '/^publish = false$/d' rs/crates/bindings/paigasus-wasm/Cargo.toml
git diff --stat
git -c commit.gpgsign=false commit -q -am 'chore: scratch m1b make paigasus-wasm cargo-publishable' --no-verify
(cd rs && RUST_LOG=release_plz_core=debug,git_cmd=trace release-plz update >$S/sma685/m1b.log 2>&1); echo "rc=$?"
grep -n '^version' rs/crates/libs/paigasus-kernel/Cargo.toml rs/crates/bindings/*/Cargo.toml
grep -n 'paigasus-wasm' $S/sma685/m1b.log
```

Verbatim output (key lines):

```
rs/crates/bindings/paigasus-wasm/Cargo.toml | 1 -
1 file changed, 1 deletion(-)
rc=0
[2m2026-09-25T20:15:26.581609Z[0m [32m INFO[0m paigasus-kernel: next version is 0.1.1
[2m2026-09-25T20:15:26.582926Z[0m [32m INFO[0m paigasus-wasm: next version is 0.1.1
[2m2026-09-25T20:15:26.585071Z[0m [32m INFO[0m paigasus-proto-derive: next version is 0.3.0
[2m2026-09-25T20:15:26.585741Z[0m [32m INFO[0m paigasus-proto: next version is 0.3.0
```

Result table:

| Crate | Before | After | Expected |
|---|---|---|---|
| paigasus-kernel | 0.1.0 | 0.1.1 | 0.1.1 |
| paigasus-wasm (control, `publish = false` removed) | 0.1.0 | 0.1.1 | 0.1.1 |
| paigasus-node-bindings | 0.1.0 | 0.1.0 | 0.1.0 |
| paigasus-py-bindings | 0.1.0 | 0.1.0 | 0.1.0 |

MEASURED: `git diff --stat` shows the manifest changed by exactly one deleted line, so the
manifest did carry an exact `publish = false` line. `release-plz update` exits 0.
`paigasus-wasm` moves to 0.1.1, the group maximum, once it is Cargo-publishable. The other
two bindings stay at 0.1.0.

**B1 cause verdict: CONFIRMED.** Removing Cargo `publish = false` alone makes release-plz
process the crate and take the `version_group` value. This matches §2.1's read of
`updater.rs:283-302` and `project.rs:110-114`.

**Watchdog:** the run finished at once. The watchdog never triggered.

## M1c

Command (as run):

```bash
git reset -q --hard m1-scratch && git clean -fdq -e target -e node_modules && git status --short
python3 - rs/release-plz.toml <<'PY'
import sys
p = sys.argv[1]; s = open(p).read()
old = '[[package]]\nname = "paigasus-wasm"\n'
assert s.count(old) == 1, "paigasus-wasm entry not found once"
s = s.replace(old, old + 'git_only = true\n')
open(p, "w").write(s)
PY
git -c commit.gpgsign=false commit -q -am 'chore: scratch m1c git_only on paigasus-wasm' --no-verify
(cd rs && RUST_LOG=release_plz_core=debug,git_cmd=trace release-plz update >$S/sma685/m1c.log 2>&1); echo "rc=$?"
grep -n '^version' rs/crates/libs/paigasus-kernel/Cargo.toml rs/crates/bindings/*/Cargo.toml
grep -n -i 'paigasus-wasm\|error' $S/sma685/m1c.log
```

Verbatim output (full error text):

```
rc=1
Error: failed to determine next versions

Caused by:
    0: create unreleased repo for spinning worktrees
    1: failed to open repository at ".../clone/rs"
    2: could not find repository at '.../clone/rs'; class=Repository (6); code=NotFound (-3)
```

(Paths shortened for this file; the log holds the full scratchpad path.)

Result table:

| Crate | Before | After |
|---|---|---|
| paigasus-kernel | 0.1.0 | 0.1.0 (unchanged; the run errored) |
| paigasus-wasm | 0.1.0 | 0.1.0 (unchanged; the run errored) |

MEASURED: `release-plz update` errors and exits 1. No manifest changed; the tree stayed
clean, because the error happens before any file is written. This is one of the two
outcomes the plan allows for ("either 0.1.1, or an error").

This error is NOT the `cargo package` failure the spec names for SMA-658 M7 (an
unpublished workspace dependency). The debug log shows release-plz entered
"Processing 1 packages in git_only mode". It then tried to open a git repository directly
at the Cargo manifest directory (`rs/`), not at the clone's repository root. That directory
has no `.git` of its own. The repository root is the clone's top level, so git2 reports
`NotFound`. This is a different failure mode. A `git_only` package whose manifest
directory is not the repository root cannot spin its "unreleased" worktree at all. This is
true for this release-plz version, on this repo's layout.

**M1c verdict: an error, of a kind not previously described in the spec.** The §2.1
inference is that `git_only = true` would move `paigasus-wasm` to 0.1.1. This run neither
confirms nor refutes that inference through a `cargo package` failure. An earlier, unrelated
failure blocks the run first. Record this as open: the SMA-407 fixture's outcome cannot be
reproduced this way on this repo layout. The exact cause needs a repository-root-vs-manifest-
dir fix, or a different fixture shape. Do not guess past this point.

**Watchdog:** the run finished at once (errored quickly). The watchdog never triggered.

## M2

Command, run A (as run):

```bash
git reset -q --hard m1-scratch && git clean -fdq -e target -e node_modules && git status --short
git switch -q -c m2 m1-scratch
git rev-parse --abbrev-ref --symbolic-full-name '@{upstream}' ; echo "upstream-rc=$?"
(cd rs && RUST_LOG=release_plz_core=debug,git_cmd=trace GIT_TRACE=$S/sma685/m2a.gittrace \
  release-plz update >$S/sma685/m2a.log 2>&1); echo "run A rc=$?"
grep -n '^version' rs/crates/libs/paigasus-kernel/Cargo.toml
grep -n 'no upstream configured\|paigasus-kernel' $S/sma685/m2a.log
grep -n 'checkout' $S/sma685/m2a.gittrace
```

Verbatim output, run A (key lines):

```
fatal: no upstream configured for branch 'm2'
upstream-rc=128
run A rc=0
rs/crates/libs/paigasus-kernel/Cargo.toml:9:version = "0.1.1"
[2m2026-09-25T20:17:11.257745Z[0m [33m WARN[0m no upstream configured for branch m2
[2m2026-09-25T20:17:14.528787Z[0m [32m INFO[0m paigasus-kernel: next version is 0.1.1
81:...trace: built-in: git checkout m2
```

MEASURED: with no upstream, the kernel moves to 0.1.1, the same as M1. This matches the
expectation for run A.

**Watchdog (run A):** the run finished at once. The watchdog never triggered.

Command, run B (as run):

```bash
git reset -q --hard m1-scratch && git clean -fdq -e target -e node_modules && git status --short
git rev-parse main origin/main
git branch -u origin/main
git rev-parse --abbrev-ref --symbolic-full-name '@{upstream}'
(cd rs && RUST_LOG=release_plz_core=debug,git_cmd=trace GIT_TRACE=$S/sma685/m2b.gittrace \
  release-plz update >$S/sma685/m2b.log 2>&1); echo "run B rc=$?"
grep -n '^version' rs/crates/libs/paigasus-kernel/Cargo.toml
grep -n 'already up to date\|next version calculated\|diff: Diff' $S/sma685/m2b.log
grep -n 'checkout' $S/sma685/m2b.gittrace
git branch --unset-upstream
```

Verbatim output, run B (key lines):

```
cb77239361f67660eff5f4366ff447ea78e24a22
cb77239361f67660eff5f4366ff447ea78e24a22
branch 'm2' set up to track 'origin/main'.
origin/main
run B rc=0
rs/crates/libs/paigasus-kernel/Cargo.toml:9:version = "0.1.0"
[2m2026-09-25T20:17:23.139329Z[0m [34mDEBUG[0m next version calculated starting from commits after `64c96242cab3509b4a6d7cb5ef53b9af648be896`
[2m2026-09-25T20:17:23.139334Z[0m [32m INFO[0m paigasus-kernel: already up to date
75:...trace: built-in: git checkout main
```

MEASURED: before run B, `main` and `origin/main` are both `cb772393...`. The upstream reads
`origin/main` (a different name than the local branch `m2`). The kernel stays at 0.1.0. The
log reads `paigasus-kernel: already up to date`. The git trace shows a checkout of `main`,
not `m2`. This matches the expectation for run B in full.

Result table (M2):

| Run | Upstream | Kernel before | Kernel after | Log line | Checkout |
|---|---|---|---|---|---|
| A | none | 0.1.0 | 0.1.1 | `no upstream configured` | `m2` |
| B | `origin/main` | 0.1.0 | 0.1.0 (unchanged) | `paigasus-kernel: already up to date` | `main` |

**B2 cause verdict: CONFIRMED.** With no upstream, release-plz walks from the local branch
`m2` and sees the scratch commit. With an upstream of a different name (`origin/main`), it
walks from `main` instead. It sees no new kernel commit past the tag, and reports the
kernel as already up to date. This matches §2.2's read of `git_cmd/src/lib.rs:44-65` and
`lib.rs:177-179`.

**Watchdog (run B):** the run finished at once. The watchdog never triggered.

## M3 and mutations

Recorded by Task 4.
