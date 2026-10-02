# SMA-665: the service image SBOM lists the Ubuntu packages of the chisel cut

- Linear: SMA-665
- Date: 2026-09-27
- Areas: `rs/Dockerfile`, `rs/docker/`, `ci/images/`, `.github/workflows/images.yml`,
  `.github/workflows/release.yml`, `docs/ops/RUNBOOK-containers.md`, `rs/CLAUDE.md`
- Related: SMA-658 (the release path, § 4.5, M8, AC 7, P10), SMA-500 (the chisel runtime base,
  § 4.5), SMA-688 (the SBOM floor per image kind, D5)
- Revision 3, 2026-10-01. **Approved by Sven.** Revision 2 was written unattended by the
  feature-factory Stage 1 agent and folds in the spec-challenger critique (see "Challenge
  changelog" at the end). Revision 3 applies Sven's answers to Q1 to Q6 (§ 11) and re-checks every
  cited path and line against `origin/main` `68df3372` (see "Re-check on 2026-10-01" at the end).

## 1. The problem

The runtime base of `paigasus-iam` and `paigasus-gateway` is a `chisel cut` of Ubuntu 24.04
(`rs/Dockerfile`, stage `rootfs`). The cut writes only the files of the selected slices:
`base-files_base`, `base-files_release-info`, `libc6_libs`, `libgcc-s1_libs`,
`ca-certificates_data`. It writes no `/var/lib/dpkg/status` and no `.deb` file.

syft 1.52.0 (the `.prototools` pin) finds Debian-family OS packages with two catalogers only:
`dpkg-db-cataloger` (needs `/var/lib/dpkg/status` or `/var/lib/dpkg/status.d/*`) and
`deb-archive-cataloger` (needs `.deb` files). So the SBOM that `release.yml` attests shows the
Rust crates (through `cargo auditable`), but no `libc6`, no `libgcc-s1`, no `ca-certificates` and
no `base-files`. An SBOM-based scanner cannot see the OS content of the image.

Measured facts from the issue (2026-09-20, syft 1.52.0, chisel v1.4.2):

- Before `cargo auditable`: `packages=2 libc6=false cargo=0`. The two entries are `gcc-14` (the
  elf-binary cataloger reads a version string in `libgcc_s.so.1`) and SPDX's `DocumentRoot-Image`.
- The `base-files_chisel` slice alone does not help. It writes `/var/lib/chisel/manifest.wall`,
  and syft 1.52.0 has no cataloger for that format.
- `cargo auditable build` closes the crate half. That half is on `main` and is out of scope here.

Upstream state (checked 2026-09-27): syft issue anchore/syft#3824 ("Support for Canonical's
Chiseled Containers") is open. The draft PR anchore/syft#5091 (2026-07-22) adds a chisel manifest
cataloger. It is not merged and not released.

## 2. Acceptance criteria

- **AC1.** For each of `iam` and `gateway`, on each of `amd64` and `arm64`, the SPDX SBOM that
  syft makes from the real OCI archive lists one Debian package entry for each package in the
  chisel fetch list of that build. Each entry has the exact version and architecture that chisel
  installed. The list is not fixed in this spec: the `*_copyright` essentials of the slices can
  pull in more packages than `base-files`, `libc6`, `libgcc-s1` and `ca-certificates` (M4).
- **AC2.** The check for AC1 runs against the real image, not a fixture. It runs in
  `images.yml` (each pull request that touches the image path) and in `release.yml`
  (`images-build-<svc>`, before the upload and the attestation). A failure stops the release.
- **AC3.** The check compares the SBOM with a second record of the same chisel run: the chisel
  package-fetch list that `ci/images/run.sh build-oci` already writes
  (`chisel-manifest-<key>-<arch>.txt`). The two records are not independent (R1). The Debian
  entries of the SBOM and the fetch list must be the same set of (name, version, arch). A package
  that is missing from the SBOM fails. A package that the SBOM lists twice fails. A Debian entry
  in the SBOM that is not in the list fails.
- **AC4.** The runtime image stays chiseled: no shell, no package manager, no `jq`, no `zstd`.
  `assert_base_intact` passes without change. The final stage still COPYs only the rootfs and the
  binary (`assert_pins`).
- **AC5.** The cargo floor keeps its crate rule (`cargo >= 1`) and adds the OS rule. The npm
  floor (consoles) does not change. The console release chains call the floor with the same
  arguments as the service chains and still pass.
- **AC6.** A self-test proves the generator on a fixture manifest, and a mutation proves that the
  new floor rows bite.

## 3. Options (from the issue)

1. **Wait for, or contribute, a chisel-aware syft cataloger.** Rejected for now (Sven, Q1,
   2026-10-01: generate the status now, do not wait for anchore/syft#5091). The upstream PR
   is a draft with no release date. A syft bump is a `.prototools` pin change, so this option
   stays open for later (§ 7, D6).
2. **Generate dpkg-shaped status data from the chisel manifest at build time.** Chosen. syft's
   existing `dpkg-db-cataloger` then finds the packages with no syft change.
3. **Move the runtime base off a pure chisel cut.** Rejected. This reverses the SMA-500
   runtime-base decision, and it needs its own decision and ADR. The SBOM gap does not justify it.

## 4. Decisions

- **D1. Add the `base-files_chisel` slice to the cut.** It writes
  `/var/lib/chisel/manifest.wall`. This is the authoritative record of what chisel installed:
  one `package` object for each package (`name`, `version`, `sha256`, `arch`), and one `path`
  object for each file, with the slices that own it. The file stays in the image. A future syft
  with a chisel cataloger can read it directly.
  **This replaces a decision of SMA-500.** SMA-500 § 4.5
  (`docs/superpowers/specs/2026-08-19-sma-500-service-container-images-design.md`, the paragraph
  on `chisel-manifest-<service>.txt`) chose not to write a manifest into `/rootfs`, because the
  manifest would then ship in each image. This spec ships it on purpose: without an in-image
  record of the packages, no SBOM tool can see them. The cost is a few KB (M5) and the D6 risk.
  Sven decided on 2026-10-01 (Q5) that this reversal needs NO Notion ADR. This spec is the
  record of the reversal of SMA-500 § 4.5. Sven also decided (Q4) that `manifest.wall` stays in
  the image after the generator runs.
- **D2. Generate one dpkg status file for each package, in the `status.d` layout, in the
  `rootfs` stage.** A POSIX `sh` script, `rs/docker/chisel-dpkg-status.sh`, reads the manifest
  and writes `/rootfs/var/lib/dpkg/status.d/<name>`, one stanza in each file:

  ```text
  Package: libc6
  Status: install ok installed
  Architecture: amd64
  Version: 2.39-0ubuntu8.6
  Source: glibc
  ```

  This is the layout that distroless uses. The console images already use a distroless base, and
  the npm floor already finds `libc6` in them with syft 1.52.0, so syft reads this layout in this
  repo today. A `status.d` directory also does not look like a working dpkg installation (R2).
  `zstd` and `jq` come from `apt-get install` in the `rootfs` stage only, so they never reach the
  final image. The files are in `/rootfs`, so the final stage COPY rule in `assert_pins` does not
  change.
- **D3. The `Source:` field comes from the pool directory of the fetch line.** In scope (Sven,
  Q2, 2026-10-01). A scanner such as
  grype matches Ubuntu security data by SOURCE package (`glibc`, not `libc6`). An Ubuntu pool
  path is `pool/<component>/<prefix>/<source>/<binary>_<version>_<arch>.deb`. So the fourth path
  segment is the source name, for example `pool/main/g/glibc/libc6_…`. The `RUN` that runs
  `chisel cut` writes the chisel output to a file in the stage (see § 5.1), and the generator
  reads the fetch lines from that file. The rule:
  - For each manifest package, exactly one fetch line must have the same binary name, version and
    arch. If none or more than one exists, the build FAILS.
  - If the fetch line does not parse as the shape above, the build FAILS.
  - If the source segment differs from the binary name, write `Source: <source>`.
  - If the source segment equals the binary name, write no `Source:` line. dpkg itself omits the
    field in that case.
  This needs no network, no `dpkg-deb`, no chisel cache and no `apt-cache`. The earlier design
  (read `dpkg-deb -f` from the chisel cache, with an `apt-cache` fallback) is dropped: it depended
  on an unmeasured cache path (old M1), and `apt-cache` can disagree with the fetch when the
  archive changes between `apt-get update` and the cut. The cost: the `Source:` line carries no
  source version (`Source: gcc-14 (14.2.0-…)` form). This is a gap only when a binary version
  differs from its source version (R5).
- **D4. Write per-package file ownership only in a format that syft reads (conditional on M2).**
  The side goal is to let syft's binary-overlap rule drop the false `gcc-14` ELF entry. The
  first revision wrote `info/<name>.list`. The challenger expects that syft reads ownership from
  `<name>.md5sums` (and `.conffiles`), not from `.list`. This is not checked against the syft
  source. So: M2 measures which file syft 1.52.0 reads for the `status.d` layout. If it is
  `status.d/<name>.md5sums`, the generator writes it: the md5 of each regular file that the
  package's slices own (from the manifest `path` objects, where the slice name is
  `<package>_<slice>`), with paths relative to the root, as dpkg writes them. If M2 shows no
  effect, drop D4 and its test rows. D4 is not an acceptance criterion.
- **D5. `sbom-summary` gains `deb=<count>`. `sbom-floor` gains a required `--arch ARCH`.** The
  caller is the SAME for every kind: `sbom-floor --service KEY --arch ARCH SBOM`. The script,
  not the shell, decides what the kind needs:
  - For kind `cargo`, the script reads `chisel-manifest-<key>-<arch>.txt` from the working
    directory (`--chisel-dir DIR` overrides the directory, for tests). A missing, unreadable or
    empty file is exit 2. A line that does not parse is exit 2. One name with two versions in the
    list is exit 2.
  - For kind `npm`, the script does not read a chisel list. `--arch` is accepted and ignored.
  The cargo floor then needs all of:
  - `cargo >= 1` (unchanged);
  - `libc6=true`;
  - set equality between the fetch list and the SBOM's Debian entries, on (name, version, arch):
    each list entry matches exactly one SBOM entry, and `deb` equals the list length.
  The SBOM side is PARSED, not string-matched. For each SPDX package, the script takes each
  `externalRefs` locator that starts with `pkg:deb/`, parses it as a purl (type `deb`, namespace
  ignored), percent-decodes the name and the version, and reads the `arch` qualifier. It ignores
  the other qualifiers. It drops a leading `N:` epoch from the SBOM version, because a pool file
  name has no epoch. If a `pkg:deb/` purl has no `arch` qualifier, that is a floor failure
  (exit 3), because D7 claims the arch.
  Hand-written purls in T1 do not prove the real encoding. M2 records the real purl text of the
  real image, and T1 adds rows with `~`, `+`, `%2B`, `%7E` and `1%3A`.
- **D6. The "exactly one" rule is the tripwire for a syft bump. The release path takes this risk
  on purpose.** When a later syft adds a chisel cataloger (anchore/syft#5091, and #4816 about
  duplicate entries), syft can report each package twice: once from the generated status and
  once from `manifest.wall`. The floor then fails, and the person who bumps syft must remove the
  generator (§ 7). A `.prototools` change matches the `images.yml` filter, so the bump PR shows
  the red. But `images.yml` is not a required check, so such a PR can still merge, and the next
  release then stops at the floor. This spec accepts that: the failure is loud and fails closed,
  and it does not ship a wrong SBOM. The alternative was to delete `manifest.wall` after the
  generator runs. Sven rejected it on 2026-10-01 (Q4): the file stays, and this risk stays.
- **D7. The two callers pass the arch of the same job.** `images.yml` (the `SBOM for each
  archive` step) and `release.yml` (the `Report the SBOM contents and hold its floor` step) pass
  `--arch "${ARCH}"`. `build-oci` writes `chisel-manifest-<key>-<arch>.txt` at the repo root in
  the same job, so it describes the same build as the archive. The floor compares the arch, so
  this claim is checked and not only assumed.
- **D8. No change to the console images.** They use a distroless base with dpkg data, and their
  npm floor already needs `libc6`. They only receive the new `--arch` argument (D5).
- **D9. The generator lives at `rs/docker/chisel-dpkg-status.sh`, and `images.yml` gains the
  filter entry `rs/docker/**`.** The build context is `rs/`, so the plain
  `docker build -f rs/Dockerfile rs/` in the Dockerfile header keeps working. The challenger's
  other option (put the script under `ci/images/` and pass a named build context) is rejected:
  it breaks that plain command.
- **D10. The generator self-test (T2) is a `--target` test stage in `rs/Dockerfile`, built by
  `images.yml`.** Sven decided this on 2026-10-01 (Q3). There is no new `repo:*` moon gate, so
  the `ci-targets` block and `ci.yml`'s `T=(…)` array do not change. The stage is
  `FROM rootfs AS chisel-dpkg-status-test`. It MUST come BEFORE the final `FROM scratch` stage,
  for two reasons. First, a build with no `--target` builds the LAST stage, so a test stage at the
  end would replace the image. Second, `assert_pins` in `ci/images/run.sh` reads the final stage
  as the text from the last `FROM` line to the end of the file. A test stage at the end would
  become that "final stage", and the COPY rule would then check the wrong stage. BuildKit builds
  only the stages that the target needs, so a normal build skips the test stage.

## 5. Design

### 5.1 `rs/Dockerfile`, stage `rootfs`

- `apt-get install` adds `jq zstd` next to `ca-certificates curl`.
- The cut adds `base-files_chisel`.
- `COPY --chmod=0755 docker/chisel-dpkg-status.sh /usr/local/bin/` into the `rootfs` stage (the
  build context is `rs/`). The script does not depend on the git exec bit.
- The `chisel cut` command writes its output to a file and then prints it, with no pipe, because
  dash has no `pipefail`:
  `chisel cut … >/tmp/cut.log 2>&1 || { cat /tmp/cut.log; exit 1; }; cat /tmp/cut.log`.
  The `cat` keeps the `Fetching pool/…` lines in the `--progress=plain` build log, so
  `extract_chisel_manifest` in `ci/images/run.sh` does not change.
- Then `chisel-dpkg-status.sh /rootfs /tmp/cut.log`.
- The stage keeps `--no-cache-filter=rootfs` behaviour from `run.sh`, so the status data always
  matches the cut of this build.
- The comment above the `cargo auditable` step (`rs/Dockerfile` lines 20-26 on `68df3372`) says
  only that `cargo auditable` makes the SBOM list the Rust crates. It does not mention the OS
  half. Add one sentence there, or a comment above the `chisel cut` `RUN`, that the OS half comes
  from `base-files_chisel` plus the generator (this spec).
- `assert_pins` reads `chisel cut --release ubuntu-X.Y` with a `grep -oE` on the Dockerfile text.
  The new redirect after the slice list does not change that match. Keep the text
  `chisel cut --release ubuntu-24.04` on one line.

### 5.2 `rs/docker/chisel-dpkg-status.sh`

- Input: a rootfs root and the cut log. Reads `<root>/var/lib/chisel/manifest.wall`.
- Output: `<root>/var/lib/dpkg/status.d/<name>`, and `<root>/var/lib/dpkg/status.d/<name>.md5sums`
  only if D4 survives M2.
- Reads the manifest without a pipe: `zstd -dc manifest.wall > /tmp/manifest.jsonl`, check the
  exit status, then `jq` on the file, check the exit status. A pipe under dash can give a partial
  package list and exit 0.
- Checks the jsonwall header (the first line): `schema` must be the one known value (measured in
  M4; `1.0` expected), and the header `count` must equal the number of the other lines. An unknown
  schema or a count mismatch fails.
- Fails (non-zero, one message on stderr) when: the manifest is missing; it holds no `package`
  object; the header check fails; a `Source:` lookup fails (D3);
  `<root>/var/lib/dpkg/status` or `<root>/var/lib/dpkg/status.d` already exists (a chisel that
  starts to write dpkg data would otherwise be silently overwritten or doubled).
- Output is deterministic for one cut: one file for each package, lines in a fixed order, and
  `.md5sums` lines sorted by path.
- SPDX header. Runs under `/bin/sh` (dash) in the stage, so no bash-only syntax.
- The file is a tracked `.sh` file, so `repo:actionlint` check 13 (no early-exit reader after a
  pipe) applies to it. Do not pipe into `head`, `grep -q`/`-m` or `awk … exit`. Also run
  `shellcheck -s sh` on it.

### 5.3 `ci/images/release_decision.py`

- `sbom_summary` adds `deb` (the count of `pkg:deb/` purls).
- New `sbom_debs(doc) -> list[tuple[str, str, str]]`: (name, version, arch) for each `pkg:deb/`
  purl in the SBOM, parsed and percent-decoded per D5, epoch dropped. A `pkg:deb/` purl that does
  not parse is exit 2. This is the producer of the `debs` argument below.
- New `parse_chisel_list(text) -> list[tuple[str, str, str]]`: (name, version, arch) from the
  fetch lines. Exit 2 on an empty input, a line that does not parse, or one name with two
  versions.
- `sbom_floor(key, kinds, summary, *, debs, chisel)`: `chisel` is `None` for kind `npm`. For kind
  `cargo` it applies D5.
- The CLI adds `--arch` (required) and `--chisel-dir` (default `.`) to `sbom-floor`. `main` reads
  the list file only when the kind is `cargo`, and fails closed (exit 2) when the file is missing.
  The decision is never made in shell from whether the file exists, because that fails open.
- The docstring's subcommand list changes to match.

### 5.4 Callers

- The `&image-build-steps` anchor in `.github/workflows/release.yml` is used by FOUR jobs:
  `images-build-iam`, `images-build-gateway`, `images-build-iam-console` and
  `images-build-gateway-console`. The edit in the anchor adds `--arch "${ARCH}"` to the
  `sbom-floor` call. It adds no flag that is kind-specific, so the same line is correct for the
  console chains (D5). On `68df3372` the anchor is at `release.yml:1198`, the aliases are at
  `:1802`, `:1883` and `:1956`, and the step is `Report the SBOM contents and hold its floor
  (spec AC 7, SMA-688 D5)` at `:1257`, with the `sbom-floor` call at `:1266`. The comment in that
  step (lines 1263-1265) says that a cargo image needs Rust crates. It changes to say that a
  cargo image also needs the Debian entries of its chisel list.
- `.github/workflows/images.yml`: the service SBOM loop (step `SBOM for each archive`, the call
  at line 172 on `68df3372`) and the console loop (step `Console release sequence`, the call at
  line 207) both add `--arch "${ARCH}"`. The `pull_request` filter (lines 29-71; SMA-675 added
  four `ts/` entries to it) adds `rs/docker/**` (D9). `repo:actionlint` check 5 reds a `paths:`
  glob that matches no tracked file, so the entry and the script land in the same commit or
  later. The T2 step (`docker buildx build --target chisel-dpkg-status-test -f rs/Dockerfile rs/`)
  goes before `Build both images as OCI archives`, next to `Decision script self-test`.
- No guard in `ci/actionlint/` reads the SBOM steps (checked on `origin/main`, 2026-09-27), so no
  guard changes. Re-checked on `68df3372`: `sbom-floor` occurs only in
  `ci/images/release_decision.py` and in the two workflows. Two guards read `rs/Dockerfile`
  itself. `ci/affected-graph/cargo_moon_parity.py` A8 checks each cargo invocation in it; this
  spec adds no cargo invocation. `ci/actionlint/run.sh` lists `rs/Dockerfile` as a required input
  of `repo:affected-smoke`, so this PR selects that gate. The PR also selects `repo:actionlint`
  (a workflow changes). See the root `CLAUDE.md` for the local bash of each gate.

### 5.5 Documentation

- `docs/ops/RUNBOOK-containers.md`: the runbook has NO SBOM section on `68df3372` (the earlier
  revision assumed one). Add a new subsection directly after § 7's "Which libc is in the image I
  am running?" (line 542). It says that the OS packages come from generated dpkg `status.d` data,
  how to read them, the D6 rule for a syft bump, R2 (no dpkg in the image) and R4 (partial
  packages). That existing subsection names the `chisel-manifest-<service>-<arch>.txt` file; add
  one sentence that the image now also carries `/var/lib/chisel/manifest.wall`. "Verify a
  published image" (the per-platform SBOM attestation text, lines 694-698) gets one sentence that
  points to the new subsection. The new text must not name the moon CI report file: actionlint
  check 12 then needs a `moon-diagnosis` marker in the file.
- `rs/CLAUDE.md`: REWRITE the `cargo auditable` bullet (lines 274-278 on `68df3372`). The text
  "The OS-package half of the SBOM is still empty" and "The `base-files_chisel` slice does NOT
  help" become false with this change. The new text says that the slice plus the generator fill
  the OS half, and that a syft bump that adds a chisel cataloger reds the floor on purpose (D6).
- `rs/CLAUDE.md`: the `images.yml` filter bullet (lines 235-249 on `68df3372`) adds
  `rs/docker/**` to its `rs/` list.

## 6. Test strategy

- **T1, self-test rows in `release_decision.py`** (the existing `self_test` table):
  - `sbom_summary` counts `deb`.
  - `sbom_debs`: a plain purl; a purl with `%2B` and one with a raw `+`; a version with `~` and
    one with `%7E`; an epoch `1%3A` and a raw `1:`, which both drop to the pool version; a purl
    with no `arch` qualifier; a purl that does not parse (exit 2).
  - `parse_chisel_list`: a real fetch line (text from M4); an empty input (exit 2); a line that
    does not parse (exit 2); one name with two versions (exit 2).
  - Floor pass: crates, libc6, and one Debian entry for each list entry, with the same arch.
  - Floor pass for kind `npm` with the same arguments as the cargo call (`--arch`), with no list
    file present.
  - Floor fail: a list entry that the SBOM does not have; a package that the SBOM has twice (a
    fixture SBOM with two entries; this is the proof of "exactly one", see T4); a Debian entry in
    the SBOM that is not in the list; `libc6=false`; a version mismatch; an arch mismatch.
  - Usage (exit 2): kind `cargo` with no list file; kind `cargo` with an empty list file; no
    `--arch`.
- **T2, generator self-test.** Fixtures: a jsonwall as PLAIN TEXT under `ci/images/fixtures/`,
  compressed with `zstd` at test time (no committed binary); a cut-log text with fetch lines. The
  test asserts the exact `status.d` text, and each failure mode in § 5.2. Rows for D3: a package
  whose source name differs (`libc6` from `glibc`, a `Source:` line); a package whose source name
  equals its name (`base-files`, no `Source:` line); a manifest package with no fetch line (fail);
  a fetch line that does not parse (fail); a header `count` mismatch (fail); an unknown `schema`
  (fail). If D4 survives, rows for the `.md5sums` text. The host is a `--target` self-test stage
  in `rs/Dockerfile` (D10, Sven's answer to Q3), placed after `rootfs` and BEFORE the final
  `FROM scratch` stage, and never used by the final image. It has the same tools (`jq`, `zstd`, dash) as the stage that runs the script, so
  it needs no extra container setup. `images.yml` runs it with
  `docker build --target chisel-dpkg-status-test`. The macOS dev host has no `dpkg`, and agents
  must not install host software.
- **T3, real image (AC1, AC2).** `images.yml` builds both services, runs syft on the archive and
  runs the new floor. A pull request builds amd64 only (`images.yml` matrix). The arm64 half of
  AC1 needs a `workflow_dispatch` run of `images.yml` on the branch, or the first `main` run. The
  PR records which one gave the arm64 evidence.
- **T4, mutations (AC6).** Each must red the named check, and each mutation must actually run
  (not die at a syntax error):
  - Remove `base-files_chisel` from the cut → the build fails in the generator.
  - Remove the generator call → T3 floor fails (list entries missing).
  - Change the version in one generated stanza (for example append `x`) → T3 floor fails on the
    version-mismatch rule.
  - Change `sbom_floor` to skip the list check → the T1 fail rows red.
  - Change `sbom_floor` to skip the reverse direction (`deb` == list length) → the T1 "entry not
    in the list" row reds.
  The first revision's mutation "write each stanza twice" is dropped. syft probably merges two
  identical stanzas into one package (same ID), so the mutation is probably inert. T1's
  two-entry fixture proves "exactly one" instead. If M2 shows that syft does NOT merge them, the
  mutation can come back as an extra row.
- **T5, base unchanged.** `ci/images/run.sh smoke` (`assert_base_intact`) and `assert_pins` pass.

## 7. Measurements for the plan

- **M1.** Dropped (D3 no longer reads the chisel cache).
- **M2.** With the generated `status.d` data, syft 1.52.0 on the real archive:
  - the exact purl text for each package, including the percent-encoding of `+`, `~` and `:`
    (record it verbatim, and copy it into T1);
  - that `base-files_release-info` supplies `/etc/os-release` for the `distro` qualifier;
  - whether syft needs the `Status:` line;
  - which ownership file syft reads for `status.d` (`<name>.md5sums`, `.conffiles`, `.list`).
    This decides D4.
  - whether syft reports two identical stanzas as one package (decides the T4 note).
  Take M2 BEFORE the plan fixes D4 and T4.
- **M3.** Whether the `gcc-14` ELF entry disappears with the D4 ownership file. Record the result
  either way; it is not an acceptance criterion. If the `gcc-14` entry stays, it is a `pkg:generic`
  or binary purl, not `pkg:deb`, so it does not affect the D5 set equality. The plan checks this.
- **M4.** The exact fetch-line shape of chisel v1.4.2 in `--progress=plain` output, on both
  architectures, for `parse_chisel_list` and D3. Also the full current fetch list on each arch,
  from a recent `chisel-manifests-<arch>` artifact of `images.yml` (AC1 does not fix the list).
  Also the jsonwall header `schema` value and `count` semantics of `manifest.wall`.
- **M5.** The size change of the image (expected: a few KB). `assert_base_intact` keeps its
  200 MB ceiling.

## 8. Files expected to change

- `rs/Dockerfile`
- `rs/docker/chisel-dpkg-status.sh` (new)
- `ci/images/release_decision.py`
- `ci/images/fixtures/…` (new generator fixtures, plain text; the name is not `build/`, see the
  root `.gitignore` trap)
- `ci/images/run.sh`: no change expected. Q3 put the generator self-test in `rs/Dockerfile`
  (D10), and `build-oci` does not change. A change is needed only if `assert_pins` must learn
  about the new test stage.
- `.github/workflows/images.yml` (the two SBOM loops, the `rs/docker/**` filter entry, the T2 step)
- `.github/workflows/release.yml` (the `&image-build-steps` anchor; covers four jobs)
- `docs/ops/RUNBOOK-containers.md`
- `rs/CLAUDE.md` (the `cargo auditable` bullet and the filter bullet)

## 9. Out of scope

- A chisel-aware syft cataloger (option 1). Revisit when anchore/syft#5091 ships.
- Moving off the chisel base (option 3).
- Reproducible chisel cuts (a pinned archive snapshot); SMA-658 § 12 keeps that.
- The console images (D8), except the new `--arch` argument.
- Vulnerability scanning of the SBOM (a grype gate). This ticket makes the data correct; it adds
  no scanner.

## 10. Risks

- **R1.** The generated status is a statement that the image makes about itself. It is only as
  correct as chisel's manifest. AC3 cross-checks it against the fetch log, which comes from the
  same chisel run, so both can be wrong together only if chisel itself lies about what it fetched.
  D3 also takes the source name from the same fetch log.
- **R2.** A generated dpkg data directory can make a scanner think that `dpkg` is present. It is
  not, and no tool in the image reads the data. The `status.d` layout (D2) is the distroless
  form, which scanners already treat as a package list without dpkg. The runbook says so.
- **R3.** The generator adds build failure modes (D3, § 5.2). That is intended (fail closed).
- **R4.** `Status: install ok installed` says that the whole package is installed. The chisel
  cut installs only some slices of it. A scanner then reports a CVE for a file that is not in the
  image. This gives false positives, not false negatives. The runbook says so, and says that a
  finding must be checked against the files in the image. Sven accepted these false positives on
  2026-10-01 (Q6).
- **R5.** The `Source:` line has no source version (D3). If a binary version differs from its
  source version, a scanner that needs the source version uses the binary version. For the
  current noble packages this is expected to be rare. M2 records the real `upstream` qualifier.
- **R6.** A future syft that reads `manifest.wall` stops the release at the floor (D6). This is
  a loud, fail-closed stop, taken on purpose.

## 11. Open questions (all answered by Sven on 2026-10-01)

- **Q1.** Is a generated dpkg status file acceptable to Sven as a statement in the image, or
  does he prefer to wait for the upstream syft cataloger (option 1)? The issue lists both; this
  spec picks option 2 because the upstream PR is a draft.
  **Answer:** generate the dpkg status now. Do not wait for anchore/syft#5091.
- **Q2.** Is `Source:` (D3) in scope, or is name + version enough for this ticket? The issue's AC
  names only "the Ubuntu packages". This spec includes it because CVE matching needs it, and the
  pool-directory method adds no network and no new tool.
  **Answer:** `Source:` is in scope (D3).
- **Q3.** Where does the generator self-test (T2) run: a `--target` test stage in `rs/Dockerfile`
  built by `images.yml` (the default here), an `ubuntu:24.04` container step in `images.yml`, or
  a new `repo:*` moon gate? A new gate needs registration in the `ci-targets` block and in
  `ci.yml`'s `T=(…)` array.
  **Answer:** a `--target` test stage in `rs/Dockerfile`, built by `images.yml` (D10). No new
  moon gate.
- **Q4.** Must `manifest.wall` stay in the image after the generator runs? Keeping it gives a
  future chisel-aware syft the authoritative record, and costs the D6 release stop on that bump.
  Deleting it removes that stop, and a later syft bump then needs the slice output again. This
  spec keeps it (D1, D6).
  **Answer:** keep `manifest.wall` in the image. The D6 release stop on a syft bump stays.
- **Q5.** Does "the image states synthetic dpkg data about itself" need a Notion ADR under the
  repo rule for significant choices? It reverses one sentence of SMA-500 § 4.5 (D1). This spec
  treats it as an implementation choice inside the SMA-658 SBOM decision, not as a new ADR.
  **Answer:** NO Notion ADR. This spec records the reversal of SMA-500 § 4.5 (D1).
- **Q6.** Are the scanner false positives from partial packages (R4) acceptable, or do they block
  the use of this SBOM for a later grype gate? This spec accepts them and documents them.
  **Answer:** the false positives are accepted (R4). The runbook documents them.

## Challenge changelog

Revision 2, 2026-09-27. Challenger verdict: **APPROVE WITH CHANGES** (1 blocker, 4 major,
13 minor, 5 questions). Each finding was checked against the repo on `main` (`83d446fc`).

Folded:

- BLOCKER, the four jobs on `&image-build-steps`. Confirmed: `release.yml:1198` defines the
  anchor, and `:1796`, `:1877`, `:1950` reuse it, two of them for the console chains. D5 now
  uses the same caller for every kind (`--service KEY --arch ARCH`). The script reads the list for
  kind `cargo` only and fails closed when it is missing. T1 adds an npm row with the cargo
  arguments. AC5 and § 5.4 say so.
- MAJOR, the `images.yml` filter. Confirmed: `images.yml:33-62` lists single `rs/` files.
  Added `rs/docker/**` (D9), the `rs/CLAUDE.md` filter bullet and § 8.
- MAJOR, the D3 `Source:` rule. Folded by a new method: the source name comes from the pool
  directory of the fetch line (the challenger's alternative). The rule is defined for a missing
  line, a line that does not parse, and a source name that equals the binary name. The cache and
  `apt-cache` paths, and M1, are dropped. T2 has a row for each case.
- MAJOR, the purl string match. The SBOM side is now parsed and percent-decoded (`sbom_debs`).
  T1 adds rows for `~`, `+`, `%2B`, `%7E`, `1%3A`. M2 records the real encoding.
- MAJOR, `.list` files. D4 is now conditional on M2 measuring which ownership file syft reads.
- MAJOR, the inert "write each stanza twice" mutation. Replaced with a version-change mutation;
  T1's two-entry fixture proves "exactly one".
- MINOR: AC1 refers to the fetch list, not four fixed names (M4 records the real list); set
  equality and an arch compare in the floor (D5, D7); AC3 no longer says "independent"; no pipe
  under dash, and a jsonwall header check (§ 5.2); D1 says that it replaces SMA-500 § 4.5, and
  `rs/CLAUDE.md:274-278` is rewritten, not only extended; D6 states the release-path risk (R6,
  Q4); R4 for partial packages; the `status.d` layout (D2); the T1 wording (the epoch row is a
  pass row, the "epoch-free" row is removed, three exit-2 rows added); plain-text fixtures and a
  `--target` test stage (T2, Q3); `COPY --chmod=0755`; `sbom_debs` named as the producer of
  `debs`; the arm64 evidence needs a dispatch (T3); actionlint check 13 applies (§ 5.2).
- QUESTIONS: the ADR question is Q5, the fetch list is M4, `manifest.wall` is Q4, the arch and
  set match are folded into D5, the false positives are Q6.

Rejected:

- The `ci/images/` location with a named build context (offered in the filter finding as a second
  option): it breaks the plain `docker build -f rs/Dockerfile rs/` command. `rs/docker/**` in the
  filter fixes the same gap.
- "Copy the `Source:` field exactly, it can be `name (version)`": not applicable after the change
  to the pool-directory method, which gives the name only. The lost source version is R5.
- Deleting `manifest.wall` after the generator runs (offered in the D6 finding): not adopted in
  this revision. The spec keeps the file and states the risk; Sven decides in Q4.

## Re-check on 2026-10-01

Revision 3 re-checked every cited path and line against `origin/main` `68df3372`. Revision 2 used
`83d446fc`. In that range, one commit touches the files of this spec: SMA-675 (`52d51f82`, PR 339).
It changed `images.yml`, `release.yml`, `ci/images/run.sh` and `docs/ops/RUNBOOK-containers.md`.
No SMA-718 commit is on `main`, so no service image version changed in this range. The line
numbers in the "Challenge changelog" above are for `83d446fc` and stay as a record.

Changed in this revision:

- § 5.4: the anchor aliases moved from `release.yml:1796`, `:1877`, `:1950` to `:1802`, `:1883`,
  `:1956`. The anchor stays at `:1198`. The step names the SMA-688 D5 floor, not an "SMA-658
  comment"; the text now names the comment lines 1263-1265.
- § 5.4: the `images.yml` `pull_request` filter is now lines 29-71, not 33-62 (SMA-675 added four
  `ts/` entries). The two `sbom-floor` calls are at lines 172 and 207. Added the check 5 rule (a
  filter glob must match a tracked file) and the place of the T2 step.
- § 5.4: added the two guards that read `rs/Dockerfile` (A8 in `cargo_moon_parity.py`, and the
  `repo:affected-smoke` required inputs in `ci/actionlint/run.sh`). Neither needs a change.
- § 5.1: the `cargo auditable` comment in `rs/Dockerfile` does not say "the OS half is out of
  scope". The bullet now says to add a sentence, not to change one. Added the `assert_pins` grep
  on `chisel cut --release ubuntu-X.Y`.
- § 5.5: the runbook has no SBOM section. The text now names where the new subsection goes
  (after "Which libc is in the image I am running?", line 542) and the attestation text at lines
  694-698. The `rs/CLAUDE.md` filter bullet is lines 235-249, not 235-244. The `cargo auditable`
  bullet is still lines 274-278.
- D10 (new): the test stage must come before the final `FROM scratch` stage. Reason: a build with
  no `--target` builds the last stage, and `assert_pins` reads the last `FROM` as the final stage.
- § 8: `ci/images/run.sh` is not expected to change, because Q3 put T2 in `rs/Dockerfile`.

Confirmed unchanged: `rs/Dockerfile` stage `rootfs` and its slice list; `chisel cut` v1.4.2;
`syft = "1.52.0"` in `.prototools`; `extract_chisel_manifest` and the file name
`chisel-manifest-<service>-<arch>.txt` at the repo root (`build_oci_service`); `assert_base_intact`
with its 200 MB ceiling; the `sbom_summary` and `sbom_floor(key, kinds, summary)` functions in
`ci/images/release_decision.py`; `rs/.dockerignore` does not exclude `docker/`; no `rs/docker/`
directory exists yet; the SMA-500 § 4.5 paragraph on `chisel-manifest-<service>.txt`.
