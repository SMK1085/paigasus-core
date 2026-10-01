<!-- moon-diagnosis:ok -->
<!-- The marker above is for check 12 of repo:actionlint. This plan names the ciReport token only in
     a negative check (Task 4 Step 5), which proves that the new docs do not name the file. -->
# SMA-665 Chisel dpkg Status SBOM Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the SPDX SBOM of `paigasus-iam` and `paigasus-gateway` list one Debian entry for each Ubuntu package of the chisel cut, and make the release floor fail when the SBOM and the chisel fetch list of the same build disagree.

**Architecture:** The `rootfs` stage of `rs/Dockerfile` adds the `base-files_chisel` slice, which writes `/var/lib/chisel/manifest.wall`. A new POSIX `sh` script, `rs/docker/chisel-dpkg-status.sh`, reads that manifest and the `Fetching pool/…` lines of the cut log. It writes one dpkg stanza and one `.md5sums` file for each package into `/rootfs/var/lib/dpkg/status.d/` (the distroless layout). syft 1.52.0's `dpkg-db-cataloger` then finds the packages with no syft change. `ci/images/release_decision.py sbom-floor` gains `--arch`. For a cargo key it reads `chisel-manifest-<key>-<arch>.txt` and requires set equality on (name, version, arch) between that list and the SBOM's `pkg:deb` purls. A `--target` test stage in `rs/Dockerfile` runs the generator's self-test in `images.yml`.

**Tech Stack:** POSIX `sh` (dash), `jq`, `zstd`, Docker BuildKit, chisel v1.4.2, syft 1.52.0, Python 3.12 (stdlib only), GitHub Actions YAML, Markdown.

**Spec:** `docs/superpowers/specs/2026-10-01-sma-665-chisel-dpkg-status-sbom-design.md` (Revision 3, approved by Sven). Read it first. This plan cites its sections as AC1 to AC6, D1 to D10, T1 to T5, M2 to M5, R1 to R6 and Q1 to Q6.

## Global Constraints

- Worktree: `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-665-chisel-dpkg-status-sbom`, branch `feature/sma-665-chisel-dpkg-status-sbom`. Start every Bash command with `cd <worktree> &&`. Never commit, check out or reset in `/Users/smaschek/dev/paigasus/paigasus-core` itself.
- Prefix every command with `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" PROTO_REPORTER=text`. Run `syft` from inside the worktree: outside it, the proto shim cannot find the syft plugin.
- Scratchpad for notes and temporary files: `S=/private/tmp/claude-501/-Users-smaschek-dev-paigasus-paigasus-core/34e7e9c8-7c11-4f0c-ae92-59e16bea1f6a/scratchpad`. Shell state does not persist between Bash calls, so set `S` (and the `PATH` prefix) at the top of each command that uses it. Do not write measurement notes into the repo. Write SBOM files into `out/` (ignored), never into the repo root (`sbom-*.spdx.json` is NOT ignored).
- Conventional commits: scope `rs` for `rs/`, `ci` for `ci/` and the workflows, `docs` for `docs/ops/` and `rs/CLAUDE.md`. End each message with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. No `--no-verify`, no `--no-gpg-sign`, no `git commit --amend`, no `git reset`, no force push. If signing fails with "failed to fill whole buffer" or "communication with agent failed", stop: 1Password is locked.
- Before each commit, `git -C <worktree> branch --show-current` must print `feature/sma-665-chisel-dpkg-status-sbom`.
- Do not leave background jobs running. To wait for a long build, use the Monitor tool with an until-loop. Do not install host software (no `brew`, no global `npm`). A Docker container is allowed.
- Pins that do not change: chisel `v1.4.2` and its two sha384 values, `syft = "1.52.0"` in `.prototools`, the `ubuntu:24.04@sha256:008173c2…` and `rust:1.95.0-bookworm@sha256:6258907a…` FROM lines.
- `rs/docker/*.sh` runs under `/bin/sh` (dash): no pipe at all (dash has no `pipefail`, and `repo:actionlint` check 13 bans a pipe into `head`, `grep -q`/`-m` or `awk … exit`), no bash syntax, `shellcheck -s sh` clean. Each new source file opens with `# SPDX-License-Identifier: Apache-2.0`.
- `rs/Dockerfile`: the test stage `chisel-dpkg-status-test` must stay ABOVE the final `FROM scratch` (D10). The final stage keeps exactly its two COPY lines (`assert_pins`). The text `chisel cut --release ubuntu-24.04` stays on one line. Put the word `cargo` on a comment line only: `ci/affected-graph/cargo_moon_parity.py` A8 reads every non-comment line of `rs/Dockerfile` for a cargo invocation.
- `ci/images/release_decision.py`: stdlib only, never exit 1, `ruff check --config py/pyproject.toml` clean (line length 200).
- No new moon gate. The `ci-targets` block in `CLAUDE.md` and `ci.yml`'s `T=(…)` array do not change (D10).
- New doc text must not name the moon CI report file: `repo:actionlint` check 12 then needs a `moon-diagnosis` marker in that file.
- Write all new prose (comments, docs, commit bodies) in ASD-STE100 Simplified Technical English: short sentences, active voice, no idiom.
- Do not commit a mutation. Restore each mutation with the Edit tool (not `git checkout`), then confirm with `git diff`.

## Measurements taken while planning (2026-10-01, arm64, Docker Desktop on the dev Mac)

The spec says to take M2 and M4 before the plan fixes D4 and T4. The planner took them on a copy of the `rootfs` stage with the prototype of the generator below, and syft 1.52.0 on an OCI archive of that rootfs. amd64 is not measured locally. Task 5 and the pull request's `images.yml` run give the amd64 evidence.

- **M4, fetch-line shape.** In the cut log, each fetch line reads `2026/10/01 20:09:23 Fetching pool/main/g/glibc/libc6_2.39-0ubuntu8.9_arm64.deb...` (three dots at the end). `extract_chisel_manifest` writes `Fetching pool/main/g/glibc/libc6_2.39-0ubuntu8.9_arm64.deb` (no dots). `~` and `+` stay literal in a pool file name. A pool file name has no epoch.
- **M4, the full arm64 fetch list (7 packages):** `base-files 13ubuntu10.5 arm64`, `ca-certificates 20260601~24.04.1 all`, `libssl3t64 3.0.13-0ubuntu3.16 arm64` (source `openssl`), `openssl 3.0.13-0ubuntu3.16 arm64`, `gcc-14-base 14.2.0-4ubuntu2~24.04.1 arm64` (source `gcc-14`), `libc6 2.39-0ubuntu8.9 arm64` (source `glibc`), `libgcc-s1 14.2.0-4ubuntu2~24.04.1 arm64` (source `gcc-14`). The `*_copyright` essentials and `openssl_data` pull in the three packages beyond the four that the spec names.
- **M4, jsonwall header.** `{"jsonwall":"1.0","schema":"1.0","count":154}` for a file of 154 lines. So `count` is the number of lines INCLUDING the header line. The spec (§ 5.2) said "the number of the other lines". The generator uses the measured rule.
- **M4, manifest objects.** `{"kind":"package","name":…,"version":…,"sha256":…,"arch":…}`; `{"kind":"path","path":…,"mode":…,"slices":[…]}` plus `"sha256"` and `"size"` for a regular file, `"link"` for a symlink, and neither for a directory (path ends in `/`); `{"kind":"slice",…}`; `{"kind":"content",…}`. `manifest.wall` itself has a `path` object with no `sha256`. The file is 3474 bytes compressed.
- **M2, purl text.** syft writes `pkg:deb/ubuntu/libc6@2.39-0ubuntu8.9?arch=arm64&distro=ubuntu-24.04&upstream=glibc`. `~` stays raw (`ca-certificates@20260601~24.04.1?arch=all&distro=ubuntu-24.04`). A test stanza with version `1:2.3+dfsg-1~x` gave `zz-epoch@1%3A2.3%2Bdfsg-1~x?arch=arm64&distro=ubuntu-24.04`: `+` is `%2B`, the epoch colon is `%3A`. The `distro` qualifier is present, so `base-files_release-info` supplies `/etc/os-release` (a symlink to `/usr/lib/os-release`). The `upstream` qualifier carries the `Source:` name.
- **M2, `Status:` line.** syft still lists a stanza with no `Status:` line. The generator writes the line anyway (D2).
- **M2 and M3, ownership (decides D4).** Without `.md5sums` files, syft lists the 7 packages AND `pkg:deb/ubuntu/gcc-14@14.2.0-4ubuntu2~24.04.1?distro=ubuntu` (found by `elf-binary-package-cataloger` in `libgcc_s.so.1`). That purl is `pkg:deb` with NO arch qualifier. The spec (M3) expected a `pkg:generic` purl. Under D5 this entry fails the floor twice (no arch, and an entry that is not in the list). With `status.d/<name>.md5sums` files, the `gcc-14` entry is gone and the SBOM has exactly the 7 list entries. So **D4 survives and is required**, not optional: syft reads `<name>.md5sums` for the `status.d` layout.
- **M2, duplicate stanzas.** Four identical `libc6` stanzas in three files (two files with one stanza each, one file with the stanza twice) gave TWO `libc6` entries. syft does not merge all copies. T1's "a list entry the SBOM has twice" row proves "exactly one". The spec's dropped mutation "write each stanza twice" stays dropped.
- **Generator prototype.** The final generator text in Task 1 ran on the real arm64 cut. syft then listed exactly the 7 `pkg:deb` purls of the fetch list, each with `arch=` and `distro=ubuntu-24.04`, and no `gcc-14` entry. The T2 self-test in Task 1 passed 15 of 15 rows in a container from the pinned `ubuntu:24.04` with `jq` and `zstd`, and `shellcheck -s sh` was clean. The T1 rows in Task 3 passed 94 of 94 rows on a scratch copy, `ruff` was clean, and the two T4 floor mutations each redded the named rows.

## Decisions taken while planning (record them in the PR body)

- **P1. The generator fixtures live in `rs/docker/fixtures/`, not `ci/images/fixtures/` (spec § 8, T2).** The build context of `rs/Dockerfile` is `rs/`, so the test stage cannot COPY from `ci/images/`. D9 rejected a named build context, because it breaks the plain `docker build -f rs/Dockerfile rs/` command. The `images.yml` filter entry `rs/docker/**` (D9) covers the fixtures too.
- **P2. D4 is required** (see M2/M3 above). The generator always writes `.md5sums` for a package that owns at least one regular file, and writes no `.md5sums` file for a package with none.
- **P3. The jsonwall `count` includes the header line** (measured, M4).
- **P4. The generator matches a manifest version with an epoch (`1:1.3.dfsg-3`) against the pool version without it (`1.3.dfsg-3`).** The stanza keeps the full version, as dpkg does. No current package has an epoch, but `zlib1g` does in noble, so a slice change could bring one in. The spec did not cover this; without it the build would fail on the first epoch package.
- **P5. The floor also checks the list against `--arch`.** Each list entry must have the arch of `--arch` or `all`. This is an extra reason inside the D5 floor, with its own T1 row.
- **P6. The generator writes into a temporary directory and moves `status.d` into place only at the end.** A failed run leaves no `status.d` behind. T2 checks this for each fail row.
- **P7. `sbom_floor` takes `debs`, `chisel` and `arch` as required keyword arguments.** A cargo key with `chisel=None` is a `UsageError` (exit 2), never a pass.

## Review Focus

The five input classes or failure modes that the spec implies but that no unit test row fully exercises, most likely first:

1. **The amd64 cut differs from the arm64 cut** (another fetch list, multiarch paths `x86_64-linux-gnu`). A reasonable person expects the amd64 floor to pass the same way. Pinned by Task 5 Step 9: the pull request's `images.yml` amd64 leg runs the real floor, and the reviewer reads its `M8` line and the floor output.
2. **A package version with an epoch** in the real archive. Expected: the build passes, the stanza keeps the epoch, and the floor matches the SBOM (epoch dropped) with the pool version. Pinned by the `zlib1g` fixture row in Task 1 (T2 pass row) and the two epoch rows in Task 3 (T1).
3. **A future syft that reads `manifest.wall`** (D6, R6). Expected: the floor fails loudly with "expected exactly once". Pinned by the T1 row "a list entry the SBOM has twice" in Task 3, and by the runbook text in Task 4.
4. **A console release chain on the new `--arch` argument.** Expected: the npm floor passes with the same arguments as the cargo chain and reads no list file. Pinned by the T1 row "cli: npm with the cargo arguments and no list file passes" in Task 3, and by the `Console release sequence` step of the pull request's `images.yml` run (Task 5 Step 9).
5. **The release job reads the list from the wrong directory.** Expected: `release.yml` runs `sbom-floor` from the repo root, where `build-oci` wrote `chisel-manifest-<key>-<arch>.txt` in the same job (D7). A missing list is exit 2, never a pass. Pinned by the T1 row "cli: cargo with no list file is exit 2" in Task 3. No test runs `release.yml` itself; the reviewer checks that the step has no `working-directory:`.

---

### Task 1: The generator, its fixtures and its self-test

