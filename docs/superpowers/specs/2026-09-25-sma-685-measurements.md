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

**Container ruling (scope decision, not a measurement).** The Host section above MEASURED a
512-byte pipe on this Mac. That is below the 8192-byte floor. `run.sh` needs bash 4+.
Homebrew bash 5 hangs on its heredocs under this pipe state. So the controller ruled that no
M3 step runs `ci/version-lockstep/run.sh` on the host. Every M3 step and every mutation ran
inside a Linux container instead, built once from a Dockerfile in `$S/sma685/m3img/`.

The image starts `FROM rust:1.95.0-bookworm`. It adds `python3`, `python3-pip`, `curl`,
`xz-utils`, and `ca-certificates` from apt. It installs `uv==0.11.16` via
`pip3 install --break-system-packages`. It installs Node 24.16.0 from the official
`nodejs.org` Linux `arm64` tarball (the build host reports `aarch64`). It installs
`pnpm@11.3.0` via `npm i -g`.

The build tagged the image `sma685-m3`. It printed `v24.16.0`, `11.3.0`, `Python 3.11.2`,
`uv 0.11.16 (aarch64-unknown-linux-gnu)`, and `cargo 1.95.0`. MEASURED: the build finished
with no error. The version-print step shows all five tools present.

Every container run mounted the clone at `/w`. It added anonymous volumes over
`/w/ts/node_modules`, `/w/py/.venv`, and `/w/rs/target`. So the host's macOS files were
never read or overwritten:

```bash
docker run --rm -v "$C":/w -v /w/ts/node_modules -v /w/py/.venv -v /w/rs/target \
  -v <script>:/tmp/step.sh -w /w sma685-m3 bash /tmp/step.sh
```

Each container run first ran `pnpm -C ts install --frozen-lockfile` inside the container.
The napi build needs `@napi-rs/cli`, and the mounted `node_modules` volume starts empty.

Per the controller's instruction, Step 1's host-side `pnpm -C ts install` and `uv sync`
were skipped. The container path installs its own `ts/node_modules`. It does not read
`py/.venv` for these steps. So that host provisioning step does no work here. `proto
install` was also not run on the host for M3.1 through the mutations. It was run once, on
the host, ahead of M3.4 only (see that section).

### M3.1 — the old `run.sh` (`--write`, then `--check`)

The clone's tree was reset to `m1-result` on the host. The container then ran
`pnpm -C ts install --frozen-lockfile`, `bash ci/version-lockstep/run.sh --write`, and
`bash ci/version-lockstep/run.sh`. This used the `run.sh` already at that commit (the one at
`cb772393`).

Verbatim output (key lines):

```
version-lockstep: wrote 6 site(s)
write-rc=0
group kernel: source of truth = 0.1.1
group proto: source of truth = 0.3.0
FAIL: [kernel] cargo-package rs/crates/bindings/paigasus-py-bindings/Cargo.toml: expected '0.1.1', found '0.1.0'
FAIL: [kernel] cargo-package rs/crates/bindings/paigasus-node-bindings/Cargo.toml: expected '0.1.1', found '0.1.0'
FAIL: [kernel] cargo-package rs/crates/bindings/paigasus-wasm/Cargo.toml: expected '0.1.1', found '0.1.0'
FAIL: [kernel] cargo-lock rs/Cargo.lock: expected '0.1.1', found '<absent or non-uniform>'
check-rc=1
```

MEASURED: `--write` exits 0 and writes 6 sites. `--check` exits 1 with exactly 4 `FAIL:`
rows: the three binding `cargo-package` rows and the kernel `cargo-lock` row. This matches
the brief's expected value in full, with no correction needed.

### M3.2 — the new `run.sh` (`--write`, then `--check`)

The clone's tree was reset to `m1-result` on the host again. The new `run.sh` (Task 3, at
the worktree HEAD) was then copied over the clone's copy. The container ran
`pnpm -C ts install --frozen-lockfile`, `bash ci/version-lockstep/run.sh --write`, and
`bash ci/version-lockstep/run.sh`.

