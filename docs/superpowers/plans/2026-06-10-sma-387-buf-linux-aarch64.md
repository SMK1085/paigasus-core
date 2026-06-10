# SMA-387: buf Linux-aarch64 Asset Resolution Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the vendored buf proto plugin fail loudly on Linux-aarch64 (instead of silently resolving a nonexistent asset), and land the real fix upstream: platform-scoped `{arch}`/`{libc}` overrides in proto's TOML schema engine.

**Architecture:** Two independent deliverables. (1) Upstream PR to `moonrepo/plugins` adding per-platform `arch`/`libc` maps to `PlatformMapper`, resolved platform-map → global `install.*` map → raw value, with an explicit identity-override test. (2) In this repo, an interim `archs = ["x86_64"]` restriction in `.proto/plugins/buf.toml` plus comment inoculation in `lefthook.toml`. The flip-over to the platform-scoped shape happens in a successor Linear issue once upstream ships in a proto release.

**Tech Stack:** Rust (proto_pdk WASM plugin, nextest, moon), proto TOML schema plugins, GitHub CLI, Docker (arm64 verification), Linear MCP.

**Spec:** [`docs/superpowers/specs/2026-06-10-sma-387-buf-linux-aarch64-design.md`](../specs/2026-06-10-sma-387-buf-linux-aarch64-design.md)

**Task ordering matters:** Task 3 produces the upstream PR number; Task 5 produces the successor issue key; Task 6's TOML comment references both. Do not reorder.

---

### Task 1: Upstream — fork, branch, failing tests

The schema engine lives in `moonrepo/plugins` → `tools/internal-schema`. Tests are sandbox-driven: each test loads a fixture TOML, fakes a host OS/arch, calls `download_prebuilt`, and asserts the resolved names/URLs.

**Files (in the fork clone, NOT this repo):**
- Create: `tools/internal-schema/tests/__fixtures__/schemas/arch-overrides.toml`
- Modify: `tools/internal-schema/tests/download_test.rs` (append a new `mod`)

- [ ] **Step 1: Fork and clone**

```bash
gh repo fork moonrepo/plugins --clone ~/dev/oss/moonrepo-plugins
cd ~/dev/oss/moonrepo-plugins
git checkout -b schema-platform-arch-overrides
```

- [ ] **Step 2: Read before editing**

Read `tools/internal-schema/src/schema.rs` (struct `PlatformMapper`, struct `InstallSchema`) and `tools/internal-schema/src/proto.rs` (`fn interpolate_tokens` ~line 206, its callers in `download_prebuilt` and `locate_executables`). Confirm the shapes match what Tasks 1–2 assume; if upstream drifted, adapt mechanically (the resolution-order requirement is the invariant, not line numbers).

- [ ] **Step 3: Add the fixture** `tools/internal-schema/tests/__fixtures__/schemas/arch-overrides.toml`

Models buf exactly: global remap needed by macOS, identity override needed by Linux. Plus a libc override table to exercise the second map.

```toml
name = "arch-override-test"
type = "cli"

[platform.linux]
download-file = "tool-Linux-{arch}"

[platform.linux.arch]
aarch64 = "aarch64"

[platform.macos]
download-file = "tool-Darwin-{arch}"

[install]
download-url = "https://example.com/v{version}/{download_file}"

[install.arch]
aarch64 = "arm64"

[resolve]
git-url = "https://github.com/example/tool"
```

- [ ] **Step 4: Add the libc fixture** `tools/internal-schema/tests/__fixtures__/schemas/libc-overrides.toml`

```toml
name = "libc-override-test"
type = "cli"

[platform.linux]
download-file = "tool-{arch}-unknown-linux-{libc}"

[platform.linux.libc]
gnu = "musl"

[install]
download-url = "https://example.com/v{version}/{download_file}"

[resolve]
git-url = "https://github.com/example/tool"
```

- [ ] **Step 5: Append tests to** `tools/internal-schema/tests/download_test.rs`