**Files:**
- Create: `rs/docker/chisel-dpkg-status.sh`
- Create: `rs/docker/chisel-dpkg-status-test.sh`
- Create: `rs/docker/fixtures/manifest.jsonl`, `rs/docker/fixtures/cut.log`
- Create: `rs/docker/fixtures/root/etc/issue`, `rs/docker/fixtures/root/usr/lib/x86_64-linux-gnu/libc.so.6`, `rs/docker/fixtures/root/usr/share/doc/libc6/copyright`
- Create: `rs/docker/fixtures/expected/{base-files,base-files.md5sums,ca-certificates,libc6,libc6.md5sums,zlib1g}`

**Interfaces:**
- Consumes: nothing from other tasks.
- Produces: `rs/docker/chisel-dpkg-status.sh ROOT CUT_LOG` (exit 0 on success, exit 1 with one `chisel-dpkg-status: …` line on stderr on any failure, and then no `ROOT/var/lib/dpkg/status.d`). `rs/docker/chisel-dpkg-status-test.sh GENERATOR FIXTURE_DIR` (exit 0 and `chisel-dpkg-status self-test: 15 rows OK`, else exit 1). Task 2 calls both from `rs/Dockerfile` with the paths `/usr/local/bin/chisel-dpkg-status.sh` and `/fixtures`.

- [ ] **Step 1: Write the fixtures**

The fixture is a jsonwall in PLAIN TEXT (the test compresses it with `zstd` at run time, so no binary is committed). The `count` is 20: the file has 20 lines, the header included (M4). The `sha256` values are dummies: the generator does not read them. The md5 values in `expected/` are the md5 of the three root files exactly as written below (no other bytes).

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-665-chisel-dpkg-status-sbom
mkdir -p rs/docker/fixtures/root/etc rs/docker/fixtures/root/usr/lib/x86_64-linux-gnu \
  rs/docker/fixtures/root/usr/share/doc/libc6 rs/docker/fixtures/expected
printf 'fixture issue\n' > rs/docker/fixtures/root/etc/issue
printf 'fixture libc\n' > rs/docker/fixtures/root/usr/lib/x86_64-linux-gnu/libc.so.6
printf 'fixture libc6 copyright\n' > rs/docker/fixtures/root/usr/share/doc/libc6/copyright
```

Write `rs/docker/fixtures/manifest.jsonl` with exactly this content (20 lines):

```text
{"jsonwall":"1.0","schema":"1.0","count":20}
{"kind":"content","slice":"base-files_base","path":"/etc/issue"}
{"kind":"content","slice":"ca-certificates_data","path":"/etc/ssl/certs/"}
{"kind":"content","slice":"libc6_copyright","path":"/usr/share/doc/libc6/copyright"}
{"kind":"content","slice":"libc6_libs","path":"/usr/lib/x86_64-linux-gnu/libc.so.6"}
{"kind":"content","slice":"libc6_libs","path":"/usr/lib64/ld-linux-x86-64.so.2"}
{"kind":"package","name":"base-files","version":"13ubuntu10.5","sha256":"1111111111111111111111111111111111111111111111111111111111111111","arch":"amd64"}
{"kind":"package","name":"ca-certificates","version":"20260601~24.04.1","sha256":"2222222222222222222222222222222222222222222222222222222222222222","arch":"all"}
{"kind":"package","name":"libc6","version":"2.39-0ubuntu8.9","sha256":"3333333333333333333333333333333333333333333333333333333333333333","arch":"amd64"}
{"kind":"package","name":"zlib1g","version":"1:1.3.dfsg-3.1ubuntu2.1","sha256":"7777777777777777777777777777777777777777777777777777777777777777","arch":"amd64"}
{"kind":"path","path":"/etc/","mode":"0755","slices":["base-files_base"]}
{"kind":"path","path":"/etc/issue","mode":"0644","slices":["base-files_base"],"sha256":"4444444444444444444444444444444444444444444444444444444444444444","size":14}
{"kind":"path","path":"/etc/ssl/certs/","mode":"0755","slices":["ca-certificates_data"]}
{"kind":"path","path":"/usr/lib/x86_64-linux-gnu/libc.so.6","mode":"0755","slices":["libc6_libs"],"sha256":"5555555555555555555555555555555555555555555555555555555555555555","size":13}
{"kind":"path","path":"/usr/lib64/ld-linux-x86-64.so.2","mode":"0777","slices":["libc6_libs"],"link":"../lib/x86_64-linux-gnu/ld-linux-x86-64.so.2"}
{"kind":"path","path":"/usr/share/doc/libc6/copyright","mode":"0644","slices":["libc6_copyright"],"sha256":"6666666666666666666666666666666666666666666666666666666666666666","size":24}
{"kind":"slice","name":"base-files_base"}
{"kind":"slice","name":"ca-certificates_data"}
{"kind":"slice","name":"libc6_copyright"}
{"kind":"slice","name":"libc6_libs"}
```

Write `rs/docker/fixtures/cut.log` with exactly this content:

```text
2026/10/01 20:09:21 Selecting slices...
2026/10/01 20:09:23 Fetching pool/main/b/base-files/base-files_13ubuntu10.5_amd64.deb...
2026/10/01 20:09:23 Fetching pool/main/c/ca-certificates/ca-certificates_20260601~24.04.1_all.deb...
2026/10/01 20:09:23 Fetching pool/main/g/glibc/libc6_2.39-0ubuntu8.9_amd64.deb...
2026/10/01 20:09:23 Fetching pool/main/z/zlib/zlib1g_1.3.dfsg-3.1ubuntu2.1_amd64.deb...
2026/10/01 20:09:23 Extracting files from package "base-files"...
2026/10/01 20:09:23 Generating manifest at /var/lib/chisel/manifest.wall...
```

Write the six expected files with exactly this content:

`rs/docker/fixtures/expected/base-files`:
```text
Package: base-files
Status: install ok installed
Architecture: amd64
Version: 13ubuntu10.5
```

`rs/docker/fixtures/expected/base-files.md5sums` (two spaces between the sum and the path):
```text
73be2d9ed0c438b6346dcf4cfa854261  etc/issue
```

`rs/docker/fixtures/expected/ca-certificates` (no `.md5sums` file: the package owns no regular file in the fixture):
```text
Package: ca-certificates
Status: install ok installed
Architecture: all
Version: 20260601~24.04.1
```

`rs/docker/fixtures/expected/libc6`:
```text
Package: libc6
Status: install ok installed
Architecture: amd64
Version: 2.39-0ubuntu8.9
Source: glibc
```

`rs/docker/fixtures/expected/libc6.md5sums` (the symlink `/usr/lib64/ld-linux-x86-64.so.2` is NOT listed: it has no `sha256`):
```text
58d1a5433c09ff3cd1102e6dd2f8b045  usr/lib/x86_64-linux-gnu/libc.so.6
233e3e2fc8bb1ea90fe5e9631d439c71  usr/share/doc/libc6/copyright
```

`rs/docker/fixtures/expected/zlib1g` (the version keeps its epoch; the pool line has none, P4):
```text
Package: zlib1g
Status: install ok installed
Architecture: amd64
Version: 1:1.3.dfsg-3.1ubuntu2.1
Source: zlib
```

Check the md5 values against the files (macOS `md5 -q`):

```bash
md5 -q rs/docker/fixtures/root/etc/issue rs/docker/fixtures/root/usr/lib/x86_64-linux-gnu/libc.so.6 rs/docker/fixtures/root/usr/share/doc/libc6/copyright
```

Expected: `73be2d9ed0c438b6346dcf4cfa854261`, `58d1a5433c09ff3cd1102e6dd2f8b045`, `233e3e2fc8bb1ea90fe5e9631d439c71`. Also check that git does not ignore a fixture: `git check-ignore -v rs/docker/fixtures/root/usr/lib/x86_64-linux-gnu/libc.so.6 rs/docker/fixtures/cut.log` must print nothing and exit 1.

- [ ] **Step 2: Write the self-test**

Create `rs/docker/chisel-dpkg-status-test.sh`:

```sh
#!/bin/sh
# SPDX-License-Identifier: Apache-2.0
#
# SMA-665 T2: the self-test of chisel-dpkg-status.sh. It runs in the chisel-dpkg-status-test
# stage of rs/Dockerfile (spec D10), which has the same jq, zstd and dash as the rootfs stage.
#
# Usage: chisel-dpkg-status-test.sh GENERATOR FIXTURE_DIR
#
# One pass row compares the whole status.d tree with FIXTURE_DIR/expected. Each fail row
# changes one input and must see a non-zero exit, its own stderr message (so that a syntax
# error cannot pass as a failure) and no status.d directory left behind.
set -eu

[ "$#" -eq 2 ] || { echo "usage: chisel-dpkg-status-test.sh GENERATOR FIXTURE_DIR" >&2; exit 2; }
gen=$1
fix=$2
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
rows=0
failed=0

ok() {
  rows=$((rows + 1))
  echo "ok   $1"
}

bad() {
  rows=$((rows + 1))
  failed=$((failed + 1))
  echo "FAIL $1: $2" >&2
}

# setup ROW MANIFEST_JSONL CUT_LOG: a fresh root under $work/ROW from the fixture files.
setup() {
  mkdir "$work/$1"
  cp -R "$fix/root" "$work/$1/root"
  mkdir -p "$work/$1/root/var/lib/chisel"
  zstd -q -o "$work/$1/root/var/lib/chisel/manifest.wall" "$2"
  cp "$3" "$work/$1/cut.log"
}

# expect_fail ROW STDERR_FRAGMENT: run the generator on the ROW root, which must fail.
expect_fail() {
  d="$work/$1/root/var/lib/dpkg/status.d"
  before=absent
  if [ -e "$d" ]; then before=present; fi
  if sh "$gen" "$work/$1/root" "$work/$1/cut.log" > "$work/$1/out" 2> "$work/$1/err"; then
    bad "$1" "the generator exited 0"
    return 0
  fi
  if ! grep -F -q -e "$2" "$work/$1/err"; then
    bad "$1" "stderr does not name '$2'; it is: $(cat "$work/$1/err")"
    return 0
  fi
  if [ "$before" = absent ] && [ -e "$d" ]; then
    bad "$1" "the failed run left $d behind"
    return 0
  fi
  ok "$1"
}

m="$fix/manifest.jsonl"
l="$fix/cut.log"
v="$work/inputs"
mkdir "$v"

# Pass: the exact status.d tree, and manifest.wall stays in the image (spec D1, Q4).
setup pass "$m" "$l"
if sh "$gen" "$work/pass/root" "$work/pass/cut.log" 2> "$work/pass/err"; then
  if ! diff -r "$fix/expected" "$work/pass/root/var/lib/dpkg/status.d" > "$work/pass/diff"; then
    bad pass "status.d differs from $fix/expected: $(cat "$work/pass/diff")"
  elif [ ! -f "$work/pass/root/var/lib/chisel/manifest.wall" ]; then
    bad pass "the generator removed manifest.wall"
  else
    ok pass
  fi
else
  bad pass "the generator failed: $(cat "$work/pass/err")"
fi

# Usage and missing inputs.
mkdir "$work/usage"
if sh "$gen" "$work/usage" > /dev/null 2> "$work/usage/err"; then
  bad usage "the generator exited 0 with one argument"
elif ! grep -F -q -e "usage:" "$work/usage/err"; then
  bad usage "stderr does not name 'usage:'"
else
  ok usage
fi

setup no-manifest "$m" "$l"
rm "$work/no-manifest/root/var/lib/chisel/manifest.wall"
expect_fail no-manifest "no chisel manifest"

setup no-cut-log "$m" "$l"
rm "$work/no-cut-log/cut.log"
expect_fail no-cut-log "no chisel cut log"

setup not-zstd "$m" "$l"
cp "$m" "$work/not-zstd/root/var/lib/chisel/manifest.wall"
expect_fail not-zstd "zstd cannot decompress"

# Existing dpkg data (a chisel that writes it itself).
setup status-exists "$m" "$l"
mkdir -p "$work/status-exists/root/var/lib/dpkg"
: > "$work/status-exists/root/var/lib/dpkg/status"
expect_fail status-exists "status already exists"

setup status-d-exists "$m" "$l"
mkdir -p "$work/status-d-exists/root/var/lib/dpkg/status.d"
expect_fail status-d-exists "status.d already exists"

# The jsonwall header.
sed '1s/"count":20/"count":21/' "$m" > "$v/count.jsonl"
setup header-count "$v/count.jsonl" "$l"
expect_fail header-count "its header is not jsonwall 1.0"

sed '1s/"schema":"1.0"/"schema":"2.0"/' "$m" > "$v/schema.jsonl"
setup header-schema "$v/schema.jsonl" "$l"
expect_fail header-schema "its header is not jsonwall 1.0"

# No package object. The count is corrected, so the header check passes and this row reaches
# the package check.
grep -v '"kind":"package"' "$m" > "$v/nopkg.tmp"
sed '1s/"count":20/"count":16/' "$v/nopkg.tmp" > "$v/nopkg.jsonl"
setup no-package "$v/nopkg.jsonl" "$l"
expect_fail no-package "holds no package object"

# The fetch lines (spec D3).
grep -v 'libc6_' "$l" > "$v/nofetch.log"
setup no-fetch-line "$m" "$v/nofetch.log"
expect_fail no-fetch-line "libc6 2.39-0ubuntu8.9 amd64 has 0 matching fetch line(s)"

cp "$l" "$v/twofetch.log"
grep 'libc6_' "$l" >> "$v/twofetch.log"
setup two-fetch-lines "$m" "$v/twofetch.log"
expect_fail two-fetch-lines "libc6 2.39-0ubuntu8.9 amd64 has 2 matching fetch line(s)"