Verbatim output (key lines):

```
version-lockstep: wrote 9 site(s)
write-rc=0
group kernel: source of truth = 0.1.1
group proto: source of truth = 0.3.0
== all 20 version-lockstep sites agree ==
check-rc=0
rs/crates/libs/paigasus-kernel/Cargo.toml:9:version = "0.1.1"
rs/crates/bindings/paigasus-node-bindings/Cargo.toml:3:version = "0.1.1"
rs/crates/bindings/paigasus-py-bindings/Cargo.toml:3:version = "0.1.1"
rs/crates/bindings/paigasus-wasm/Cargo.toml:3:version = "0.1.1"
rs/crates/libs/paigasus-proto-derive/Cargo.toml:8:version = "0.3.0"
rs/crates/libs/paigasus-proto/Cargo.toml:6:version = "0.3.0"
```

`git diff --stat` (in the clone, after the run) touched 12 files. These are the copied-in
`ci/version-lockstep/run.sh` itself, and the two kernel/proto `pyproject.toml` sites. They
also include `py/uv.lock`, `rs/Cargo.lock`, and the three binding `Cargo.toml` files. Two
more are the node binding's generated `index.js` and `package.json`. The last is the wasm
binding's `package.json`.

MEASURED: `--write` exits 0 and writes 9 sites, three more than M3.1 (the three
`publish = false` bindings). `--check` exits 0 and reports all 20 sites agree. The kernel and
the three bindings read version 0.1.1. Both proto crates read version 0.3.0. This matches the
brief's expected value in full.

### M3.3 — lockdiff

The `lockdiff.py` script from the brief compared M3.2's `rs/Cargo.lock` against
`$S/sma685/lock-m1-result` (the lock before any `--write`) and against `$S/sma685/lock-base`
(the lock at `cb772393`).

Verbatim output, against `lock-m1-result`:

```
workspace	paigasus-node-bindings	['0.1.0']	->	['0.1.1']
workspace	paigasus-py-bindings	['0.1.0']	->	['0.1.1']
workspace	paigasus-wasm	['0.1.0']	->	['0.1.1']
```

Verbatim output, against `lock-base`:

```
workspace	paigasus-kernel	['0.1.0']	->	['0.1.1']
workspace	paigasus-node-bindings	['0.1.0']	->	['0.1.1']
workspace	paigasus-proto	['0.2.0']	->	['0.3.0']
workspace	paigasus-proto-derive	['0.2.0']	->	['0.3.0']
workspace	paigasus-py-bindings	['0.1.0']	->	['0.1.1']
workspace	paigasus-wasm	['0.1.0']	->	['0.1.1']
```

MEASURED: against `lock-m1-result`, the diff shows the three binding workspace entries only.
It shows zero third-party lines. This is three, not the spec §4 M3.3 figure of four. M1's
own `cargo update --workspace` already moved the kernel lock entry before this diff's
baseline was taken. So three is the correct count against `m1-result`. This corrects the
spec's own worked number; it is not a new defect.

MEASURED: against `lock-base`, the diff shows four kernel-family workspace entries (kernel
and the three bindings) and two proto workspace entries. It shows zero third-party lines.
This matches the brief's expected value for the `lock-base` comparison in full.

### M3.4 — the kernel-ts test

This step ran on the host, not the container, per the controller's instruction. The
container's `node_modules` is an anonymous volume, so the host needed its own copy. On the
host, in the clone, `proto install` exited 0. Then `pnpm -C ts install --frozen-lockfile`
exited 0. Then `moon run paigasus-kernel-ts:test --force` ran, watched with a 10-minute
no-progress stop that read `ps` on the background process.

Verbatim output (tail):

```
paigasus-kernel-ts:test |  Test Files  11 passed (11)
paigasus-kernel-ts:test |       Tests  264 passed (264)
paigasus-kernel-ts:test |    Start at  22:44:12
paigasus-kernel-ts:test |    Duration  381ms (import 67%, transform 22%, tests 9%, worker 1%)
Tasks: 4 completed
 Time: 24s 492ms
```