Follow the existing style in that file (`create_empty_proto_sandbox` + `create_schema_plugin_with_config` + full `DownloadPrebuiltOutput` assertion). Add:

```rust
mod arch_overrides {
    use super::*;

    // THE test that protects paigasus-core's flip-over: an identity override
    // (platform value == raw arch) must shadow the global remap, not be
    // skipped as a no-op.
    #[tokio::test(flavor = "multi_thread")]
    async fn platform_identity_override_beats_global_remap() {
        let sandbox = create_empty_proto_sandbox();
        let plugin = sandbox
            .create_schema_plugin_with_config(
                "arch-override-test",
                locate_fixture("schemas").join("arch-overrides.toml"),
                |config| {
                    config.host(HostOS::Linux, HostArch::Arm64);
                },
            )
            .await;

        let output = plugin
            .download_prebuilt(DownloadPrebuiltInput {
                context: PluginContext {
                    version: VersionSpec::parse("1.0.0").unwrap(),
                    ..Default::default()
                },
                ..Default::default()
            })
            .await;

        assert_eq!(output.download_name.unwrap(), "tool-Linux-aarch64");
        assert_eq!(
            output.download_url,
            "https://example.com/v1.0.0/tool-Linux-aarch64"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn global_remap_applies_when_platform_has_no_override() {
        let sandbox = create_empty_proto_sandbox();
        let plugin = sandbox
            .create_schema_plugin_with_config(
                "arch-override-test",
                locate_fixture("schemas").join("arch-overrides.toml"),
                |config| {
                    config.host(HostOS::MacOS, HostArch::Arm64);
                },
            )
            .await;

        let output = plugin
            .download_prebuilt(DownloadPrebuiltInput {
                context: PluginContext {
                    version: VersionSpec::parse("1.0.0").unwrap(),
                    ..Default::default()
                },
                ..Default::default()
            })
            .await;

        assert_eq!(output.download_name.unwrap(), "tool-Darwin-arm64");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn unmapped_arch_passes_through_raw() {
        let sandbox = create_empty_proto_sandbox();
        let plugin = sandbox
            .create_schema_plugin_with_config(
                "arch-override-test",
                locate_fixture("schemas").join("arch-overrides.toml"),
                |config| {
                    config.host(HostOS::Linux, HostArch::X64);
                },
            )
            .await;

        let output = plugin
            .download_prebuilt(DownloadPrebuiltInput {
                context: PluginContext {
                    version: VersionSpec::parse("1.0.0").unwrap(),
                    ..Default::default()
                },
                ..Default::default()
            })
            .await;

        assert_eq!(output.download_name.unwrap(), "tool-Linux-x86_64");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn platform_libc_override_beats_detected_libc() {
        let sandbox = create_empty_proto_sandbox();
        let plugin = sandbox
            .create_schema_plugin_with_config(
                "libc-override-test",
                locate_fixture("schemas").join("libc-overrides.toml"),
                |config| {
                    config.host(HostOS::Linux, HostArch::X64);
                },
            )
            .await;

        let output = plugin
            .download_prebuilt(DownloadPrebuiltInput {
                context: PluginContext {
                    version: VersionSpec::parse("1.0.0").unwrap(),
                    ..Default::default()
                },
                ..Default::default()
            })
            .await;

        // Detected host libc is gnu; the platform map remaps it to musl.
        assert_eq!(
            output.download_name.unwrap(),
            "tool-x86_64-unknown-linux-musl"
        );
    }
}
```

If `DownloadPrebuiltInput`/`PluginContext` field names differ from the existing tests in the file, copy the exact invocation shape from `supports_linux_arm64` at the top of the file — the assertion targets (`download_name`, `download_url`) are the contract.

- [ ] **Step 6: Build the WASM and run the new tests — expect FAIL**

```bash
just build
cargo nextest run --workspace arch_overrides
```