sed 's/libc6_2.39-0ubuntu8.9_amd64/libc6_2.39-0ubuntu8.9_arm64/' "$l" > "$v/arch.log"
setup arch-mismatch "$m" "$v/arch.log"
expect_fail arch-mismatch "libc6 2.39-0ubuntu8.9 amd64 has 0 matching fetch line(s)"

sed 's/libc6_2.39-0ubuntu8.9_amd64/libc6_2.39-0ubuntu8.8_amd64/' "$l" > "$v/version.log"
setup version-mismatch "$m" "$v/version.log"
expect_fail version-mismatch "libc6 2.39-0ubuntu8.9 amd64 has 0 matching fetch line(s)"

cp "$l" "$v/unparsable.log"
echo '2026/10/01 20:09:23 Fetching pool/main/glibc/libc6_2.39-0ubuntu8.9_amd64.deb...' >> "$v/unparsable.log"
setup unparsable-fetch-line "$m" "$v/unparsable.log"
expect_fail unparsable-fetch-line "1 fetch line(s)"

if [ "$failed" -ne 0 ]; then
  echo "chisel-dpkg-status self-test: $failed of $rows rows failed" >&2
  exit 1
fi
echo "chisel-dpkg-status self-test: $rows rows OK"
```

- [ ] **Step 3: Run the self-test to verify it fails**

The test needs dash, `jq` and `zstd`. The dev Mac has no `dpkg` world, so run it in a container from the pinned base image (a container is allowed; a host install is not):

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-665-chisel-dpkg-status-sbom
docker run --rm -v "$PWD/rs/docker:/d:ro" \
  ubuntu:24.04@sha256:008173c23f95b170204355c12626cb5a965d779a7e1283b09e9cffbb1bf33ca3 \
  sh -c 'apt-get update -qq >/dev/null && apt-get install -y -qq --no-install-recommends jq zstd >/dev/null && sh /d/chisel-dpkg-status-test.sh /d/chisel-dpkg-status.sh /d/fixtures'; echo "rc=$?"
```

Expected: `FAIL pass: the generator failed: sh: 0: cannot open /d/chisel-dpkg-status.sh: No such file` and FAIL lines for the fail rows whose stderr does not name their message, then `rc=1`.

- [ ] **Step 4: Write the generator**

Create `rs/docker/chisel-dpkg-status.sh`:

```sh
#!/bin/sh
# SPDX-License-Identifier: Apache-2.0
#
# SMA-665: writes dpkg status data for the packages of a chisel cut, so that syft's
# dpkg-db-cataloger lists them in the image SBOM. syft 1.52.0 has no cataloger for chisel's own
# manifest (/var/lib/chisel/manifest.wall).
#
# Usage: chisel-dpkg-status.sh ROOT CUT_LOG
#   ROOT     the chisel cut root; it must hold var/lib/chisel/manifest.wall (the
#            base-files_chisel slice).
#   CUT_LOG  the output of `chisel cut`; its `Fetching pool/...` lines give each package's
#            source name.
#
# Writes ROOT/var/lib/dpkg/status.d/<package> (one stanza) and, when the package owns a
# regular file, ROOT/var/lib/dpkg/status.d/<package>.md5sums. This is the distroless layout.
# syft reads the .md5sums file as file ownership: without it, syft's ELF cataloger also reports
# libgcc_s.so.1 as a second package, gcc-14, with a pkg:deb purl and no arch (spec M2, M3).
#
# It fails closed (exit 1, one line on stderr) on any input it does not understand, and then
# writes nothing. It runs under dash: no pipes (dash has no pipefail), no bash syntax.
set -eu

die() {
  echo "chisel-dpkg-status: $*" >&2
  exit 1
}

[ "$#" -eq 2 ] || die "usage: chisel-dpkg-status.sh ROOT CUT_LOG"
root=$1
log=$2
wall="$root/var/lib/chisel/manifest.wall"
dpkg_dir="$root/var/lib/dpkg"

[ -f "$wall" ] || die "no chisel manifest at $wall; the cut needs the base-files_chisel slice"
[ -f "$log" ] || die "no chisel cut log at $log"
# A chisel that starts to write dpkg data itself must not be overwritten or doubled.
[ ! -e "$dpkg_dir/status" ] || die "$dpkg_dir/status already exists; chisel writes dpkg data itself now"
[ ! -e "$dpkg_dir/status.d" ] || die "$dpkg_dir/status.d already exists; chisel writes dpkg data itself now"

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

zstd -q -dc "$wall" > "$tmp/manifest.jsonl" || die "zstd cannot decompress $wall"

# The jsonwall header. MEASURED (spec M4, chisel v1.4.2): {"jsonwall":"1.0","schema":"1.0",
# "count":N}, where N is the number of lines INCLUDING the header line.
jq -e -s 'length >= 1 and .[0].jsonwall == "1.0" and .[0].schema == "1.0" and .[0].count == length' \
  "$tmp/manifest.jsonl" > /dev/null \
  || die "the manifest $wall does not parse, or its header is not jsonwall 1.0, schema 1.0 with a count equal to its line count"

jq -r -s '.[1:][] | select(.kind == "package") | "\(.name) \(.version) \(.arch)"' \
  "$tmp/manifest.jsonl" > "$tmp/packages" || die "jq cannot read the package objects of $wall"
[ -s "$tmp/packages" ] || die "the manifest $wall holds no package object"

# Every fetch line must parse. The source name is the pool directory (spec D3):
#   Fetching pool/<component>/<prefix>/<source>/<name>_<version>_<arch>.deb...
grep -oE 'Fetching pool/[^ ]*' "$log" > "$tmp/fetch.raw" || true
sed -n 's#^Fetching pool/[^/]*/[^/]*/\([^/_]*\)/\([^/_]*\)_\([^/_]*\)_\([^/_.]*\)\.deb\.\.\.$#\2 \3 \4 \1#p' \
  "$tmp/fetch.raw" > "$tmp/fetch"
raw_n=$(wc -l < "$tmp/fetch.raw")
ok_n=$(wc -l < "$tmp/fetch")
[ "$raw_n" -eq "$ok_n" ] \
  || die "$((raw_n - ok_n)) fetch line(s) in $log do not parse as pool/<component>/<prefix>/<source>/<name>_<version>_<arch>.deb"

out="$tmp/status.d"
mkdir "$out"
chmod 0755 "$out"
while read -r name version arch; do
  # A pool file name has no epoch: 1:1.3.dfsg-3 is fetched as <name>_1.3.dfsg-3_<arch>.deb. The
  # stanza keeps the full version, as dpkg does.
  case $version in
    *:*) pool_version=${version#*:} ;;
    *) pool_version=$version ;;
  esac
  match=$(awk -v n="$name" -v v="$pool_version" -v a="$arch" \
    '$1 == n && $2 == v && $3 == a { c++; s = $4 } END { print c + 0, s }' "$tmp/fetch")
  count=${match%% *}
  source=${match#* }
  [ "$count" -eq 1 ] \
    || die "package $name $version $arch has $count matching fetch line(s) in $log; expected exactly 1"
  {
    printf 'Package: %s\n' "$name"
    printf 'Status: install ok installed\n'
    printf 'Architecture: %s\n' "$arch"
    printf 'Version: %s\n' "$version"
    # dpkg omits Source: when the source name equals the package name.
    if [ "$source" != "$name" ]; then
      printf 'Source: %s\n' "$source"
    fi
  } > "$out/$name"
  # The regular files of the package's slices (a slice name is <package>_<slice>). A directory
  # or a symlink has no sha256 in the manifest.
  jq -r --arg p "$name" \
    'select(.kind == "path" and has("sha256") and any(.slices[]; split("_")[0] == $p)) | .path' \
    "$tmp/manifest.jsonl" > "$tmp/paths" || die "jq cannot read the path objects of $wall"
  : > "$tmp/md5"
  while read -r path; do
    sum=$(md5sum "$root$path") || die "md5sum cannot read $root$path"
    printf '%s  %s\n' "${sum%% *}" "${path#/}" >> "$tmp/md5"
  done < "$tmp/paths"
  if [ -s "$tmp/md5" ]; then
    LC_ALL=C sort -k 2 "$tmp/md5" > "$out/$name.md5sums"
  fi
done < "$tmp/packages"

mkdir -p "$dpkg_dir"
mv "$out" "$dpkg_dir/status.d"
```

Then `chmod 0755 rs/docker/chisel-dpkg-status.sh rs/docker/chisel-dpkg-status-test.sh` (the Dockerfile uses `COPY --chmod=0755`, so the build does not depend on the git exec bit, but a local run is easier with it).

- [ ] **Step 5: Run the self-test to verify it passes, and lint both scripts**

Run the `docker run` command of Step 3 again.
Expected: 15 `ok` lines (`pass`, `usage`, `no-manifest`, `no-cut-log`, `not-zstd`, `status-exists`, `status-d-exists`, `header-count`, `header-schema`, `no-package`, `no-fetch-line`, `two-fetch-lines`, `arch-mismatch`, `version-mismatch`, `unparsable-fetch-line`), then `chisel-dpkg-status self-test: 15 rows OK` and `rc=0`.

```bash
shellcheck -s sh rs/docker/chisel-dpkg-status.sh rs/docker/chisel-dpkg-status-test.sh; echo "sc=$?"
grep -n '|' rs/docker/chisel-dpkg-status.sh rs/docker/chisel-dpkg-status-test.sh
```

Expected: `sc=0`. The `grep` may print only `||` lines and the `|` inside the `jq` and `sed` program texts. It must print no shell pipe.

- [ ] **Step 6: Mutation proof of the epoch rule and the md5sums rule (AC6)**

Each mutation must red the self-test, and each must run (not die at a syntax error). Restore each with the Edit tool. The file is not committed yet, so `git diff` cannot show the restore: check it with `grep -n 'pool_version=\${version#\*:}' rs/docker/chisel-dpkg-status.sh` and `grep -n 'if \[ -s "\$tmp/md5" \]' rs/docker/chisel-dpkg-status.sh` (one line each) and with Step 3 below.

1. In `rs/docker/chisel-dpkg-status.sh`, change `    *:*) pool_version=${version#*:} ;;` to `    *:*) pool_version=$version ;;`. Run Step 3's command. Expected: `FAIL pass: the generator failed: chisel-dpkg-status: package zlib1g 1:1.3.dfsg-3.1ubuntu2.1 amd64 has 0 matching fetch line(s) …` and `rc=1`. Restore.
2. Change `  if [ -s "$tmp/md5" ]; then` to `  if false; then`. Run Step 3's command. Expected: `FAIL pass: status.d differs from /d/fixtures/expected: Only in /d/fixtures/expected: base-files.md5sums …` and `rc=1`. Restore.
3. Run Step 5 again. Expected: 15 rows OK.

- [ ] **Step 7: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-665-chisel-dpkg-status-sbom
git branch --show-current   # must print feature/sma-665-chisel-dpkg-status-sbom
git add rs/docker/
git commit -m "feat(rs): generate dpkg status.d data from the chisel manifest (SMA-665)

The script reads /var/lib/chisel/manifest.wall and the fetch lines of the
chisel cut. It writes one dpkg stanza and one md5sums file for each
package, in the distroless status.d layout. The source name comes from
the pool directory of the fetch line. Any input that the script does not
understand stops the build, and the script then writes nothing.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: The cut writes the chisel manifest and the dpkg data, and images.yml runs the self-test

**Files:**
- Modify: `rs/Dockerfile:20-26` (the `cargo auditable` comment), `rs/Dockerfile:39-61` (stage `rootfs`), and add the stage `chisel-dpkg-status-test` between line 61 and the final `FROM scratch` (line 63)
- Modify: `.github/workflows/images.yml:36-37` (the `pull_request` filter) and `:135-136` (add the T2 step after `Decision script self-test`)

**Interfaces:**
- Consumes: `rs/docker/chisel-dpkg-status.sh ROOT CUT_LOG`, `rs/docker/chisel-dpkg-status-test.sh GENERATOR FIXTURE_DIR` and `rs/docker/fixtures/` from Task 1.
- Produces: an image whose `/var/lib/dpkg/status.d/` holds one stanza and (for a package with regular files) one `.md5sums` file for each package of the cut, and whose `/var/lib/chisel/manifest.wall` stays in place. A build target `chisel-dpkg-status-test`. Task 3's floor and Task 5's evidence rely on the image content.

- [ ] **Step 1: Run the T2 target to verify it fails**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-665-chisel-dpkg-status-sbom
docker buildx build --progress=plain --no-cache-filter=chisel-dpkg-status-test --target chisel-dpkg-status-test -f rs/Dockerfile rs/; echo "rc=$?"
```

Expected: `ERROR: failed to solve: target stage "chisel-dpkg-status-test" could not be found` and a non-zero rc.

- [ ] **Step 2: Add the OS half to the `cargo auditable` comment**

In `rs/Dockerfile`, replace:

```dockerfile
# `cargo auditable` embeds the crate list in the binary, which is what makes the image SBOM list
# the Rust dependencies. MEASURED on 2026-09-20 (spec § 4.5): a plain `cargo build` gives
```

with:

```dockerfile
# `cargo auditable` embeds the crate list in the binary, which is what makes the image SBOM list
# the Rust dependencies. The OS half of the SBOM comes from the `rootfs` stage below: the
# `base-files_chisel` slice and docker/chisel-dpkg-status.sh (SMA-665).
# MEASURED on 2026-09-20 (spec § 4.5): a plain `cargo build` gives
```

- [ ] **Step 3: Change the `rootfs` stage**

In `rs/Dockerfile`, replace:

```dockerfile
RUN set -eux; \
    arch="$(dpkg --print-architecture)"; \