MEASURED: the task finished on its own within seconds. The 10-minute watchdog never
triggered. All 11 test files and all 264 tests passed, with no failing test. This matches
the brief's expected pass. It also re-confirms SMA-680 M2 part B (a manual four-crate bump
against `committed-wasm.test.ts`), now on this three-binding stamp fix.

### Mutations

Each mutation reset the clone to `m1-result` on the host. It then copied in the new
`run.sh`, and inserted the marked line with a small Python script. Each script asserted its
anchor text matched exactly once before writing. A non-matching anchor would abort the
insert, not silently do nothing. The container then ran `pnpm -C ts install --frozen-lockfile`,
`--write`, `--check`, and `--self-test` in one pass.

**Mutation 1** inserts `cargo-package) printf '0' ;;  # MUTATION-SMA-685`. This goes directly
after `write_site`'s `local kind="$1" target="$2" version="$3" abs="$REPO_ROOT/$2"` line and
its following `case "$kind" in` line. So the writer claims success on a `cargo-package` site
without writing it.

Verbatim output (key lines):

```
version-lockstep: wrote 6 site(s)
write-rc=0
FAIL: [kernel] cargo-package rs/crates/bindings/paigasus-py-bindings/Cargo.toml: expected '0.1.1', found '0.1.0'
FAIL: [kernel] cargo-package rs/crates/bindings/paigasus-node-bindings/Cargo.toml: expected '0.1.1', found '0.1.0'
FAIL: [kernel] cargo-package rs/crates/bindings/paigasus-wasm/Cargo.toml: expected '0.1.1', found '0.1.0'
FAIL: [kernel] cargo-lock rs/Cargo.lock: expected '0.1.1', found '<absent or non-uniform>'
check-rc=1
FAIL: self-test: F1 rc=0 got='0', expected rc 0 and 1
selftest-rc=1
```

MEASURED: `--write` exits 0 but writes only 6 sites. The mutated arm intercepts all three
`cargo-package` writes before they touch a file. `--check` exits 1, with the three binding
rows red, plus the kernel `cargo-lock` row (the lock is unstamped for the same reason).
`--self-test` exits 1, on a writer-level fixture (`F1`), not the production-call-site table.
This matches the brief's expected `--check` and `--self-test` result.

**Mutation 2** inserts `cargo-package) continue ;;  # MUTATION-SMA-685`. This goes directly
after `stamp_sites`' `case "$kind" in` line. So the per-site loop skips every
`cargo-package` site before it reaches the writer.

Verbatim output (key lines):

```
version-lockstep: wrote 6 site(s)
write-rc=0
FAIL: [kernel] cargo-package rs/crates/bindings/paigasus-py-bindings/Cargo.toml: expected '0.1.1', found '0.1.0'
FAIL: [kernel] cargo-package rs/crates/bindings/paigasus-node-bindings/Cargo.toml: expected '0.1.1', found '0.1.0'
FAIL: [kernel] cargo-package rs/crates/bindings/paigasus-wasm/Cargo.toml: expected '0.1.1', found '0.1.0'
FAIL: [kernel] cargo-lock rs/Cargo.lock: expected '0.1.1', found '<absent or non-uniform>'
check-rc=1
FAIL: self-test: stamp_sites left cargo-package rs/crates/bindings/paigasus-py-bindings/Cargo.toml at '0.1.0', expected 9.9.9
selftest-rc=1
```

MEASURED: `--write` exits 0 but writes only 6 sites, for the same reason as Mutation 1.
`--check` exits 1, with the same three binding rows and the kernel `cargo-lock` row red.
`--self-test` exits 1, this time on the production-call-site table. It reports that
`stamp_sites` left a binding site unstamped. This matches the brief's expected `--check` and
`--self-test` result. It also confirms that this self-test table covers the real production
call site, not only the writer fixtures.

After each mutation, `git -C $W status --short` (the worktree, not the clone) was empty.
Both mutations touched only the clone's copy of `ci/version-lockstep/run.sh`. The worktree
never changed.