Expected: `platform_identity_override_beats_global_remap` FAILS with `tool-Linux-arm64` (global remap wins because the platform map doesn't exist yet) and `platform_libc_override_beats_detected_libc` FAILS with `...-gnu`. The macOS and x64 pass-through tests PASS (they only exercise existing behavior).

- [ ] **Step 7: Commit (fork repo conventions: `new:`/`fix:` subjects)**

```bash
git add tools/internal-schema/tests/
git commit -m "new: Add failing tests for platform-scoped arch overrides in schemas."
```

---

### Task 2: Upstream — implement platform-scoped arch/libc overrides

**Files (fork clone):**
- Modify: `tools/internal-schema/src/schema.rs` (struct `PlatformMapper`)
- Modify: `tools/internal-schema/src/proto.rs` (`interpolate_tokens` + its call sites)

- [ ] **Step 1: Add the maps to `PlatformMapper`** in `schema.rs`

```rust
#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct PlatformMapper {
    pub arch: HashMap<HostArch, String>,
    pub archs: Vec<HostArch>,
    pub archive_prefix: Option<String>,
    pub checksum_file: Option<String>,
    pub download_file: String,
    #[deprecated]
    pub exes_dir: Option<PathBuf>,
    pub exes_dirs: Vec<PathBuf>,
    pub exe_path: Option<PathBuf>,
    pub libc: HashMap<HostLibc, String>,
    #[deprecated]
    pub bin_path: Option<PathBuf>,
}
```

(`HashMap`, `HostArch`, `HostLibc` are already imported in this file — verify, don't re-import.)

- [ ] **Step 2: Thread the platform through `interpolate_tokens`** in `proto.rs`

Change the signature to take the resolved `PlatformMapper` and make the resolution order platform map → global map → raw:

```rust
fn interpolate_tokens(
    value: &str,
    version: &VersionSpec,
    schema: &Schema,
    platform: &PlatformMapper,
    env: &HostEnvironment,
) -> String {
    let arch = env.arch.to_rust_arch();
    let os = env.os.to_string();

    let mut value = value
        .replace("{version}", &version.to_string())
        .replace(
            "{arch}",
            platform
                .arch
                .get(&env.arch)
                .or_else(|| schema.install.arch.get(&env.arch))
                .unwrap_or(&arch),
        )
        .replace("{os}", &os);

    // Avoid detecting musl unless requested
    if value.contains("{libc}") {
        let libc = env.libc.to_string();

        value = value.replace(
            "{libc}",
            platform
                .libc
                .get(&env.libc)
                .or_else(|| schema.install.libc.get(&env.libc))
                .unwrap_or(&libc),
        );
    }
    // ... keep the rest of the function (version-part token handling) unchanged
```

Keep the existing parameter order/types if they differ — only the added `platform` parameter and the two `.or_else` chains are the change.

- [ ] **Step 3: Update every call site**

All callers already hold `platform` from `get_platform(&schema, &env)` (in `register_tool`, `download_prebuilt`, `locate_executables`). Add `platform` to each `interpolate_tokens(...)` call:

```bash
grep -n 'interpolate_tokens(' tools/internal-schema/src/proto.rs
```

Expected call sites (line numbers approximate): 292, 294, 310, 328, 334, 399. Each becomes e.g. `interpolate_tokens(&platform.download_file, version, &schema, platform, &env)`.

- [ ] **Step 4: Build + run the new tests — expect PASS**

```bash
just build
cargo nextest run --workspace arch_overrides
```

Expected: all 3 PASS.

- [ ] **Step 5: Run the full internal-schema suite + lint — expect no regressions**

```bash
cargo nextest run -p internal_schema_tool
moon run internal-schema:lint internal-schema:check 2>/dev/null || cargo clippy -p internal_schema_tool -- -D warnings
```

Expected: PASS. (Project name/task ids may differ — `moon query projects | grep -i schema` to find them; lint via whatever `just lint` targets if the above doesn't resolve.)

- [ ] **Step 6: Commit**

```bash
git add tools/internal-schema/src/
git commit -m "new: Support platform-scoped arch and libc overrides in schemas."
```

---

### Task 3: Upstream — changelog + PR

**Files (fork clone):**
- Modify: `tools/internal-schema/CHANGELOG.md`

- [ ] **Step 1: Add changelog entry**

Read the top of `tools/internal-schema/CHANGELOG.md` and mimic its format (Unreleased section if one exists, else a new version heading following the existing pattern):

```markdown
#### 🚀 Updates

- Added `[platform.*.arch]` and `[platform.*.libc]` tables for platform-scoped
  `{arch}`/`{libc}` token overrides. Resolution order: platform map → global
  `[install.arch]`/`[install.libc]` → raw value. Identity overrides (mapping a
  value to itself) shadow the global map.
```

Do NOT bump the version in `Cargo.toml` unless the repo's CONTRIBUTING/recent merged PRs show contributors doing so — releases look maintainer-driven (`internal_schema_tool-vX` tags).

- [ ] **Step 2: Commit and push**

```bash
git add tools/internal-schema/CHANGELOG.md
git commit -m "internal: Add changelog entry."
git push -u origin schema-platform-arch-overrides
```

- [ ] **Step 3: Open the PR**

```bash
gh pr create --repo moonrepo/plugins \
  --title "new: Support platform-scoped arch and libc overrides in TOML schemas" \
  --body "$(cat <<'EOF'
Addresses moonrepo/proto#896.

Some tools name release assets with different arch strings per OS. The
motivating case (from #896): buf uses `buf-Linux-aarch64` but
`buf-Darwin-arm64` / `buf-Windows-arm64.exe`, so the global `[install.arch]`
remap can't be right on all three platforms — today a TOML schema for buf
silently resolves a nonexistent asset on Linux-arm64.

This adds optional `[platform.<os>.arch]` and `[platform.<os>.libc]` tables to
the schema. Token resolution order: platform-scoped map → global
`[install.*]` map → raw Rust value. Both new fields are `#[serde(default)]`,
so existing schemas are unaffected.

For buf:

```toml
[install.arch]
aarch64 = "arm64"          # macOS, Windows

[platform.linux.arch]
aarch64 = "aarch64"        # identity override: Linux keeps the raw name
```

Identity overrides (platform value == raw value) intentionally shadow the
global map and carry an explicit test — schemas distributed to mixed-version
proto clients rely on "keep the global remap + identity-override one platform"
as the only backwards-compatible migration shape (old engines ignore the
unknown platform table and keep working).

This also covers #896's ripgrep example via the arch map alone, since the
mapped value may embed a full triple: `x86_64 = "x86_64-unknown-linux-musl"`.

Docs PR for `non-wasm-plugin.mdx`: incoming, will cross-link.
EOF
)"
```

Record the PR URL/number — Tasks 4, 5, and 6 reference it as `moonrepo/plugins#NN`.

---

### Task 4: Upstream — docs PR (moonrepo/moon website)

**Files (separate fork clone of moonrepo/moon):**
- Modify: `website/docs/proto/non-wasm-plugin.mdx`

- [ ] **Step 1: Fork, clone (shallow), branch**

```bash
gh repo fork moonrepo/moon --clone ~/dev/oss/moonrepo-moon -- --depth 10
cd ~/dev/oss/moonrepo-moon
git checkout -b docs-schema-platform-arch-overrides
```

- [ ] **Step 2: Edit the page**

Find the `[install.arch]` documentation in `website/docs/proto/non-wasm-plugin.mdx` (`grep -n 'install.arch' website/docs/proto/non-wasm-plugin.mdx`). After the global-map explanation, add (match the page's surrounding heading level and prose style):

```markdown
Architecture and libc mappings can also be scoped to a single platform with
`[platform.<os>.arch]` and `[platform.<os>.libc]`, which take precedence over
the global `[install.arch]` and `[install.libc]` maps. This handles tools that
name assets differently per operating system. For example, buf uses
`buf-Linux-aarch64` but `buf-Darwin-arm64`:

```toml
[install.arch]
aarch64 = "arm64"

[platform.linux.arch]
aarch64 = "aarch64"
```

Resolution order for the `{arch}` and `{libc}` tokens: platform-scoped map →
global map → the raw Rust constant.
```

- [ ] **Step 3: Commit, push, PR**

```bash
git add website/docs/proto/non-wasm-plugin.mdx
git commit -m "docs: Document platform-scoped arch/libc overrides for TOML schemas."
git push -u origin docs-schema-platform-arch-overrides
gh pr create --repo moonrepo/moon \
  --title "docs: Document platform-scoped arch/libc overrides for TOML schemas" \
  --body "Documents moonrepo/plugins#NN (platform-scoped \`[platform.*.arch]\` / \`[platform.*.libc]\` overrides). Companion docs change."
```

(Replace `#NN` with the Task 3 PR number.) Cross-link by commenting the docs PR URL on the plugins PR:

```bash
gh pr comment <plugins-pr-url> --body "Docs PR: <moon-pr-url>"
```

---

### Task 5: Create the successor Linear issue

No repo files. Uses Linear MCP (`save_issue`, create mode — no `id`).

- [ ] **Step 1: Create the issue**

Team `Sven Maschek`, project `Paigasus Polyglot`, milestone `MVP`, priority Low (4), `relatedTo: ["SMA-387"]`. Title:

> Flip buf.toml to platform-scoped arch remap (blocked on upstream proto release)

Description (replace `#NN` with the Task 3 PR number):

```markdown
Successor to SMA-387. **Blocked externally** on moonrepo/plugins#NN shipping in
a proto release (chain: PR merge → schema_tool release → proto release pinning
it → contributors upgrade). Do not start until a proto release notes the new
schema fields.

When unblocked, in one PR:

- [ ] `.proto/plugins/buf.toml`: keep `[install.arch] aarch64 = "arm64"`, add
      `[platform.linux.arch] aarch64 = "aarch64"` (identity override — safe on
      stale schema engines, which ignore unknown platform tables), drop the
      `archs = ["x86_64"]` restriction under `[platform.linux]`.
      **Do NOT move the remap to macos/windows tables and drop the global** —
      that breaks every macOS/Windows-arm64 contributor on a stale proto
      (raw `aarch64` → nonexistent `buf-Darwin-aarch64`).
- [ ] `.prototools`: pin proto itself (`proto = "x.y.z"`, the release carrying
      schema_tool with platform-scoped overrides) so the floor is enforced by
      CI (`moonrepo/setup-toolchain`) and proto's own pin check.
- [ ] `.proto/plugins/lefthook.toml`: apply the same platform-scoped pattern
      (its global remap currently survives only because lefthook publishes
      duplicate `Linux_aarch64`/`Linux_arm64` assets).
- [ ] Verify on Linux-arm64 (`docker run --platform linux/arm64`) that
      `proto install buf` now downloads `buf-Linux-aarch64`.

Design: docs/superpowers/specs/2026-06-10-sma-387-buf-linux-aarch64-design.md §3.
```

Record the new issue key — Task 6's TOML comment references it as `SMA-NNN`.

---

### Task 6: Interim repo changes (buf.toml + lefthook.toml)

**Files (this repo, branch `feature/sma-387-fix-linux-aarch64-buf-asset-resolution-in-vendored-buftoml`):**
- Modify: `.proto/plugins/buf.toml`
- Modify: `.proto/plugins/lefthook.toml`

- [ ] **Step 1: Rewrite `buf.toml` header comment and add the archs restriction**

Replace the `TODO(SMA-387)` paragraph (lines 8–10) and add one line under `[platform.linux]`. Full target state of the changed regions (replace `SMA-NNN` and `#NN` with Task 5's issue key and Task 3's PR number):

```toml
# Vendored proto TOML plugin for the buf CLI.
#
# Source: https://github.com/stk0vrfl0w/proto-toml-plugins (plugins/buf.toml, MIT).
# Vendored in SMA-360 rather than referenced by URL: the upstream repo is
# effectively unmaintained, and this schema only resolves official, checksummed
# `bufbuild/buf` GitHub release binaries — nothing to maintain, so we own it.
#
# Linux is restricted to x86_64 (SMA-387): buf's Linux-arm64 asset is named
# `buf-Linux-aarch64`, but proto's TOML schema supports only the global
# [install.arch] remap, which must stay "arm64" for macOS/Windows. The archs
# restriction makes Linux-arm64 fail loudly instead of silently downloading a
# nonexistent asset. Flip-over is SMA-NNN, once moonrepo/plugins#NN ships in a
# proto release: keep [install.arch], add `[platform.linux.arch] aarch64 =
# "aarch64"` (identity override — safe on stale schema engines, which ignore
# unknown platform tables), drop the archs line, and pin proto in .prototools.

name = "buf"
type = "cli"

[platform.linux]
archs = ["x86_64"]
download-file = "buf-Linux-{arch}"
checksum-file = "sha256.txt"
```

Everything from `[platform.macos]` down is unchanged.

- [ ] **Step 2: Inoculate `lefthook.toml`**

Append to its header comment block (after line 8, before `name = "lefthook"`):

```toml
#
# The global aarch64→arm64 remap is safe for Linux only because lefthook
# publishes duplicate Linux_aarch64 and Linux_arm64 assets (identical sha256).
# If upstream drops that alias, apply the platform-scoped pattern from
# .proto/plugins/buf.toml (SMA-387).
```

- [ ] **Step 3: Verify the macOS happy path still works (forces a fresh download)**

```bash
export PATH="$HOME/.proto/bin:$HOME/.proto/shims:$PATH"
proto uninstall buf 1.70.0 --yes && proto install buf
buf --version
```

Expected: download of `buf-Darwin-arm64` succeeds, `buf --version` prints `1.70.0`. (If `proto uninstall` flags differ, `rm -rf ~/.proto/tools/buf` achieves the same.)

- [ ] **Step 4: Verify the loud-fail on Linux-aarch64 (the interim's only new behavior)**

```bash
docker run --rm --platform linux/arm64 -v "$PWD":/repo -w /repo ubuntu:24.04 bash -c '
  apt-get update -qq && apt-get install -y -qq curl git ca-certificates unzip xz-utils gzip >/dev/null &&
  curl -fsSL https://moonrepo.dev/install/proto.sh | bash -s -- --yes &&
  export PATH="$HOME/.proto/bin:$HOME/.proto/shims:$PATH" &&
  proto install buf'
```

Expected: non-zero exit with an unsupported-architecture error that names buf and arm64/aarch64 (emitted by the schema engine's `check_supported_os_and_arch` *before* any download). **Copy the exact error text** — it goes in the repo PR description as AC evidence. If instead the command attempts a download of `buf-Linux-arm64`, the archs line is wrong (typo / wrong table) — fix before proceeding.

- [ ] **Step 5: Commit**

```bash
git add .proto/plugins/buf.toml .proto/plugins/lefthook.toml
git commit -m "fix(repo): fail loudly for buf on Linux-aarch64 (SMA-387)"
```

---

### Task 7: Repo PR

- [ ] **Step 1: Push and open the PR**

```bash
git push -u origin feature/sma-387-fix-linux-aarch64-buf-asset-resolution-in-vendored-buftoml
gh pr create \
  --title "fix(repo): fail loudly for buf on Linux-aarch64 (SMA-387)" \
  --body "$(cat <<'EOF'
Interim fix for SMA-387 (design + spec committed on this branch).

proto's TOML schema supports only a global `[install.arch]` remap, which must
stay `aarch64 = "arm64"` for macOS/Windows but is wrong for Linux
(`buf-Linux-aarch64` is the real asset). Until the platform-scoped override
lands upstream (moonrepo/plugins#NN), Linux is restricted to x86_64 so
Linux-arm64 fails loudly instead of silently downloading a nonexistent asset.
Flip-over tracked in SMA-NNN (blocked-external).

## AC evidence

Asset names verified against the bufbuild/buf v1.70.0 release:
- macOS-arm64 → `buf-Darwin-arm64` ✓ (remap)
- macOS-x86_64 → `buf-Darwin-x86_64` ✓
- Linux-x86_64 → `buf-Linux-x86_64` ✓
- Linux-aarch64 → real asset is `buf-Linux-aarch64`; remap would resolve
  `buf-Linux-arm64` (does not exist) → now fails loudly instead:

Observed on `docker run --platform linux/arm64` (proto + this repo's
.prototools):

```
<paste exact error text from Task 6 Step 4>
```

macOS-arm64 fresh install re-verified post-change (`proto install buf` →
`buf --version` = 1.70.0).

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

Replace `#NN`/`SMA-NNN` and paste the captured error text. Branch name auto-links the PR to SMA-387 in Linear — do not attach links manually.

- [ ] **Step 2: Watch CI to green**

```bash
gh pr checks --watch
```

Expected: all green (Linux-x86_64 CI exercises the modified plugin on toolchain setup).

---

### Task 8: Linear bookkeeping on SMA-387

No repo files. Uses Linear MCP.

- [ ] **Step 1: Comment the research findings on SMA-387** (`save_comment`)

```markdown
Research outcome (full detail in
docs/superpowers/specs/2026-06-10-sma-387-buf-linux-aarch64-design.md):

- The hoped-for "platform-scoped arch override" does not exist in proto's TOML
  schema — `{arch}` resolves from the single global `[install.arch]` map
  (moonrepo/plugins, tools/internal-schema). Confirmed against source.
- Upstream already wants it: moonrepo/proto#896 cites buf as the motivating
  example. Submitted moonrepo/plugins#NN adding `[platform.*.arch]`/`[platform.*.libc]`
  overrides (+ docs PR moonrepo/moon#MM).
- Interim (this issue): `archs = ["x86_64"]` restriction so Linux-arm64 fails
  loudly instead of silently fetching a nonexistent asset. Verified in an
  arm64 container; error text in the PR.
- lefthook.toml has the same remap but is safe (duplicate aarch64/arm64
  assets, identical sha256) — comment added there.
- Flip-over to the real fix: SMA-NNN (blocked-external on the upstream chain:
  PR merge → schema_tool release → proto release → contributor upgrade).
```

- [ ] **Step 2: Amend the AC in the issue description** (`save_issue` with `id: SMA-387`)

Edit the two AC checkboxes to:

```markdown
## Acceptance criteria

- [ ] Interim: vendored `buf.toml` fails loudly (unsupported-arch error) on
      Linux-aarch64 instead of resolving a nonexistent asset. (proto's TOML
      schema has no platform-scoped arch override today; the override ships
      via moonrepo/plugins#NN, flip-over tracked in SMA-NNN.)
- [ ] Verified that macOS-arm64, macOS-x86_64, Linux-x86_64 resolve to
      existing `bufbuild/buf` release assets, and that Linux-aarch64 errors
      before download (correct resolution moves to SMA-NNN).
```

Keep the rest of the description intact.

- [ ] **Step 3: After the repo PR merges — close SMA-387** (`save_issue`, `state: Done`), ticking both AC boxes in the description.

---

## Execution notes

- Tasks 1–4 run in fork clones under `~/dev/oss/` — never inside paigasus-core.
- Task 6 Step 4 (docker) needs network inside the container; if the proto
  install script URL changes, `https://moonrepo.dev/proto` documents the
  current one-liner.
- If upstream review on Task 3 stalls or demands a different design, the
  interim (Tasks 5–8) is independent and proceeds regardless — only the
  successor issue's body may need its PR reference updated.