```

with:

```dockerfile
# SMA-665. The `base-files_chisel` slice writes /var/lib/chisel/manifest.wall, chisel's own record
# of the cut. The image keeps it on purpose: this reverses SMA-500 § 4.5 (SMA-665 spec D1, Q4,
# Q5). syft 1.52.0 cannot read that file, so docker/chisel-dpkg-status.sh below turns it into
# dpkg status.d data, which syft reads. jq and zstd are for that script. They stay in this stage
# and never reach the final image. The cut writes its output to a file, not into a pipe (dash has
# no pipefail), and then prints it: extract_chisel_manifest in ci/images/run.sh reads the
# `Fetching pool/...` lines from the build log, and the script reads them from /tmp/cut.log.
RUN set -eux; \
    arch="$(dpkg --print-architecture)"; \
```

Replace:

```dockerfile
    apt-get install -y --no-install-recommends ca-certificates curl; \
```

with:

```dockerfile
    apt-get install -y --no-install-recommends ca-certificates curl jq zstd; \
```

Replace:

```dockerfile
    chisel cut --release ubuntu-24.04 --root /rootfs \
      base-files_base base-files_release-info \
      libc6_libs libgcc-s1_libs ca-certificates_data

FROM scratch
```

with:

```dockerfile
    chisel cut --release ubuntu-24.04 --root /rootfs \
      base-files_base base-files_release-info base-files_chisel \
      libc6_libs libgcc-s1_libs ca-certificates_data \
      >/tmp/cut.log 2>&1 || { cat /tmp/cut.log; exit 1; }; \
    cat /tmp/cut.log
COPY --chmod=0755 docker/chisel-dpkg-status.sh /usr/local/bin/chisel-dpkg-status.sh
RUN chisel-dpkg-status.sh /rootfs /tmp/cut.log

# SMA-665 T2 (spec D10): the self-test of docker/chisel-dpkg-status.sh. images.yml builds it with
# `--target chisel-dpkg-status-test`, and BuildKit then skips the Rust builder. A normal build
# never builds it. This stage MUST stay above the final FROM scratch: a build with no --target
# builds the LAST stage, and assert_pins in ci/images/run.sh reads the last FROM as the final stage.
FROM rootfs AS chisel-dpkg-status-test
COPY docker/fixtures/ /fixtures/
COPY --chmod=0755 docker/chisel-dpkg-status-test.sh /usr/local/bin/chisel-dpkg-status-test.sh
RUN chisel-dpkg-status-test.sh /usr/local/bin/chisel-dpkg-status.sh /fixtures

FROM scratch
```

- [ ] **Step 4: Run the T2 target to verify it passes**

Run Step 1's command again.
Expected: in the plain log, the `cat /tmp/cut.log` output shows `Fetching pool/main/b/base-files/base-files_…_arm64.deb...` lines, the generator RUN finishes, the test RUN prints 15 `ok` lines and `chisel-dpkg-status self-test: 15 rows OK`, and `rc=0`.

- [ ] **Step 5: Check the rootfs content and the guards that read `rs/Dockerfile`**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-665-chisel-dpkg-status-sbom
rm -rf "$S/rootfs-out"
docker buildx build --progress=plain --target rootfs --output "type=local,dest=$S/rootfs-out" -f rs/Dockerfile rs/ > "$S/rootfs-build.log" 2>&1; echo "rc=$?"
ls -la "$S/rootfs-out/rootfs/var/lib/dpkg/status.d" "$S/rootfs-out/rootfs/var/lib/chisel"
cat "$S/rootfs-out/rootfs/var/lib/dpkg/status.d/libc6"
grep -oE 'Fetching pool/[^ ]+\.deb' "$S/rootfs-build.log" > "$S/fetch.txt"; wc -l < "$S/fetch.txt"
ls "$S/rootfs-out/rootfs/usr/bin" > "$S/rootfs-bin.txt"; grep -xE 'jq|zstd|dpkg|sh|bash' "$S/rootfs-bin.txt"; echo "grep-rc=$?"
grep -niE '^FROM[[:space:]]' rs/Dockerfile
grep -oE 'chisel cut --release ubuntu-[0-9]+\.[0-9]+' rs/Dockerfile
```

Expected: `rc=0`. `status.d` holds one file for each fetched package (7 on arm64 on 2026-10-01: `base-files`, `ca-certificates`, `gcc-14-base`, `libc6`, `libgcc-s1`, `libssl3t64`, `openssl`) and `.md5sums` files for the packages that own a regular file (`openssl` owns only symlinks and directories, so it has none). `manifest.wall` is present. The `libc6` stanza has `Source: glibc`. The fetch count equals the count of stanzas. `grep-rc=1` (no `jq`, `zstd`, `dpkg` or shell in `/rootfs/usr/bin`: AC4). The last FROM line is `FROM scratch`, the line before it is `FROM rootfs AS chisel-dpkg-status-test`, and the chisel grep prints `chisel cut --release ubuntu-24.04`.

Run the A8 guard (`ci/affected-graph/cargo_moon_parity.py` reads `rs/Dockerfile`):

```bash
/bin/bash ci/affected-graph/run.sh; echo "rc=$?"
```

Expected: rc 0, and no A8 row that names `rs/Dockerfile`. Use `/bin/bash` (3.2): this suite deadlocks under Homebrew bash 5 on this host (root `CLAUDE.md`).

- [ ] **Step 6: Mutation proof: the cut without `base-files_chisel` stops the build (T4, AC6)**

In `rs/Dockerfile`, change `base-files_base base-files_release-info base-files_chisel \` to `base-files_base base-files_release-info \`. Run:

```bash
docker buildx build --progress=plain --target rootfs -f rs/Dockerfile rs/ > "$S/mut-slice.log" 2>&1; echo "rc=$?"; grep -F 'chisel-dpkg-status: no chisel manifest' "$S/mut-slice.log"
```

Expected: a non-zero rc, and the grep prints `chisel-dpkg-status: no chisel manifest at /rootfs/var/lib/chisel/manifest.wall; the cut needs the base-files_chisel slice`. Restore the line with the Edit tool. `git diff rs/Dockerfile` must then show only the Step 2 and Step 3 changes.

- [ ] **Step 7: Add the filter entry and the T2 step to `images.yml`**

In `.github/workflows/images.yml`, in the `pull_request` `paths:` list, replace:

```yaml
      - 'rs/Dockerfile'
      - 'rs/.dockerignore'
```

with:

```yaml
      - 'rs/Dockerfile'
      - 'rs/.dockerignore'
      # SMA-665 D9: the chisel dpkg status generator, its self-test and its fixtures.
      # rs/Dockerfile COPYs them, and only the image build reads them.
      - 'rs/docker/**'
```

Replace:

```yaml
      - name: Decision script self-test
        run: uv run --no-project --python '>=3.12' python3 ci/images/release_decision.py --self-test

```

with:

```yaml
      - name: Decision script self-test
        run: uv run --no-project --python '>=3.12' python3 ci/images/release_decision.py --self-test

      # SMA-665 T2 (spec D10): the self-test of rs/docker/chisel-dpkg-status.sh. The target is the
      # chisel-dpkg-status-test stage of rs/Dockerfile, so BuildKit builds the rootfs stage and the
      # test, not the Rust builder. --no-cache-filter makes the test RUN on every build.
      - name: Chisel dpkg status generator self-test
        run: docker buildx build --progress=plain --no-cache-filter=chisel-dpkg-status-test --target chisel-dpkg-status-test -f rs/Dockerfile rs/

```

The `push` filter already has `rs/**`. `repo:actionlint` check 5 reds a `paths:` glob that matches no tracked file. Task 1 committed `rs/docker/`, so the glob matches.

- [ ] **Step 8: Lint the workflow**

```bash
actionlint .github/workflows/images.yml; echo "rc=$?"
git ls-files 'rs/docker/**' | wc -l
```

Expected: `rc=0`, and a count of 14 or more tracked files.

- [ ] **Step 9: Commit**

```bash
git branch --show-current   # must print feature/sma-665-chisel-dpkg-status-sbom
git add rs/Dockerfile .github/workflows/images.yml
git commit -m "feat(rs): write the chisel manifest and dpkg status data into the image (SMA-665)

The cut adds the base-files_chisel slice, and the rootfs stage runs the
generator on the cut. jq and zstd stay in the rootfs stage. A new
chisel-dpkg-status-test stage runs the generator self-test, and
images.yml builds it with --target. The stage is above the final FROM
scratch, so assert_pins still reads the right final stage.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: The SBOM floor compares the Debian entries with the chisel fetch list

**Files:**
- Modify: `ci/images/release_decision.py` (docstring lines 22-27, imports 37-46, constants after line 62, `sbom_summary` lines 243-280, `sbom_floor` lines 299-323, self-test helpers and rows lines 404-557, CLI lines 615-647)
- Modify: `.github/workflows/images.yml:162-173` (step `SBOM for each archive`) and `:207` (step `Console release sequence`)
- Modify: `.github/workflows/release.yml:1263-1266` (the `&image-build-steps` anchor; it covers four jobs: `images-build-iam`, `images-build-gateway`, `images-build-iam-console`, `images-build-gateway-console`)

**Interfaces:**
- Consumes: the file `chisel-manifest-<key>-<arch>.txt` that `ci/images/run.sh build-oci` writes at the repo root (unchanged; lines `Fetching pool/<component>/<prefix>/<source>/<name>_<version>_<arch>.deb`).
- Produces:
  - `Deb = tuple[str, str, str]` (name, version, arch; arch `""` when a purl has none).
  - `sbom_summary(doc) -> dict[str, str]` with keys `packages, libc6, cargo, npm, next, deb` in that order.
  - `parse_deb_purl(locator: str) -> Deb`, `sbom_debs(doc: dict[str, Any]) -> list[Deb]`, `parse_chisel_list(text: str) -> list[Deb]` (all raise `UsageError` on bad input).
  - `sbom_floor(key: str, kinds: dict[str, str], summary: dict[str, str], *, debs: list[Deb], chisel: list[Deb] | None, arch: str) -> str`.
  - CLI: `release_decision.py sbom-floor --service KEY --arch ARCH [--chisel-dir DIR] SPDX_JSON`. Exit 0 pass, 2 usage or unreadable input (missing, empty or unparsable list for a cargo key; no `--arch`; a bad `--arch`), 3 a failed floor.

- [ ] **Step 1: Write the failing self-test rows**

Make these edits in `ci/images/release_decision.py`. The rows need two new imports now. `tempfile` is read when the self-test starts, and `contextlib` when a CLI row runs.

Replace:

```python
import argparse
import hashlib
```

with:

```python
import argparse
import contextlib
import hashlib
```

Replace:

```python
import tarfile
import tomllib
```

with:

```python
import tarfile
import tempfile
import tomllib
```

Replace the `_summary` helper:

```python
def _summary(**values: str) -> dict[str, str]:
    """An sbom_summary() result with every count at zero, then `values` on top."""
    return {"packages": "0", "libc6": "false", "cargo": "0", "npm": "0", "next": "false", **values}
```

with the helper plus the new fixtures and helpers:

```python
def _summary(**values: str) -> dict[str, str]:
    """An sbom_summary() result with every count at zero, then `values` on top."""
    return {"packages": "0", "libc6": "false", "cargo": "0", "npm": "0", "next": "false", "deb": "0", **values}


# SMA-665. A chisel fetch list of three packages, and the SBOM entries that match it. The texts
# are MEASURED (spec M2, M4) on the arm64 cut of 2026-10-01, with the arch set to amd64.
BASE_FILES: Deb = ("base-files", "13ubuntu10.5", "amd64")
CA_CERTS: Deb = ("ca-certificates", "20260601~24.04.1", "all")
LIBC6: Deb = ("libc6", "2.39-0ubuntu8.9", "amd64")
CHISEL = [BASE_FILES, CA_CERTS, LIBC6]
CHISEL_TEXT = (
    "Fetching pool/main/b/base-files/base-files_13ubuntu10.5_amd64.deb\n"
    "Fetching pool/main/c/ca-certificates/ca-certificates_20260601~24.04.1_all.deb\n"
    "Fetching pool/main/g/glibc/libc6_2.39-0ubuntu8.9_amd64.deb\n"
)


def _cargo_floor(debs: list[Deb], chisel: list[Deb] | None = CHISEL, arch: str = "amd64", **values: str) -> object:
    """The cargo floor of `iam` with one crate and libc6, unless `values` says otherwise."""
    return sbom_floor("iam", FLOOR_KINDS, _summary(**{"cargo": "1", "libc6": "true", **values}), debs=debs, chisel=chisel, arch=arch)


def _deb_doc(*locators: str) -> dict[str, Any]:
    """An SPDX document with one package for each purl."""
    return {"packages": [{"name": "p", "externalRefs": [{"referenceLocator": locator}]} for locator in locators]}


def _main_rc(argv: list[str]) -> int:
    """main()'s exit code, with its output kept off the self-test's own output."""
    with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
        return main(argv)


def _cli_rows(tmp: Path) -> list[tuple[str, Callable[[], object], object]]:
    """SMA-665: the sbom-floor CLI. The cargo key reads its list file; the npm key reads none."""
    cargo_sbom = tmp / "cargo.spdx.json"
    cargo_sbom.write_text(json.dumps({"packages": [
        {"name": "serde", "externalRefs": [{"referenceLocator": "pkg:cargo/serde@1.0.228"}]},
        {"name": "base-files", "externalRefs": [{"referenceLocator": "pkg:deb/ubuntu/base-files@13ubuntu10.5?arch=amd64&distro=ubuntu-24.04"}]},
        {"name": "ca-certificates", "externalRefs": [{"referenceLocator": "pkg:deb/ubuntu/ca-certificates@20260601~24.04.1?arch=all&distro=ubuntu-24.04"}]},
        {"name": "libc6", "externalRefs": [{"referenceLocator": "pkg:deb/ubuntu/libc6@2.39-0ubuntu8.9?arch=amd64&distro=ubuntu-24.04&upstream=glibc"}]},
    ]}))
    npm_sbom = tmp / "npm.spdx.json"
    npm_sbom.write_text(json.dumps({"packages": [
        {"name": "libc6", "externalRefs": [{"referenceLocator": "pkg:deb/debian/libc6@2.36-9+deb12u10?arch=amd64&distro=debian-12"}]},
        {"name": "next", "externalRefs": [{"referenceLocator": "pkg:npm/next@16.3.5"}]},
    ]}))
    with_list = tmp / "with-list"
    with_list.mkdir()
    (with_list / "chisel-manifest-iam-amd64.txt").write_text(CHISEL_TEXT)
    empty_list = tmp / "empty-list"
    empty_list.mkdir()
    (empty_list / "chisel-manifest-iam-amd64.txt").write_text("")
    no_list = tmp / "no-list"
    no_list.mkdir()

    def floor(*args: str) -> Callable[[], object]:
        return lambda: _main_rc(["sbom-floor", *args])

    return [
        ("cli: cargo with its chisel list passes", floor("--service", "iam", "--arch", "amd64", "--chisel-dir", str(with_list), str(cargo_sbom)), 0),
        ("cli: cargo with no list file is exit 2", floor("--service", "iam", "--arch", "amd64", "--chisel-dir", str(no_list), str(cargo_sbom)), 2),
        ("cli: cargo with an empty list file is exit 2", floor("--service", "iam", "--arch", "amd64", "--chisel-dir", str(empty_list), str(cargo_sbom)), 2),
        ("cli: cargo with the list of another arch is exit 2", floor("--service", "iam", "--arch", "arm64", "--chisel-dir", str(with_list), str(cargo_sbom)), 2),
        ("cli: no --arch is exit 2", floor("--service", "iam", "--chisel-dir", str(with_list), str(cargo_sbom)), 2),
        ("cli: an --arch that is not an architecture is exit 2", floor("--service", "iam", "--arch", "../amd64", "--chisel-dir", str(with_list), str(cargo_sbom)), 2),
        ("cli: npm with the cargo arguments and no list file passes", floor("--service", "iam-console", "--arch", "amd64", "--chisel-dir", str(no_list), str(npm_sbom)), 0),
    ]
```

Replace:

```python
def self_test() -> int:
    tar, manifest, config = _fixture_archive()
```

with:

```python
def self_test() -> int:
    with tempfile.TemporaryDirectory() as tmp:
        return _self_test(Path(tmp))


def _self_test(tmp: Path) -> int:
    tar, manifest, config = _fixture_archive()
```

Add `"deb": "0"` to the four whole-dict `sbom_summary` rows. Replace:

```python
            {"packages": "2", "libc6": "true", "cargo": "1", "npm": "0", "next": "false"},
```

with:

```python
            {"packages": "2", "libc6": "true", "cargo": "1", "npm": "0", "next": "false", "deb": "0"},
```

Replace both occurrences (the rows "sbom: an empty document" and "sbom: missing packages key gives empty list (no error)") of:

```python
{"packages": "0", "libc6": "false", "cargo": "0", "npm": "0", "next": "false"}),
```

with:

```python
{"packages": "0", "libc6": "false", "cargo": "0", "npm": "0", "next": "false", "deb": "0"}),
```

Replace:

```python
            {"packages": "3", "libc6": "true", "cargo": "0", "npm": "2", "next": "true"},
```

with:

```python
            {"packages": "3", "libc6": "true", "cargo": "0", "npm": "2", "next": "true", "deb": "0"},
```

Replace the floor rows:

```python
        ("floor: cargo with one crate", lambda: sbom_floor("iam", FLOOR_KINDS, _summary(cargo="1")), "cargo"),
        ("floor: cargo with no crate", lambda: sbom_floor("iam", FLOOR_KINDS, _summary()), "FloorError"),
        ("floor: npm with npm, libc6 and next", lambda: sbom_floor("iam-console", FLOOR_KINDS, _summary(npm="67", libc6="true", next="true")), "npm"),
        ("floor: npm with no npm package", lambda: sbom_floor("iam-console", FLOOR_KINDS, _summary(libc6="true", next="true")), "FloorError"),
        ("floor: npm without libc6", lambda: sbom_floor("iam-console", FLOOR_KINDS, _summary(npm="67", next="true")), "FloorError"),
        ("floor: npm without next", lambda: sbom_floor("iam-console", FLOOR_KINDS, _summary(npm="67", libc6="true")), "FloorError"),
        # A cargo floor must not accept an npm image: crates are the cargo rule, npm is not.
        ("floor: a cargo key with only npm packages", lambda: sbom_floor("iam", FLOOR_KINDS, _summary(npm="67", libc6="true", next="true")), "FloorError"),
        ("floor: an unknown key", lambda: sbom_floor("billing", FLOOR_KINDS, _summary(cargo="1")), "FloorError"),
        ("floor: an unknown kind", lambda: sbom_floor("x", {"x": "pip"}, _summary(cargo="1")), "FloorError"),
```

with:

```python
        ("floor: cargo with crates, libc6 and the chisel list", lambda: _cargo_floor(CHISEL), "cargo"),
        ("floor: cargo with no crate", lambda: _cargo_floor(CHISEL, cargo="0"), "FloorError"),
        ("floor: npm with npm, libc6 and next", lambda: sbom_floor("iam-console", FLOOR_KINDS, _summary(npm="67", libc6="true", next="true"), debs=[], chisel=None, arch="amd64"), "npm"),
        ("floor: npm with no npm package", lambda: sbom_floor("iam-console", FLOOR_KINDS, _summary(libc6="true", next="true"), debs=[], chisel=None, arch="amd64"), "FloorError"),
        ("floor: npm without libc6", lambda: sbom_floor("iam-console", FLOOR_KINDS, _summary(npm="67", next="true"), debs=[], chisel=None, arch="amd64"), "FloorError"),
        ("floor: npm without next", lambda: sbom_floor("iam-console", FLOOR_KINDS, _summary(npm="67", libc6="true"), debs=[], chisel=None, arch="amd64"), "FloorError"),
        # A cargo floor must not accept an npm image: crates are the cargo rule, npm is not.
        ("floor: a cargo key with only npm packages", lambda: _cargo_floor(CHISEL, cargo="0", npm="67", next="true"), "FloorError"),
        ("floor: an unknown key", lambda: sbom_floor("billing", FLOOR_KINDS, _summary(cargo="1"), debs=[], chisel=None, arch="amd64"), "FloorError"),
        ("floor: an unknown kind", lambda: sbom_floor("x", {"x": "pip"}, _summary(cargo="1"), debs=[], chisel=None, arch="amd64"), "FloorError"),
        # SMA-665 D5: the cargo floor's OS rule. Mutation: drop `reasons += _chisel_reasons(…)`,
        # and every FloorError row below reds. Mutation: drop the reverse (count) block in
        # _chisel_reasons, and "an SBOM entry not in the list" reds.
        ("floor: cargo without libc6", lambda: _cargo_floor(CHISEL, libc6="false"), "FloorError"),
        ("floor: a list entry the SBOM does not have", lambda: _cargo_floor([BASE_FILES, CA_CERTS]), "FloorError"),
        ("floor: a list entry the SBOM has twice", lambda: _cargo_floor([*CHISEL, LIBC6]), "FloorError"),
        ("floor: an SBOM entry not in the list", lambda: _cargo_floor([*CHISEL, ("openssl", "3.0.13-0ubuntu3.16", "amd64")]), "FloorError"),
        ("floor: a version mismatch", lambda: _cargo_floor([BASE_FILES, CA_CERTS, ("libc6", "2.39-0ubuntu8.8", "amd64")]), "FloorError"),
        ("floor: an arch mismatch", lambda: _cargo_floor([BASE_FILES, CA_CERTS, ("libc6", "2.39-0ubuntu8.9", "arm64")]), "FloorError"),
        ("floor: an SBOM entry with no arch qualifier", lambda: _cargo_floor([*CHISEL, ("gcc-14", "14.2.0-4ubuntu2~24.04.1", "")]), "FloorError"),
        ("floor: a list of another arch", lambda: _cargo_floor(CHISEL, arch="arm64"), "FloorError"),
        ("floor: cargo with no chisel list is exit 2", lambda: _cargo_floor(CHISEL, chisel=None), "UsageError"),
        # SMA-665: sbom-summary counts pkg:deb purls.
        ("sbom: counts deb purls", lambda: sbom_summary(_deb_doc("pkg:deb/ubuntu/libc6@2.39-0ubuntu8.9?arch=amd64", "pkg:cargo/serde@1.0.228"))["deb"], "1"),
        # SMA-665 D5: sbom_debs parses each pkg:deb purl. The first, fourth, sixth and eighth
        # texts are MEASURED syft 1.52.0 output (spec M2).
        (
            "debs: a plain purl",
            lambda: sbom_debs(_deb_doc("pkg:deb/ubuntu/libc6@2.39-0ubuntu8.9?arch=arm64&distro=ubuntu-24.04&upstream=glibc")),
            [("libc6", "2.39-0ubuntu8.9", "arm64")],
        ),
        ("debs: %2B decodes to +", lambda: sbom_debs(_deb_doc("pkg:deb/ubuntu/zz@2.3%2Bdfsg-1?arch=arm64")), [("zz", "2.3+dfsg-1", "arm64")]),
        ("debs: a raw + stays +", lambda: sbom_debs(_deb_doc("pkg:deb/ubuntu/zz@2.3+dfsg-1?arch=arm64")), [("zz", "2.3+dfsg-1", "arm64")]),
        (
            "debs: a raw ~ stays ~",
            lambda: sbom_debs(_deb_doc("pkg:deb/ubuntu/ca-certificates@20260601~24.04.1?arch=all&distro=ubuntu-24.04")),
            [CA_CERTS],
        ),
        ("debs: %7E decodes to ~", lambda: sbom_debs(_deb_doc("pkg:deb/ubuntu/ca-certificates@20260601%7E24.04.1?arch=all")), [CA_CERTS]),
        (
            "debs: a 1%3A epoch is dropped",
            lambda: sbom_debs(_deb_doc("pkg:deb/ubuntu/zz-epoch@1%3A2.3%2Bdfsg-1~x?arch=arm64&distro=ubuntu-24.04")),
            [("zz-epoch", "2.3+dfsg-1~x", "arm64")],
        ),
        ("debs: a raw 1: epoch is dropped", lambda: sbom_debs(_deb_doc("pkg:deb/ubuntu/zz@1:2.3?arch=arm64")), [("zz", "2.3", "arm64")]),
        (
            "debs: no arch qualifier gives an empty arch",
            lambda: sbom_debs(_deb_doc("pkg:deb/ubuntu/gcc-14@14.2.0-4ubuntu2~24.04.1?distro=ubuntu")),
            [("gcc-14", "14.2.0-4ubuntu2~24.04.1", "")],
        ),
        ("debs: no namespace", lambda: sbom_debs(_deb_doc("pkg:deb/libc6@2.39?arch=amd64")), [("libc6", "2.39", "amd64")]),
        ("debs: other purl types are ignored", lambda: sbom_debs(_deb_doc("pkg:cargo/serde@1.0.228", "pkg:npm/next@16.3.5")), []),
        ("debs: no version", lambda: sbom_debs(_deb_doc("pkg:deb/ubuntu/libc6?arch=amd64")), "UsageError"),
        ("debs: too many path segments", lambda: sbom_debs(_deb_doc("pkg:deb/ubuntu/x/libc6@2.39?arch=amd64")), "UsageError"),
        ("debs: a qualifier with no =", lambda: sbom_debs(_deb_doc("pkg:deb/ubuntu/libc6@2.39?arch")), "UsageError"),
        # SMA-665 D5: parse_chisel_list reads the fetch list. The text is MEASURED (spec M4).
        ("chisel: a real fetch list", lambda: parse_chisel_list(CHISEL_TEXT), CHISEL),
        ("chisel: an empty list", lambda: parse_chisel_list(""), "UsageError"),
        ("chisel: only blank lines", lambda: parse_chisel_list("\n\n"), "UsageError"),
        ("chisel: a line that does not parse", lambda: parse_chisel_list("Fetching pool/main/glibc/libc6_2.39_amd64.deb\n"), "UsageError"),
        ("chisel: the raw log form with ... does not parse", lambda: parse_chisel_list("Fetching pool/main/g/glibc/libc6_2.39_amd64.deb...\n"), "UsageError"),
        (
            "chisel: one name with two versions",
            lambda: parse_chisel_list("Fetching pool/main/g/glibc/libc6_2.39-1_amd64.deb\nFetching pool/main/g/glibc/libc6_2.39-2_amd64.deb\n"),
            "UsageError",
        ),
```

Append the CLI rows at the end of the table. Replace:

```python
        ("kinds: not TOML", lambda: chain_kinds("[chain.iam\n"), "UsageError"),
    ]
```

with:

```python
        ("kinds: not TOML", lambda: chain_kinds("[chain.iam\n"), "UsageError"),
        *_cli_rows(tmp),
    ]
```

- [ ] **Step 2: Run the self-test to verify it fails**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-665-chisel-dpkg-status-sbom
uv run --no-project --python '>=3.12' python3 ci/images/release_decision.py --self-test; echo "rc=$?"
```

Expected: the run does not pass. It prints FAIL rows for the `sbom:` dict rows (no `deb` key), and then stops with a traceback such as `TypeError: sbom_floor() got an unexpected keyword argument 'debs'`, or `NameError: name 'sbom_debs' is not defined`. rc is not 0.

- [ ] **Step 3: Write the implementation**

Replace the docstring part:

```python
  sbom-summary SPDX_JSON
      packages=, libc6=true|false, cargo=, npm=, next=true|false (a measurement, spec M8).
  sbom-floor --service KEY SPDX_JSON
      SMA-688 D5: the sbom-summary lines, then kind= and floor=pass. One floor for each image
      kind, read from ci/images/chains.toml. A failed floor or an unknown key prints floor=fail
      and exits 3.
```

with:

```python
  sbom-summary SPDX_JSON
      packages=, libc6=true|false, cargo=, npm=, next=true|false, deb= (a measurement, spec M8).
  sbom-floor --service KEY --arch ARCH [--chisel-dir DIR] SPDX_JSON
      SMA-688 D5: the sbom-summary lines, then kind= and floor=pass. One floor for each image
      kind, read from ci/images/chains.toml. SMA-665 D5: a cargo key also reads
      DIR/chisel-manifest-KEY-ARCH.txt (DIR is the working directory by default), and its SBOM's
      Debian entries must be the same set of (name, version, arch) as that chisel fetch list.
      An npm key reads no list and ignores --arch. A missing, empty or unparsable list is
      exit 2. A failed floor or an unknown key prints floor=fail and exits 3.
```

Replace:

```python
from typing import TYPE_CHECKING, Any
```

with:

```python
from typing import TYPE_CHECKING, Any
from urllib.parse import unquote
```

Replace:

```python
Version = tuple[int, int, int]
```

with:

```python
Version = tuple[int, int, int]

# SMA-665. One Debian package: (name, version, arch). The arch is "" when a purl has no arch
# qualifier.
Deb = tuple[str, str, str]

# SMA-665 D3/D5. A line of chisel-manifest-<key>-<arch>.txt, as extract_chisel_manifest in
# ci/images/run.sh writes it. MEASURED (spec M4, chisel v1.4.2):
#   Fetching pool/main/g/glibc/libc6_2.39-0ubuntu8.9_arm64.deb
# A pool file name carries no epoch, and `~` and `+` stay literal in it.
CHISEL_FETCH_RE = re.compile(r"Fetching pool/[^/\s]+/[^/\s]+/[^/\s]+/(?P<name>[^/_\s]+)_(?P<version>[^/_\s]+)_(?P<arch>[^/_.\s]+)\.deb")
ARCH_RE = re.compile(r"[a-z0-9]+")
EPOCH_RE = re.compile(r"^\d+:")
```

Replace the whole `sbom_summary` function:

```python
def sbom_summary(doc: dict[str, Any]) -> dict[str, str]:
    """Spec M8 (SMA-658): does the SBOM see the OS packages and the Rust crates at all?

    SMA-688 adds the npm count and whether `next` is there. A console image has no Rust crates,
    and its floor reads those two instead."""
    # A missing "packages" key defaults to []; a present key must be a list (even if falsey).
    packages_value = doc.get("packages")
    packages = [] if packages_value is None else _require_list(packages_value, "packages")
    names = set()
    cargo = 0
    npm = 0
    has_next = False
    for p in packages:
        if not isinstance(p, dict):
            raise UsageError(f"packages must be a list of objects, not {type(p).__name__!r}")
        names.add(str(p.get("name", "")))
        # A missing "externalRefs" key defaults to []; a present key must be a list (even if falsey).
        refs_value = p.get("externalRefs")
        refs = [] if refs_value is None else _require_list(refs_value, "a package's externalRefs")
        for ref in refs:
            if not isinstance(ref, dict):
                raise UsageError(f"an externalRefs entry must be a JSON object, not {type(ref).__name__!r}")
            locator = str(ref.get("referenceLocator", ""))
            if locator.startswith("pkg:cargo/"):
                cargo += 1
            elif locator.startswith("pkg:npm/"):
                npm += 1
                # The purl of `next` itself. A scoped `@next/env` is pkg:npm/%40next/env@…,
                # so it does not match.
                if locator.startswith("pkg:npm/next@"):
                    has_next = True
    return {
        "packages": str(len(packages)),
        "libc6": "true" if "libc6" in names else "false",
        "cargo": str(cargo),
        "npm": str(npm),
        "next": "true" if has_next else "false",
    }
```

with `_sbom_packages`, the new `sbom_summary`, and the four new functions:

```python
def _sbom_packages(doc: dict[str, Any]) -> list[tuple[str, list[str]]]:
    """(name, its referenceLocator strings) for each SPDX package. A wrong shape is exit 2."""
    # A missing "packages" key defaults to []; a present key must be a list (even if falsey).
    packages_value = doc.get("packages")
    packages = [] if packages_value is None else _require_list(packages_value, "packages")
    entries: list[tuple[str, list[str]]] = []
    for p in packages:
        if not isinstance(p, dict):
            raise UsageError(f"packages must be a list of objects, not {type(p).__name__!r}")
        # A missing "externalRefs" key defaults to []; a present key must be a list (even if falsey).
        refs_value = p.get("externalRefs")
        refs = [] if refs_value is None else _require_list(refs_value, "a package's externalRefs")
        locators: list[str] = []
        for ref in refs:
            if not isinstance(ref, dict):
                raise UsageError(f"an externalRefs entry must be a JSON object, not {type(ref).__name__!r}")
            locators.append(str(ref.get("referenceLocator", "")))
        entries.append((str(p.get("name", "")), locators))
    return entries


def sbom_summary(doc: dict[str, Any]) -> dict[str, str]:
    """Spec M8 (SMA-658): does the SBOM see the OS packages and the Rust crates at all?

    SMA-688 adds the npm count and whether `next` is there. A console image has no Rust crates,
    and its floor reads those two instead. SMA-665 adds `deb`, the count of pkg:deb purls."""
    entries = _sbom_packages(doc)
    names = {name for name, _locators in entries}
    locators = [locator for _name, refs in entries for locator in refs]
    return {
        "packages": str(len(entries)),
        "libc6": "true" if "libc6" in names else "false",
        "cargo": str(sum(1 for locator in locators if locator.startswith("pkg:cargo/"))),
        "npm": str(sum(1 for locator in locators if locator.startswith("pkg:npm/"))),
        # The purl of `next` itself. A scoped `@next/env` is pkg:npm/%40next/env@…, so it does
        # not match.
        "next": "true" if any(locator.startswith("pkg:npm/next@") for locator in locators) else "false",
        "deb": str(sum(1 for locator in locators if locator.startswith("pkg:deb/"))),
    }


def parse_deb_purl(locator: str) -> Deb:
    """SMA-665 D5: (name, version, arch) of a pkg:deb purl. The name, the version and the arch
    are percent-decoded. The namespace, the other qualifiers and a subpath are ignored. A leading
    `N:` epoch is dropped, because a pool file name has none. A purl with no arch qualifier gives
    arch "": that is a floor failure (exit 3), not a parse failure.

    MEASURED (spec M2, syft 1.52.0): syft writes `~` raw, `+` as %2B and the epoch colon as %3A,
    for example pkg:deb/ubuntu/zz-epoch@1%3A2.3%2Bdfsg-1~x?arch=arm64&distro=ubuntu-24.04. A
    purl writer may also leave `+` raw or write `~` as %7E; both decode to the same version."""
    if not locator.startswith("pkg:deb/"):
        raise UsageError(f"not a pkg:deb purl: {locator!r}")
    body = locator.removeprefix("pkg:deb/").partition("#")[0]
    body, _sep, query = body.partition("?")
    path, at, version_text = body.rpartition("@")
    segments = path.split("/")
    if not at or not version_text or not 1 <= len(segments) <= 2 or not all(segments):
        raise UsageError(f"a pkg:deb purl that does not parse: {locator!r}")
    arch = ""
    for pair in query.split("&") if query else []:
        key, eq, value = pair.partition("=")
        if not eq or not key:
            raise UsageError(f"a pkg:deb purl qualifier that does not parse: {locator!r}")
        if key == "arch":
            arch = unquote(value)
    name = unquote(segments[-1])
    version = EPOCH_RE.sub("", unquote(version_text), count=1)
    if not name or not version:
        raise UsageError(f"a pkg:deb purl with an empty name or version: {locator!r}")
    return (name, version, arch)


def sbom_debs(doc: dict[str, Any]) -> list[Deb]:
    """SMA-665 D5: (name, version, arch) for each pkg:deb purl in the SBOM, in document order.
    A pkg:deb purl that does not parse is exit 2."""
    return [parse_deb_purl(locator) for _name, refs in _sbom_packages(doc) for locator in refs if locator.startswith("pkg:deb/")]


def parse_chisel_list(text: str) -> list[Deb]:
    """SMA-665 D5: (name, version, arch) for each line of a chisel fetch list. An empty list, a
    line that does not parse, and one name on two lines are exit 2."""
    entries: list[Deb] = []
    seen: dict[str, str] = {}
    for line in text.splitlines():
        if not line.strip():
            continue
        match = CHISEL_FETCH_RE.fullmatch(line.strip())
        if match is None:
            raise UsageError(f"a chisel fetch line that does not parse: {line!r}")
        name, version, arch = match["name"], match["version"], match["arch"]
        if name in seen:
            raise UsageError(f"the chisel fetch list names {name} twice ({seen[name]} and {version})")
        seen[name] = version
        entries.append((name, version, arch))
    if not entries:
        raise UsageError("the chisel fetch list is empty")
    return entries


def _chisel_reasons(debs: list[Deb], chisel: list[Deb], arch: str) -> list[str]:
    """SMA-665 D5: the SBOM's Debian entries and the chisel fetch list must be the same set of
    (name, version, arch). Each list entry matches exactly one SBOM entry (forward), and the two
    counts are equal (reverse). Together the two rules are set equality."""
    reasons: list[str] = []
    for name, version, list_arch in chisel:
        if list_arch not in (arch, "all"):
            reasons.append(f"the chisel fetch list names {name} {version} for {list_arch}, not for {arch}")
    for name, version, deb_arch in debs:
        if not deb_arch:
            reasons.append(f"the SBOM lists {name} {version} with no arch qualifier")
    # Forward: each list entry is in the SBOM exactly once.
    for entry in chisel:
        found = debs.count(entry)
        if found != 1:
            reasons.append(f"the SBOM lists {' '.join(entry)} {found} times; expected exactly once")
    # Reverse: no Debian entry in the SBOM is outside the list.
    if len(debs) != len(chisel):
        extra = sorted(set(debs) - set(chisel))
        reasons.append(f"the SBOM has {len(debs)} Debian entries and the chisel fetch list has {len(chisel)}; not in the list: {extra}")
    return reasons
```

Replace the head of `sbom_floor`:

```python
def sbom_floor(key: str, kinds: dict[str, str], summary: dict[str, str]) -> str:
    """SMA-688 D5. The kind of `key` when its SBOM reaches that kind's floor. Else FloorError.

    cargo: at least one Rust crate (the binary was built with cargo auditable; the SMA-658 rule).
    npm:   at least one npm package, libc6 (the distroless base's dpkg data, spec M3) and `next`.
    """
    if key not in kinds:
        raise FloorError(f"{key!r} names no chain in ci/images/chains.toml (known: {sorted(kinds)})")
    kind = kinds[key]
    reasons: list[str] = []
    if kind == "cargo":
        if int(summary["cargo"]) < 1:
            reasons.append("the SBOM lists no Rust crates; the binary was not built with cargo auditable")
```

with:

```python
def sbom_floor(key: str, kinds: dict[str, str], summary: dict[str, str], *, debs: list[Deb], chisel: list[Deb] | None, arch: str) -> str:
    """SMA-688 D5. The kind of `key` when its SBOM reaches that kind's floor. Else FloorError.

    cargo: at least one Rust crate (the binary was built with cargo auditable; the SMA-658 rule),
           libc6, and (SMA-665 D5) the SBOM's Debian entries `debs` equal the chisel fetch list
           `chisel` as a set of (name, version, arch). `chisel` must not be None for this kind.
    npm:   at least one npm package, libc6 (the distroless base's dpkg data, spec M3) and `next`.
           `debs`, `chisel` and `arch` are not read.
    """
    if key not in kinds:
        raise FloorError(f"{key!r} names no chain in ci/images/chains.toml (known: {sorted(kinds)})")
    kind = kinds[key]
    reasons: list[str] = []
    if kind == "cargo":
        if chisel is None:
            raise UsageError(f"the cargo floor for {key!r} needs the chisel fetch list")
        if int(summary["cargo"]) < 1:
            reasons.append("the SBOM lists no Rust crates; the binary was not built with cargo auditable")
        if summary["libc6"] != "true":
            reasons.append("the SBOM does not list libc6; the chisel cut's generated dpkg status data is missing")
        reasons += _chisel_reasons(debs, chisel, arch)
```

The rest of `sbom_floor` (the `npm` branch, the unknown-kind branch and the `if reasons:` raise) does not change.

Add the two CLI arguments. Replace:

```python
    p_floor.add_argument("--service", required=True)
    p_floor.add_argument("sbom", type=Path)
```

with:

```python
    p_floor.add_argument("--service", required=True)
    p_floor.add_argument("--arch", required=True)
    p_floor.add_argument("--chisel-dir", type=Path, default=Path())
    p_floor.add_argument("sbom", type=Path)
```

Replace the `sbom-floor` branch of `main`:

```python
        elif args.command == "sbom-floor":
            summary = sbom_summary(_read_sbom(args.sbom))
            _emit(summary)
            kind = sbom_floor(args.service, chain_kinds(_read_text(CHAINS_TOML)), summary)
            _emit({"kind": kind, "floor": "pass"})
```

with:

```python
        elif args.command == "sbom-floor":
            if ARCH_RE.fullmatch(args.arch) is None:
                raise UsageError(f"not an architecture: {args.arch!r}")
            doc = _read_sbom(args.sbom)
            summary = sbom_summary(doc)
            _emit(summary)
            kinds = chain_kinds(_read_text(CHAINS_TOML))
            # SMA-665 D5: the KIND decides whether a chisel list is read, never whether a file
            # exists. A missing list for a cargo key is exit 2 (fail closed).
            debs: list[Deb] = []
            chisel: list[Deb] | None = None
            if kinds.get(args.service) == "cargo":
                chisel = parse_chisel_list(_read_text(args.chisel_dir / f"chisel-manifest-{args.service}-{args.arch}.txt"))
                debs = sbom_debs(doc)
            kind = sbom_floor(args.service, kinds, summary, debs=debs, chisel=chisel, arch=args.arch)
            _emit({"kind": kind, "floor": "pass"})
```

- [ ] **Step 4: Run the self-test and ruff to verify they pass**

```bash
uv run --no-project --python '>=3.12' python3 ci/images/release_decision.py --self-test; echo "rc=$?"
R=py/.venv/bin/ruff; [ -x "$R" ] || R=/Users/smaschek/dev/paigasus/paigasus-core/py/.venv/bin/ruff
"$R" check --config py/pyproject.toml ci/images/release_decision.py; echo "ruff rc=$?"
```

Expected: `release_decision self-test: 94 rows OK`, `rc=0`, `All checks passed!` and `ruff rc=0`. (If the worktree has no `py/.venv`, `R` is the main checkout's ruff, from the same lock. `repo:ruff-ci` in Task 5 runs the pinned one.)

- [ ] **Step 5: Mutation proof of the two floor rules (T4, AC6)**

1. In `sbom_floor`, change `        reasons += _chisel_reasons(debs, chisel, arch)` to `        pass`. Run the self-test. Expected: rc 3 and 7 FAIL rows: "a list entry the SBOM does not have", "a list entry the SBOM has twice", "an SBOM entry not in the list", "a version mismatch", "an arch mismatch", "an SBOM entry with no arch qualifier", "a list of another arch". Restore with the Edit tool.
2. In `_chisel_reasons`, change `    if len(debs) != len(chisel):` to `    if False:`. Run the self-test. Expected: rc 3 and exactly 1 FAIL row: `floor: an SBOM entry not in the list`. Restore with the Edit tool.
3. Run the self-test again: 94 rows OK. `git diff ci/images/release_decision.py` must show no `pass` line in `sbom_floor` and no `if False:`.

- [ ] **Step 6: Pass `--arch` from the three callers**

In `.github/workflows/images.yml`, replace:

```yaml
      # This step is a MEASUREMENT for spec M8. It checks that syft finds libc6 and the Rust
      # crates. SMA-688: the step now also checks the cargo SBOM floor. Each pull request
      # proves the floor that release.yml uses.
```

with:

```yaml
      # This step is a MEASUREMENT for spec M8. It checks that syft finds libc6 and the Rust
      # crates. SMA-688: the step now also checks the cargo SBOM floor. Each pull request
      # proves the floor that release.yml uses. SMA-665: the cargo floor also needs one Debian
      # entry for each package of chisel-manifest-<key>-<arch>.txt, which build-oci wrote above
      # in this job. So the floor takes --arch.
```

Replace:

```yaml
            uv run --no-project --python '>=3.12' python3 ci/images/release_decision.py sbom-floor --service "$key" "sbom-${crate}-${ARCH}.spdx.json"
```

with:

```yaml
            uv run --no-project --python '>=3.12' python3 ci/images/release_decision.py sbom-floor --service "$key" --arch "${ARCH}" "sbom-${crate}-${ARCH}.spdx.json"
```

Replace:

```yaml
            uv run --no-project --python '>=3.12' python3 ci/images/release_decision.py sbom-floor --service "$key" "sbom-paigasus-${key}-${ARCH}.spdx.json"
```

with:

```yaml
            uv run --no-project --python '>=3.12' python3 ci/images/release_decision.py sbom-floor --service "$key" --arch "${ARCH}" "sbom-paigasus-${key}-${ARCH}.spdx.json"
```

In `.github/workflows/release.yml` (inside the `&image-build-steps` anchor, step `Report the SBOM contents and hold its floor (spec AC 7, SMA-688 D5)`), replace:

```yaml
          # One floor for each image kind, read from ci/images/chains.toml. A cargo image needs
          # Rust crates (cargo auditable); a console image needs npm packages, libc6 and next.
          # A failed floor exits 3 and names what is missing.
          uv run --no-project --python '>=3.12' python3 ci/images/release_decision.py sbom-floor --service "${SERVICE}" "$sbom"
```

with:

```yaml
          # One floor for each image kind, read from ci/images/chains.toml. A cargo image needs
          # Rust crates (cargo auditable), libc6, and exactly one Debian entry for each package of
          # its chisel fetch list (SMA-665: chisel-manifest-<key>-<arch>.txt, which build-oci
          # wrote at the repo root in this job). A console image needs npm packages, libc6 and
          # next, and reads no list. Every kind takes --arch. A failed floor exits 3 and names
          # what is missing. A missing list for a cargo image exits 2.
          uv run --no-project --python '>=3.12' python3 ci/images/release_decision.py sbom-floor --service "${SERVICE}" --arch "${ARCH}" "$sbom"
```

Check that no other caller exists and that the workflows lint:

```bash
git grep -n 'sbom-floor' -- ':!ci/images/release_decision.py' ':!docs/'
actionlint .github/workflows/images.yml .github/workflows/release.yml; echo "rc=$?"
```

Expected: exactly three lines (two in `images.yml`, one in `release.yml`), each with `--arch "${ARCH}"`; `rc=0`. The release step has no `working-directory:` (Review Focus 5).

- [ ] **Step 7: Commit**

```bash
git branch --show-current   # must print feature/sma-665-chisel-dpkg-status-sbom
git add ci/images/release_decision.py .github/workflows/images.yml .github/workflows/release.yml
git commit -m "feat(ci): hold the cargo SBOM floor to the chisel fetch list (SMA-665)

sbom-floor takes --arch for every image kind. For a cargo key it reads
chisel-manifest-<key>-<arch>.txt and needs the SBOM's pkg:deb entries to
be the same set of name, version and arch. Each list entry must be in
the SBOM exactly once. A missing list is exit 2. An npm key reads no
list. sbom-summary adds the deb count.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: Documentation

**Files:**
- Modify: `docs/ops/RUNBOOK-containers.md:542-558` (subsection "Which libc is in the image I am running?"), add a subsection before line 560 (`## Release tooling (SMA-658, PR 1)`), and `:694-698` (the per-platform SBOM attestation text in "Verify a published image")
- Modify: `rs/CLAUDE.md:235-249` (the `images.yml` filter bullet) and `:274-278` (the `cargo auditable` bullet)

**Interfaces:**
- Consumes: the behaviour of Tasks 1 to 3.
- Produces: no code.

- [ ] **Step 1: Extend "Which libc is in the image I am running?"**

In `docs/ops/RUNBOOK-containers.md`, replace:

```markdown
This is the answerable half of a real limit. `chisel cut` uses the **live** Ubuntu archive (see
```

with:

```markdown
Since SMA-665, the image itself also carries a record of the cut: `/var/lib/chisel/manifest.wall`
(chisel's own manifest) and `/var/lib/dpkg/status.d/libc6` (see the next subsection).

This is the answerable half of a real limit. `chisel cut` uses the **live** Ubuntu archive (see
```

- [ ] **Step 2: Add the new subsection**

Insert this text directly before the line `## Release tooling (SMA-658, PR 1)`:

````markdown
### Which Ubuntu packages does the image SBOM list?

Since SMA-665, the SBOM of `paigasus-iam` and `paigasus-gateway` lists one Debian entry for each
Ubuntu package of the chisel cut. Two kinds of file in the image give that data:

- `/var/lib/chisel/manifest.wall` is chisel's own record of the cut, from the `base-files_chisel`
  slice. It is a zstd-compressed jsonwall file. syft 1.52.0 cannot read it.
- `/var/lib/dpkg/status.d/<package>` holds one dpkg status stanza for each package.
  `/var/lib/dpkg/status.d/<package>.md5sums` lists the files of that package that the cut
  installed. `rs/docker/chisel-dpkg-status.sh` writes both in the `rootfs` stage of
  `rs/Dockerfile`, from the manifest and from the `Fetching pool/…` lines of the cut. This is the
  layout of the distroless images, and syft's `dpkg-db-cataloger` reads it. The `.md5sums` files
  are necessary: without them, syft also lists `libgcc_s.so.1` as a second package, `gcc-14`.

To read the data of an image:

```bash
docker create --name sbom-probe <image>
docker cp sbom-probe:/var/lib/dpkg/status.d - | tar -tv
docker cp sbom-probe:/var/lib/dpkg/status.d/libc6 - | tar -xO
docker rm sbom-probe
```

The release checks this data. `ci/images/release_decision.py sbom-floor` compares the Debian
entries of the SBOM with `chisel-manifest-<service>-<arch>.txt` of the same build. Each package
of that list must be in the SBOM exactly once, with the same version and architecture. The SBOM
must have no other Debian entry. A failure stops the release before the upload.

Know these limits before you use the SBOM for a vulnerability scan:

- **The image has no `dpkg`.** The `status.d` data is a package list only. No tool in the image
  reads it.
- **The packages are partial.** Each stanza says `Status: install ok installed`, but the cut
  installs only some slices of the package. A scanner can report a CVE for a file that is not in
  the image. These are false positives, not false negatives. Sven accepted them (SMA-665 Q6).
  Before you act on a finding, find the file of the finding in the `.md5sums` file of the
  package, or in `manifest.wall`.
- **The `Source:` line has no source version.** A scanner that needs the source version uses the
  binary version.
- **A syft bump can stop the release.** A later syft can get a chisel cataloger
  (anchore/syft#5091). It then lists each package twice: once from `status.d` and once from
  `manifest.wall`. The floor then fails with "expected exactly once". This is intended. In the
  PR that bumps syft, remove `rs/docker/chisel-dpkg-status.sh` and its `RUN` line in
  `rs/Dockerfile`, then make the floor pass again. `images.yml` shows the failure on that PR, but
  it is not a required check, so the PR can merge red, and the next release then stops at the
  floor.
````

- [ ] **Step 3: Point "Verify a published image" to the new subsection**

Replace:

```markdown
amd64 and one for arm64, because each architecture has its own SBOM. So an SBOM query against the
index digest finds nothing. Get the two per-platform digests from the index itself:
```

with:

```markdown
amd64 and one for arm64, because each architecture has its own SBOM. So an SBOM query against the
index digest finds nothing. Each service SBOM lists the Rust crates and the Ubuntu packages of the
chisel cut (see "Which Ubuntu packages does the image SBOM list?" in § 7). Get the two
per-platform digests from the index itself:
```

- [ ] **Step 4: Update `rs/CLAUDE.md`**

Replace:

```markdown
- The `pull_request` filter of `images.yml` lists the image build inputs. For `rs/` these are
  `rs/Dockerfile`, `rs/Cargo.{lock,toml}`, `rs/rust-toolchain.toml` and `rs/.dockerignore`. For
```

with:

```markdown
- The `pull_request` filter of `images.yml` lists the image build inputs. For `rs/` these are
  `rs/Dockerfile`, `rs/docker/**` (the chisel dpkg status generator, its self-test and its
  fixtures, SMA-665), `rs/Cargo.{lock,toml}`, `rs/rust-toolchain.toml` and `rs/.dockerignore`. For
```

Replace:

```markdown
- `rs/Dockerfile` builds the services with **`cargo auditable`**, which is what makes the image
  SBOM list the Rust crates. MEASURED (SMA-658, 2026-09-20): a plain `cargo build` gives `cargo=0`.
  The OS-package half of the SBOM is still empty and is tracked as SMA-665: syft 1.52.0 reads only
  `/var/lib/dpkg/status` or `.deb` files, and a chisel cut writes neither. The `base-files_chisel`
  slice does NOT help — it writes a chisel-specific manifest that syft cannot read.
```

with:

```markdown
- `rs/Dockerfile` builds the services with **`cargo auditable`**, which is what makes the image
  SBOM list the Rust crates. MEASURED (SMA-658, 2026-09-20): a plain `cargo build` gives `cargo=0`.
  The OS-package half comes from SMA-665. syft 1.52.0 reads Debian packages only from
  `/var/lib/dpkg/status`, `/var/lib/dpkg/status.d/*` or `.deb` files, and it cannot read the
  chisel manifest. So the cut adds the `base-files_chisel` slice (it writes
  `/var/lib/chisel/manifest.wall`, which stays in the image), and `rs/docker/chisel-dpkg-status.sh`
  turns that manifest into `status.d` data: one stanza and one `.md5sums` file for each package.
  The `.md5sums` files are NOT optional. MEASURED (SMA-665 M2): without them, syft also lists
  `libgcc_s.so.1` as a package `gcc-14` with a `pkg:deb` purl and no arch, and the cargo SBOM floor
  fails. That floor compares the SBOM's `pkg:deb` entries with `chisel-manifest-<key>-<arch>.txt`
  of the same build. A syft bump that adds a chisel cataloger (anchore/syft#5091) reds that floor
  ON PURPOSE, because each package then appears twice: remove the generator in the same PR
  (SMA-665 D6).
- The generator self-test is the `chisel-dpkg-status-test` stage of `rs/Dockerfile`. `images.yml`
  builds it with `--target`. Keep it ABOVE the final `FROM scratch`: a build with no `--target`
  builds the last stage, and `assert_pins` reads the last `FROM` as the final stage. Its fixtures
  are in `rs/docker/fixtures/`, because the build context is `rs/`. The generator runs under dash:
  no pipe, no bash syntax.
```

- [ ] **Step 5: Check the doc guards**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-665-chisel-dpkg-status-sbom
git grep -n 'ciReport' -- docs/ops/RUNBOOK-containers.md rs/CLAUDE.md; echo "rc=$?"
wc -l rs/CLAUDE.md
```

Expected: `rc=1` (no match: actionlint check 12 needs no marker). Read both changed sections once more for STE style.

- [ ] **Step 6: Commit**

```bash
git branch --show-current   # must print feature/sma-665-chisel-dpkg-status-sbom
git add docs/ops/RUNBOOK-containers.md rs/CLAUDE.md
git commit -m "docs(rs): document the chisel dpkg status data in the image SBOM (SMA-665)

The runbook says where the OS packages of the SBOM come from, how to
read them, and the limits: no dpkg in the image, partial packages, no
source version, and the stop on a syft bump. rs/CLAUDE.md replaces the
statement that the OS half of the SBOM is empty.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: Real-image evidence, the image mutations and the gate graph

This task changes no committed file. It gives the AC1 to AC5 evidence and the T3 and T4 proofs. Record each result (command, rc, the key output lines) in your report to the controller, for the PR body. The local Docker host is arm64, so this task gives the arm64 half of AC1. The pull request's `images.yml` run gives the amd64 half.

**Files:**
- None committed. Outputs go to `out/` and the repo-root `chisel-manifest-*.txt` (both ignored) and to `$S`.

**Interfaces:**
- Consumes: Tasks 1 to 4.
- Produces: evidence only.

- [ ] **Step 1: Build both service archives (arm64)**

The first build compiles the Rust tree in `--release` and can take 20 minutes or more. Start it in the background with output to a file, and wait with the Monitor tool (until-loop on the exit marker). Do not end your turn while it runs.

Run this with `run_in_background: true`, then wait with Monitor on `grep -q '^EXIT=' "$S/build-oci.log"`:

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-665-chisel-dpkg-status-sbom
( /bin/bash ci/images/run.sh build-oci iam out && /bin/bash ci/images/run.sh build-oci gateway out; echo "EXIT=$?" ) > "$S/build-oci.log" 2>&1
```

Use `/bin/bash` (3.2): `ci/images/run.sh` uses no bash-4 feature, and `assert_pins` uses here-strings, which can hang under Homebrew bash 5 on a small-pipe host (root `CLAUDE.md`, SMA-612).
Expected: `EXIT=0`, `pins OK: …` (so `assert_pins` accepts the new test stage and the unchanged final stage), `built out/paigasus-iam-arm64.oci.tar` and `built out/paigasus-gateway-arm64.oci.tar`, and the files `chisel-manifest-iam-arm64.txt` and `chisel-manifest-gateway-arm64.txt` at the repo root.

- [ ] **Step 2: SBOM and floor for each archive (T3, AC1, AC2, AC3)**

```bash
for key in iam gateway; do
  syft "oci-archive:out/paigasus-${key}-arm64.oci.tar" -o "spdx-json=out/sbom-paigasus-${key}-arm64.spdx.json"
  uv run --no-project --python '>=3.12' python3 ci/images/release_decision.py sbom-floor --service "$key" --arch arm64 "out/sbom-paigasus-${key}-arm64.spdx.json"; echo "rc=$?"
  jq -r '.packages[].externalRefs[]?.referenceLocator | select(startswith("pkg:deb/"))' "out/sbom-paigasus-${key}-arm64.spdx.json"
  cat "chisel-manifest-${key}-arm64.txt"
done
```

Expected for each key: `deb=` equals the line count of the chisel list, `libc6=true`, `cargo=` above 0, `kind=cargo`, `floor=pass`, `rc=0`. Each `pkg:deb` purl has `arch=` and matches one list line. There is no `gcc-14@` purl (M3). Record the purl text verbatim.

- [ ] **Step 3: Image content (AC4, M5)**

```bash
docker load -i out/paigasus-iam-arm64.oci.tar
docker create --name sma665-probe paigasus-iam:dev > /dev/null
docker export sma665-probe > "$S/iam-export.tar"
docker rm sma665-probe > /dev/null
tar -tvf "$S/iam-export.tar" > "$S/iam-files.txt"
grep -E ' (var/lib/dpkg/status\.d/|var/lib/chisel/manifest\.wall)' "$S/iam-files.txt"
grep -E ' (usr/)?bin/(jq|zstd|dpkg|sh|bash)$' "$S/iam-files.txt"; echo "grep-rc=$?"
```

Expected: the `status.d` files and `manifest.wall` are listed (record their sizes: M5 is the sum; a few KB expected). `grep-rc=1`: no `jq`, `zstd`, `dpkg` or shell in the image. (`docker load` of an OCI archive works on this Mac's containerd store. CI uses `load-oci` instead.) If `docker load` names a different tag, use the tag it prints. The full `assert_base_intact` check (no shell, CA bundle, 200 MB ceiling) runs in the `images.yml` smoke step (T5).

- [ ] **Step 4: Mutation proof: no generator call (T4, AC6)**

In `rs/Dockerfile`, change `RUN chisel-dpkg-status.sh /rootfs /tmp/cut.log` to `RUN true`. Then:

```bash
/bin/bash ci/images/run.sh build-oci iam out > "$S/mut-nogen.log" 2>&1; echo "rc=$?"
syft "oci-archive:out/paigasus-iam-arm64.oci.tar" -o "spdx-json=out/sbom-mut-nogen.spdx.json"
uv run --no-project --python '>=3.12' python3 ci/images/release_decision.py sbom-floor --service iam --arch arm64 out/sbom-mut-nogen.spdx.json; echo "rc=$?"
```

Expected: the build passes (the builder stage comes from cache; only `rootfs` re-runs). The floor prints `floor=fail` and exits 3. Its stderr names `the SBOM does not list libc6` and `the SBOM lists libc6 … 0 times`. Restore the line with the Edit tool and check `git diff rs/Dockerfile` is empty.

- [ ] **Step 5: Mutation proof: a changed version in one stanza (T4, AC6)**

In `rs/docker/chisel-dpkg-status.sh`, change `    printf 'Version: %s\n' "$version"` to `    printf 'Version: %sx\n' "$version"`. Run the three commands of Step 4 again (log `$S/mut-version.log`, SBOM `out/sbom-mut-version.spdx.json`).
Expected: the floor exits 3. Its stderr names each list entry, for example `the SBOM lists libc6 2.39-0ubuntu8.9 arm64 0 times; expected exactly once`. (The reverse count rule does not fire here: the SBOM still has as many Debian entries as the list. The forward rule catches the change.) Restore with the Edit tool and check `git diff rs/docker/` is empty.

- [ ] **Step 6: Mutation proof: no `.md5sums` files (M3, P2)**

In `rs/docker/chisel-dpkg-status.sh`, change `  if [ -s "$tmp/md5" ]; then` to `  if false; then`. Run the three commands of Step 4 again (log `$S/mut-md5.log`, SBOM `out/sbom-mut-md5.spdx.json`).
Expected: the floor exits 3, and its stderr names `the SBOM lists gcc-14 14.2.0-4ubuntu2~24.04.1 with no arch qualifier`. Restore with the Edit tool and check `git diff rs/docker/` is empty. Then rebuild once (Step 1 for `iam` only) so `out/` holds an unmutated archive again.

- [ ] **Step 7: The repo gates this change selects**

This PR selects `repo:actionlint` (workflows changed, a new `.sh` file), `repo:affected-smoke` (`rs/Dockerfile` is one of its required inputs) and `repo:ruff-ci` (`ci/**/*.py`). Run each with the bash it needs (root `CLAUDE.md`, "This development Mac only"):

```bash
/bin/bash ci/affected-graph/run.sh; echo "affected-smoke rc=$?"
/opt/homebrew/bin/bash ci/ruff/run.sh; echo "ruff-ci rc=$?"
/opt/homebrew/bin/bash ci/actionlint/run.sh; echo "actionlint rc=$?"
```

Expected: rc 0 for each. Read the actionlint preflight line first. If it says the pipe holds only 512 bytes (rc 2 with a `small` message), the host is in the SMA-612 small-pipe state. Then run that gate in a Linux container instead (see the memory note "Small pipe: run gates in a Linux container"), or record that CI gives its verdict. Then run the full graph as CI does, with the command between the `ci-targets` markers in the root `CLAUDE.md`, and read the per-gate bash notes for any gate that fails only locally.

- [ ] **Step 8: Clean up local outputs**

```bash
docker image rm paigasus-iam:dev > /dev/null 2>&1 || true
rm -f "$S/iam-export.tar"
git status --short
```

Expected: `git status --short` prints nothing (`out/` and `chisel-manifest-*.txt` are ignored).

- [ ] **Step 9: CI evidence after the push (done in the PR stage)**

After the branch is pushed, the pull request runs `images.yml` on amd64 (the filter matches `rs/Dockerfile`, `rs/docker/**`, `ci/images/**` and the workflow). Record from its log: the `Chisel dpkg status generator self-test` step (15 rows OK), the two `M8 image=paigasus-… arch=amd64 … deb=N` lines with `floor=pass`, the `Console release sequence` floors (`kind=npm`, `floor=pass`), and the smoke step's `no shell, … CA certs, … MB` lines (AC4, T5). Then start the arm64 leg on the branch with `gh workflow run images.yml --ref feature/sma-665-chisel-dpkg-status-sbom` and record its two arm64 `M8` lines. The PR body says which run gave the arm64 evidence (T3): the local arm64 build of Step 2, the dispatch, or both.

## Self-review against the spec

- AC1 → Task 2 (image content), Task 5 Steps 2 and 9 (both services, arm64 locally and by dispatch, amd64 on the PR).
- AC2 → Task 3 Step 6 (both callers), Task 5 Step 9 (`images.yml`); `release.yml` runs the same anchor step before the upload.
- AC3 → Task 3 (`_chisel_reasons`: forward exactly once, reverse count, no-arch, list arch).
- AC4 → Task 2 Step 5 and Task 5 Step 3 (no `jq`, `zstd`, `dpkg` or shell); `assert_pins` unchanged and passes in Task 5 Step 1.
- AC5 → Task 3 (cargo keeps `cargo >= 1`, adds libc6 and the list; npm unchanged; the CLI npm row with the cargo arguments).
- AC6 → Task 1 Step 5 (T2), Task 1 Step 6, Task 2 Step 6, Task 3 Step 5, Task 5 Steps 4 to 6 (T4).
- D1 → Task 2 Step 3 (slice, the comment records the SMA-500 § 4.5 reversal, no ADR per Q5). D2, D3 → Task 1. D4 → Task 1 (required, P2). D5 → Task 3. D6 → Task 3 "twice" row, Task 4 runbook. D7 → Task 3 Step 6. D8 → Task 3 (consoles get `--arch` only). D9 → Task 1 location, Task 2 Step 7 filter. D10 → Task 2 Step 3 test stage above `FROM scratch`, Task 2 Step 7 images.yml step.
- M2, M3, M4 → taken while planning (above). M5 → Task 5 Step 3.
- § 5.5 documentation → Task 4.
