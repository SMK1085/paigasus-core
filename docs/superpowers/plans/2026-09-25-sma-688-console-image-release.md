# SMA-688 Console Image Release Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Publish the `iam-console` and `gateway-console` images to Docker Hub and GHCR from `release.yml`, behind their own approval, and pin every chart default image tag to a published version.

**Architecture:** One chain registry, `ci/images/chains.toml`, names the four image chains. `release_plan.py`, `release_decision.py`, `helm_render.py` and `release_guard.py` read it, and `ci/release-plan/run.sh` writes one pair of plan outputs for each key it lists. Each console gets the same four-job chain as a service, and the chain aliases the three existing step anchors, which become generic in `SERVICE`. `repo:helm-render` row 8 holds the chart tags equal to the version files.

**Tech Stack:** GitHub Actions (YAML anchors, no merge keys), Python 3.12 stdlib (`tomllib`, `json`), PyYAML 6.0.3 (helm-render, release guard), bash (3.2-compatible), Docker buildx, crane 0.22.1, cosign 3.1.3, syft 1.52.0, helm (proto-pinned), Moon 2.5.3, proto 0.61.1, pnpm, Prettier.

**Spec:** docs/superpowers/specs/2026-09-25-sma-688-console-image-release-design.md

## Global Constraints

- Work only in `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-688-console-image-release`, on branch `feature/sma-688-console-image-release`. A subagent starts pinned to the main checkout: run EnterWorktree with that `path` first, then confirm `git branch --show-current` prints `feature/sma-688-console-image-release`. Stop if it does not.
- Every new source file opens with an SPDX header: `# SPDX-License-Identifier: Apache-2.0` (TOML, Python, YAML, bash), `//` for TypeScript. A new `CHANGELOG.md` copies the service changelogs, which carry no SPDX line.
- Prefix every command with `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`. Export `PROTO_REPORTER=text` in any script that captures `proto` or shim output (`uv`, `node`, `helm`). A workflow job that captures shim output sets `PROTO_REPORTER: text` in `env:`.
- Python in `ci/` runs through the invocation its own `run.sh` uses. Copy it exactly:
  - release plan: `uv run --locked --project ci/release-plan --python '>=3.12' python3 ci/release-plan/release_plan.py …`
  - release guard: `uv run --locked --project py python3 ci/actionlint/release_guard.py …`
  - release decision: `uv run --no-project --python '>=3.12' python3 ci/images/release_decision.py …`
  - helm-render: `bash ci/helm-render/run.sh [--self-test|--negative-control]` (it resolves its own locked venv).
  - lint: `uv run --locked --project py ruff check --config py/pyproject.toml ci/`
- Chain keys: `iam`, `gateway`, `iam-console`, `gateway-console`. Release name `paigasus-<key>`. Git tag `paigasus-<key>-v<version>`. Plan outputs `skip_<key>` and `version_<key>`, with the hyphen.
- Registries: `ghcr.io/smk1085/paigasus-<key>` and `docker.io/smaschek/paigasus-<key>`.
- Console version source: the top-level `version` of `ts/apps/<app>/package.json`. Both consoles go from `0.0.0` to `0.1.0`. Version `0.0.0` never releases.
- Chart tags: `tag: "0.1.0"` on all four `image` blocks in `charts/paigasus/values.yaml`. `Chart.yaml` stays at `appVersion: "0.0.0"`.
- The `--fixture-count` floor for `release_guard.py` goes from 150 to 162 (12 new FIXTURES rows). It is pinned in `ci/actionlint/run.sh:4607` and three times in `ci/affected-graph/ci_targets.py` (`:1035`, `:2642`, `:2882-2884`). Change all four in one commit.
- `SELF_TEST_COUNT` in `ci/actionlint/run.sh` stays 16. Add rows to existing tables only.
- Shell rules: no pipe into an early-exit reader (`grep -q`, `grep -m`, `head`, `awk … exit`, `sed … q`); use process substitution `< <(printf '%s\n' "$x")`, a file argument, or `sed -n 1p`. No `mapfile`, no `declare -A`, no `${var,,}`, no new `<<<` here-string. An empty array under `set -u` needs a guard in bash 3.2.
- Conventional commits with a scope from `rs|py|ts|contracts|ci|docs|deps|release|repo|claude|workspace`, a lowercase subject, and the trailer `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. No `#NNN` line and no line of the form `token: value` in a commit body (write "V16 asserts …", never "V16: …" at the start of a body line).
- Never `git commit --amend`, never `git reset`, never `--no-verify`, never a bare `git stash`. Commit only at the end of a task.
- Run every command in the foreground. Never use `run_in_background`.
- The worktree sandbox can refuse a compound `git` or `gh` command. Then write the command into a script in your scratchpad and run `/bin/bash <script>`.
- A 1Password "failed to fill whole buffer" error on commit means 1Password is locked. Ask the user to unlock it. Do not bypass signing.
- Prose in documents and in code comments: ASD-STE100 Simplified Technical English. Short sentences, active voice, no idiom.
- `release.yml` cannot run locally. Its first test of the changed anchors is the next live release (spec R3). Each task states what only CI or the live run proves.

## Review Focus

The five input classes that the spec implies and that tests miss most often, most likely first. Each one has a pinning test in the task named.

1. **Key-prefix collision.** `iam` is a string prefix of `iam-console`, and `gateway` of `gateway-console`. Any prefix test, glob or unanchored regex selects the wrong chain: an artifact `pattern: image-iam-*`, a `grep '^skip_iam'` without `=`, a tag `startswith`, a floating-tag regex, `approval_for_job`. Pins: Task 1 `SERVICE_FIXTURES` rows "iam at 0.1.0 does not read the iam-console tag" and "iam-console does not read the iam tag"; Task 2 negative-control row 5 counts each of nine keys with `^<key>=`; Task 3 row "floating: iam ignores iam-console tags"; Task 6 rows "SMA-688 publish-images-iam-console behind approve-images-iam", "SMA-688 V17 pattern: image-iam-*" and helper `_sma688_console_approvals`; Task 9 row "check8a two image blocks for one chain".
2. **A registry or a key list that is partial, empty or unreadable.** An empty key list writes no chain output, and an unwritten output runs the chain with an empty version. Pins: Task 1 rows "a missing chains.toml …" and the seven `_REGISTRY_SHAPE_CASES`; Task 2 rows 6, 9 and 10 to 13. Task 1 rows "sed parity: …" (trailing spaces, a trailing comment, a quoted key): `--assert` fails when the sed read of `chains.toml` finds other keys than tomllib. `--keys` prints the tomllib keys, so the same check also proves that a valid `--keys` answer leaves out no key that the sed fallback names. Row 6 removes `uv`, row 10 gives an empty `--keys` answer and row 11 an invalid key: each must read the keys from `chains.toml` with sed, write all nine outputs fail-safe and exit 0. Row 9 omits console keys from the decision: the fail-safe branch must write all nine outputs. Rows 12 and 13 fail `--keys` with a missing or an empty `chains.toml`: only then does the wrapper exit 2, with `nothing_to_release=false` as its only line.
3. **A console `package.json` of the wrong shape.** No `version`, `"version": 1`, invalid JSON. Each must be inconclusive for that key only. Pins: Task 1 rows "a console package.json with no version / with a non-string version / with invalid JSON …", plus "the cargo version file must be the manifest the workspace resolves".
4. **An SBOM that looks right and is not.** A console SBOM with only the scoped `@next/env` and no `next`, an SBOM with no `libc6`, a cargo image with zero crates, an unknown key (exit 3, not 2). Pins: Task 3 rows "sbom: @next/env is not next", "floor: npm without libc6", "floor: an unknown key".
5. **A chart default that renders and names no image.** An empty tag (falls back to `appVersion` `0.0.0`), a `0.0.0` version, a repository no chain names, two blocks for one chain, `STUB_VALUES` that sets an image key, and row 3b that no longer sees a bump once tags are explicit. Pins: Task 9 rows "check8a an empty tag", "check8a a 0.0.0 version", "check8a an unknown repository", "check8b STUB_VALUES sets an image key", "check8b a rendered image with the wrong tag", and the helm measurement in Task 9 Step 1.

---

## File Structure

| File | Change | Responsibility |
| -- | -- | -- |
| `ci/images/chains.toml` | Create | The chain registry: key, kind, version file, changelog, GHCR and Docker Hub names. |
| `ci/release-plan/release_plan.py` | Modify | Read the registry; cargo and npm version readers; `--keys`; outputs for every key; `--assert` changelog for every key; fixture rows. |
| `ci/release-plan/run.sh` | Modify | Get the keys from `--keys`, loop over them in the presence check, the fail-safe write and the extraction; when `--keys` gives no usable list, read the keys from `chains.toml` with sed and take the fail-safe branch (exit 0); exit 2 only when that read also finds no key; negative-control rows 5, 6, 9 to 13. |
| `ci/affected-graph/ci_targets.py` | Modify | Re-pin `RELEASE_PLAN_SH_CALL_SITES`; re-pin the check-10 floor (three sites); `SELF_TASK_EXPECTED_GLOBS["helm-render"]`. |
| `ci/images/release_decision.py` | Modify | `npm` and `next` in `sbom-summary`; `sbom-floor --service <key>` (exit 3); `title` in `labels`; self-test rows. |
| `ci/images/run.sh` | Modify | Key-to-kind and key-to-zone maps; console `build-oci`; `version_for` by key; `smoke <console-key>`; `smoke_consoles` takes `<zone>=<image>` and calls `assert_fresh`. |
| `.github/workflows/images.yml` | Modify | The console release sequence on every PR; the SBOM floor for the services too. |
| `ci/actionlint/release_guard.py` | Modify | Console keys in `CHAIN_APPROVALS`; derived `OIDC_PUBLISH_JOBS`; V15, V16, V17; console template, fixtures and helpers. |
| `ci/actionlint/run.sh` | Modify | Check-10 floor 150 → 162 (`:4607`). |
| `ci/actionlint/README.md` | Modify | The check-10 floor number in its row (`:46`). |
| `.github/workflows/release.yml` | Modify | Generic anchors (exact-name downloads, key digest file, `sbom-floor`, title check); four plan outputs; eight console jobs. |
| `ts/apps/iam-console/package.json`, `ts/apps/gateway-console/package.json` | Modify | `version` `0.0.0` → `0.1.0`. |
| `ts/apps/iam-console/CHANGELOG.md`, `ts/apps/gateway-console/CHANGELOG.md` | Create | The `0.1.0` section that `--assert` demands. |
| `charts/paigasus/values.yaml` | Modify | `tag: "0.1.0"` on four images, and the new comment. |
| `ci/helm-render/helm_render.py` | Modify | Row 8a, row 8b, row 3b with cleared tags, `EXPECTED_ROW_LABELS` (23), self-test rows. |
| `moon.yml` | Modify | Five literal inputs on `repo:helm-render`. |
| `charts/paigasus/tests/golden/iam-only.yaml`, `iam-and-gateway.yaml` | Modify | `:0.0.0` → `:0.1.0` on the image lines only. |
| `.github/CLAUDE.md`, `charts/CLAUDE.md`, `ci/release-plan/README.md`, `ci/helm-render/README.md`, `charts/paigasus/README.md`, `docs/ops/RUNBOOK-chart.md`, `docs/ops/RUNBOOK-containers.md` | Modify | Spec § 10, and the rollout text of spec § 8. |

**Not changed, and why:**

- `ci/helm-render/run.sh`: row 8 adds no fixture directory and no call site, so `HELM_RENDER_SH_CALL_SITES` and `FIXTURE_TABLE` stay as they are.
- `ci.yml` `T=(…)`, the root `CLAUDE.md` marker command, `SELF_SCHEDULED_GATES`, `REQUIRED_REPO_TASKS`: no new `repo:*` task.
- Moon inputs of release-plan and images: neither is a Moon task. `ci/release-plan` runs as `repo:actionlint` check 11 (`inputs: ['**/*']`), and `ci/images` runs from `images.yml` only (`rs/CLAUDE.md` "Container images"). Only `repo:helm-render` reads `chains.toml` through Moon.
- `ts/pnpm-lock.yaml`: a private app's own `version` is not in the lockfile. Task 8 proves it.

## Task order, and why it differs from the spec's list

1. Registry and `release_plan.py`. 2. `ci/release-plan/run.sh`. 3. `release_decision.py`. 4. `ci/images/run.sh`. 5. `images.yml`. 6. `release_guard.py`. 7. `release.yml`. 8. The console bump. 9. The chart and `repo:helm-render`. 10. Documentation. 11. Final verification.

- **Task 8 comes before Task 9.** Row 8a fails on a version of `0.0.0`, and the consoles are at `0.0.0` until the bump. With the bump first, row 8a is green in the same task that adds it.
- **Task 6 comes before Task 7.** After Task 6 the guard's own `--self-test` is green, but the guard run over the real `release.yml` is RED until Task 7 adds the console jobs and changes the anchors. Task 6 lists the exact expected violations. The real-file run is green at the end of Task 7. `repo:actionlint` check 10 is red on the commits between the two tasks. Push nothing between them.

---

### Task 1: The chain registry and the release plan checker

**Files:**
- Create: `ci/images/chains.toml`
- Modify: `ci/release-plan/release_plan.py` (module docstring `:1-20`; imports `:23-36`; `EXPECTED_SERVICES` block `:281-285`; `service_skips` and `service_state` `:311-348`; `SERVICE_FIXTURES` `:801-819`; `_mixed_service_tree` `:840-859`; `_malformed_service_version_tree` `:862-887`; `_service_version_format_is_rejected` `:890-903`; `_service_unparsable_version_asserts_three_tree` `:906-952`; `_service_state_is_a_separate_failure_domain` `:987-1003`; `_changelog_undecodable_asserts_three` `:1013-1043`; `_malformed_config_asserts_three` `:554-585`; `_broken_crate_manifest_tree` `:707-721`; `COLLECTION_ROWS` `:1052-1082`; `_assert_repo` `:1175-1210`; `main` `:1216-1254`)
- Test: the in-file `COLLECTION_ROWS` and `SERVICE_FIXTURES` tables.

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `ci/images/chains.toml` with `[chain.<key>]` tables in the order `iam`, `gateway`, `iam-console`, `gateway-console`; fields `kind`, `version_file`, `changelog`, `ghcr`, `hub`.
  - `release_plan.py --keys <repo_root>`: prints one key per line, in file order, exit 0. On an unreadable registry it prints no key, writes `release-plan: <Error>: <reason>` on stderr, and exits 3.
  - Runtime output: `skip_<key>=true|false` and `version_<key>=<version or empty>` for every registry key, in registry order, then `nothing_to_release=true|false` last.
  - Functions `release_name(key)`, `chain_registry(repo_root)`, `cargo_version(repo_root, key, entry)`, `npm_version(repo_root, key, entry)`, `service_state(repo_root, tags)` (the argument is now the REPO root, not `rs/`), `service_skips(version, key, tags)`.

- [ ] **Step 1: Write the failing tests**

In `ci/release-plan/release_plan.py`, add `import json` between `import io` and `import re` (`:25-26`).

Replace `SERVICE_FIXTURES` and `_service_fixture_rows` (`:801-819`) with this. The rows now carry the chain key.

```python
# SMA-658. A chain is SKIPPED when it has no real version yet, or when its tag already exists.
# Anything else RUNS, which is the fail-safe direction: spec § 4.3 step 3 makes a run for an
# already released version a no-op.
#
# SMA-688: each row names its chain key. `iam` is a string prefix of `iam-console`, so the last
# three rows prove that the tag test compares the WHOLE tag name. Mutation: replace the set
# membership in service_skips with `any(t.startswith(release_name(key)) for t in tags)`, and the
# row "iam at 0.1.0 does not read the iam-console tag" reds.
SERVICE_FIXTURES: list[tuple[str, str, str, set[str], bool]] = [
    ("a version with no tag -> run", "iam", "0.1.0", set(), False),
    ("the tag already exists -> skip", "iam", "0.1.0", {"paigasus-iam-v0.1.0"}, True),
    ("still 0.0.0 -> skip", "iam", "0.0.0", set(), True),
    ("a newer version than the tag -> run", "iam", "0.2.0", {"paigasus-iam-v0.1.0"}, False),
    ("a tag that only PREFIXES the wanted one -> run", "iam", "0.1.0", {"paigasus-iam-v0.1.0-rc1"}, False),
    ("iam at 0.1.0 does not read the iam-console tag -> run", "iam", "0.1.0",
     {"paigasus-iam-console-v0.1.0"}, False),
    ("iam-console does not read the iam tag -> run", "iam-console", "0.1.0",
     {"paigasus-iam-v0.1.0"}, False),
    ("iam-console with its own tag -> skip", "iam-console", "0.1.0",
     {"paigasus-iam-console-v0.1.0"}, True),
]


def _service_fixture_rows() -> str | None:
    for label, key, version, tags, want in SERVICE_FIXTURES:
        got = service_skips(version, key, tags)
        if got != want:
            return f"{label!r}: expected {want}, got {got}"
    return None
```

Directly above `_mixed_service_tree` (`:840`), add the fixture registry and its two helpers:

```python
# SMA-688. The registry that every synthetic tree below carries: the same four keys and the same
# paths as the real ci/images/chains.toml. A tree that reaches _assert_repo or service_state must
# hold it and both console package.json files at 0.0.0 (spec § 4.2). Without them each console
# adds a "could not be read" problem, and a fixture that expects exactly one problem passes for
# the wrong reason.
_FIXTURE_CHAINS_TOML = """\
[chain.iam]
kind = "cargo"
version_file = "rs/crates/services/paigasus-iam/Cargo.toml"
changelog = "rs/crates/services/paigasus-iam/CHANGELOG.md"
ghcr = "ghcr.io/smk1085/paigasus-iam"
hub = "docker.io/smaschek/paigasus-iam"

[chain.gateway]
kind = "cargo"
version_file = "rs/crates/services/paigasus-gateway/Cargo.toml"
changelog = "rs/crates/services/paigasus-gateway/CHANGELOG.md"
ghcr = "ghcr.io/smk1085/paigasus-gateway"
hub = "docker.io/smaschek/paigasus-gateway"

[chain.iam-console]
kind = "npm"
version_file = "ts/apps/iam-console/package.json"
changelog = "ts/apps/iam-console/CHANGELOG.md"
ghcr = "ghcr.io/smk1085/paigasus-iam-console"
hub = "docker.io/smaschek/paigasus-iam-console"

[chain.gateway-console]
kind = "npm"
version_file = "ts/apps/gateway-console/package.json"
changelog = "ts/apps/gateway-console/CHANGELOG.md"
ghcr = "ghcr.io/smk1085/paigasus-gateway-console"
hub = "docker.io/smaschek/paigasus-gateway-console"
"""
_FIXTURE_CHAINS: dict[str, dict[str, str]] = tomllib.loads(_FIXTURE_CHAINS_TOML)["chain"]


def _write_chain_fixture(repo_root: Path, *, console_versions: dict[str, str] | None = None,
                         console_texts: dict[str, str] | None = None) -> None:
    """Write ci/images/chains.toml and both console package.json files under `repo_root`.

    A console is at 0.0.0 unless `console_versions` names it. `console_texts` replaces the whole
    text of a console's package.json, for the malformed-shape rows.
    """
    (repo_root / CHAIN_REGISTRY).parent.mkdir(parents=True, exist_ok=True)
    (repo_root / CHAIN_REGISTRY).write_text(_FIXTURE_CHAINS_TOML)
    for key in ("iam-console", "gateway-console"):
        path = repo_root / _FIXTURE_CHAINS[key]["version_file"]
        path.parent.mkdir(parents=True, exist_ok=True)
        version = (console_versions or {}).get(key, "0.0.0")
        default = json.dumps({"name": f"@paigasus/{key}", "version": version, "private": True},
                             indent=2) + "\n"
        path.write_text((console_texts or {}).get(key, default))


def _git_commit_and_tag(tmp: str, tags: tuple[str, ...]) -> None:
    """One commit, then each tag in `tags`. Moved here unchanged from
    _service_unparsable_version_asserts_three_tree, so that several trees can share it."""
    subprocess.run(["git", "init", "-q", tmp], check=True)
    subprocess.run(["git", "-C", tmp, "config", "user.email", "release-plan-self-test@example.com"],
                    check=True)
    subprocess.run(["git", "-C", tmp, "config", "user.name", "release-plan self-test"], check=True)
    subprocess.run(["git", "-C", tmp, "add", "-A"], check=True)
    # `-c commit.gpgsign=false` / `-c tag.gpgSign=false`: this repo's global git config signs
    # every commit and tag (1Password-backed SSH signing). A throwaway fixture tree must not
    # depend on that being unlocked, and an unsigned, unannotated tag is all `repo_tags` reads.
    subprocess.run(["git", "-C", tmp, "-c", "commit.gpgsign=false", "commit", "-q", "-m", "init"],
                    check=True)
    for tag in tags:
        subprocess.run(["git", "-C", tmp, "-c", "tag.gpgSign=false", "tag", tag], check=True)


def _complete_chain_tree(tmp: str, *, versions: dict[str, str] | None = None,
                         changelogs: dict[str, str] | None = None,
                         console_texts: dict[str, str] | None = None,
                         tags: tuple[str, ...] = ("unrelated-tag",)) -> Path:
    """A whole repository that `_assert_repo` reads as CLEAN while every chain is at 0.0.0.

    The derived releasable set equals EXPECTED_RELEASABLE, the tree carries a tag, and every chain
    of the fixture registry resolves. So a fixture built on it gets exactly the problem it names
    and no other. `versions` maps a chain key to its version (default 0.0.0). `changelogs` maps a
    chain key to the text of its CHANGELOG.md. `console_texts` replaces a console's package.json.
    """
    all_versions = {"iam": "0.0.0", "gateway": "0.0.0", "iam-console": "0.0.0",
                    "gateway-console": "0.0.0", **(versions or {})}
    repo_root = Path(tmp)
    rs_root = repo_root / "rs"
    for name in ("paigasus-kernel", "paigasus-proto", "paigasus-proto-derive"):
        d = rs_root / "crates" / "libs" / name
        d.mkdir(parents=True)
        (d / "Cargo.toml").write_text(f'[package]\nname = "{name}"\nversion = "1.0.0"\n')
    for key in ("iam", "gateway"):
        d = rs_root / "crates" / "services" / release_name(key)
        d.mkdir(parents=True)
        (d / "Cargo.toml").write_text(
            f'[package]\nname = "{release_name(key)}"\nversion = "{all_versions[key]}"\n'
            f'publish = false\n')
    (rs_root / "Cargo.toml").write_text(
        '[workspace]\nmembers = ["crates/libs/*", "crates/services/*"]\n')
    (rs_root / "release-plz.toml").write_text("")
    _write_chain_fixture(
        repo_root,
        console_versions={k: all_versions[k] for k in ("iam-console", "gateway-console")},
        console_texts=console_texts)
    for key, text in (changelogs or {}).items():
        (repo_root / _FIXTURE_CHAINS[key]["changelog"]).write_text(text)
    _git_commit_and_tag(tmp, tags)
    return repo_root
```

Replace `_mixed_service_tree` (`:840-859`) with this. Only the docstring and the last two lines are new:

```python
# S9 (SMA-658). Spec § 6.1's fail-safe direction as a fixture: a service that cannot be read is
# inconclusive for THAT SERVICE alone — never a silent skip, and never cross-talk into the other
# service or into run()'s own kernel verdict. This mirrors _broken_crate_manifest_tree's shape,
# but leaves `paigasus-iam` OUT of `[workspace] members` entirely (rather than malforming its
# manifest), so `crate_manifests(rs_root)["paigasus-iam"]` fails with a plain KeyError that never
# touches the scan for the healthy `paigasus-gateway` crate or for run()'s own kernel packages.
# SMA-688: the tree also carries the chain registry and both consoles at 0.0.0.
def _mixed_service_tree(tmp: str) -> Path:
    rs_root = Path(tmp) / "rs"
    kernel_dir = rs_root / "crates" / "libs" / "paigasus-kernel"
    gateway_dir = rs_root / "crates" / "services" / "paigasus-gateway"
    kernel_dir.mkdir(parents=True)
    gateway_dir.mkdir(parents=True)
    (rs_root / "Cargo.toml").write_text(
        '[workspace]\nmembers = ["crates/libs/*", "crates/services/paigasus-gateway"]\n')
    (rs_root / "release-plz.toml").write_text("")
    (kernel_dir / "Cargo.toml").write_text(
        '[package]\nname = "paigasus-kernel"\nversion = "0.1.0"\n')
    (gateway_dir / "Cargo.toml").write_text(
        '[package]\nname = "paigasus-gateway"\nversion = "0.1.0"\npublish = false\n')
    _write_chain_fixture(Path(tmp))
    return rs_root
```

In `_malformed_service_version_tree` (`:862-887`), add this sentence at the end of the docstring, and add `_write_chain_fixture(Path(tmp))` directly before `return rs_root`:

```python
    SMA-688: the tree carries the chain registry and both consoles at 0.0.0, so the consoles
    read as a clean skip and add nothing to the iam verdict under test.
```

Replace the body of `_service_version_format_is_rejected` (`:890-903`) with:

```python
def _service_version_format_is_rejected() -> str | None:
    """Mutation this row proves: delete the `_SERVICE_VERSION_RE.fullmatch` test in
    service_state, and iam reads (True-or-False, '0.1.0-rc1') instead of (False, '')."""
    tmp = tempfile.mkdtemp()
    try:
        rs_root = _malformed_service_version_tree(tmp)
        state = service_state(rs_root.parent, set())
        if state.get("iam") != (False, ""):
            return (f"the malformed iam version did not read as (False, '') (run with an empty, "
                     f"never-matching plan version): {state.get('iam')!r}")
        if state.get("gateway") != (False, "0.1.0"):
            return (f"the healthy gateway crate was affected by iam's malformed version: "
                     f"{state.get('gateway')!r}")
        return None
    finally:
        shutil.rmtree(tmp)
```

Replace `_service_unparsable_version_asserts_three_tree` (`:906-952`) with the version that delegates to `_complete_chain_tree`:

```python
def _service_unparsable_version_asserts_three_tree(tmp: str) -> Path:
    """A tree built so `_assert_repo` reports EXACTLY ONE problem if the fix under
    `_service_unparsable_version_asserts_three` is present, and ZERO if it is not.

    PR 2 review, finding 1. `_complete_chain_tree` derives exactly EXPECTED_RELEASABLE, carries a
    tag, and holds gateway and both consoles at the legitimate 0.0.0 skip (SMA-688: the chain
    registry and both console package.json files are part of it). That leaves iam's `0.1.0-rc1`
    — a version `_SERVICE_VERSION_RE` cannot parse — as the ONLY thing that can make
    `_assert_repo` report a problem, which is what proves the fix is load-bearing.
    """
    return _complete_chain_tree(tmp, versions={"iam": "0.1.0-rc1"})
```

Replace the body of `_service_state_is_a_separate_failure_domain` (`:987-1003`) with:

```python
def _service_state_is_a_separate_failure_domain() -> str | None:
    """Mutation this row proves: move service_state's per-key `try` outside the loop, and the
    healthy gateway crate reads (False, '') with iam."""
    tmp = tempfile.mkdtemp()
    try:
        rs_root = _mixed_service_tree(tmp)
        state = service_state(rs_root.parent, set())
        if state.get("iam") != (False, ""):
            return f"the missing iam crate did not read as (False, ''): {state.get('iam')!r}"
        if state.get("gateway") != (False, "0.1.0"):
            return (f"the healthy gateway crate was affected by iam's failure: "
                    f"{state.get('gateway')!r}")
        try:
            run(Path(tmp), "push")
        except Exception as exc:  # deliberately broad; run() must never raise (see its docstring)
            return f"run() raised {type(exc).__name__}: {exc} — a service failure must not reach it"
        return None
    finally:
        shutil.rmtree(tmp)
```

In `_malformed_config_asserts_three` (`:554-585`), add `_write_chain_fixture(Path(tmp))` directly after the `crate_dir.mkdir(parents=True)` line, and append to its docstring:

```python
    SMA-688: the tree carries the chain registry. Collection fails first, so `_assert_repo`
    returns 3 before it reads the registry. Mutation this row proves: remove the `try` around
    collection in _assert_repo, and the helper sees a traceback, not 3.
```

In `_broken_crate_manifest_tree` (`:707-721`), add `_write_chain_fixture(Path(tmp))` directly before `return rs_root`, and append to its docstring:

```python
    SMA-688: the tree carries the chain registry, so the only untyped failure is the crate
    manifest's. Mutation proved by its two users: narrow the broad `except Exception` in
    _assert_repo or in run() to `except InconclusiveError`.
```

Replace `_changelog_undecodable_asserts_three` (`:1013-1043`) with:

```python
def _changelog_undecodable_asserts_three() -> str | None:
    """Mutation this row proves: narrow `except (OSError, UnicodeDecodeError)` in _assert_repo to
    `except OSError`, and the non-UTF-8 CHANGELOG.md escapes as a traceback.

    SMA-688: the tree carries the chain registry, and the row now also asserts the stderr names
    the changelog. This tree has other problems too (no tag, a derived releasable set that is
    not EXPECTED_RELEASABLE), so rc 3 alone does not prove that the decode error was reported.
    """
    tmp = tempfile.mkdtemp()
    try:
        rs_root = Path(tmp) / "rs"
        kernel_dir = rs_root / "crates" / "libs" / "paigasus-kernel"
        iam_dir = rs_root / "crates" / "services" / "paigasus-iam"
        kernel_dir.mkdir(parents=True)
        iam_dir.mkdir(parents=True)
        (rs_root / "Cargo.toml").write_text(
            '[workspace]\nmembers = ["crates/libs/paigasus-kernel", '
            '"crates/services/paigasus-iam"]\n')
        (rs_root / "release-plz.toml").write_text(
            '[[package]]\nname = "paigasus-iam"\nrelease = false\n')
        (kernel_dir / "Cargo.toml").write_text(
            '[package]\nname = "paigasus-kernel"\nversion = "1.0.0"\n')
        (iam_dir / "Cargo.toml").write_text(
            '[package]\nname = "paigasus-iam"\nversion = "0.1.0"\npublish = false\n')
        # An invalid UTF-8 byte (0xFF is never valid in any UTF-8 sequence position).
        (iam_dir / "CHANGELOG.md").write_bytes(b"## [0.1.0]\n\xff\n")
        _write_chain_fixture(Path(tmp))
        subprocess.run(["git", "init", "-q", tmp], check=True)
        try:
            with contextlib.redirect_stderr(io.StringIO()) as err:
                rc = _assert_repo(Path(tmp))
        except Exception as exc:  # deliberately broad; catching it IS the RED signal pre-fix
            return (f"_assert_repo raised {type(exc).__name__}: {exc} for a non-UTF-8 "
                    f"CHANGELOG.md instead of returning 3 — this is the rc=1 crash")
        if rc != 3:
            return f"_assert_repo returned {rc} for a non-UTF-8 CHANGELOG.md, expected 3"
        if "CHANGELOG.md cannot be read" not in err.getvalue():
            return (f"_assert_repo returned 3 but did not name the undecodable changelog: "
                    f"{err.getvalue()!r}")
        return None
    finally:
        shutil.rmtree(tmp)
```

Directly after `_changelog_undecodable_asserts_three`, add the new SMA-688 rows:

```python
# SMA-688. The malformed registry shapes. Each one must be InconclusiveError whose message holds
# the marker, and `--keys` must then print NO key and exit 3. An empty or partial key list would
# leave a chain output unwritten, and ci/release-plan/run.sh would run that chain with an empty
# version (spec § 4.2, last bullet).
_REGISTRY_SHAPE_CASES: tuple[tuple[str, str, str], ...] = (
    ("no chain table", "[other]\nx = 1\n", "has no [chain.<key>] table"),
    ("an empty chain table", "chain = {}\n", "has no [chain.<key>] table"),
    ("an entry that is not a table", "[chain]\niam = 3\n", "is not a table"),
    ("an entry with no version_file",
     '[chain.iam]\nkind = "cargo"\nchangelog = "c"\nghcr = "g"\nhub = "h"\n',
     "has no string version_file"),
    ("an unknown kind",
     '[chain.iam]\nkind = "pip"\nversion_file = "v"\nchangelog = "c"\nghcr = "g"\nhub = "h"\n',
     "kind 'pip' is not one of"),
    ("a key outside [a-z][a-z0-9-]*",
     '[chain.Iam_X]\nkind = "cargo"\nversion_file = "v"\nchangelog = "c"\nghcr = "g"\nhub = "h"\n',
     "is not [a-z][a-z0-9-]*"),
    ("invalid TOML", "[chain.iam\n", "cannot read"),
)


def _keys_of(repo_root: Path) -> tuple[int, str]:
    """`main(["--keys", repo_root])`: its return code and its stdout. Stderr is discarded."""
    out = io.StringIO()
    with contextlib.redirect_stdout(out), contextlib.redirect_stderr(io.StringIO()):
        rc = main(["--keys", str(repo_root)])
    return rc, out.getvalue()


def _keys_prints_the_registry() -> str | None:
    """`--keys` prints the registry keys, one on each line, in FILE order, and exits 0.

    ci/release-plan/run.sh reads this output to name the chain outputs it writes. Mutation: print
    `sorted(registry)` (gateway, gateway-console, iam, iam-console), or all keys on one line, and
    this row reds.
    """
    tmp = tempfile.mkdtemp()
    try:
        _write_chain_fixture(Path(tmp))
        rc, out = _keys_of(Path(tmp))
        want = "iam\ngateway\niam-console\ngateway-console\n"
        if rc != 0 or out != want:
            return f"--keys returned rc {rc} and printed {out!r}, want rc 0 and {want!r}"
        return None
    finally:
        shutil.rmtree(tmp)


def _missing_registry_is_inconclusive() -> str | None:
    """A tree with no ci/images/chains.toml. `--keys` exits 3 and prints nothing, and
    service_state names no key at all.

    Mutation: fall back to a hard-coded key list when the registry cannot be read, and this row
    reds on the non-empty stdout.
    """
    tmp = tempfile.mkdtemp()
    try:
        rc, out = _keys_of(Path(tmp))
        if rc != 3 or out:
            return f"--keys on a tree with no registry returned rc {rc} and printed {out!r}, want rc 3 and ''"
        with contextlib.redirect_stderr(io.StringIO()) as err:
            state = service_state(Path(tmp), set())
        if state != {}:
            return f"service_state named keys without a registry: {state!r}"
        if "the chain registry is inconclusive" not in err.getvalue():
            return f"service_state did not say why it named no key: {err.getvalue()!r}"
        return None
    finally:
        shutil.rmtree(tmp)


def _malformed_registry_is_inconclusive() -> str | None:
    """Each _REGISTRY_SHAPE_CASES entry: chain_registry raises with the case's own marker, and
    `--keys` exits 3 with no stdout.

    Mutation: delete any one validation in chain_registry, and its case reds with "did not raise"
    or with the wrong marker. The marker match, not a bare `except InconclusiveError`, is what
    makes a neutered check visible (the _tag_name_override_is_inconclusive lesson).
    """
    for label, text, marker in _REGISTRY_SHAPE_CASES:
        tmp = tempfile.mkdtemp()
        try:
            (Path(tmp) / CHAIN_REGISTRY).parent.mkdir(parents=True)
            (Path(tmp) / CHAIN_REGISTRY).write_text(text)
            try:
                chain_registry(Path(tmp))
            except InconclusiveError as exc:
                if marker not in str(exc):
                    return f"{label}: InconclusiveError for the wrong reason: {exc!r} (want {marker!r})"
            else:
                return f"{label}: chain_registry did not raise"
            rc, out = _keys_of(Path(tmp))
            if rc != 3 or out:
                return f"{label}: --keys returned rc {rc} and printed {out!r}, want rc 3 and ''"
        finally:
            shutil.rmtree(tmp)
    return None


def _only_iam_console_bumped() -> str | None:
    """AC 1: a release that bumps one console runs exactly that console's chain.

    iam and gateway are at 0.1.0 and tagged, iam-console is at 0.1.0 and untagged, and
    gateway-console is still 0.0.0. Mutation: read every console from one package.json, or look
    the iam-console tag up under the iam release name, and this row reds.
    """
    tmp = tempfile.mkdtemp()
    try:
        repo_root = _complete_chain_tree(
            tmp, versions={"iam": "0.1.0", "gateway": "0.1.0", "iam-console": "0.1.0"})
        got = service_state(repo_root, {"paigasus-iam-v0.1.0", "paigasus-gateway-v0.1.0"})
        want = {"iam": (True, "0.1.0"), "gateway": (True, "0.1.0"),
                "iam-console": (False, "0.1.0"), "gateway-console": (True, "0.0.0")}
        if got != want:
            return f"service_state returned {got!r}, want {want!r}"
        return None
    finally:
        shutil.rmtree(tmp)


def _console_package_inconclusive(text: str, what: str) -> str | None:
    """One malformed iam-console package.json: that key reads (False, ''), and no other key
    changes. Mutation: move service_state's per-key `try` outside the loop, and the other keys
    read (False, '') too."""
    tmp = tempfile.mkdtemp()
    try:
        repo_root = _complete_chain_tree(
            tmp, versions={"iam": "0.1.0", "gateway": "0.1.0", "gateway-console": "0.1.0"},
            console_texts={"iam-console": text})
        with contextlib.redirect_stderr(io.StringIO()) as err:
            got = service_state(repo_root, set())
        want = {"iam": (False, "0.1.0"), "gateway": (False, "0.1.0"),
                "iam-console": (False, ""), "gateway-console": (False, "0.1.0")}
        if got != want:
            return f"{what}: service_state returned {got!r}, want {want!r}"
        if "iam-console is inconclusive" not in err.getvalue():
            return f"{what}: no 'iam-console is inconclusive' line on stderr: {err.getvalue()!r}"
        return None
    finally:
        shutil.rmtree(tmp)


def _console_package_no_version() -> str | None:
    return _console_package_inconclusive(
        '{"name": "@paigasus/iam-console", "private": true}\n', "no version")


def _console_package_numeric_version() -> str | None:
    return _console_package_inconclusive(
        '{"name": "@paigasus/iam-console", "version": 1}\n', '"version": 1')


def _console_package_invalid_json() -> str | None:
    return _console_package_inconclusive(
        '{"name": "@paigasus/iam-console", "version": "0.1.0",\n', "invalid JSON")


def _console_assert_case(changelog: str) -> tuple[int, str]:
    """_assert_repo over a clean tree with iam-console at 0.1.0 and this CHANGELOG.md text."""
    tmp = tempfile.mkdtemp()
    try:
        repo_root = _complete_chain_tree(
            tmp, versions={"iam-console": "0.1.0"}, changelogs={"iam-console": changelog})
        with contextlib.redirect_stderr(io.StringIO()) as err:
            rc = _assert_repo(repo_root)
        return rc, err.getvalue()
    finally:
        shutil.rmtree(tmp)


def _console_changelog_missing_asserts_three() -> str | None:
    """A console at 0.1.0 with no `## [0.1.0]` section fails --assert (spec § 4.2).

    Mutation: restrict the changelog loop in _assert_repo to `kind == "cargo"`, and this row reds.
    Its twin below proves the tree has no OTHER problem, so rc 3 here is this cause alone.
    """
    rc, err = _console_assert_case("# Changelog\n\n## [Unreleased]\n")
    if rc != 3:
        return f"_assert_repo returned {rc} for iam-console 0.1.0 with no changelog section, expected 3"
    if "paigasus-iam-console is at 0.1.0 but" not in err or "`## [0.1.0]` heading" not in err:
        return f"_assert_repo returned 3 but did not name the missing console section: {err!r}"
    return None


def _console_changelog_present_asserts_zero() -> str | None:
    """The twin of the row above: the same tree WITH the section is clean."""
    rc, err = _console_assert_case("# Changelog\n\n## [Unreleased]\n\n## [0.1.0] - 2026-09-25\n")
    if rc != 0:
        return f"_assert_repo returned {rc} for a clean console tree, expected 0: {err!r}"
    return None


def _main_prints_every_chain_output() -> str | None:
    """The runtime path prints `skip_<key>` and `version_<key>` for EVERY registry key, in
    registry order, and the verdict line last.

    Mutation: loop over a hard-coded ("iam", "gateway") in main(), and the two console pairs are
    missing. ci/release-plan/run.sh would then take its fail-safe branch on every push.
    """
    tmp = tempfile.mkdtemp()
    try:
        _complete_chain_tree(tmp, versions={"iam": "0.1.0", "iam-console": "0.1.0"},
                             tags=("paigasus-iam-v0.1.0",))
        out = io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(io.StringIO()):
            rc = main(["--event-name", "push", tmp])
        lines = out.getvalue().splitlines()
        got = [ln for ln in lines if ln.startswith(("skip_", "version_"))]
        want = ["skip_iam=true", "version_iam=0.1.0", "skip_gateway=true", "version_gateway=0.0.0",
                "skip_iam-console=false", "version_iam-console=0.1.0",
                "skip_gateway-console=true", "version_gateway-console=0.0.0"]
        if rc != 0 or got != want:
            return f"main printed {got!r} (rc {rc}), want {want!r}"
        if not lines or not lines[-1].startswith("nothing_to_release="):
            return f"the verdict line is not last: {lines[-1:]!r}"
        return None
    finally:
        shutil.rmtree(tmp)


def _cargo_version_file_mismatch_is_inconclusive() -> str | None:
    """The cargo reader finds the crate through the workspace members. The registry's
    version_file must name that same manifest, because ci/helm-render reads version_file directly.

    Mutation: delete the resolve() comparison in cargo_version, and iam reads (False, '0.1.0').
    """
    tmp = tempfile.mkdtemp()
    try:
        repo_root = _complete_chain_tree(tmp, versions={"iam": "0.1.0"})
        registry = repo_root / CHAIN_REGISTRY
        registry.write_text(registry.read_text().replace(
            "rs/crates/services/paigasus-iam/Cargo.toml", "rs/crates/services/paigasus-iam/Other.toml"))
        with contextlib.redirect_stderr(io.StringIO()):
            state = service_state(repo_root, set())
        if state.get("iam") != (False, ""):
            return f"a version_file that names another file did not read as (False, ''): {state.get('iam')!r}"
        if state.get("gateway") != (True, "0.0.0"):
            return f"the gateway chain was affected: {state.get('gateway')!r}"
        return None
    finally:
        shutil.rmtree(tmp)


def _sed_parity_case(header: str) -> str | None:
    """_assert_repo over a clean tree whose `[chain.iam]` header line is replaced by `header`.

    tomllib still reads all four keys from each variant, and the row asserts that first. Without
    that guard, a variant that tomllib also rejects would fail --assert as an unreadable registry,
    and the row would pass for the wrong reason. _console_changelog_present_asserts_zero is the
    twin: the same tree with the bare header is clean, so rc 3 here is the parity check alone.
    """
    tmp = tempfile.mkdtemp()
    try:
        repo_root = _complete_chain_tree(tmp)
        registry = repo_root / CHAIN_REGISTRY
        text = registry.read_text()
        if "[chain.iam]\n" not in text:
            return "the fixture registry has no bare [chain.iam] line to replace"
        registry.write_text(text.replace("[chain.iam]\n", header + "\n", 1))
        keys = list(chain_registry(repo_root))
        if keys != ["iam", "gateway", "iam-console", "gateway-console"]:
            return f"tomllib does not read the four keys from {header!r}: {keys!r}"
        with contextlib.redirect_stderr(io.StringIO()) as err:
            rc = _assert_repo(repo_root)
        if rc != 3:
            return f"_assert_repo returned {rc} for the header {header!r}, expected 3"
        if "the sed read of" not in err.getvalue() or "['gateway', 'gateway-console', 'iam-console']" not in err.getvalue():
            return f"_assert_repo returned 3 but did not report the sed/tomllib mismatch: {err.getvalue()!r}"
        return None
    finally:
        shutil.rmtree(tmp)


def _sed_parity_trailing_spaces() -> str | None:
    """`[chain.iam]  `: tomllib reads iam, and run.sh's sed read does not (spec § 4.3).

    Mutation: delete the sed parity check in _assert_repo (the `if registry:` block), and this row
    reds with rc 0. Also: widen CHAIN_HEADER_SED_RE to allow trailing spaces without the same
    change to run.sh, and this row reds, which is the prompt to change the twin.
    """
    return _sed_parity_case("[chain.iam]  ")


def _sed_parity_trailing_comment() -> str | None:
    """`[chain.iam] # the IAM service`: tomllib reads iam, and the sed read does not.

    Mutation: delete the sed parity check in _assert_repo, and this row reds with rc 0.
    """
    return _sed_parity_case("[chain.iam] # the IAM service")


def _sed_parity_quoted_key() -> str | None:
    """`[chain."iam"]`: tomllib reads the key iam, and the sed read does not.

    Mutation: delete the sed parity check in _assert_repo, and this row reds with rc 0.
    """
    return _sed_parity_case('[chain."iam"]')


def _sed_parity_fixture_registry() -> str | None:
    """The fixture registry, which has the same headers as the real ci/images/chains.toml, gives
    the same keys to both readers. The real file is checked by `--assert .` (Step 4) and by
    repo:actionlint check 11. This row does not read the real file through `__file__`, because
    run.sh rows 7 and 8 run --self-test on a COPY of this file in a temp directory.

    Mutation: change CHAIN_HEADER_SED_RE so that it drops a hyphenated key (for example
    `[a-z][a-z0-9]*`), and this row reds.
    """
    got = sed_chain_keys(_FIXTURE_CHAINS_TOML)
    if got != list(_FIXTURE_CHAINS):
        return f"sed_chain_keys read {got!r} from the fixture registry, want {list(_FIXTURE_CHAINS)!r}"
    return None
```

Append these entries to the end of `COLLECTION_ROWS` (after `:1081`, before the closing `)`):

```python
    ("SMA-688 --keys prints the registry keys in file order", _keys_prints_the_registry),
    ("SMA-688 a missing chains.toml is inconclusive for every key", _missing_registry_is_inconclusive),
    ("SMA-688 a malformed chains.toml is inconclusive, never an empty key list",
     _malformed_registry_is_inconclusive),
    ("SMA-688 a tree that bumps only iam-console runs only that chain", _only_iam_console_bumped),
    ("SMA-688 a console package.json with no version is inconclusive for that key only",
     _console_package_no_version),
    ("SMA-688 a console package.json with a non-string version is inconclusive for that key only",
     _console_package_numeric_version),
    ("SMA-688 a console package.json with invalid JSON is inconclusive for that key only",
     _console_package_invalid_json),
    ("SMA-688 a console at 0.1.0 with no changelog section fails --assert",
     _console_changelog_missing_asserts_three),
    ("SMA-688 a console at 0.1.0 with its changelog section passes --assert",
     _console_changelog_present_asserts_zero),
    ("SMA-688 main prints skip_<key> and version_<key> for every key", _main_prints_every_chain_output),
    ("SMA-688 the cargo version file must be the manifest the workspace resolves",
     _cargo_version_file_mismatch_is_inconclusive),
    ("SMA-688 sed parity: a chain header with trailing spaces fails --assert",
     _sed_parity_trailing_spaces),
    ("SMA-688 sed parity: a chain header with a trailing comment fails --assert",
     _sed_parity_trailing_comment),
    ("SMA-688 sed parity: a quoted chain key fails --assert", _sed_parity_quoted_key),
    ("SMA-688 sed parity: the fixture registry gives both readers the same keys",
     _sed_parity_fixture_registry),
```

- [ ] **Step 2: Run it, expect FAIL**

Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" PROTO_REPORTER=text
uv run --locked --project ci/release-plan --python '>=3.12' python3 ci/release-plan/release_plan.py --self-test; echo "rc=$?"
```
Expected: the import succeeds (the new names are used only inside functions), and `rc=3`. The red lines include `FAIL 'SMA-658 service skip rows': raised KeyError: 'iam-console'` (the old `service_skips` still indexes `EXPECTED_SERVICES`), `FAIL 'SMA-658 service_state is a separate failure domain': raised NameError: name 'CHAIN_REGISTRY' is not defined`, and a `raised NameError` line for each new SMA-688 row.

- [ ] **Step 3: Implement**

Create `ci/images/chains.toml`:

```toml
# SPDX-License-Identifier: Apache-2.0
#
# SMA-688. The chain registry: one entry for each image that .github/workflows/release.yml
# publishes (spec D10). The table name is the chain key. The release name of a chain is
# `paigasus-<key>`. It is the git tag prefix (`paigasus-<key>-v<version>`) and the image title
# label.
#
# Four readers use this file. Each one reads it with tomllib, which is stdlib:
#   ci/release-plan/release_plan.py   the keys, kind, version_file, changelog (the plan outputs)
#   ci/images/release_decision.py     kind (the SBOM floor)
#   ci/helm-render/helm_render.py     kind, version_file, ghcr (row 8a)
#   ci/actionlint/release_guard.py    the keys, ghcr, hub (V16)
# ci/images/run.sh keeps its own bash tables. images.yml builds every key on each pull request
# that changes ci/images/**, so a key that this file adds and run.sh does not know fails there.
#
# kind = "cargo": version_file is the crate's Cargo.toml, and the version is `[package] version`.
# kind = "npm":   version_file is the app's package.json, and the version is the top-level
#                 "version" string.
# A version bump also changes the tag in charts/paigasus/values.yaml. repo:helm-render row 8a
# fails otherwise.

[chain.iam]
kind = "cargo"
version_file = "rs/crates/services/paigasus-iam/Cargo.toml"
changelog = "rs/crates/services/paigasus-iam/CHANGELOG.md"
ghcr = "ghcr.io/smk1085/paigasus-iam"
hub = "docker.io/smaschek/paigasus-iam"

[chain.gateway]
kind = "cargo"
version_file = "rs/crates/services/paigasus-gateway/Cargo.toml"
changelog = "rs/crates/services/paigasus-gateway/CHANGELOG.md"
ghcr = "ghcr.io/smk1085/paigasus-gateway"
hub = "docker.io/smaschek/paigasus-gateway"

[chain.iam-console]
kind = "npm"
version_file = "ts/apps/iam-console/package.json"
changelog = "ts/apps/iam-console/CHANGELOG.md"
ghcr = "ghcr.io/smk1085/paigasus-iam-console"
hub = "docker.io/smaschek/paigasus-iam-console"

[chain.gateway-console]
kind = "npm"
version_file = "ts/apps/gateway-console/package.json"
changelog = "ts/apps/gateway-console/CHANGELOG.md"
ghcr = "ghcr.io/smk1085/paigasus-gateway-console"
hub = "docker.io/smaschek/paigasus-gateway-console"
```

In `release_plan.py`, append this paragraph to the module docstring, directly before its closing `"""` (`:20`):

```python
SMA-688. The image chains come from ci/images/chains.toml, not from a list in this file. A chain
that cannot be read RUNS (the fail-safe direction above). A registry that cannot be read names no
chain at all: `--keys` then exits 3. ci/release-plan/run.sh then reads the keys from the registry
with sed and writes every chain output fail-safe. It fails the plan job only when that read also
finds no key (spec § 4.3).
```

Replace the `EXPECTED_SERVICES` block (`:281-285`, the comment and the dict) with:

```python
# SMA-688. The chain registry (spec D10). It replaces SMA-658's EXPECTED_SERVICES pin: the keys,
# their kinds, their version files and their changelogs live in ONE file, and four readers use
# it. A registry that cannot be read is inconclusive for EVERY key and never yields an empty key
# list: an empty list writes no chain output, and a chain output that nobody writes runs that
# chain with an empty version.
CHAIN_REGISTRY = Path("ci") / "images" / "chains.toml"
CHAIN_KINDS = frozenset({"cargo", "npm"})
_CHAIN_KEY_RE = re.compile(r"[a-z][a-z0-9-]*")
_CHAIN_FIELDS = ("kind", "version_file", "changelog", "ghcr", "hub")

# SMA-688. The ONE copy in this file of the registry header shape that ci/release-plan/run.sh
# reads with sed when `--keys` fails (spec § 4.3):
#     sed -n 's/^\[chain\.\([a-z][a-z0-9-]*\)\]$/\1/p' "$REPO_ROOT/ci/images/chains.toml"
# That sed line in run.sh's github_output() is the TWIN of this pattern. Change one, and change
# the other in the same commit. _assert_repo compares the keys this pattern finds with the
# tomllib keys. So a header that tomllib reads and the sed read misses (trailing spaces, a
# trailing comment, a quoted key, a dotted key under a bare [chain]) fails --assert on the pull
# request, and the sed fallback can never name only part of the chains.
CHAIN_HEADER_SED_RE = re.compile(r"^\[chain\.([a-z][a-z0-9-]*)\]$")


def sed_chain_keys(text: str) -> list[str]:
    """The keys that run.sh's sed fallback reads from this registry text, in file order.

    Split on "\\n" only, like sed: a CRLF line keeps its "\\r", so neither reader matches it.
    """
    return [m.group(1) for line in text.split("\n") if (m := CHAIN_HEADER_SED_RE.match(line))]


def release_name(key: str) -> str:
    """`paigasus-<key>`: the git tag prefix and the image title label of a chain (spec § 4.1)."""
    return f"paigasus-{key}"


def chain_registry(repo_root: Path) -> dict[str, dict[str, str]]:
    """key -> entry, in file order. Every failure is InconclusiveError, and nothing returns {}."""
    cfg = load_toml(repo_root / CHAIN_REGISTRY)
    chains = cfg.get("chain")
    if not isinstance(chains, dict) or not chains:
        raise InconclusiveError(f"{CHAIN_REGISTRY} has no [chain.<key>] table")
    out: dict[str, dict[str, str]] = {}
    for key, entry in chains.items():
        if _CHAIN_KEY_RE.fullmatch(key) is None:
            raise InconclusiveError(f"{CHAIN_REGISTRY}: the chain key {key!r} is not [a-z][a-z0-9-]*")
        if not isinstance(entry, dict):
            raise InconclusiveError(f"{CHAIN_REGISTRY}: [chain.{key}] is not a table")
        for field in _CHAIN_FIELDS:
            value = entry.get(field)
            if not isinstance(value, str) or not value:
                raise InconclusiveError(f"{CHAIN_REGISTRY}: [chain.{key}] has no string {field}")
        if entry["kind"] not in CHAIN_KINDS:
            raise InconclusiveError(
                f"{CHAIN_REGISTRY}: [chain.{key}] kind {entry['kind']!r} is not one of "
                f"{sorted(CHAIN_KINDS)}")
        out[key] = {field: entry[field] for field in _CHAIN_FIELDS}
    return out
```

Keep `_SERVICE_VERSION_RE` (`:287-297`), `_CHANGELOG_HEADING` and `changelog_names_version` as they are. Replace `service_skips` and `service_state` (`:311-348`) with:

```python
def service_skips(version: str, key: str, tags: set[str]) -> bool:
    """Spec § 6.1 (SMA-658). Skip when there is no real version yet, or when the tag exists.

    SMA-688: the tag name comes from release_name(key), the one place that builds it. The test is
    set MEMBERSHIP of the whole tag name, never a prefix test: `iam` is a string prefix of
    `iam-console`, so a prefix test would read one chain's tag as the other's.
    """
    if version == "0.0.0":
        return True
    return tag_for(release_name(key), version) in tags


def cargo_version(repo_root: Path, key: str, entry: dict[str, str]) -> str:
    """The SMA-658 cargo reader: the crate comes from the workspace members, by its release name.

    SMA-688 adds one check. The registry's version_file must be that same manifest, because
    ci/helm-render reads version_file directly. Two readers that read two files would let the
    chart tag and the release plan disagree.
    """
    manifest = crate_manifests(repo_root / "rs")[release_name(key)]
    if manifest.resolve() != (repo_root / entry["version_file"]).resolve():
        raise InconclusiveError(
            f"{CHAIN_REGISTRY} names {entry['version_file']} for {key}, but the workspace "
            f"resolves {release_name(key)} to {manifest}")
    pkg = load_toml(manifest).get("package") or {}
    version = pkg.get("version")
    if not isinstance(version, str):
        raise InconclusiveError(f"{release_name(key)} has no literal [package] version in {manifest}")
    return version


def npm_version(repo_root: Path, key: str, entry: dict[str, str]) -> str:
    """SMA-688. The top-level `version` string of a console's package.json (spec D1)."""
    path = repo_root / entry["version_file"]
    try:
        doc = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise InconclusiveError(f"cannot read {path}: {exc}") from exc
    version = doc.get("version") if isinstance(doc, dict) else None
    if not isinstance(version, str):
        raise InconclusiveError(
            f"{path} has no top-level string version for {key} (got {type(version).__name__})")
    return version


def service_state(repo_root: Path, tags: set[str]) -> dict[str, tuple[bool, str]]:
    """chain key -> (skip, version). A SEPARATE FAILURE DOMAIN from the kernel verdict.

    Each key is read in its own try. A failure for one key writes skip=False for that key — the
    fail-safe direction, because spec § 4.3 step 3 (SMA-658) makes a run for an already released
    version a no-op — and leaves the other keys and the kernel verdict untouched.

    SMA-688: the keys come from ci/images/chains.toml, and `repo_root` is the repository root,
    no longer `rs/`. When the registry itself cannot be read, this returns {} and says so on
    stderr: it cannot name a key, so it writes none. ci/release-plan/run.sh asks `--keys` first.
    In that state it reads the keys from the registry with sed and writes every chain output
    fail-safe, and it fails the plan job only when the sed read finds no key either.
    """
    try:
        registry = chain_registry(repo_root)
    except Exception as exc:  # deliberately broad; see the docstring above.
        print(f"release-plan: the chain registry is inconclusive ({type(exc).__name__}: {exc}) "
              f"— no chain output can be named", file=sys.stderr)
        return {}
    readers = {"cargo": cargo_version, "npm": npm_version}
    out: dict[str, tuple[bool, str]] = {}
    for key, entry in registry.items():
        try:
            version = readers[entry["kind"]](repo_root, key, entry)
            if version != "0.0.0" and _SERVICE_VERSION_RE.fullmatch(version) is None:
                raise InconclusiveError(
                    f"{release_name(key)}'s version {version!r} in {entry['version_file']} "
                    f"is not MAJOR.MINOR.PATCH")
            out[key] = (service_skips(version, key, tags), version)
        except Exception as exc:  # deliberately broad; see the docstring above.
            print(f"release-plan: {key} is inconclusive ({type(exc).__name__}: {exc}) — run",
                  file=sys.stderr)
            out[key] = (False, "")
    return out
```

In `_assert_repo`, replace the block from the comment `# SMA-658 spec § 3.1: V-a is a bump by hand` (`:1175`) to the end of the `for` loop (`:1210`) with:

```python
    # SMA-658 spec § 3.1, widened by SMA-688 to every chain in ci/images/chains.toml: a version is
    # bumped by hand, so nothing else can catch a forgotten changelog entry. `repo:actionlint`
    # check 11 runs --assert on every pull request, which is what makes this a gate rather than a
    # convention.
    try:
        registry = chain_registry(repo_root)
    except InconclusiveError as exc:
        problems.append(f"the chain registry cannot be read ({exc}). Every chain output of the "
                        f"release plan depends on it.")
        registry = {}
    # SMA-688. The sed twin (see CHAIN_HEADER_SED_RE). When `--keys` fails, run.sh names the chain
    # outputs from the sed read alone, so that read must find the SAME keys as tomllib. `--keys`
    # prints the tomllib keys, so this one check also proves that a valid `--keys` answer holds
    # every key the fallback would name.
    if registry:
        try:
            sed_keys = sed_chain_keys((repo_root / CHAIN_REGISTRY).read_text(encoding="utf-8"))
        except (OSError, UnicodeDecodeError) as exc:
            problems.append(f"{CHAIN_REGISTRY} cannot be read as text ({exc}).")
        else:
            if set(sed_keys) != set(registry):
                problems.append(
                    f"the sed read of {CHAIN_REGISTRY} finds {sorted(set(sed_keys))}, but tomllib "
                    f"finds {sorted(registry)}. Write each chain header as a bare "
                    f"`[chain.<key>]` line, with nothing after the `]`: ci/release-plan/run.sh "
                    f"reads the keys with sed when `--keys` fails.")
    for key, (_skip, version) in service_state(repo_root, tags).items():
        name = release_name(key)
        # PR 2 review finding 1: an empty version and "0.0.0" are NOT the same state. `""` means
        # service_state could not read a literal MAJOR.MINOR.PATCH version at all (a missing
        # file, a non-string version, or one that fails _SERVICE_VERSION_RE) — that is a
        # REPOSITORY PROBLEM this gate exists to catch. "0.0.0" means the version was read fine
        # and simply has not shipped yet, which is a legitimate skip.
        if not version:
            problems.append(
                f"{name}'s version could not be read (see the '{key} is inconclusive' line "
                f"above). A hand-bumped chain must carry a literal MAJOR.MINOR.PATCH version in "
                f"{registry[key]['version_file']}.")
            continue
        if version == "0.0.0":
            continue
        changelog = repo_root / registry[key]["changelog"]
        try:
            text = changelog.read_text(encoding="utf-8")
        # SMA-658 fix round 1: UnicodeDecodeError is a ValueError, not an OSError, so a
        # non-UTF-8 CHANGELOG.md escaped this except and crashed main() at rc 1 — the same class
        # SMA-608 fixed for the collection layer. Named explicitly, like load_toml's
        # `except (OSError, tomllib.TOMLDecodeError)` above.
        except (OSError, UnicodeDecodeError) as exc:
            problems.append(f"{name} is at {version} but {changelog} cannot be read "
                            f"({exc}). A hand-bumped chain needs a changelog section.")
            continue
        if not changelog_names_version(text, version):
            problems.append(f"{name} is at {version} but {changelog} has no "
                            f"`## [{version}]` heading. Add the section in the same PR as the "
                            f"bump: nothing else records what that release contains.")
```

In `main` (`:1216-1254`): add `ap.add_argument("--keys", action="store_true")` after the `--collection-count` line; insert the `--keys` arm between `root = Path(args.repo_root)` and `if args.do_assert:`; and change the output loop. The changed part of `main` reads:

```python
    if args.self_test:
        return self_test()
    root = Path(args.repo_root)
    if args.keys:
        # SMA-688. ci/release-plan/run.sh reads this list to name the chain outputs it writes:
        # one key on each line, in file order. A registry that cannot be read prints NO key and
        # exits 3, because an empty or partial list would leave a chain output unwritten. The
        # catch is broad for the reason _assert_repo's is: this mode never exits 1.
        try:
            registry = chain_registry(root)
        except Exception as exc:  # deliberately broad; see the comment above.
            print(f"release-plan: {type(exc).__name__}: {exc}", file=sys.stderr)
            return 3
        for key in registry:
            print(key)
        return 0
    if args.do_assert:
        return _assert_repo(root)

    nothing, reason = run(root, args.event_name)
    print(f"release-plan: {reason}")
    # SMA-658. The chain lines are computed in their own failure domain, so a broken chain read
    # cannot change the kernel verdict above. `repo_tags` is read once more here on purpose:
    # `run()` swallows its own failure, and a second failure here must land on the per-chain
    # fail-safe rather than on the kernel one. SMA-688: one pair of lines for every registry key.
    try:
        tags = repo_tags(root)
    except Exception as exc:  # deliberately broad; the fail-safe direction is "run".
        print(f"release-plan: tags are inconclusive ({type(exc).__name__}: {exc}) — run",
              file=sys.stderr)
        tags = set()
    for key, (skip, version) in service_state(root, tags).items():
        print(f"skip_{key}={'true' if skip else 'false'}")
        print(f"version_{key}={version}")
    print(f"nothing_to_release={'true' if nothing else 'false'}")
    return 0
```

- [ ] **Step 4: Run, expect PASS**

Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" PROTO_REPORTER=text
uv run --locked --project ci/release-plan --python '>=3.12' python3 ci/release-plan/release_plan.py --self-test; echo "rc=$?"
uv run --locked --project ci/release-plan --python '>=3.12' python3 ci/release-plan/release_plan.py --keys .
uv run --locked --project ci/release-plan --python '>=3.12' python3 ci/release-plan/release_plan.py --assert .; echo "rc=$?"
uv run --locked --project ci/release-plan --python '>=3.12' python3 ci/release-plan/release_plan.py --event-name push .
uv run --locked --project py ruff check --config py/pyproject.toml ci/release-plan/
sed -n 's/^\[chain\.\([a-z][a-z0-9-]*\)\]$/\1/p' ci/images/chains.toml
```
Expected: `rc=0` and no `FAIL` line from `--self-test` (stderr may show `release-plan: iam is inconclusive …` lines from the rows that test that case; they are not failures). `--keys .` prints exactly `iam`, `gateway`, `iam-console`, `gateway-console`, in that order. `--assert .` gives `rc=0` (the consoles are at `0.0.0`). That rc also proves that the real registry passes the sed parity check. The last command is the sed read that Task 2 puts in `run.sh`, and it prints the same four keys in the same order as `--keys .`. The runtime run prints eight `skip_`/`version_` lines, with `skip_iam-console=true` and `version_iam-console=0.0.0`, then `nothing_to_release=…`. ruff prints `All checks passed!`.

Also run the existing negative control. It must still pass, because its rows build trees without a registry and read only the verdict line:
```bash
bash ci/release-plan/run.sh --negative-control; echo "rc=$?"
```
Expected: `== release-plan negative control passed ==` and `rc=0`. The wrapper still reads only the SMA-658 keys until Task 2, and its rows read only those keys too. Rows 7 and 8 copy `release_plan.py` to a temp directory; the new rows build their own trees and never read `__file__`, so the mutants still fail for the named reason. Any red here is a defect of this task: stop and diagnose.

- [ ] **Step 5: Commit**

```bash
git add ci/images/chains.toml ci/release-plan/release_plan.py
git commit -m "$(cat <<'EOF'
feat(ci): read the image chains from one registry in the release plan (SMA-688)

ci/images/chains.toml names the four image chains, their kind, their
version file and their changelog. release_plan.py reads it, prints one
skip and one version line for every key, and adds --keys. A console
reads its version from its package.json. A registry that cannot be read
names no key, so no chain output is written for a guessed list.
The assert mode also fails when a sed read of the registry headers finds
other keys than tomllib, because the wrapper falls back to that read.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 2: The release-plan wrapper writes every key's outputs

**Files:**
- Modify: `ci/release-plan/run.sh` (header `:9-14`; `github_output` `:66-133`; negative-control rows 5 `:243-273`, 6 `:275-315`, 9 `:383-425`; new rows 10 to 13 before `rm -rf "$tmp"` `:427`)
- Modify: `ci/affected-graph/ci_targets.py` (`RELEASE_PLAN_SH_CALL_SITES` and its comment `:1170-1234`)
- Test: `bash ci/release-plan/run.sh --negative-control`; `python3 ci/affected-graph/ci_targets.py --self-test`.

**Interfaces:**
- Consumes: `release_plan.py --keys <root>` and the runtime output lines from Task 1.
- Produces: `ci/release-plan/run.sh --github-output` appends `nothing_to_release=`, then `skip_<key>=` and `version_<key>=` for each key, to `$GITHUB_OUTPUT`, and exits 0. When `--keys` fails, prints nothing, or prints a line outside `[a-z][a-z0-9-]*`, it reads the keys from `ci/images/chains.toml` with `sed -n 's/^\[chain\.\([a-z][a-z0-9-]*\)\]$/\1/p'`, tests each key against the same shape, and takes the fail-safe branch: `nothing_to_release=false`, and `skip_<key>=false` and an empty `version_<key>=` for every key. It does not run the decision in that case, and it exits 0. This keeps the SMA-603 C1 contract for a runner without `uv` (spec § 4.3). Only when the sed read also finds no key (a missing, unreadable or empty `chains.toml`) does it append `nothing_to_release=false` only and exit 2. Task 10 records both paths in `ci/release-plan/README.md`.

- [ ] **Step 1: Write the failing tests**

In `negative_control()`, replace row 5's line-count block (from `local gh_out_tmp rc line_count` `:256` to `rm -f "$gh_out_tmp"` `:273`) with:

```bash
  local gh_out_tmp rc line_count key
  gh_out_tmp="$(mktemp)"
  rc=0
  GITHUB_OUTPUT="$gh_out_tmp" bash "$0" --github-output >/dev/null 2>&1 || rc=$?
  if [ "$rc" -ne 0 ]; then
    printf '  FAIL the --github-output wrapper exited %s against the real repo, expected 0\n' \
      "$rc" >&2
    failures=$((failures + 1))
  fi
  line_count="$(grep -cE '^nothing_to_release=(true|false)$' "$gh_out_tmp" || true)"
  if [ "$line_count" != "1" ]; then
    printf '  FAIL GITHUB_OUTPUT held %s matching verdict line(s), expected exactly 1\n' \
      "$line_count" >&2
    printf '  --- %s contents ---\n' "$gh_out_tmp" >&2
    cat "$gh_out_tmp" >&2
    failures=$((failures + 1))
  fi
  # SMA-688. Every chain output exactly once. `^<key>=` with the `=` is load-bearing: `skip_iam`
  # is a string prefix of `skip_iam-console`, so a pattern without it counts two keys as one.
  for key in skip_iam version_iam skip_gateway version_gateway \
    skip_iam-console version_iam-console skip_gateway-console version_gateway-console; do
    line_count="$(grep -c "^${key}=" "$gh_out_tmp" || true)"
    if [ "$line_count" != "1" ]; then
      printf '  FAIL row 5: GITHUB_OUTPUT held %s line(s) for %s, expected exactly 1\n' \
        "$line_count" "$key" >&2
      failures=$((failures + 1))
    fi
  done
  rm -f "$gh_out_tmp"
```

Replace row 6 (`:275-315`, from its comment to `rm -f "$nouv_out"`) with the block below. It adds the `_expect_failsafe_outputs` helper, which rows 6, 10 and 11 share, puts `sed` on the restricted `PATH`, and asserts all nine outputs. The rc assertion stays `-ne 0`:

```bash
  # SMA-688. $1 a row label, $2 an output file. Asserts the fail-safe write of spec § 4.3: exactly
  # nine lines, nothing_to_release=false, and skip_<key>=false and an EMPTY version_<key>= for each
  # of the four registry keys. The exact line count also catches a line for a key that is not in
  # the registry (for example skip_IAM_X) and a second verdict line.
  _expect_failsafe_outputs() {
    local label="$1" file="$2" key n
    n="$(grep -c '' "$file" || true)"
    if [ "$n" != "9" ]; then
      printf '  FAIL %s: GITHUB_OUTPUT held %s line(s), expected the 9 fail-safe lines\n' \
        "$label" "$n" >&2
      printf '  --- %s contents ---\n' "$file" >&2
      cat "$file" >&2
      failures=$((failures + 1))
    fi
    if ! grep -qx 'nothing_to_release=false' "$file"; then
      printf '  FAIL %s: nothing_to_release=false was not written\n' "$label" >&2
      failures=$((failures + 1))
    fi
    for key in iam gateway iam-console gateway-console; do
      if ! grep -qx "skip_${key}=false" "$file" || ! grep -qx "version_${key}=" "$file"; then
        printf '  FAIL %s: the fail-safe pair skip_%s=false and version_%s= was not written\n' \
          "$label" "$key" "$key" >&2
        failures=$((failures + 1))
      fi
    done
  }

  # Row 6 — THE C1 REGRESSION ROW, widened by SMA-688. `--github-output` must still exit 0 and
  # write the fail-safe outputs when `uv` cannot be found at all. The `uv` preflight lived above
  # the mode dispatch until the SMA-603 fix wave, so the runtime arm exited 2 and wrote NOTHING on
  # a runner with no proto toolchain — the `plan` job then failed, and because every consumer
  # carries a status-function-free `if:` (implicit success()), the whole publish path skipped.
  # Reordering this file re-arms that trap in one edit, which is why the row exists.
  #
  # SMA-688: without `uv` the wrapper cannot run `release_plan.py --keys`. It reads the keys from
  # ci/images/chains.toml with sed, and writes all nine outputs fail-safe (spec § 4.3). So the
  # restricted PATH now also holds `sed`, and the row asserts all nine outputs, not the verdict only.
  #
  # A hermetic PATH, not a guessed one. `PATH=/usr/bin:/bin` would be a silent no-op on any host
  # that installs uv system-wide; a directory holding symlinks to exactly the externals this arm
  # needs cannot. The precondition is asserted rather than assumed.
  local nouv_dir nouv_out rc6=0 t tpath
  nouv_dir="$tmp/nouv-path"
  mkdir -p "$nouv_dir"
  for t in bash dirname grep sed tail; do
    tpath="$(command -v "$t" || true)"
    if [ -z "$tpath" ]; then
      printf '  FAIL row 6 cannot build its restricted PATH: %s is not on PATH\n' "$t" >&2
      failures=$((failures + 1))
    else
      ln -s "$tpath" "$nouv_dir/$t"
    fi
  done
  if ( PATH="$nouv_dir"; export PATH; command -v uv >/dev/null 2>&1 ); then
    printf '  FAIL row 6 proves nothing: uv is still reachable under the restricted PATH\n' >&2
    failures=$((failures + 1))
  fi
  nouv_out="$(mktemp)"
  PATH="$nouv_dir" GITHUB_EVENT_NAME=push GITHUB_OUTPUT="$nouv_out" \
    "$nouv_dir/bash" "$0" --github-output >/dev/null 2>&1 || rc6=$?
  if [ "$rc6" -ne 0 ]; then
    printf '  FAIL --github-output exited %s with uv unreachable, expected 0 (the fail-safe arm)\n' \
      "$rc6" >&2
    failures=$((failures + 1))
  fi
  if ! grep -qx 'nothing_to_release=false' "$nouv_out"; then
    printf '  FAIL --github-output with uv unreachable did not write nothing_to_release=false\n' >&2
    printf '  --- %s contents ---\n' "$nouv_out" >&2
    cat "$nouv_out" >&2
    failures=$((failures + 1))
  fi
  _expect_failsafe_outputs "row 6 (uv unreachable)" "$nouv_out"
  rm -f "$nouv_out"
```

Replace row 9's stub, its key loop and its `rm -f "$stub_out"` (`:403-425`) with the block below. Row 9 keeps its `--keys` answer valid, so it tests the fail-safe branch of the decision, not the sed read:

```bash
  cat > "$stub_root/release_plan.py" <<'PYEOF'
import sys
if "--keys" in sys.argv:
    print("iam\ngateway\niam-console\ngateway-console")
    raise SystemExit(0)
print("release-plan: fixture -- a malformed decision missing three chain keys")
print("nothing_to_release=true")
print("skip_iam=true")
print("version_iam=1.0.0")
PYEOF
  stub_out="$(mktemp)"
  GITHUB_OUTPUT="$stub_out" GITHUB_EVENT_NAME=push bash "$stub_root/run.sh" --github-output \
    >/dev/null 2>&1 || rc9=$?
  if [ "$rc9" -ne 0 ]; then
    printf '  FAIL row 9: the wrapper exited %s against a checker output missing one chain\n' \
      "$rc9" >&2
    printf '       key, expected 0 (the fail-safe direction)\n' >&2
    failures=$((failures + 1))
  fi
  # SMA-688: all nine outputs. The stub names four keys and prints one of them, so every other
  # key must come from the fail-safe branch.
  for key in nothing_to_release skip_iam version_iam skip_gateway version_gateway \
    skip_iam-console version_iam-console skip_gateway-console version_gateway-console; do
    if ! grep -q "^${key}=" "$stub_out"; then
      printf '  FAIL row 9: %s was not written when the checker output was missing a chain key\n' \
        "$key" >&2
      failures=$((failures + 1))
    fi
  done
  if ! grep -qx 'skip_iam-console=false' "$stub_out"; then
    printf '  FAIL row 9: the fail-safe branch did not write skip_iam-console=false\n' >&2
    failures=$((failures + 1))
  fi
  rm -f "$stub_out"
```

Directly before `rm -rf "$tmp"` (`:427`), add rows 10 to 13. Rows 10 and 11 copy the real `ci/images/chains.toml` from Task 1 into their stub root, so the sed read finds the four keys:

```bash
  # Rows 10 to 13 — SMA-688, the keys branch (spec § 4.3). Each stub replaces release_plan.py in a
  # COPY of this directory under a stub repository root (the row-9 technique), and gives a --keys
  # answer the wrapper must refuse. The stub's decision run prints a COMPLETE decision for all four
  # keys with every skip_ set to true. So a wrapper that runs the decision after a refused --keys
  # answer and trusts it writes true, and the fail-safe assertions red.
  #   Row 10: --keys prints no key.              chains.toml is present -> the bash read, exit 0.
  #   Row 11: --keys prints the invalid IAM_X.   chains.toml is present -> the bash read, exit 0.
  #   Row 12: --keys fails (exit 3).             chains.toml is missing -> exit 2.
  #   Row 13: --keys fails (exit 3).             chains.toml is empty   -> exit 2.
  # In rows 12 and 13 no source names a key, so the wrapper cannot name the chain outputs. It
  # must write nothing_to_release=false only: exactly one line, so no skip_ line.
  _keys_branch_row() { # $1 row, $2 a Python literal: the --keys stdout, $3 the --keys exit code, $4 copy | missing | empty
    local label="$1" keys_body="$2" keys_exit="$3" registry="$4" stub out rc=0 want=2 n
    stub="$tmp/keys-stub-${label}"
    mkdir -p "$stub/ci/release-plan" "$stub/ci/images"
    cp -R "$HERE/." "$stub/ci/release-plan/"
    case "$registry" in
      copy)    cp "$REPO_ROOT/ci/images/chains.toml" "$stub/ci/images/chains.toml"; want=0 ;;
      empty)   : > "$stub/ci/images/chains.toml" ;;
      missing) : ;;
    esac
    printf 'import sys\nif "--keys" in sys.argv:\n    sys.stdout.write(%s)\n    raise SystemExit(%s)\nprint("nothing_to_release=true")\nfor k in ("iam", "gateway", "iam-console", "gateway-console"):\n    print(f"skip_{k}=true")\n    print(f"version_{k}=1.0.0")\n' \
      "$keys_body" "$keys_exit" > "$stub/ci/release-plan/release_plan.py"
    out="$(mktemp)"
    GITHUB_OUTPUT="$out" GITHUB_EVENT_NAME=push bash "$stub/ci/release-plan/run.sh" --github-output \
      >/dev/null 2>&1 || rc=$?
    if [ "$rc" -ne "$want" ]; then
      printf '  FAIL row %s: the wrapper exited %s after a --keys answer it must not trust, expected %s\n' \
        "$label" "$rc" "$want" >&2
      failures=$((failures + 1))
    fi
    if [ "$want" -eq 0 ]; then
      _expect_failsafe_outputs "row $label" "$out"
    else
      n="$(grep -c '' "$out" || true)"
      if [ "$n" != "1" ] || ! grep -qx 'nothing_to_release=false' "$out"; then
        printf '  FAIL row %s: expected nothing_to_release=false as the only line, got %s line(s)\n' \
          "$label" "$n" >&2
        printf '  --- %s contents ---\n' "$out" >&2
        cat "$out" >&2
        failures=$((failures + 1))
      fi
    fi
    rm -f "$out"
  }
  _keys_branch_row 10 "''" 0 copy
  _keys_branch_row 11 "'iam\\nIAM_X\\n'" 0 copy
  _keys_branch_row 12 "''" 3 missing
  _keys_branch_row 13 "''" 3 empty
```

- [ ] **Step 2: Run it, expect FAIL**

Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" PROTO_REPORTER=text
bash ci/release-plan/run.sh --negative-control; echo "rc=$?"
```
Expected: `rc=1`, with these FAIL lines (measured with a stub checker against the old `github_output()`):
- row 5: `FAIL row 5: GITHUB_OUTPUT held 0 line(s) for skip_iam-console, expected exactly 1`, and the same line for the three other console outputs;
- row 6: `FAIL row 6 (uv unreachable): GITHUB_OUTPUT held 5 line(s), expected the 9 fail-safe lines`, and a `fail-safe pair … was not written` line for each console key (the old wrapper exits 0 here, but writes only the two service pairs);
- row 9: a `… was not written when the checker output was missing a chain key` line for each of the four console outputs, and `the fail-safe branch did not write skip_iam-console=false`;
- rows 10 and 11: `held 5 line(s)`, `nothing_to_release=false was not written`, and a `fail-safe pair` line for all four keys (the old wrapper trusts the stub's decision and writes `true`);
- rows 12 and 13: `FAIL row 12: the wrapper exited 0 after a --keys answer it must not trust, expected 2` and `expected nothing_to_release=false as the only line, got 5 line(s)`, and the same two lines for row 13.

- [ ] **Step 3: Implement**

Replace the header comment lines `:9-10` with:

```bash
# Exit codes: 0 pass | 1 the repo is wrong | 2 infrastructure failed — EXCEPT --github-output,
# which exits 0 on every run that can name the chain keys, and 2 only when neither
# `release_plan.py --keys` nor a bash read of ci/images/chains.toml names one. See the comment on
# that arm.
```

Replace the whole `github_output()` function (`:66-133`, from its comment block to its closing brace) with the block below. It puts two new functions in front of it: `keys_are_valid`, the one shape test for both key sources, and `write_failsafe`, the one fail-safe write that both callers share. The shared function keeps each pinned write line unique in the file:

```bash
# SMA-688. $1 a key list, one key on each line. Returns 0 when the list holds at least one key and
# every line is a key of the shape [a-z][a-z0-9-]*. Both key sources go through this one test:
# the `release_plan.py --keys` answer and the sed read of ci/images/chains.toml. A key is later
# split by `for key in $keys`, so the shape also keeps spaces and glob characters out.
keys_are_valid() {
  [ -n "$1" ] && ! grep -qvE '^[a-z][a-z0-9-]*$' < <(printf '%s\n' "$1")
}

# SMA-688. The fail-safe branch. $1 the rc for the annotation, $2 the key list. It writes
# nothing_to_release=false, and skip_<key>=false and an EMPTY version_<key>= for every key, and
# exits 0.
#
# SMA-658. The image chains read their own outputs, and an unset output makes the chain RUN
# (spec § 4.1). The version is written EMPTY, deliberately, rather than left unwritten: an output
# GitHub never saw and one written as an empty string behave differently in a `release.yml`
# expression, and the chains must not depend on that distinction. An empty version means
# "unknown" here, never a real version: the publish job's label compare fails on it before the
# first registry write.
write_failsafe() {
  local key
  printf '::warning::release-plan could not decide (rc=%s) — building, which is the fail-safe direction\n' "$1"
  printf 'nothing_to_release=false\n' >> "${GITHUB_OUTPUT:-/dev/stdout}"
  for key in $2; do
    printf 'skip_%s=false\nversion_%s=\n' "$key" "$key" >> "${GITHUB_OUTPUT:-/dev/stdout}"
  done
  exit 0
}

# THE RUNTIME ARM, and the one place in this repo where a checker failure must NOT fail its
# caller. A failed `plan` job SKIPS its dependents rather than building them — GitHub applies an
# implicit success() to a job-level `if:` with no status function — so a broken decision that
# exited non-zero would stop the release entirely. Fail-safe here means: write false, warn
# loudly, exit 0, and let the matrix build. The --self-test/--negative-control/--assert modes
# keep the normal contract, and CI runs those.
#
# SMA-688, the chain keys (spec § 4.3). The keys come from `release_plan.py --keys`, which reads
# ci/images/chains.toml with tomllib. When that call fails, prints no key, or prints a line
# outside [a-z][a-z0-9-]* (for example, `uv` is missing on the runner), the arm reads the keys
# from the same file with sed, which needs no toolchain. It then takes the fail-safe branch for
# every key, and does NOT run the decision: a toolchain that cannot name the keys cannot be
# trusted to decide. That keeps the SMA-603 C1 contract. Negative-control rows 6, 10 and 11
# assert it.
#
# ONE EXIT 2. When the sed read also finds no key (chains.toml is missing, unreadable or empty),
# the arm cannot name the chain outputs it must write, and a chain output that nobody writes runs
# that chain with an empty version. So it writes nothing_to_release=false and EXITS 2: the plan
# job fails, and every chain and the kernel release skip until a fix. Rows 12 and 13 assert it.
#
# THIS ARM CALLS NO PREFLIGHT, deliberately. An ABSENT `uv` makes the command substitutions
# below exit 127, which `|| keys_rc=$?` and `|| rc=$?` catch like any other failure. `set -e`
# does not fire on them: each assignment is the left operand of an `||` list.
github_output() {
  local rc=0 out keys keys_rc=0 key failsafe=0
  keys="$(uv run --locked --project "$HERE" --python '>=3.12' python3 "$HERE/release_plan.py" --keys "$REPO_ROOT")" || keys_rc=$?
  if [ "$keys_rc" -ne 0 ] || ! keys_are_valid "$keys"; then
    # The sed pattern admits only a bare `[chain.<key>]` header line, with the key in the same
    # shape that keys_are_valid tests. The test below still runs, so both sources pass one rule.
    # TWIN: CHAIN_HEADER_SED_RE in release_plan.py holds the same pattern, and --assert fails
    # when this read and tomllib find different keys. Change both in the same commit.
    keys="$(sed -n 's/^\[chain\.\([a-z][a-z0-9-]*\)\]$/\1/p' "$REPO_ROOT/ci/images/chains.toml" 2>/dev/null)" || keys=
    if ! keys_are_valid "$keys"; then
      printf '::error::release-plan could not name the chain keys (--keys rc=%s, and ci/images/chains.toml names no key). It writes nothing_to_release=false and fails the plan job: a chain output that nobody writes would run that chain with an empty version.\n' "$keys_rc"
      printf 'nothing_to_release=%s\n' false >> "${GITHUB_OUTPUT:-/dev/stdout}"
      exit 2
    fi
    printf '::warning::release-plan --keys gave no usable key list (rc=%s). The keys come from a sed read of ci/images/chains.toml.\n' "$keys_rc"
    write_failsafe "$keys_rc" "$keys"
  fi
  out="$(uv run --locked --project "$HERE" --python '>=3.12' python3 \
    "$HERE/release_plan.py" --event-name "${GITHUB_EVENT_NAME:-}" "$REPO_ROOT" 2>&1)" || rc=$?
  printf '%s\n' "$out"
  # SMA-658 fix round 2, widened by SMA-688 to every key. Under `set -euo pipefail`, each
  # `grep -E ... | tail -n 1` pipeline below exits 1 (aborting this function before its
  # `exit 0`) when its pattern has ZERO matches. So a checker that prints a verdict but omits
  # one chain key — a partial write — must route into the fail-safe branch, which writes every
  # key explicitly. Do not replace a missing-key check with `|| true` on the pipelines: that
  # would let the key go unwritten silently.
  #
  # `for key in $keys` splits on purpose: keys_are_valid admits only [a-z][a-z0-9-]* lines, so
  # a key holds no space and no glob character.
  if [ "$rc" -ne 0 ] || ! grep -qE '^nothing_to_release=(true|false)$' < <(printf '%s\n' "$out"); then
    failsafe=1
  fi
  for key in $keys; do
    if ! grep -qE "^skip_${key}=(true|false)\$" < <(printf '%s\n' "$out") \
      || ! grep -qE "^version_${key}=" < <(printf '%s\n' "$out"); then
      failsafe=1
    fi
  done
  if [ "$failsafe" -ne 0 ]; then
    write_failsafe "$rc" "$keys"
  fi
  # `tail -n 1` guards against a second, forged verdict line ahead of the genuine one — e.g. a
  # releasable package name containing a newline could make the reason line above emit a
  # literal `nothing_to_release=true`, and both would otherwise match the grep and both would
  # get appended. The genuine verdict from `main()` is always printed LAST, so taking only the
  # final match is safe.
  printf '%s\n' "$out" | grep -E '^nothing_to_release=(true|false)$' | tail -n 1 \
    >> "${GITHUB_OUTPUT:-/dev/stdout}"
  # One grep for each key and output, each taken LAST for the same forged-line reason. The `=`
  # after the key is load-bearing: `skip_iam` is a string prefix of `skip_iam-console`.
  for key in $keys; do
    printf '%s\n' "$out" | grep -E "^skip_${key}=(true|false)\$" | tail -n 1 \
      >> "${GITHUB_OUTPUT:-/dev/stdout}"
    printf '%s\n' "$out" | grep -E "^version_${key}=" | tail -n 1 \
      >> "${GITHUB_OUTPUT:-/dev/stdout}"
  done
  exit 0
}
```

In `ci/affected-graph/ci_targets.py`, replace the comment block and tuple from `# SMA-603 fix wave (C2/2a/2b) — the same class a third time` (`:1170`) to the closing `)` of `RELEASE_PLAN_SH_CALL_SITES` (`:1234`) with:

```python
# SMA-603 fix wave (C2/2a/2b) — the same class a third time, for ci/release-plan/run.sh. Before
# this tuple, NOTHING pinned a single line inside that file: SELF_SCHEDULED_GATES cannot see it
# (release-plan is not a Moon task at all — it runs as ci/actionlint/run.sh check 11), and
# ACTIONLINT_SH_INDENTED_CALL_SITES pins only the CALLS, in the other file. So every line below
# could be deleted with every existing pin still green.
#
# What each group closes:
#   - The `--github-output` and `--negative-control` flag parses, and the two dispatch arms.
#     `output)   github_output ;;` is a WHOLE line on purpose: putting `require_uv` back on it is
#     the SMA-603 C1 defect, where a runner without the proto toolchain failed the plan job.
#   - SMA-688, the key sources (spec § 4.3): the shape test that both sources pass, the `--keys`
#     capture, its guard, the sed read of ci/images/chains.toml, the guard on that read, its
#     exit-2 verdict write, and the call into the fail-safe branch. Delete the sed read and a
#     runner without `uv` exits 2 again, which is the C1 defect. Delete a guard and an empty key
#     list writes no chain output; every chain then runs with an empty version.
#   - The fail-safe guard for the verdict, and the per-key presence check (two physical lines).
#     Delete one and a partial checker output aborts under pipefail or writes nothing.
#   - The fail-safe WRITE of the verdict and of each key's pair, and the second call into the
#     fail-safe branch. Delete one and an undecidable run leaves an output unset.
#   - The per-key extraction line for skip_. It carries the `=` that separates `skip_iam` from
#     `skip_iam-console`.
#   - The ASSERTION lines of rows 6, 7, 8 and 10 to 13, the shared fail-safe line count, and the
#     control's failure report, for the reason WORKFLOW_CREDENTIALS_SH_CALL_SITES measured:
#     deleting every assertion left the structural pins byte-identical and the control exited 0
#     having asserted nothing. Rows 10 and 12 are pinned by their calls: one proves the sed read,
#     one proves the exit 2.
#
# REACHABILITY IS NOT AUTOMATIC. moon.yml lists `ci/release-plan/**/*` among repo:affected-smoke's
# inputs and ci/actionlint/run.sh's T_AFFECTED_SMOKE_REQUIRED_INPUTS floors that entry. Without
# them the PR deleting these lines is exactly the PR that does not schedule this gate.
#
# Matched as stripped WHOLE LINES, like the release-parity and workflow-credentials haystacks and
# for both of their reasons: the `case` arms and the `if` bodies are indented, so a column-0 rule
# would reject the real executing lines, while a substring rule would let a COMMENTED-OUT copy
# satisfy the pin. Every entry was verified to occur EXACTLY ONCE in run.sh before it was written
# here (re-verified for SMA-688, on the stripped line text).
RELEASE_PLAN_SH_CALL_SITES = (
    "--github-output)     MODE=output; shift ;;",
    "--negative-control)  MODE=negctl; shift ;;",
    "output)   github_output ;;",
    "negctl)   require_uv; negative_control ;;",
    "[ -n \"$1\" ] && ! grep -qvE '^[a-z][a-z0-9-]*$' < <(printf '%s\\n' \"$1\")",
    "keys=\"$(uv run --locked --project \"$HERE\" --python '>=3.12' python3 \"$HERE/release_plan.py\" --keys \"$REPO_ROOT\")\" || keys_rc=$?",
    'if [ "$keys_rc" -ne 0 ] || ! keys_are_valid "$keys"; then',
    "keys=\"$(sed -n 's/^\\[chain\\.\\([a-z][a-z0-9-]*\\)\\]$/\\1/p' \"$REPO_ROOT/ci/images/chains.toml\" 2>/dev/null)\" || keys=",
    'if ! keys_are_valid "$keys"; then',
    "printf 'nothing_to_release=%s\\n' false >> \"${GITHUB_OUTPUT:-/dev/stdout}\"",
    'write_failsafe "$keys_rc" "$keys"',
    "if [ \"$rc\" -ne 0 ] || ! grep -qE '^nothing_to_release=(true|false)$' < <(printf '%s\\n' \"$out\"); then",
    "if ! grep -qE \"^skip_${key}=(true|false)\\$\" < <(printf '%s\\n' \"$out\") \\",
    "|| ! grep -qE \"^version_${key}=\" < <(printf '%s\\n' \"$out\"); then",
    'write_failsafe "$rc" "$keys"',
    "printf 'nothing_to_release=false\\n' >> \"${GITHUB_OUTPUT:-/dev/stdout}\"",
    "printf 'skip_%s=false\\nversion_%s=\\n' \"$key\" \"$key\" >> \"${GITHUB_OUTPUT:-/dev/stdout}\"",
    "printf '%s\\n' \"$out\" | grep -E \"^skip_${key}=(true|false)\\$\" | tail -n 1 \\",
    'if [ "$n" != "9" ]; then',
    'if [ "$rc6" -ne 0 ]; then',
    "if ! grep -qx 'nothing_to_release=false' \"$nouv_out\"; then",
    '_expect_failsafe_outputs "row 6 (uv unreachable)" "$nouv_out"',
    'if [ "$mut_rc" != "3" ]; then',
    "printf 'release-plan negative control: %d row(s) failed\\n' \"$failures\" >&2",
    'if [ "$mut8_rc" != "3" ]; then',
    "if ! grep -q \"a non-table \\[workspace\\] is inconclusive\" < <(printf '%s\\n' \"$mut8_out\"); then",
    'if [ "$rc" -ne "$want" ]; then',
    "_keys_branch_row 10 \"''\" 0 copy",
    "_keys_branch_row 12 \"''\" 3 missing",
)
```

- [ ] **Step 4: Run, expect PASS**

Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" PROTO_REPORTER=text
bash ci/release-plan/run.sh --negative-control; echo "rc=$?"
bash ci/release-plan/run.sh --self-test; echo "rc=$?"
bash ci/release-plan/run.sh --assert; echo "rc=$?"
GITHUB_OUTPUT=/dev/stdout GITHUB_EVENT_NAME=push bash ci/release-plan/run.sh --github-output
python3 ci/affected-graph/ci_targets.py --self-test; echo "rc=$?"
python3 - <<'PY'
import sys
sys.path.insert(0, "ci/affected-graph")
import ci_targets
lines = [ln.strip() for ln in open("ci/release-plan/run.sh", encoding="utf-8")]
for site in ci_targets.RELEASE_PLAN_SH_CALL_SITES:
    print(lines.count(site), site)
PY
```
Expected: the negative control prints `== release-plan negative control passed ==` and `rc=0`. `--self-test` and `--assert` give `rc=0`. The `--github-output` run prints nine `…=` output lines after the checker text. The `ci_targets.py --self-test` gives `rc=0`. The last script prints `1` in front of each of the 29 pinned sites; any `0` or `2` is a defect in the tuple or in run.sh. (The code blocks above were assembled into a copy of run.sh and checked this way before this plan was written: 29 of 29 sites occur once, `bash -n` passes under `/bin/bash` 3.2 and Homebrew bash 5, and rows 5, 6 and 9 to 13 pass under `/bin/bash` 3.2 with a stub checker.)

Then run the whole affected-graph suite, which reads the real run.sh through `ci_targets.py`. It needs system bash 3.2:
```bash
/bin/bash ci/affected-graph/run.sh; echo "rc=$?"
```
Expected: `rc=0`, and the `ci-targets` row prints `PASS`.

- [ ] **Step 5: Commit**

```bash
git add ci/release-plan/run.sh ci/affected-graph/ci_targets.py
git commit -m "$(cat <<'EOF'
feat(ci): write the plan outputs for every registry key (SMA-688)

The wrapper reads the chain keys from release_plan.py --keys and loops
over them for the presence check, the fail-safe write and the
extraction. When --keys gives no usable list, for example because uv
is missing, it reads the keys from ci/images/chains.toml with sed and
writes every output fail-safe, and it still exits 0. It exits 2 only
when that read also finds no key, because a chain output that nobody
writes runs that chain with an empty version. Negative control rows 5,
6, 9, 10 and 11 cover all nine outputs, and rows 12 and 13 the exit 2.
RELEASE_PLAN_SH_CALL_SITES is re-pinned in the same commit.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 3: The release decision learns the console SBOM floor and the title label

**Files:**
- Modify: `ci/images/release_decision.py` (docstring `:3-28`; imports `:32-40`; exceptions `:59-64`; `labels_from` `:193-211`; `sbom_summary` `:222-241`; `_outcome` `:311-317`; self-test rows `:320-429`; `main` `:447-507`)
- Test: `release_decision.py --self-test`.

**Interfaces:**
- Consumes: `ci/images/chains.toml` (Task 1), field `kind`.
- Produces:
  - `sbom-summary SBOM` prints `packages=`, `libc6=`, `cargo=`, `npm=`, `next=` in that order.
  - `sbom-floor --service KEY SBOM` prints the five summary lines, then `kind=<kind>` and `floor=pass`, exit 0. On a failed floor or an unknown key it prints `floor=fail` on stdout and `release_decision: <reason>` on stderr, exit 3. An unreadable SBOM or registry is exit 2.
  - `labels ARCHIVE` prints `version=`, `revision=`, `title=`. A missing title label is exit 2.

- [ ] **Step 1: Write the failing tests**

In `self_test()`, replace the four existing `sbom:` rows whose expected value is a dict (`"sbom: counts libc6 and cargo packages"`, `"sbom: an empty document"`, `"sbom: missing packages key gives empty list (no error)"`) with these values, and replace the first `labels:` row. Then append the new rows before the closing `]` (`:418`):

```python
        (
            "labels: a version, revision and title label",
            lambda: labels_from(
                _fixture_archive(
                    config_extra={
                        "config": {
                            "Labels": {
                                "org.opencontainers.image.version": "1.2.3",
                                "org.opencontainers.image.revision": "abc123",
                                "org.opencontainers.image.title": "paigasus-iam-console",
                            }
                        }
                    }
                )[0]
            ),
            {"version": "1.2.3", "revision": "abc123", "title": "paigasus-iam-console"},
        ),
```

```python
        (
            "sbom: counts libc6 and cargo packages",
            lambda: sbom_summary({"packages": [{"name": "libc6"}, {"name": "serde", "externalRefs": [{"referenceLocator": "pkg:cargo/serde@1.0.228"}]}]}),
            {"packages": "2", "libc6": "true", "cargo": "1", "npm": "0", "next": "false"},
        ),
        ("sbom: an empty document", lambda: sbom_summary({}), {"packages": "0", "libc6": "false", "cargo": "0", "npm": "0", "next": "false"}),
```

```python
        ("sbom: missing packages key gives empty list (no error)", lambda: sbom_summary({}), {"packages": "0", "libc6": "false", "cargo": "0", "npm": "0", "next": "false"}),
        # SMA-688: the console SBOM (spec M1: 67 npm, 10 deb, next 16.3.5).
        (
            "sbom: counts npm packages and sees next",
            lambda: sbom_summary({"packages": [
                {"name": "libc6"},
                {"name": "next", "externalRefs": [{"referenceLocator": "pkg:npm/next@16.3.5"}]},
                {"name": "react", "externalRefs": [{"referenceLocator": "pkg:npm/react@19.3.0"}]},
            ]}),
            {"packages": "3", "libc6": "true", "cargo": "0", "npm": "2", "next": "true"},
        ),
        # `@next/env` is a DIFFERENT package. Its purl is pkg:npm/%40next/env@…, so it must not
        # read as `next`. Mutation: test `"next" in locator`, and this row reds.
        (
            "sbom: @next/env is not next",
            lambda: sbom_summary({"packages": [{"name": "@next/env", "externalRefs": [{"referenceLocator": "pkg:npm/%40next/env@16.3.5"}]}]})["next"],
            "false",
        ),
        # labels: the title is required (SMA-688, the decide step's identity check).
        (
            "labels: Labels present but missing the title key (missing-label case)",
            lambda: labels_from(_fixture_archive(config_extra={"config": {"Labels": {
                "org.opencontainers.image.version": "1.2.3",
                "org.opencontainers.image.revision": "abc123"}}})[0]),
            "UsageError",
        ),
        # floating: `iam` is a string prefix of `iam-console`. The pattern is a fullmatch, so the
        # console's tags never hold back the service's floating tags. Mutation: use re.match
        # with no `$`, and this row reds.
        ("floating: iam ignores iam-console tags", lambda: floating("iam", "0.1.0", ["paigasus-iam-console-v0.9.0"])["move"], "true"),
        # sbom-floor (SMA-688 D5): a pass and a fail for each kind, an unknown key, an unknown kind.
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
        # chain_kinds reads the registry text; a wrong shape is exit 2, not 3.
        ("kinds: a registry", lambda: chain_kinds('[chain.iam]\nkind = "cargo"\n[chain.iam-console]\nkind = "npm"\n'), {"iam": "cargo", "iam-console": "npm"}),
        ("kinds: no chain table", lambda: chain_kinds("[other]\nx = 1\n"), "UsageError"),
        ("kinds: an entry with no kind", lambda: chain_kinds("[chain.iam]\nversion_file = 'x'\n"), "UsageError"),
        ("kinds: not TOML", lambda: chain_kinds("[chain.iam\n"), "UsageError"),
```

Directly above `def self_test()` (`:320`), add the two fixture helpers:

```python
# SMA-688. The kinds that the floor rows use: one key of each kind, the same as the registry.
FLOOR_KINDS = {"iam": "cargo", "iam-console": "npm"}


def _summary(**values: str) -> dict[str, str]:
    """An sbom_summary() result with every count at zero, then `values` on top."""
    return {"packages": "0", "libc6": "false", "cargo": "0", "npm": "0", "next": "false", **values}
```

- [ ] **Step 2: Run it, expect FAIL**

Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" PROTO_REPORTER=text
uv run --no-project --python '>=3.12' python3 ci/images/release_decision.py --self-test; echo "rc=$?"
```
Expected: a traceback ending in `NameError: name 'sbom_floor' is not defined` (the first floor row) and `rc=1`. `_outcome` catches only `UsageError` and `ConflictError`, so a missing name escapes.

- [ ] **Step 3: Implement**

Add `import tomllib` after `import tarfile` (`:38`).

Replace the `sbom-summary` entry of the module docstring (`:21-22`) and add two entries after it:

```python
  sbom-summary SPDX_JSON
      packages=, libc6=true|false, cargo=, npm=, next=true|false (a measurement, spec M8).
  sbom-floor --service KEY SPDX_JSON
      SMA-688 D5: the sbom-summary lines, then kind= and floor=pass. One floor for each image
      kind, read from ci/images/chains.toml. A failed floor or an unknown key prints floor=fail
      and exits 3.
```

In the `labels ARCHIVE` entry (`:10-13`), change `version=, revision= (the org.opencontainers.image.* labels)` to `version=, revision=, title= (the org.opencontainers.image.* labels)`.

After `class ConflictError` (`:63-64`), add:

```python
class FloorError(Exception):
    """SMA-688: an SBOM below its image kind's floor, or a key that names no chain: exit 3."""


# SMA-688 D5. The chain registry sits beside this file. Only `kind` is read here.
CHAINS_TOML = Path(__file__).resolve().with_name("chains.toml")
```

Replace `labels_from` (`:193-211`) with the same function plus the title:

```python
def labels_from(tar: tarfile.TarFile) -> dict[str, str]:
    """SMA-658 C1: `crane config "oci-archive:…"` cannot read a local archive. MEASURED against
    crane 0.22.1: `Error: fetching config: parsing reference … could not parse reference`, and
    with a missing file it tries to resolve a host called `oci-archive`. `oci-archive:` is a
    syft/skopeo transport; `crane config` takes a REGISTRY reference only. This reads the labels
    straight out of the archive `release_decision.py` already has on disk, so `release.yml` never
    shells out to `crane config` for a config it already extracted.

    SMA-688: it also returns the title. The publish job compares it with `paigasus-<key>`, so an
    archive of another chain stops before the first registry write, even when both chains share
    one version."""
    config, _manifest_digest, _config_digest = _image_config_from(tar)
    config_obj_value = config.get("config")
    config_obj = {} if config_obj_value is None else _require_dict(config_obj_value, "the image config's 'config' object")
    labels_value = config_obj.get("Labels")
    label_map = {} if labels_value is None else _require_dict(labels_value, "the image config's Labels")
    version = str(label_map.get("org.opencontainers.image.version", ""))
    revision = str(label_map.get("org.opencontainers.image.revision", ""))
    title = str(label_map.get("org.opencontainers.image.title", ""))
    if not version:
        raise UsageError("the image config carries no org.opencontainers.image.version label")
    if not revision:
        raise UsageError("the image config carries no org.opencontainers.image.revision label")
    if not title:
        raise UsageError("the image config carries no org.opencontainers.image.title label")
    return {"version": version, "revision": revision, "title": title}
```

Replace `sbom_summary` (`:222-241`) with:

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


def chain_kinds(text: str) -> dict[str, str]:
    """SMA-688: key -> kind, from the text of ci/images/chains.toml. A wrong shape is exit 2."""
    try:
        data = tomllib.loads(text)
    except tomllib.TOMLDecodeError as exc:
        raise UsageError(f"the chain registry is not TOML: {exc}") from exc
    chains = _require_dict(data.get("chain"), "the chain registry's [chain] table")
    kinds: dict[str, str] = {}
    for key, entry in chains.items():
        kind = _require_dict(entry, f"[chain.{key}]").get("kind")
        if not isinstance(kind, str):
            raise UsageError(f"[chain.{key}] has no string kind")
        kinds[key] = kind
    return kinds


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
    elif kind == "npm":
        if int(summary["npm"]) < 1:
            reasons.append("the SBOM lists no npm packages")
        if summary["libc6"] != "true":
            reasons.append("the SBOM does not list libc6; the base image's package data is gone")
        if summary["next"] != "true":
            reasons.append("the SBOM does not list next; the console's own framework is not seen")
    else:
        raise FloorError(f"{key!r} has kind {kind!r}, and no SBOM floor exists for that kind")
    if reasons:
        raise FloorError(f"the {kind} floor failed for {key}: " + "; ".join(reasons))
    return kind
```

Replace `_outcome` (`:311-317`) with:

```python
def _outcome(fn: Callable[[], object]) -> object:
    try:
        return fn()
    except UsageError:
        return "UsageError"
    except ConflictError:
        return "ConflictError"
    except FloorError:
        return "FloorError"
```

Before `def main` (`:447`), add a shared SBOM reader:

```python
def _read_sbom(path: Path) -> dict[str, Any]:
    try:
        doc = json.loads(_read_text(path))
    except json.JSONDecodeError as exc:
        raise UsageError(f"{path} is not JSON: {exc}") from exc
    if not isinstance(doc, dict):
        raise UsageError(f"{path} is not an SPDX JSON object")
    return doc
```

In `main`, add the parser after `p_sbom` (`:464-465`):

```python
    p_floor = sub.add_parser("sbom-floor")
    p_floor.add_argument("--service", required=True)
    p_floor.add_argument("sbom", type=Path)
```

Replace the `elif args.command == "sbom-summary":` arm (`:489-496`) with the two arms, and add the `FloorError` handler after the `ConflictError` handler (`:503-506`):

```python
        elif args.command == "sbom-summary":
            _emit(sbom_summary(_read_sbom(args.sbom)))
        elif args.command == "sbom-floor":
            summary = sbom_summary(_read_sbom(args.sbom))
            _emit(summary)
            kind = sbom_floor(args.service, chain_kinds(_read_text(CHAINS_TOML)), summary)
            _emit({"kind": kind, "floor": "pass"})
```

```python
    except FloorError as exc:
        print("floor=fail")
        print(f"release_decision: {exc}", file=sys.stderr)
        return 3
```

- [ ] **Step 4: Run, expect PASS**

Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" PROTO_REPORTER=text
uv run --no-project --python '>=3.12' python3 ci/images/release_decision.py --self-test; echo "rc=$?"
d="$(mktemp -d)"
printf '%s\n' '{"packages":[{"name":"libc6"},{"name":"next","externalRefs":[{"referenceLocator":"pkg:npm/next@16.3.5"}]}]}' > "$d/ok.json"
printf '%s\n' '{"packages":[{"name":"next","externalRefs":[{"referenceLocator":"pkg:npm/next@16.3.5"}]}]}' > "$d/nolibc.json"
uv run --no-project --python '>=3.12' python3 ci/images/release_decision.py sbom-floor --service iam-console "$d/ok.json"; echo "rc=$?"
uv run --no-project --python '>=3.12' python3 ci/images/release_decision.py sbom-floor --service iam-console "$d/nolibc.json"; echo "rc=$?"
uv run --no-project --python '>=3.12' python3 ci/images/release_decision.py sbom-floor --service billing "$d/ok.json"; echo "rc=$?"
uv run --no-project --python '>=3.12' python3 ci/images/release_decision.py sbom-floor --service iam "$d/ok.json"; echo "rc=$?"
rm -rf "$d"
uv run --locked --project py ruff check --config py/pyproject.toml ci/images/
```
Expected: `release_decision self-test: <N> rows OK` and `rc=0`. The four CLI runs: `kind=npm`, `floor=pass`, `rc=0`; then `floor=fail`, `…libc6…` on stderr, `rc=3`; then `floor=fail`, `'billing' names no chain`, `rc=3`; then `floor=fail`, `the cargo floor failed for iam`, `rc=3`. ruff prints `All checks passed!`.

- [ ] **Step 5: Commit**

```bash
git add ci/images/release_decision.py
git commit -m "$(cat <<'EOF'
feat(ci): an SBOM floor for each image kind and the title label (SMA-688)

sbom-summary also counts npm packages and records whether next is
present. sbom-floor reads the kind of a chain key from chains.toml and
exits 3 with a named reason when the SBOM is below that kind's floor or
the key names no chain. labels also prints the image title, so the
publish job can check which chain an archive belongs to.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 4: `ci/images/run.sh` builds and smokes the console release archive

**Files:**
- Modify: `ci/images/run.sh` (usage header `:11-18`; after `crate_for` `:57-63`; `version_for` `:298-308`; `build_oci` `:453-479`; `smoke_consoles` `:949-1333`; `USAGE` `:1519`; dispatcher arms `smoke`, `build-oci`, `all-consoles` `:1532-1575`)
- Test: bash syntax and shellcheck; a local console `build-oci` → `load-oci` → `smoke` run; `images.yml` on the PR (Task 5).

**Interfaces:**
- Consumes: nothing from earlier tasks at run time. The key list matches `ci/images/chains.toml` (Task 1).
- Produces:
  - `ci/images/run.sh build-oci <iam|gateway|iam-console|gateway-console> <outdir>` writes `<outdir>/paigasus-<key>-<arch>.oci.tar`. The console archive carries `org.opencontainers.image.title=paigasus-<key>` and `org.opencontainers.image.version=<package.json version>`.
  - `ci/images/run.sh smoke <iam-console|gateway-console>...` smokes `paigasus-<key>:dev`.
  - `smoke_consoles <zone>=<image>...`.
  - `kind_for_key <key>` prints `cargo` or `npm`; `zone_for_key <console-key>` prints `iam` or `gateway`.

- [ ] **Step 1: Check the state of SMA-670, then write the failing check**

Spec § 11: SMA-670 also changes the console part of this file. Run:
```bash
gh pr list --state all --search "SMA-670 in:title" --json number,state,title,headRefName
git fetch origin main
git log --oneline origin/main -5 -- ci/images/run.sh
```
If an SMA-670 PR is merged, rebase this branch on `origin/main` first (a normal `git rebase origin/main`, then re-run every earlier task's Step 4), and read the new `smoke_consoles` before you edit it. If SMA-670 added a runtime dependency to the console smoke (for example a session store container), the release build job must start it too: stop and report that to the controller. If the PR is open and not merged, continue; the branch that merges second resolves the conflict (spec R2).

The failing check: the console key is unknown today.
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" PROTO_REPORTER=text
/bin/bash ci/images/run.sh build-oci iam-console "$(mktemp -d)" 2>&1 | sed -n 1,8p
```
Expected: the pins line `pins OK: rustc …` (the service pins run for every key today), then `unknown service: iam-console` and a non-zero exit.

- [ ] **Step 2: Run it, expect FAIL**

The command in Step 1 is the red. Record its output.

- [ ] **Step 3: Implement**

Replace the usage lines of the header (`:11-18`) with:

```bash
# usage: ci/images/run.sh build [iam|gateway]     # [iam|gateway] scopes the build
#        ci/images/run.sh smoke [iam|gateway]...   # no argument: both images; else exactly those
#        ci/images/run.sh smoke <iam-console|gateway-console>...   # SMA-688: paigasus-<key>:dev
#        ci/images/run.sh all                       # build both + smoke; takes no service arg
#        ci/images/run.sh build-oci <key> <outdir>  # SMA-658/SMA-688: OCI archive, no --load
#        ci/images/run.sh load-oci <archive> <image-name>     # load + identity check (prints M3)
#        ci/images/run.sh rehearse <archive.oci.tar>...      # SMA-658: publish steps vs two local registries
#        ci/images/run.sh build-console [iam|gateway]   # SMA-513: console image; [iam|gateway] scopes the build
#        ci/images/run.sh all-consoles                    # SMA-513: build both consoles + smoke; takes no service arg
# <key> is a chain key of ci/images/chains.toml: iam, gateway, iam-console or gateway-console.
```

After `crate_for` (`:63`), add:

```bash
# SMA-688: every release key, mapped ONCE. A key is a chain key of ci/images/chains.toml. This
# file keeps its own bash table on purpose (spec § 4.1): images.yml runs build-oci for every key
# on each pull request that changes ci/images/**, so a key that the registry names and this
# table does not know fails that pull request.
kind_for_key() {
  case "$1" in
    iam|gateway) echo cargo ;;
    iam-console|gateway-console) echo npm ;;
    *) echo "unknown release key: $1" >&2; return 1 ;;
  esac
}

# The console zone of a console key: `iam-console` -> `iam`. The console helpers below
# (app_for, base_path_for, console_probe_path_for) take the zone, never the key.
zone_for_key() {
  case "$1" in
    iam-console) echo iam ;;
    gateway-console) echo gateway ;;
    *) echo "not a console key: $1" >&2; return 1 ;;
  esac
}
```

Replace `version_for` (`:298-308`) with:

```bash
# The version that the image's version label must equal, read from the chain's version file
# (ci/images/chains.toml). SMA-688: a console key reads the top-level "version" of its
# package.json. The sed reads the two-space-indented top-level key only; release_plan.py reads
# the same value with a JSON parser, and the publish job's label compare fails when the two
# disagree.
version_for() {
  local key="$1" kind file v
  kind="$(kind_for_key "$key")"
  if [ "$kind" = cargo ]; then
    file="rs/crates/services/$(crate_for "$key")/Cargo.toml"
    v="$(sed -n 's/^version = "\([0-9][0-9]*\.[0-9][0-9]*\.[0-9][0-9]*\)"$/\1/p' "$ROOT/$file" | sed -n 1p)"
  else
    file="ts/apps/$(app_for "$(zone_for_key "$key")")/package.json"
    v="$(sed -n 's/^  "version": "\([0-9][0-9]*\.[0-9][0-9]*\.[0-9][0-9]*\)",\{0,1\}$/\1/p' "$ROOT/$file" | sed -n 1p)"
  fi
  if [ -z "$v" ]; then
    echo "::error::no literal MAJOR.MINOR.PATCH version line in ${file}" >&2
    return 1
  fi
  echo "$v"
}
```

Rename `build_oci()` (`:453`) to `build_oci_service()`, and in its body change `version="$(version_for "$crate")"` (`:456`) to `version="$(version_for "$service")"`. Leave every other line and the comment block above it as they are. Directly after the function's closing brace (`:479`), add:

```bash
# SMA-688: the console release build. The same OCI transport as build_oci_service
# (--provenance=false --sbom=false, one image, no --load) and the same label set, with
# title=paigasus-<key>. It has no --no-cache-filter=rootfs and no chisel manifest: both exist for
# the chisel-cut service base only (spec § 5.1). The build inputs are build_console_one's.
build_oci_console() {
  local key="$1" outdir="$2" zone app base_path version arch archive
  zone="$(zone_for_key "$key")"
  app="$(app_for "$zone")"
  base_path="$(base_path_for "$zone")"
  version="$(version_for "$key")"
  arch="$(docker version --format '{{.Server.Arch}}')"
  mkdir -p "$outdir"
  archive="${outdir}/paigasus-${key}-${arch}.oci.tar"
  echo "== build-oci paigasus-${key} ${version} (${arch}) =="
  docker buildx build \
    --progress=plain \
    --provenance=false --sbom=false \
    --output "type=oci,dest=${archive},name=paigasus-${key}:dev" \
    -f "$ROOT/ts/Dockerfile" \
    --build-context "bindings=$ROOT/rs/crates/bindings" \
    --build-arg "APP=${app}" \
    --build-arg "BASE_PATH=${base_path}" \
    --label "org.opencontainers.image.title=paigasus-${key}" \
    --label "org.opencontainers.image.description=Paigasus ${zone} console" \
    --label "org.opencontainers.image.source=https://github.com/SMK1085/paigasus-core" \
    --label "org.opencontainers.image.revision=${REVISION}" \
    --label "org.opencontainers.image.version=${version}" \
    --label "org.opencontainers.image.licenses=Apache-2.0" \
    "$ROOT/ts"
  echo "  built ${archive}"
}

# SMA-688: the key decides the build. The dispatcher has already run the pins for this kind.
build_oci() {
  local key="$1" outdir="$2" kind
  kind="$(kind_for_key "$key")"
  if [ "$kind" = npm ]; then
    build_oci_console "$key" "$outdir"
  else
    build_oci_service "$key" "$outdir"
  fi
}
```

In `smoke_consoles`:

1. Change the locals line `local service app base_path console_path other name port origin status html chunk bytes code uid console_status` (`:950`) to `local spec image service app base_path console_path other name port origin status html chunk bytes code uid console_status`.
2. Replace the comment and loop head from `# The zone list comes from the caller` (`:962`) to the `for service in "$@"; do` line (`:969`) with:

```bash
  # The zone list comes from the caller, as `<zone>=<image>` words (SMA-688). `all-consoles`
  # passes `<zone>=<app>:dev`, the image build_console_one makes; `smoke <console-key>` passes
  # `<zone>=paigasus-<key>:dev`, the image load-oci makes from the release archive. An empty list
  # must not read as a pass.
  if [ "$#" -eq 0 ]; then
    echo "::error::smoke_consoles: called with no zones — nothing was smoked, and an empty run must not report OK." >&2
    return 1
  fi

  for spec in "$@"; do
    service="${spec%%=*}"
    image="${spec#*=}"
    if [ "$service" = "$spec" ] || [ -z "$image" ]; then
      echo "::error::smoke_consoles: '${spec}' is not a <zone>=<image> word." >&2
      ec=1
      continue
    fi
```

3. Directly after the `unknown console zone` block (after `continue` and `fi`, `:979-980`), add:

```bash
    # SMA-688: the image under test must be the one THIS checkout built — the rule assert_fresh
    # already applies to the services (spec § 5.1). Both image names carry this checkout's
    # revision label: build_console_one sets it, and load-oci keeps the archive's own labels.
    if ! assert_fresh "$image"; then
      ec=1
      continue
    fi
```

4. In the `docker run -d` call (`:1009`), change `"${app}:dev" 2>&1)" || run_rc=$?` to `"$image" 2>&1)" || run_rc=$?`. Change the next error line (`:1011`) to:

```bash
      echo "::error::${app}: the container did not start from ${image} — docker exited ${run_rc}; its own message follows. If the image is missing, build it first: 'ci/images/run.sh build-console ${service}', or 'build-oci' and 'load-oci' for the release archive." >&2
```

5. In the shell probe (`:1226-1234`), change `"${app}:dev"` to `"$image"` in the `docker run` line, change `echo "::error::${app}:dev has a shell;` to `echo "::error::${image} has a shell;`, and change `docker exited ${sh_rc} on ${app}:dev before` to `docker exited ${sh_rc} on ${image} before`.
6. In the staged-tree walk (`:1255`, `:1262`, `:1266`), change `"${app}:dev" \` to `"$image" \`, `docker exited ${img_rc} on ${app}:dev before` to `docker exited ${img_rc} on ${image} before`, and `inspect ${app}:dev by hand` to `inspect ${image} by hand`.

Replace `USAGE` (`:1519`) with:

```bash
USAGE="usage: ci/images/run.sh build [iam|gateway] | ci/images/run.sh build-oci <iam|gateway|iam-console|gateway-console> <outdir> | ci/images/run.sh load-oci <archive> <image-name> | ci/images/run.sh smoke [iam|gateway]... | ci/images/run.sh smoke <iam-console|gateway-console>... | ci/images/run.sh all | ci/images/run.sh rehearse <archive.oci.tar>... | ci/images/run.sh build-console [iam|gateway] | ci/images/run.sh all-consoles"
```

Replace the `smoke)` arm (`:1532-1535`) with:

```bash
  smoke)
    shift
    if [ "$#" -eq 0 ]; then smoke gateway iam; exit 0; fi
    # SMA-688. A console key smokes the RELEASE archive's image, paigasus-<key>:dev: the name that
    # images.yml and release.yml give it with load-oci. Service keys and console keys run two
    # different suites with two different EXIT traps, so one call takes one kind only.
    smoke_kind="$(kind_for_key "$1")"
    for k in "$@"; do
      k_kind="$(kind_for_key "$k")"
      if [ "$k_kind" != "$smoke_kind" ]; then
        echo "usage: ci/images/run.sh smoke takes service keys or console keys, not both: $*" >&2
        exit 1
      fi
    done
    if [ "$smoke_kind" = cargo ]; then smoke "$@"; exit 0; fi
    smoke_keys=("$@")
    set --
    for k in "${smoke_keys[@]}"; do
      k_zone="$(zone_for_key "$k")"
      set -- "$@" "${k_zone}=paigasus-${k}:dev"
    done
    smoke_consoles "$@"
    ;;
```

Replace the `build-oci)` arm (`:1545-1552`) with:

```bash
  build-oci)
    if [ -z "$target" ] || [ -z "${3:-}" ]; then
      echo "usage: ci/images/run.sh build-oci <iam|gateway|iam-console|gateway-console> <outdir>" >&2
      exit 1
    fi
    # SMA-688: the pins of the key's own Dockerfile. assert_pins reads rs/Dockerfile and the
    # chisel release; assert_console_pins reads ts/Dockerfile. An unknown key stops here.
    oci_kind="$(kind_for_key "$target")"
    if [ "$oci_kind" = npm ]; then assert_console_pins; else assert_pins; fi
    build_oci "$target" "$3"
    ;;
```

In the `all-consoles)` arm, replace the last two lines (the comment `# The zone list is passed, …` and `smoke_consoles "${console_services[@]}"`, `:1571-1574`) with:

```bash
    # The zone list is passed, not restated inside smoke_consoles, so the build loop and the smoke
    # loop cannot disagree about which zones this run covers. SMA-688: each zone is passed with
    # the image it smokes, <app>:dev, the name build_console_one gives it.
    set --
    for s in "${console_services[@]}"; do
      s_app="$(app_for "$s")"
      set -- "$@" "${s}=${s_app}:dev"
    done
    smoke_consoles "$@"
```

- [ ] **Step 4: Run, expect PASS**

Syntax, lint and the dispatcher, with no Docker work:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" PROTO_REPORTER=text
/bin/bash -n ci/images/run.sh && /opt/homebrew/bin/bash -n ci/images/run.sh && echo syntax-ok
sc="$(uv run --locked --project py python3 -c 'import shutil; print(shutil.which("shellcheck"))')"
[ -x "$sc" ] && "$sc" -S warning ci/images/run.sh; echo "shellcheck rc=$?"
/bin/bash ci/images/run.sh build-oci billing out 2>&1 | sed -n 1,3p
/bin/bash ci/images/run.sh smoke iam iam-console 2>&1 | sed -n 1,3p
```
Expected: `syntax-ok`; shellcheck `rc=0`. If shellcheck reports warnings, compare them with the same call on the old file (`old="$(mktemp)"; git show origin/main:ci/images/run.sh > "$old"; "$sc" -S warning "$old"`): only a warning that the old file does not have is a finding. Then `unknown release key: billing`, and `usage: ci/images/run.sh smoke takes service keys or console keys, not both: iam iam-console`.

The console release path, locally (about 40 s warm per build, spec M1). Docker Desktop here uses the containerd store; `load-oci` works on both stores.
```bash
out="$(mktemp -d)"
/bin/bash ci/images/run.sh build-oci iam-console "$out"
arch="$(docker version --format '{{.Server.Arch}}')"
/bin/bash ci/images/run.sh load-oci "$out/paigasus-iam-console-${arch}.oci.tar" paigasus-iam-console:dev
uv run --no-project --python '>=3.12' python3 ci/images/release_decision.py labels "$out/paigasus-iam-console-${arch}.oci.tar"
/bin/bash ci/images/run.sh smoke iam-console
```
Expected: `ts/Dockerfile: distroless Node … all 1 pnpm install(s) --frozen-lockfile`, then `built …/paigasus-iam-console-arm64.oci.tar`; an `M3 …` line and `paigasus-iam-console:dev is the archive's image`; `version=0.0.0` (Task 8 makes it 0.1.0), `revision=<HEAD sha>`, `title=paigasus-iam-console`; the smoke ends with `== CONSOLE SMOKE OK ==`. A stale image fails with `carries revision '…', expected …`, which is `assert_fresh` working.

**What only CI proves:** the amd64 leg, and the same sequence on the classic image store. Task 5 puts it on every PR.

- [ ] **Step 5: Commit**

```bash
git add ci/images/run.sh
git commit -m "$(cat <<'EOF'
feat(ci): build and smoke the console release archive (SMA-688)

build-oci accepts the two console keys. It checks the console pins,
builds ts/Dockerfile to an OCI archive with the service label set and
title paigasus-<key>, and uses no chisel step. smoke accepts a console
key and tests paigasus-<key>:dev, and the console smoke now calls
assert_fresh on the image it tests. The key maps to a kind and a zone
once, at the dispatcher.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 5: `images.yml` runs the console release sequence on every PR

**Files:**
- Modify: `.github/workflows/images.yml` (the `SBOM for each archive` step `:156-162`; a new step between it and `Build + smoke both consoles` `:164-177`)
- Test: `actionlint`; the `images` workflow on the PR.

**Interfaces:**
- Consumes: `build-oci <console-key>`, `smoke <console-key>` (Task 4); `sbom-floor --service` (Task 3).
- Produces: files `sbom-paigasus-<key>-<arch>.spdx.json` (uploaded by the existing `Upload SBOMs` step) and one `M8 image=paigasus-<key> …` line per key.

- [ ] **Step 1: Write the failing test**

The test is the workflow run itself plus actionlint. Before the edit, confirm that no step runs the console release path:
```bash
grep -n "build-oci" .github/workflows/images.yml
```
Expected: only `ci/images/run.sh build-oci iam out` and `… gateway out`.

- [ ] **Step 2: Run it, expect FAIL**

The grep above shows no console key. That is the red.

- [ ] **Step 3: Implement**

Replace the `SBOM for each archive` step (`:155-162`) with:

```yaml
      # A MEASUREMENT for spec M8: does syft see libc6 and the Rust crates? SMA-688: it also holds
      # the cargo SBOM floor now, so each pull request proves the floor that release.yml applies.
      - name: SBOM for each archive
        run: |
          for key in iam gateway; do
            crate="paigasus-${key}"
            syft "oci-archive:out/${crate}-${ARCH}.oci.tar" -o "spdx-json=sbom-${crate}-${ARCH}.spdx.json"
            summary="$(uv run --no-project --python '>=3.12' python3 ci/images/release_decision.py sbom-summary "sbom-${crate}-${ARCH}.spdx.json")"
            echo "M8 image=${crate} arch=${ARCH} $(printf '%s' "$summary" | tr '\n' ' ')"
            uv run --no-project --python '>=3.12' python3 ci/images/release_decision.py sbom-floor --service "$key" "sbom-${crate}-${ARCH}.spdx.json"
          done

      # SMA-688 (spec § 5.3). The console RELEASE sequence, exactly as release.yml's
      # images-build-<key> runs it: OCI archive, load and identity check, smoke, SBOM, SBOM floor.
      # It runs on every pull request that changes a console's package.json or ci/images/**, so
      # each new console path runs before the merge. A pull request runs amd64 only; dispatch
      # this workflow on the branch for the arm64 leg (spec R1). The all-consoles step below
      # stays: it smokes the build-console path.
      - name: Console release sequence
        run: |
          set -euo pipefail
          df -h /
          for key in iam-console gateway-console; do
            ci/images/run.sh build-oci "$key" out
            archive="out/paigasus-${key}-${ARCH}.oci.tar"
            ci/images/run.sh load-oci "$archive" "paigasus-${key}:dev"
            ci/images/run.sh smoke "$key"
            syft "oci-archive:${archive}" -o "spdx-json=sbom-paigasus-${key}-${ARCH}.spdx.json"
            summary="$(uv run --no-project --python '>=3.12' python3 ci/images/release_decision.py sbom-summary "sbom-paigasus-${key}-${ARCH}.spdx.json")"
            echo "M8 image=paigasus-${key} arch=${ARCH} $(printf '%s' "$summary" | tr '\n' ' ')"
            uv run --no-project --python '>=3.12' python3 ci/images/release_decision.py sbom-floor --service "$key" "sbom-paigasus-${key}-${ARCH}.spdx.json"
          done
          df -h /
```

- [ ] **Step 4: Run, expect PASS**

Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" PROTO_REPORTER=text
actionlint -shellcheck= .github/workflows/images.yml; echo "rc=$?"
grep -n "build-oci" .github/workflows/images.yml
```
Expected: `rc=0`; the grep now also shows `ci/images/run.sh build-oci "$key" out`. `-shellcheck=` turns shellcheck off, because actionlint can hang on this Mac's 512-byte pipe (root `CLAUDE.md`, SMA-612); CI runs the full check.

**What only CI proves:** the whole step on `ubuntu-latest` (the classic image store) and the amd64 SBOM counts (spec R1). Read the two `M8 image=paigasus-<key>-console` lines in the PR's `images` run.

- [ ] **Step 5: Commit**

```bash
git add .github/workflows/images.yml
git commit -m "$(cat <<'EOF'
ci(ci): run the console release sequence on every image PR (SMA-688)

images.yml builds each console as an OCI archive, loads it, smokes it,
takes its SBOM and holds the npm SBOM floor, the same sequence that the
release build job runs. The service SBOM step now also holds the cargo
floor.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 6: The release guard learns the console chains and V15, V16, V17

**Files:**
- Modify: `ci/actionlint/release_guard.py` (imports `:19-26`; `CHAIN_APPROVALS` `:67-79`; `OIDC_PUBLISH_JOBS` `:402-411`; new V15/V16/V17 after `chain_scope_violations` `:1385`; `check_main` `:1553-1664`; `_OK_CONSOLE_MAIN` after `:1928`; FIXTURES rows before `:2999`; `_v13_cross_workflow_sweep` `:3100-3104`; `_v11_id_token_write_required` `:3456-3517`; new helpers; `self_test` list `:3701-3722`; `main` `:3747`)
- Modify: `ci/actionlint/run.sh` (`:4607`)
- Modify: `ci/affected-graph/ci_targets.py` (`:1035`, `:2642`, `:2882-2884`)
- Modify: `ci/actionlint/README.md` (`:46`, the number `105`)
- Test: `release_guard.py --self-test`, `--fixture-count`.

**Interfaces:**
- Consumes: `ci/images/chains.toml` (Task 1), fields `ghcr` and `hub`.
- Produces:
  - `CHAIN_APPROVALS` with four keys; `OIDC_PUBLISH_JOBS`, `SCOPED_SECRET_JOBS` and `SERVICE_PLAN_GATE_EXPRS` derived from it.
  - V15 message `…: V15: job '<jid>' does not grant `<scope>: write`…`.
  - V16 messages: `…: V16: CHAIN_APPROVALS names …`, `…: V16: job 'plan' declares no outputs.<out>…`, `…: V16: no job named '<jid>' exists…`, `…: V16: job '<jid>' sets env.SERVICE to …`, `…: V16: job '<jid>' sets <VAR> to …`.
  - V17 messages: `…: V17: job '<jid>' downloads artifacts with a `pattern:`…`, `…: V17: job '<jid>' downloads artifacts without a `name:`…`.
  - `--fixture-count` prints 167.

- [ ] **Step 1: Write the failing tests**

After `_OK_IMAGES_MAIN` (after `:1928`), add the console template:

```python
# SMA-688. A SEPARATE console template, so the `.replace` edits of the existing _OK_IMAGES_MAIN
# rows keep their meaning (spec § 6.2). It is _OK_IMAGES_MAIN plus the four console plan outputs
# and the eight console jobs. The console jobs carry env.SERVICE and an exact-name download, the
# shape V16 and V17 require; the two service chains inherited from _OK_IMAGES_MAIN carry neither,
# which V16 accepts for a document that is not release.yml.
_OK_CONSOLE_MAIN = _OK_IMAGES_MAIN.replace(
    "      version_gateway: ${{ steps.decide.outputs.version_gateway }}\n",
    "      version_gateway: ${{ steps.decide.outputs.version_gateway }}\n"
    "      skip_iam-console: ${{ steps.decide.outputs.skip_iam-console }}\n"
    "      version_iam-console: ${{ steps.decide.outputs.version_iam-console }}\n"
    "      skip_gateway-console: ${{ steps.decide.outputs.skip_gateway-console }}\n"
    "      version_gateway-console: ${{ steps.decide.outputs.version_gateway-console }}\n"
) + """
  images-build-iam-console:
    needs: [plan]
    if: needs.plan.outputs.skip_iam-console != 'true'
    runs-on: ubuntu-latest
    env: {SERVICE: iam-console}
    steps: [{run: ci/images/run.sh build-oci iam-console out}]
  approve-images-iam-console:
    needs: [images-build-iam-console]
    environment: release-approval
    runs-on: ubuntu-latest
    steps: [{run: echo approved}]
  publish-images-iam-console:
    needs: [plan, images-build-iam-console, approve-images-iam-console]
    if: needs.plan.outputs.skip_iam-console != 'true'
    environment: release-images
    runs-on: ubuntu-latest
    permissions:
      packages: write
      id-token: write
      attestations: write
    env: {SERVICE: iam-console}
    steps:
      - uses: actions/download-artifact@v8
        with: {name: image-iam-console-amd64, path: in}
      - run: crane push layout ghcr.io/smk1085/paigasus-iam-console:x
  tag-iam-console:
    needs: [publish-images-iam-console]
    environment: release-publish
    runs-on: ubuntu-latest
    env: {SERVICE: iam-console}
    steps: [{run: gh api repos/o/r/git/refs}]
  images-build-gateway-console:
    needs: [plan]
    if: needs.plan.outputs.skip_gateway-console != 'true'
    runs-on: ubuntu-latest
    env: {SERVICE: gateway-console}
    steps: [{run: ci/images/run.sh build-oci gateway-console out}]
  approve-images-gateway-console:
    needs: [images-build-gateway-console]
    environment: release-approval
    runs-on: ubuntu-latest
    steps: [{run: echo approved}]
  publish-images-gateway-console:
    needs: [plan, images-build-gateway-console, approve-images-gateway-console]
    if: needs.plan.outputs.skip_gateway-console != 'true'
    environment: release-images
    runs-on: ubuntu-latest
    permissions:
      packages: write
      id-token: write
      attestations: write
    env: {SERVICE: gateway-console}
    steps:
      - uses: actions/download-artifact@v8
        with: {name: image-gateway-console-amd64, path: in}
      - run: crane push layout ghcr.io/smk1085/paigasus-gateway-console:x
  tag-gateway-console:
    needs: [publish-images-gateway-console]
    environment: release-publish
    runs-on: ubuntu-latest
    env: {SERVICE: gateway-console}
    steps: [{run: gh api repos/o/r/git/refs}]
"""
```

Append these twelve FIXTURES rows before the closing `]` of `FIXTURES` (`:2999`):

```python
    # SMA-688. The console chains (spec § 6.2). Each row wants its OWN named message.
    ("SMA-688 the console template is clean", "main", _OK_CONSOLE_MAIN, None),
    ("SMA-688 publish-images-iam-console without its own approval", "main",
     _OK_CONSOLE_MAIN.replace("    needs: [plan, images-build-iam-console, approve-images-iam-console]",
                              "    needs: [plan, images-build-iam-console]"),
     "V8c: job 'publish-images-iam-console' can reach a registry, but 'approve-images-iam-console'"),
    # `iam` is a string prefix of `iam-console`. The approval of the WRONG chain must not satisfy
    # V8c. Under code without the console keys this row names 'approve-release' instead.
    ("SMA-688 publish-images-iam-console behind approve-images-iam", "main",
     _OK_CONSOLE_MAIN.replace("    needs: [plan, images-build-iam-console, approve-images-iam-console]",
                              "    needs: [plan, images-build-iam-console, approve-images-iam]"),
     "V8c: job 'publish-images-iam-console' can reach a registry, but 'approve-images-iam-console'"),
    ("SMA-688 images-build-gateway-console gated on skip_gateway", "main",
     _OK_CONSOLE_MAIN.replace(
         "    if: needs.plan.outputs.skip_gateway-console != 'true'\n    runs-on: ubuntu-latest\n"
         "    env: {SERVICE: gateway-console}\n    steps: [{run: ci/images/run.sh build-oci gateway-console out}]",
         "    if: needs.plan.outputs.skip_gateway != 'true'\n    runs-on: ubuntu-latest\n"
         "    env: {SERVICE: gateway-console}\n    steps: [{run: ci/images/run.sh build-oci gateway-console out}]"),
     "job 'images-build-gateway-console' needs 'plan' but its if: is \"needs.plan.outputs.skip_gateway != 'true'\", not one of ["),
    ("SMA-688 V9c: skip_iam-console is declared nowhere in plan's outputs", "main",
     _OK_CONSOLE_MAIN.replace("      skip_iam-console: ${{ steps.decide.outputs.skip_iam-console }}\n", ""),
     "V9c: job 'plan' declares no outputs.skip_iam-console"),
    ("SMA-688 the Docker Hub token in tag-iam-console", "main",
     _OK_CONSOLE_MAIN.replace(
         "    env: {SERVICE: iam-console}\n    steps: [{run: gh api repos/o/r/git/refs}]",
         "    env: {SERVICE: iam-console, T: '${{ secrets.DOCKERHUB_TOKEN }}'}\n"
         "    steps: [{run: gh api repos/o/r/git/refs}]"),
     "V13: job 'tag-iam-console' reads DOCKERHUB_TOKEN"),
    ("SMA-688 V15 publish-images-iam-console without id-token: write", "main",
     _OK_CONSOLE_MAIN.replace("      id-token: write\n      attestations: write\n    env: {SERVICE: iam-console}",
                              "      attestations: write\n    env: {SERVICE: iam-console}"),
     "V15: job 'publish-images-iam-console' does not grant `id-token: write`"),
    ("SMA-688 V15 publish-images-iam-console without attestations: write", "main",
     _OK_CONSOLE_MAIN.replace("      attestations: write\n    env: {SERVICE: iam-console}",
                              "    env: {SERVICE: iam-console}"),
     "V15: job 'publish-images-iam-console' does not grant `attestations: write`"),
    ("SMA-688 V15 publish-images-iam-console without packages: write", "main",
     _OK_CONSOLE_MAIN.replace(
         "      packages: write\n      id-token: write\n      attestations: write\n    env: {SERVICE: iam-console}",
         "      id-token: write\n      attestations: write\n    env: {SERVICE: iam-console}"),
     "V15: job 'publish-images-iam-console' does not grant `packages: write`"),
    ("SMA-688 V16 SERVICE: iam in publish-images-iam-console", "main",
     _OK_CONSOLE_MAIN.replace(
         "    env: {SERVICE: iam-console}\n    steps:\n      - uses: actions/download-artifact@v8\n"
         "        with: {name: image-iam-console-amd64, path: in}",
         "    env: {SERVICE: iam}\n    steps:\n      - uses: actions/download-artifact@v8\n"
         "        with: {name: image-iam-console-amd64, path: in}"),
     "V16: job 'publish-images-iam-console' sets env.SERVICE to 'iam', not 'iam-console'"),
    # D11. `pattern: image-iam-*` also matches `image-iam-console-*`.
    ("SMA-688 V17 pattern: image-iam-* in publish-images-iam", "main",
     _OK_CONSOLE_MAIN.replace(
         "    steps: [{run: crane push layout ghcr.io/smk1085/paigasus-iam:x}]",
         "    steps:\n      - uses: actions/download-artifact@v8\n"
         "        with: {pattern: 'image-iam-*', merge-multiple: true, path: in}\n"
         "      - run: crane push layout ghcr.io/smk1085/paigasus-iam:x"),
     "V17: job 'publish-images-iam' downloads artifacts with a `pattern:`"),
    # A download with neither `name:` nor `pattern:` fetches EVERY artifact of the run.
    ("SMA-688 V17 a download with no name in publish-images-iam-console", "main",
     _OK_CONSOLE_MAIN.replace("        with: {name: image-iam-console-amd64, path: in}",
                              "        with: {path: in}"),
     "V17: job 'publish-images-iam-console' downloads artifacts without a `name:`"),
```

Directly before `def _v12_npm_floor_pinned` (`:3520`), add the two helpers:

```python
def _sma688_console_approvals() -> str | None:
    """approval_for_job compares WHOLE job ids. `iam` is a string prefix of `iam-console`.

    Mutation: compare with `job_id.startswith(f"{prefix}{service}")`, and the console jobs resolve
    to approve-images-iam. The derived tables must also cover both console keys: a hand-written
    tuple would miss them.
    """
    cases = {
        "images-build-iam-console": "approve-images-iam-console",
        "publish-images-iam-console": "approve-images-iam-console",
        "tag-iam-console": "approve-images-iam-console",
        "images-build-gateway-console": "approve-images-gateway-console",
        "publish-images-gateway-console": "approve-images-gateway-console",
        "tag-gateway-console": "approve-images-gateway-console",
        "images-build-iam": "approve-images-iam",
        "publish-images-iam": "approve-images-iam",
        "tag-iam": "approve-images-iam",
        "publish-images-iam-console-x": APPROVAL_JOB,
    }
    for jid, want in cases.items():
        got = approval_for_job(jid)
        if got != want:
            return f"approval_for_job({jid!r}) returned {got!r}, want {want!r}"
    for key in ("iam-console", "gateway-console"):
        if f"publish-images-{key}" not in SCOPED_SECRET_JOBS:
            return f"SCOPED_SECRET_JOBS does not hold publish-images-{key}"
        if f"publish-images-{key}" not in OIDC_PUBLISH_JOBS:
            return f"OIDC_PUBLISH_JOBS does not hold publish-images-{key}"
        if key not in SERVICE_PLAN_GATE_EXPRS:
            return f"SERVICE_PLAN_GATE_EXPRS does not hold {key}"
    return None


def _v16_registry_agreement() -> str | None:
    """V16 (SMA-688): release.yml, CHAIN_APPROVALS and ci/images/chains.toml agree.

    Here rather than as FIXTURES rows for V11's reason: the rule is scoped to RELEASE_WORKFLOW_NAME
    and needs a registry, and self_test() names every FIXTURES document "fixture". The healthy
    document is _OK_CONSOLE_MAIN with the env and image names that release.yml carries.
    """
    registry = {key: {"ghcr": f"ghcr.io/smk1085/paigasus-{key}",
                      "hub": f"docker.io/smaschek/paigasus-{key}"} for key in CHAIN_APPROVALS}

    def healthy() -> dict:
        doc = yaml.safe_load(_OK_CONSOLE_MAIN)
        for key in CHAIN_APPROVALS:
            for prefix in _CHAIN_JOB_PREFIXES:
                job = doc["jobs"][f"{prefix}{key}"]
                job["env"] = {**(job.get("env") or {}), "SERVICE": key}
            doc["jobs"][f"publish-images-{key}"]["env"].update(
                GHCR_IMAGE=registry[key]["ghcr"], HUB_IMAGE=registry[key]["hub"])
        return doc

    def v16(doc: dict, reg: dict) -> list[str]:
        found = registry_violations(doc, RELEASE_WORKFLOW_NAME, reg) \
            + chain_service_violations(doc["jobs"], RELEASE_WORKFLOW_NAME)
        return [ln for ln in found if ": V16:" in ln]

    if v16(healthy(), registry):
        return f"the healthy shape red: {v16(healthy(), registry)}"
    # A key in CHAIN_APPROVALS and not in chains.toml.
    narrower = {k: v for k, v in registry.items() if k != "gateway-console"}
    if not any("V16: CHAIN_APPROVALS names" in ln for ln in v16(healthy(), narrower)):
        return f"a CHAIN_APPROVALS key missing from the registry did not red: {v16(healthy(), narrower)}"
    # A key in chains.toml and nowhere else: the plan outputs and the jobs are missing too.
    wider = {**registry, "billing": {"ghcr": "ghcr.io/smk1085/paigasus-billing",
                                     "hub": "docker.io/smaschek/paigasus-billing"}}
    found = v16(healthy(), wider)
    for want in ("V16: CHAIN_APPROVALS names", "declares no outputs.skip_billing",
                 "no job named 'images-build-billing'"):
        if not any(want in ln for ln in found):
            return f"a registry-only key did not produce {want!r}: {found}"
    doc = healthy()
    doc["jobs"]["publish-images-iam-console"]["env"]["GHCR_IMAGE"] = "ghcr.io/smk1085/paigasus-iam"
    if not any("V16: job 'publish-images-iam-console' sets GHCR_IMAGE" in ln for ln in v16(doc, registry)):
        return "a GHCR_IMAGE of another chain did not red"
    doc = healthy()
    doc["jobs"]["publish-images-gateway-console"]["env"]["HUB_IMAGE"] = "docker.io/smaschek/paigasus-gateway"
    if not any("V16: job 'publish-images-gateway-console' sets HUB_IMAGE" in ln for ln in v16(doc, registry)):
        return "a HUB_IMAGE of another chain did not red"
    doc = healthy()
    del doc["jobs"]["tag-gateway-console"]["env"]["SERVICE"]
    if not any("V16: job 'tag-gateway-console' sets env.SERVICE to None" in ln for ln in v16(doc, registry)):
        return "a release.yml chain job with no SERVICE did not red"
    doc = healthy()
    del doc["jobs"]["plan"]["outputs"]["version_iam-console"]
    if not any("declares no outputs.version_iam-console" in ln for ln in v16(doc, registry)):
        return "a missing version_iam-console plan output did not red"
    # The loader fails closed: a missing file or an entry with no hub is infra (exit 2).
    with tempfile.TemporaryDirectory() as tmp:
        for label, text in (("missing", None), ("no hub", "[chain.iam]\nghcr = 'g'\n")):
            path = Path(tmp) / f"{label}.toml"
            if text is not None:
                path.write_text(text)
            try:
                with contextlib.redirect_stderr(io.StringIO()):
                    load_chain_registry(path)
            except SystemExit as exc:
                if exc.code != 2:
                    return f"load_chain_registry ({label}) exited {exc.code!r}, want 2"
                continue
            return f"load_chain_registry ({label}) returned instead of infra(2)"
    return None
```

In `_v11_id_token_write_required` (`:3456-3475`), replace the two literal image stubs `"publish-images-iam": …` and `"publish-images-gateway": …` in `doc_with` with a derived entry, so the helper covers every chain:

```python
            "release": {"permissions": grant, "steps": [{"run": "echo hi"}]},
            # SMA-688: every chain's publish job, derived from CHAIN_APPROVALS like
            # OIDC_PUBLISH_JOBS itself, so a new chain cannot slip past this helper.
            **{f"publish-images-{service}": {"permissions": grant, "steps": [{"run": "echo hi"}]}
               for service in CHAIN_APPROVALS},
```

and add this case directly before `# And the scoping: no other document may inherit release.yml's rule.` (`:3514`):

```python
    # SMA-688: a console publish job without id-token: write reds V11 too.
    console_doc = doc_with(grant, grant)
    console_doc["jobs"]["publish-images-iam-console"]["permissions"] = {
        "packages": "write", "attestations": "write"}
    if not any("job 'publish-images-iam-console' does not grant" in line for line in v11(console_doc)):
        return f"publish-images-iam-console without id-token: write did not red: {v11(console_doc)}"
```

In `_v13_cross_workflow_sweep` (`:3100-3104`), the tempdir `release.yml` now reaches V16, which reads `ci/images/chains.toml` relative to the working directory. Add the file to the tree, and append one sentence to the docstring:

```python
    rc, out, err = _run_main_in_tempdir(
        {".github/workflows/release.yml": _OK_MAIN, ".github/workflows/other.yaml": other,
         # SMA-688: main() runs V16 on a file named release.yml, and V16 reads the registry.
         # Without it, load_chain_registry fails closed (exit 2) and this row cannot reach V13.
         "ci/images/chains.toml": "[chain.iam]\nghcr = 'ghcr.io/smk1085/paigasus-iam'\n"
                                  "hub = 'docker.io/smaschek/paigasus-iam'\n"},
        entry=".github/workflows/release.yml",
    )
```

Add two entries to the helper list in `self_test()` (after `("v12 npm OIDC floor pinned in both workflows", _v12_npm_floor_pinned),`, `:3717`):

```python
        ("sma-688 approval_for_job and the derived tables cover the console keys",
         _sma688_console_approvals),
        ("sma-688 V16 registry agreement", _v16_registry_agreement),
```

- [ ] **Step 2: Run it, expect FAIL**

Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" PROTO_REPORTER=text
uv run --locked --project py python3 ci/actionlint/release_guard.py --self-test; echo "rc=$?"
```
Expected: `rc=1`, with `FAIL 'SMA-688 the console template is clean': expected clean, got: …V8c: job 'publish-images-iam-console' can reach a registry, but 'approve-release' …`, the V15/V16/V17 rows reporting `(clean)` or another message, and a `NameError` for `registry_violations` from the V16 helper (a traceback is also an acceptable red here).

- [ ] **Step 3: Implement**

Add `import tomllib` after `import tempfile` (`:24`).

Replace `CHAIN_APPROVALS` (`:76-79`) with:

```python
CHAIN_APPROVALS: dict[str, str] = {
    "iam": "approve-images-iam",
    "gateway": "approve-images-gateway",
    # SMA-688. The two console chains. `iam` is a string PREFIX of `iam-console`, and `gateway`
    # of `gateway-console`. Every lookup below compares a WHOLE job id, never a prefix, so each
    # key selects its own jobs only. V16 asserts that these keys equal ci/images/chains.toml's.
    "iam-console": "approve-images-iam-console",
    "gateway-console": "approve-images-gateway-console",
}
```

Replace `OIDC_PUBLISH_JOBS` (`:409-411`) with:

```python
# SMA-688: the image members derive from CHAIN_APPROVALS, like SCOPED_SECRET_JOBS and
# SERVICE_PLAN_GATE_EXPRS. A literal tuple would leave a new chain's publish job outside V11.
OIDC_PUBLISH_JOBS = (
    "release", "publish-pypi", "publish-npm",
    *(f"publish-images-{service}" for service in CHAIN_APPROVALS),
)
```

After `chain_scope_violations` (`:1385`), add V15, V16 and V17:

```python
# V15 (SMA-688). Every publish-images-<key> job must HOLD the three grants its steps use. V14 only
# forbids a write grant upstream of an approval; nothing required the grants to exist. Without
# `packages: write` the GHCR push fails. Without `id-token: write` cosign and both attest steps
# fail. Without `attestations: write` attest-build-provenance fails AFTER the GHCR push, so the
# chain stops with a pushed and unattested image. A job-level permissions: block sets every scope
# it omits to none, so the workflow-level block counts only for a job that declares none.
PUBLISH_JOB_GRANTS = ("packages", "id-token", "attestations")


def publish_grant_violations(doc: dict, name: str) -> list[str]:
    """V15. Applies to every document, so FIXTURES rows can reach it. A missing job is V11's and
    V16's concern, not this rule's."""
    out: list[str] = []
    jobs = doc.get("jobs") or {}
    workflow_perms = doc.get("permissions")
    for service in CHAIN_APPROVALS:
        jid = f"publish-images-{service}"
        job = jobs.get(jid)
        if not isinstance(job, dict):
            continue
        for scope in PUBLISH_JOB_GRANTS:
            grant = _grants_scope(job.get("permissions"), scope)
            if grant is None:
                grant = bool(_grants_scope(workflow_perms, scope))
            if not grant:
                out.append(f"{name}: V15: job '{jid}' does not grant `{scope}: write`. Its steps "
                           f"need it: the GHCR push, cosign or an attest step fails without it, "
                           f"and attest-build-provenance fails only after the image is pushed.")
    return out


# V16 (SMA-688). The registry agreement. ci/images/chains.toml names every image chain once (spec
# D10). This file keeps CHAIN_APPROVALS, and release.yml keeps the chain jobs, the plan outputs
# and the image names. Three hand-written copies drift unless one rule holds them equal.
CHAIN_REGISTRY_PATH = Path("ci/images/chains.toml")


def load_chain_registry(path: Path) -> dict[str, dict[str, str]]:
    """key -> {ghcr, hub}. FAIL-CLOSED, this file's convention: anything unreadable is infra."""
    try:
        with path.open("rb") as fh:
            data = tomllib.load(fh)
    except (OSError, tomllib.TOMLDecodeError) as exc:
        infra(f"cannot read the chain registry {path}: {exc}")
    chains = data.get("chain")
    if not isinstance(chains, dict) or not chains:
        infra(f"{path} has no [chain.<key>] table")
    out: dict[str, dict[str, str]] = {}
    for key, entry in chains.items():
        if not isinstance(entry, dict) or not all(
                isinstance(entry.get(field), str) and entry.get(field) for field in ("ghcr", "hub")):
            infra(f"{path}: [chain.{key}] needs string ghcr and hub values")
        out[key] = {"ghcr": entry["ghcr"], "hub": entry["hub"]}
    return out


def registry_violations(doc: dict, name: str, registry: dict[str, dict[str, str]]) -> list[str]:
    """V16a-c: the keys, the plan outputs, the chain jobs and the image names agree with the
    registry. main() calls it for RELEASE_WORKFLOW_NAME only, with the real registry."""
    out: list[str] = []
    if set(CHAIN_APPROVALS) != set(registry):
        out.append(f"{name}: V16: CHAIN_APPROVALS names {sorted(CHAIN_APPROVALS)}, but "
                   f"{CHAIN_REGISTRY_PATH} names {sorted(registry)}. Add or remove a chain in "
                   f"both, in one commit.")
    jobs = doc.get("jobs") or {}
    plan = jobs.get(PLAN_JOB)
    outs = plan.get("outputs") if isinstance(plan, dict) else None
    outs = outs if isinstance(outs, dict) else {}
    for key, entry in registry.items():
        for output in (f"skip_{key}", f"version_{key}"):
            if output not in outs:
                out.append(f"{name}: V16: job '{PLAN_JOB}' declares no outputs.{output}, but "
                           f"{CHAIN_REGISTRY_PATH} names the chain '{key}'. The chain would read "
                           f"an empty string.")
        for prefix in _CHAIN_JOB_PREFIXES:
            jid = f"{prefix}{key}"
            if not isinstance(jobs.get(jid), dict):
                out.append(f"{name}: V16: no job named '{jid}' exists, but {CHAIN_REGISTRY_PATH} "
                           f"names the chain '{key}'.")
        pub = jobs.get(f"publish-images-{key}")
        env = pub.get("env") if isinstance(pub, dict) else None
        env = env if isinstance(env, dict) else {}
        for var, field in (("GHCR_IMAGE", "ghcr"), ("HUB_IMAGE", "hub")):
            if isinstance(pub, dict) and env.get(var) != entry[field]:
                out.append(f"{name}: V16: job 'publish-images-{key}' sets {var} to "
                           f"{env.get(var)!r}, but {CHAIN_REGISTRY_PATH} names {entry[field]!r}.")
    return out


def chain_service_violations(jobs: dict, name: str) -> list[str]:
    """V16d: each chain job's env.SERVICE equals the key in its job id. Every step of a chain reads
    SERVICE, so a wrong value builds, downloads or tags another chain's image under this chain's
    approval. release.yml must declare SERVICE; another document may omit it (the fixtures)."""
    out: list[str] = []
    for key in CHAIN_APPROVALS:
        for prefix in _CHAIN_JOB_PREFIXES:
            jid = f"{prefix}{key}"
            job = jobs.get(jid)
            if not isinstance(job, dict):
                continue
            env = job.get("env")
            service = env.get("SERVICE") if isinstance(env, dict) else None
            if service is None and name != RELEASE_WORKFLOW_NAME:
                continue
            if service != key:
                out.append(f"{name}: V16: job '{jid}' sets env.SERVICE to {service!r}, not "
                           f"{key!r}. Every step of the chain reads SERVICE, so a wrong value "
                           f"handles another chain's image behind this chain's approval.")
    return out


# V17 (SMA-688 D11). A chain job downloads its artifacts by EXACT name. `pattern: image-iam-*`
# also matches `image-iam-console-*`, because `iam` is a string prefix of `iam-console`. A
# download with no `name:` at all fetches every artifact of the run.
_DOWNLOAD_ARTIFACT_ACTION = "actions/download-artifact"


def chain_download_violations(jobs: dict, name: str) -> list[str]:
    out: list[str] = []
    for jid, job in jobs.items():
        if not isinstance(job, dict) or approval_for_job(jid) == APPROVAL_JOB:
            continue
        for step in steps_of(job, f"{name}: job '{jid}'"):
            if not isinstance(step, dict) or _DOWNLOAD_ARTIFACT_ACTION not in str(step.get("uses") or ""):
                continue
            with_block = step.get("with")
            with_block = with_block if isinstance(with_block, dict) else {}
            if "pattern" in with_block:
                out.append(f"{name}: V17: job '{jid}' downloads artifacts with a `pattern:` "
                           f"({with_block['pattern']!r}). A chain key can be a string prefix of "
                           f"another chain's key; download each artifact by its exact `name:`.")
            elif not with_block.get("name"):
                out.append(f"{name}: V17: job '{jid}' downloads artifacts without a `name:`. That "
                           f"fetches every artifact of the run, other chains' archives included.")
    return out
```

In `check_main`, after `out += capability_violations(jobs, doc.get("permissions"), name)` (`:1663`), add:

```python
    # SMA-688. V15, V16d and V17: once each, outside the per-job loop, like V8 above.
    out += publish_grant_violations(doc, name)
    out += chain_service_violations(jobs, name)
    out += chain_download_violations(jobs, name)
```

and change the first line of its docstring (`:1554`) to `"""V1-V5, V7, V8a-c, V8e, V9 and V13-V17 over the release workflow (V16a-c runs from main()). V6 applies to CALLED workflows (see`.

In `main`, after `violations += check_main(main_doc, main_path.name)` (`:3747`), add:

```python
    # V16a-c (SMA-688): the chain registry agreement. Scoped to the release workflow by name,
    # like V11. The registry path is relative to the repository root, which is where check 10
    # runs this guard.
    if main_path.name == RELEASE_WORKFLOW_NAME:
        violations += registry_violations(main_doc, main_path.name,
                                          load_chain_registry(CHAIN_REGISTRY_PATH))
```

Raise the check-10 floor from 150 to 162 in all four places, in this commit:

- `ci/actionlint/run.sh:4607`: `  [ "$n" -ge 162 ] || infra "check 10: release_guard.py reports $n fixtures, expected at least 162"`
- `ci/affected-graph/ci_targets.py:1035`: `'[ "$n" -ge 162 ] || infra "check 10: release_guard.py reports $n fixtures, expected at least 162"',`
- `ci/affected-graph/ci_targets.py:2642`: `'  [ "$n" -ge 162 ] || infra "check 10: release_guard.py reports $n fixtures, expected at least 162"\n'`
- `ci/affected-graph/ci_targets.py:2882-2884`: `'  [ "$n" -ge 162 ] || infra "check 10: release_guard.py reports $n fixtures, '` and `'expected at least 162"\n',`
- `ci/actionlint/README.md:46`: change `reports at least 105 fixtures` to `reports at least 162 fixtures`.

- [ ] **Step 4: Run, expect PASS**

Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" PROTO_REPORTER=text
uv run --locked --project py python3 ci/actionlint/release_guard.py --self-test; echo "rc=$?"
uv run --locked --project py python3 ci/actionlint/release_guard.py --fixture-count
python3 ci/affected-graph/ci_targets.py --self-test; echo "rc=$?"
uv run --locked --project py ruff check --config py/pyproject.toml ci/actionlint/ ci/affected-graph/
uv run --locked --project py python3 ci/actionlint/release_guard.py .github/workflows/release.yml; echo "rc=$?"
```
Expected: `--self-test` `rc=0` with no `FAIL` line; `--fixture-count` prints `167`; `ci_targets.py --self-test` `rc=0`; ruff `All checks passed!`.

The real-file run is EXPECTED RED at this point (`rc=1`), because `release.yml` has no console jobs yet. It must print these violations and no other kind:
- `release.yml: V11: no job named 'publish-images-iam-console' exists…` and the same for `gateway-console`;
- `release.yml: V16: job 'plan' declares no outputs.skip_iam-console…` (and `version_iam-console`, `skip_gateway-console`, `version_gateway-console`);
- `release.yml: V16: no job named 'images-build-iam-console' exists…` (and `publish-images-…`, `tag-…`, for both console keys);
- `release.yml: V17: job 'publish-images-iam' downloads artifacts with a `pattern:`…` and the same for `publish-images-gateway`.

Any other violation is a defect in this task. Task 7 makes the real-file run green. Do not push between Task 6 and Task 7.

- [ ] **Step 5: Commit**

```bash
git add ci/actionlint/release_guard.py ci/actionlint/run.sh ci/actionlint/README.md ci/affected-graph/ci_targets.py
git commit -m "$(cat <<'EOF'
feat(ci): guard the console image chains (SMA-688)

The release guard knows the two console chains. OIDC_PUBLISH_JOBS now
derives from CHAIN_APPROVALS. V15 requires the three grants on every
publish job, V16 holds release.yml, CHAIN_APPROVALS and chains.toml
equal, and V17 forbids a pattern download in a chain job. A separate
console template carries the new fixtures, and the check-10 floor rises
to 162 in all four places. The real release.yml reds until the console
jobs land in the next commit.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 7: `release.yml` gets generic anchors and the console chains

**Files:**
- Modify: `.github/workflows/release.yml` (plan outputs `:423-429`; `&image-build-steps` `:1218-1258`; `&image-publish-steps` download `:1325-1330`, decide step `:1376` and after `:1398`; append after `:1804`)
- Test: the guard over the real file; `actionlint`.

**Interfaces:**
- Consumes: V8/V9/V11/V13-V17 (Task 6); `sbom-floor`, `labels` title (Task 3); `build-oci`/`smoke` console keys (Task 4); plan outputs (Task 2).
- Produces: jobs `images-build-<key>`, `approve-images-<key>`, `publish-images-<key>`, `tag-<key>` for `iam-console` and `gateway-console`; plan outputs `skip_iam-console`, `version_iam-console`, `skip_gateway-console`, `version_gateway-console`; per-arch digest files `out/digest-<key>-<arch>.txt`.

- [ ] **Step 1: Write the failing test**

The failing test is the Task 6 guard run over the real file. Run it again and keep the output:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" PROTO_REPORTER=text
uv run --locked --project py python3 ci/actionlint/release_guard.py .github/workflows/release.yml; echo "rc=$?"
```

- [ ] **Step 2: Run it, expect FAIL**

Expected: `rc=1` with exactly the V11, V16 and V17 violations listed at the end of Task 6.

- [ ] **Step 3: Implement**

Plan outputs: replace `:423-429` (the comment and the four service lines) with:

```yaml
      # SMA-658. One skip flag and one version for each image chain. The version is what
      # publish-images-<key> compares against the image's own label, so a chain can never publish
      # an archive built for a different version. SMA-688: the two console chains too.
      # release_guard.py V16 asserts one pair for every key of ci/images/chains.toml.
      skip_iam: ${{ steps.decide.outputs.skip_iam }}
      skip_gateway: ${{ steps.decide.outputs.skip_gateway }}
      skip_iam-console: ${{ steps.decide.outputs.skip_iam-console }}
      skip_gateway-console: ${{ steps.decide.outputs.skip_gateway-console }}
      version_iam: ${{ steps.decide.outputs.version_iam }}
      version_gateway: ${{ steps.decide.outputs.version_gateway }}
      version_iam-console: ${{ steps.decide.outputs.version_iam-console }}
      version_gateway-console: ${{ steps.decide.outputs.version_gateway-console }}
```

In `&image-build-steps`, change the digest write (`:1232`) to:

```yaml
          # SMA-688 D11: the file name carries the key. Two chains' artifacts can no longer hold a
          # file with the same name.
          printf '%s\n' "$manifest" > "out/digest-${SERVICE}-${ARCH}.txt"
```

Replace the step `Report the SBOM contents (spec AC 7)` (`:1240-1249`) with:

```yaml
      - name: Report the SBOM contents and hold its floor (spec AC 7, SMA-688 D5)
        run: |
          set -euo pipefail
          sbom="out/sbom-paigasus-${SERVICE}-${ARCH}.spdx.json"
          summary="$(uv run --no-project --python '>=3.12' python3 ci/images/release_decision.py sbom-summary "$sbom")"
          echo "M8 image=paigasus-${SERVICE} arch=${ARCH} ${summary}"
          # One floor for each image kind, read from ci/images/chains.toml. A cargo image needs
          # Rust crates (cargo auditable); a console image needs npm packages, libc6 and next.
          # A failed floor exits 3 and names what is missing.
          uv run --no-project --python '>=3.12' python3 ci/images/release_decision.py sbom-floor --service "${SERVICE}" "$sbom"
```

In the upload step (`:1258`), change `out/digest-${{ matrix.arch }}.txt` to `out/digest-${{ env.SERVICE }}-${{ matrix.arch }}.txt`.

In `&image-publish-steps`, replace the `Download both archives` step (`:1325-1330`) with:

```yaml
      # SMA-688 D11: two downloads, each by its EXACT artifact name. `pattern: image-iam-*` also
      # matched `image-iam-console-*`, because `iam` is a string prefix of `iam-console`.
      # release_guard.py V17 forbids a `pattern:` download in a chain job.
      - name: Download the amd64 archive
        uses: actions/download-artifact@3e5f45b2cfb9172054b4087a40e8e0b5a5461e7c  # v8.0.1
        with:
          name: image-${{ env.SERVICE }}-amd64
          path: in

      - name: Download the arm64 archive
        uses: actions/download-artifact@3e5f45b2cfb9172054b4087a40e8e0b5a5461e7c  # v8.0.1
        with:
          name: image-${{ env.SERVICE }}-arm64
          path: in
```

In the decide step, change `built_digest="$(cat "in/digest-${arch}.txt")"` (`:1376`) to `built_digest="$(cat "in/digest-${SERVICE}-${arch}.txt")"`. Directly after the version compare's closing `fi` (`:1398`), add:

```yaml
          # SMA-688: the archive must BE this chain's image. Four chains share one run, and the
          # first console release puts all four at 0.1.0, so the version compare above cannot tell
          # two chains apart. This runs before the first registry write (AC 5).
          title="$(printf '%s\n' "$labels" | sed -n 's/^title=//p')"
          if [ "$title" != "paigasus-${SERVICE}" ]; then
            echo "::error::the archive is ${title:-<no title label>}, but this chain publishes paigasus-${SERVICE}." >&2
            exit 1
          fi
```

Append the eight console jobs after `tag-gateway` (after `:1804`):

```yaml

  # SMA-688. The two console chains. Same four-job shape as the service chains above; every step
  # list is aliased from the anchors on the iam chain, so no step text differs between the four
  # chains. ci/images/chains.toml names both keys, and release_guard.py V16 asserts that this file,
  # CHAIN_APPROVALS and the registry agree. All approval jobs share `release-approval`, so one
  # approval releases every chain that waits in the same run (spec D9).
  images-build-iam-console:
    name: build the iam console image (${{ matrix.arch }})
    needs: [plan]
    # FAIL-SAFE POLARITY, exactly like the service chains: anything but the literal 'true' runs.
    if: needs.plan.outputs.skip_iam-console != 'true'
    runs-on: ${{ matrix.arch == 'arm64' && 'ubuntu-24.04-arm' || 'ubuntu-latest' }}
    timeout-minutes: 60
    permissions:
      contents: read
    strategy:
      fail-fast: false
      matrix:
        arch:
          - amd64
          - arm64
    env:
      SERVICE: iam-console
      ARCH: ${{ matrix.arch }}
      PROTO_REPORTER: text
    steps: *image-build-steps

  approve-images-iam-console:
    name: approve the iam console image release
    needs: [images-build-iam-console]
    runs-on: ubuntu-latest
    timeout-minutes: 5
    environment: release-approval
    steps:
      - run: echo "Approved. Everything below this point is irreversible."

  publish-images-iam-console:
    name: publish the iam console image
    needs: [plan, images-build-iam-console, approve-images-iam-console]
    if: needs.plan.outputs.skip_iam-console != 'true'
    runs-on: ubuntu-latest
    timeout-minutes: 30
    environment: release-images
    concurrency:
      group: release-images-iam-console
      cancel-in-progress: false
    permissions:
      contents: read
      packages: write
      id-token: write
      attestations: write
    outputs:
      digest: ${{ steps.publish.outputs.index }}
      revision: ${{ steps.publish.outputs.revision }}
    env:
      SERVICE: iam-console
      VERSION: ${{ needs.plan.outputs.version_iam-console }}
      GHCR_IMAGE: ghcr.io/smk1085/paigasus-iam-console
      HUB_IMAGE: docker.io/smaschek/paigasus-iam-console
      PROTO_REPORTER: text
    steps: *image-publish-steps

  tag-iam-console:
    name: tag the iam console release
    needs: [plan, publish-images-iam-console]
    if: needs.plan.outputs.skip_iam-console != 'true'
    runs-on: ubuntu-latest
    timeout-minutes: 10
    environment: release-publish
    concurrency:
      group: release-tag-iam-console
      cancel-in-progress: false
    permissions:
      contents: read
    env:
      SERVICE: iam-console
      VERSION: ${{ needs.plan.outputs.version_iam-console }}
      REVISION: ${{ needs.publish-images-iam-console.outputs.revision }}
    steps: *image-tag-steps

  images-build-gateway-console:
    name: build the gateway console image (${{ matrix.arch }})
    needs: [plan]
    if: needs.plan.outputs.skip_gateway-console != 'true'
    runs-on: ${{ matrix.arch == 'arm64' && 'ubuntu-24.04-arm' || 'ubuntu-latest' }}
    timeout-minutes: 60
    permissions:
      contents: read
    strategy:
      fail-fast: false
      matrix:
        arch:
          - amd64
          - arm64
    env:
      SERVICE: gateway-console
      ARCH: ${{ matrix.arch }}
      PROTO_REPORTER: text
    steps: *image-build-steps

  approve-images-gateway-console:
    name: approve the gateway console image release
    needs: [images-build-gateway-console]
    runs-on: ubuntu-latest
    timeout-minutes: 5
    environment: release-approval
    steps:
      - run: echo "Approved. Everything below this point is irreversible."

  publish-images-gateway-console:
    name: publish the gateway console image
    needs: [plan, images-build-gateway-console, approve-images-gateway-console]
    if: needs.plan.outputs.skip_gateway-console != 'true'
    runs-on: ubuntu-latest
    timeout-minutes: 30
    environment: release-images
    concurrency:
      group: release-images-gateway-console
      cancel-in-progress: false
    permissions:
      contents: read
      packages: write
      id-token: write
      attestations: write
    outputs:
      digest: ${{ steps.publish.outputs.index }}
      revision: ${{ steps.publish.outputs.revision }}
    env:
      SERVICE: gateway-console
      VERSION: ${{ needs.plan.outputs.version_gateway-console }}
      GHCR_IMAGE: ghcr.io/smk1085/paigasus-gateway-console
      HUB_IMAGE: docker.io/smaschek/paigasus-gateway-console
      PROTO_REPORTER: text
    steps: *image-publish-steps

  tag-gateway-console:
    name: tag the gateway console release
    needs: [plan, publish-images-gateway-console]
    if: needs.plan.outputs.skip_gateway-console != 'true'
    runs-on: ubuntu-latest
    timeout-minutes: 10
    environment: release-publish
    concurrency:
      group: release-tag-gateway-console
      cancel-in-progress: false
    permissions:
      contents: read
    env:
      SERVICE: gateway-console
      VERSION: ${{ needs.plan.outputs.version_gateway-console }}
      REVISION: ${{ needs.publish-images-gateway-console.outputs.revision }}
    steps: *image-tag-steps
```

Also update the chain comment above `images-build-iam` (`:1156-1160`): change `Each service has its own four jobs and its own approval job` to `Each image chain (two services, SMA-658; two consoles, SMA-688) has its own four jobs and its own approval job`.

- [ ] **Step 4: Run, expect PASS**

Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" PROTO_REPORTER=text
uv run --locked --project py python3 ci/actionlint/release_guard.py .github/workflows/release.yml; echo "rc=$?"
actionlint -shellcheck= .github/workflows/release.yml; echo "rc=$?"
grep -cE '^[[:space:]]+pattern:' .github/workflows/release.yml
```
Expected: the guard prints nothing and `rc=0`. actionlint `rc=0` (it also checks that every `needs.plan.outputs.<name>` exists). The count of `pattern:` YAML keys (comment lines start with `#` and do not match) is `1`: the kernel `publish-pypi` job's `pattern: wheel-*` (`:681`), which is not a chain job.

**What only the live run proves:** the changed anchors on the service chains (spec R3) and the console chains end to end. The next release after the merge is the first live run.

- [ ] **Step 5: Commit**

```bash
git add .github/workflows/release.yml
git commit -m "$(cat <<'EOF'
feat(ci): release the console images from release.yml (SMA-688)

Eight jobs release iam-console and gateway-console through the same
build, approve, publish and tag chain as the services. The anchors
become generic in SERVICE: each archive is downloaded by its exact
name, the digest file carries the key, the SBOM step holds the floor of
the image kind, and the decide step checks the archive's title before
the first registry write.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 8: Bump both consoles to 0.1.0

**Files:**
- Modify: `ts/apps/iam-console/package.json` (`:3`), `ts/apps/gateway-console/package.json` (`:3`)
- Create: `ts/apps/iam-console/CHANGELOG.md`, `ts/apps/gateway-console/CHANGELOG.md`
- Test: `release_plan.py --assert`; `pnpm install --frozen-lockfile`; `ts:fmt`.

**Interfaces:**
- Consumes: `--assert` over every chain (Task 1).
- Produces: both consoles at `0.1.0`, each with `## [0.1.0] - 2026-09-25`. Task 9's row 8a reads these versions. Replace the date with the merge date if the PR merges on a later day, in the same PR.

- [ ] **Step 1: Write the failing test**

Change line 3 of both `package.json` files from `"version": "0.0.0",` to:

```json
  "version": "0.1.0",
```

- [ ] **Step 2: Run it, expect FAIL**

Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" PROTO_REPORTER=text
uv run --locked --project ci/release-plan --python '>=3.12' python3 ci/release-plan/release_plan.py --assert .; echo "rc=$?"
```
Expected: `rc=3` and two lines `release-plan: paigasus-iam-console is at 0.1.0 but …/ts/apps/iam-console/CHANGELOG.md cannot be read (…No such file…)` and the same for `gateway-console`.

- [ ] **Step 3: Implement**

Create `ts/apps/iam-console/CHANGELOG.md`:

```markdown
# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

A maintainer writes this file by hand. The version source is the `version` field of this app's
`package.json`. release-plz does not process this app (SMA-688, spec D1).

## [Unreleased]

## [0.1.0] - 2026-09-25

### Added

- The first released container image: `ghcr.io/smk1085/paigasus-iam-console` and
  `docker.io/smaschek/paigasus-iam-console` (SMA-688).
```

Create `ts/apps/gateway-console/CHANGELOG.md` with the same text, with `paigasus-iam-console` replaced by `paigasus-gateway-console` in both image names.

- [ ] **Step 4: Run, expect PASS**

Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" PROTO_REPORTER=text
uv run --locked --project ci/release-plan --python '>=3.12' python3 ci/release-plan/release_plan.py --assert .; echo "rc=$?"
uv run --locked --project ci/release-plan --python '>=3.12' python3 ci/release-plan/release_plan.py --event-name push . | sed -n '/console/p'
pnpm -C ts install --frozen-lockfile; echo "rc=$?"
git status --porcelain -- ts/pnpm-lock.yaml
moon run ts:fmt; echo "rc=$?"
```
Expected: `--assert` `rc=0`. The runtime prints `skip_iam-console=false`, `version_iam-console=0.1.0`, `skip_gateway-console=false`, `version_gateway-console=0.1.0`. `pnpm install` `rc=0`, and `git status` prints nothing for the lockfile. `ts:fmt` `rc=0`. If `ts:fmt` fails on a changelog, run `pnpm -C ts exec prettier --write apps/iam-console/CHANGELOG.md apps/gateway-console/CHANGELOG.md`, read the diff, and run `moon run ts:fmt` again.

- [ ] **Step 5: Commit**

```bash
git add ts/apps/iam-console/package.json ts/apps/gateway-console/package.json ts/apps/iam-console/CHANGELOG.md ts/apps/gateway-console/CHANGELOG.md
git commit -m "$(cat <<'EOF'
feat(ts): version both console apps at 0.1.0 (SMA-688)

The package.json version is the console image version source. Each
console gets a changelog with its 0.1.0 section, which release_plan.py
--assert requires for a bumped chain. The lockfile does not change.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 9: Pin the chart tags and add `repo:helm-render` row 8

**Files:**
- Modify: `charts/paigasus/values.yaml` (`:15-17`, `:24-26`, `:45-47`, `:56-58`)
- Modify: `ci/helm-render/helm_render.py` (docstring `:3`; imports `:19-30`; constants after `:54`; `EXPECTED_ROW_LABELS` `:91-113`; `check3` `:446-461`; new row functions after `check7` `:620`; `run_checks` `:655-677`; `self_test` `:822-1020`)
- Modify: `moon.yml` (`repo:helm-render` inputs `:1001-1009`)
- Modify: `ci/affected-graph/ci_targets.py` (`SELF_TASK_EXPECTED_GLOBS["helm-render"]` `:385-400`)
- Modify: `charts/paigasus/tests/golden/iam-only.yaml`, `charts/paigasus/tests/golden/iam-and-gateway.yaml`
- Test: `bash ci/helm-render/run.sh --self-test`, `--negative-control`, the real run; `ci_targets.py`.

**Interfaces:**
- Consumes: `ci/images/chains.toml` (Task 1); console versions `0.1.0` (Task 8); service versions `0.1.0`.
- Produces: rows `8a default-image-tags` and `8b default-image-render`; functions `chain_registry(path=CHAINS_TOML)`, `chain_version(entry, root=REPO_ROOT)`, `image_blocks(values)`, `check8a(values, registry, versions)`, `check8b(values, docs, stub_values=STUB_VALUES)`; constant `CLEARED_TAGS`.

- [ ] **Step 1: Measure the cleared-tag render, then write the failing tests**

Row 3b depends on `--set <key>=` giving an EMPTY string, which `default` then replaces with `appVersion`. Measure it on the current chart:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" PROTO_REPORTER=text
helm template paigasus charts/paigasus --kube-version 1.31.0 \
  --set ingress.host=console.example.test --set ingress.tlsSecretName=console-tls \
  --set oidc.issuer=https://idp.example.test/realms/paigasus --set oidc.clientId=paigasus-console \
  --set oidc.existingSecret=s --set postgres.existingSecret=s \
  --set zones.iam.backend.apiKeysPepperSecret=p \
  --set zones.iam.console.image.tag=x1 --set zones.iam.console.image.tag= | grep 'image:'
```
Expected: `image: "ghcr.io/smk1085/paigasus-iam-console:0.0.0"` — the later empty `--set` wins and falls back to `appVersion`. If it prints `:x1` or an empty tag, stop: row 3b's design does not hold, and the spec needs a new decision.

In `helm_render.py` `self_test()`, directly before `# ---- the exit-code contract` (`:968`), add:

```python
    # ---- row 8 (SMA-688 D7): the default image tags track the image versions
    reg = {
        "iam": {"kind": "cargo", "version_file": "rs/x/Cargo.toml", "ghcr": "repo/iam"},
        "iam-console": {"kind": "npm", "version_file": "ts/x/package.json", "ghcr": "repo/iam-console"},
        "gateway-console": {"kind": "npm", "version_file": "ts/y/package.json", "ghcr": "repo/gateway-console"},
    }
    vers = {"iam": "0.1.0", "iam-console": "0.1.0", "gateway-console": "0.2.0"}

    def vals(iam="0.1.0", iam_console="0.1.0", gateway_console="0.2.0", extra=None):
        v = {"zones": {
            "iam": {"console": {"image": {"repository": "repo/iam-console", "tag": iam_console}},
                    "backend": {"image": {"repository": "repo/iam", "tag": iam}}},
            "gateway": {"console": {"image": {"repository": "repo/gateway-console", "tag": gateway_console}}},
        }}
        if extra:
            v["zones"]["gateway"]["backend"] = {"image": {"repository": extra, "tag": "0.1.0"}}
        return v

    r8a, r8b = ("8a default-image-tags",), ("8b default-image-render",)
    expect("check8a good", [check8a(vals(), reg, vers)], passing=r8a)
    expect("check8a a wrong tag", [check8a(vals(iam_console="0.0.9"), reg, vers)], fail=r8a)
    expect("check8a an empty tag", [check8a(vals(iam=""), reg, vers)], fail=r8a)
    # The tag EQUALS the version here, so only the 0.0.0 rule can red this row.
    expect("check8a a 0.0.0 version", [check8a(vals(iam="0.0.0"), reg, {**vers, "iam": "0.0.0"})], fail=r8a)
    expect("check8a an unknown repository", [check8a(vals(extra="repo/billing"), reg, vers)], fail=r8a)
    expect("check8a a chain with no image block",
           [check8a(vals(), {**reg, "gateway": {"kind": "cargo", "version_file": "g", "ghcr": "repo/gateway"}},
                    {**vers, "gateway": "0.1.0"})], fail=r8a)
    # `repo/iam` is a string prefix of `repo/iam-console`. A second block for the SAME repository
    # must red; a prefix match would also count iam-console's block as iam's.
    expect("check8a two image blocks for one chain", [check8a(vals(extra="repo/iam"), reg, vers)], fail=r8a)
    rendered = synthetic(both, tags={"iam": "0.1.0", "gateway": "0.2.0"}, backend_tag="0.1.0")
    expect("check8b good", [check8b(vals(), rendered)], passing=r8b)
    expect("check8b a rendered image with the wrong tag",
           [check8b(vals(), synthetic(both, tags={"iam": "0.0.0", "gateway": "0.2.0"}, backend_tag="0.1.0"))], fail=r8b)
    expect("check8b STUB_VALUES sets an image key",
           [check8b(vals(), rendered, stub_values=(*STUB_VALUES, ("zones.iam.console.image.tag", "x")))], fail=r8b)
    expect("check8b only two images rendered",
           [check8b(vals(), synthetic(iam_only, tags={"iam": "0.1.0"}, backend_tag="0.1.0"))], fail=r8b)
    with tempfile.TemporaryDirectory(prefix="helm-render-8-") as tmp:
        root = Path(tmp)
        (root / "Cargo.toml").write_text('[package]\nname = "x"\nversion = "0.3.0"\n')
        (root / "package.json").write_text('{"name": "x", "version": "0.4.0"}\n')
        (root / "bad.json").write_text('{"name": "x"}\n')
        (root / "chains.toml").write_text("[other]\nx = 1\n")
        if chain_version({"kind": "cargo", "version_file": "Cargo.toml"}, root) != "0.3.0":
            failures.append("chain_version: the cargo reader did not return 0.3.0")
        if chain_version({"kind": "npm", "version_file": "package.json"}, root) != "0.4.0":
            failures.append("chain_version: the npm reader did not return 0.4.0")
        expect_infra("chain_version: a package.json with no version",
                     lambda: chain_version({"kind": "npm", "version_file": "bad.json"}, root))
        expect_infra("chain_version: a missing version file",
                     lambda: chain_version({"kind": "cargo", "version_file": "none.toml"}, root))
        expect_infra("chain_registry: no [chain] table", lambda: chain_registry(root / "chains.toml"))
```

Change the arity check (`:1002-1003`) to 23:

```python
    if len(EXPECTED_ROW_LABELS) != 23:
        failures.append(f"EXPECTED_ROW_LABELS: expected 23 labels, got {len(EXPECTED_ROW_LABELS)}")
```

- [ ] **Step 2: Run it, expect FAIL**

Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" PROTO_REPORTER=text
bash ci/helm-render/run.sh --self-test; echo "rc=$?"
```
Expected: a traceback with `NameError: name 'check8a' is not defined`, and the gate reports `helm-render self-test: FAILED (rc=2)` (a traceback exits 1, which `module_rc` maps to 2).

- [ ] **Step 3: Implement**

`charts/paigasus/values.yaml`. Replace the four `tag:` lines. The iam console (`:17`):

```yaml
        # Pinned to the image version in ts/apps/iam-console/package.json (SMA-688). A version
        # bump updates this tag in the same PR; repo:helm-render row 8a fails otherwise. An empty
        # tag falls back to .Chart.AppVersion, which names no published image.
        tag: "0.1.0"
```

The iam backend (`:26`):

```yaml
        # Pinned to the image version in rs/crates/services/paigasus-iam/Cargo.toml (SMA-688).
        # repo:helm-render row 8a fails when the two differ.
        tag: "0.1.0"
```

The gateway console (`:47`):

```yaml
        # Pinned to the image version in ts/apps/gateway-console/package.json (SMA-688).
        # repo:helm-render row 8a fails when the two differ.
        tag: "0.1.0"
```

The gateway backend (`:58`):

```yaml
        # Pinned to the image version in rs/crates/services/paigasus-gateway/Cargo.toml
        # (SMA-688). This chart does not render it today; the pin stops a later chart that
        # deploys it from falling back to 0.0.0. repo:helm-render row 8a fails when the two differ.
        tag: "0.1.0"
```

`helm_render.py`. Change the docstring's first line (`:3`) to `"""repo:helm-render — checks 1, 1a, 2, 3, 4, 7 and 8 over `helm template` renders of a chart.`. Add `import tomllib` after `import tempfile` (`:27`). After `KIND_VALUES` (`:54`), add:

```python
# Row 8 (SMA-688 D7). ci/images/chains.toml names each chain's GHCR repository and version file
# (spec D10). values.yaml pins one tag on each image block. A version bump that forgets the chart
# would otherwise make a default tag name an image that does not exist: ImagePullBackOff.
CHAINS_TOML = REPO_ROOT / "ci" / "images" / "chains.toml"
# How many distinct images the iam+gateway render holds: the iam console, the iam backend and the
# gateway console. The gateway backend is never rendered (spec § 1).
RENDERED_IMAGES = 3
```

After `CHECK3_EXPECT` (`:80`), add:

```python
# Row 3b (SMA-688 § 7.3). With explicit tags an appVersion bump changes no pod template, so row 3b
# renders BOTH sides of the bump with every image tag cleared. `--set <key>=` sets the empty
# string, and the templates' `default .Chart.AppVersion` then applies (measured, Task 9 Step 1).
CLEARED_TAGS = tuple(
    arg
    for key in ("zones.iam.console.image.tag", "zones.iam.backend.image.tag",
                "zones.gateway.console.image.tag", "zones.gateway.backend.image.tag")
    for arg in ("--set", f"{key}=")
)
```

In `EXPECTED_ROW_LABELS`, add after `"7 kind-values",` (`:112`):

```python
    "8a default-image-tags",
    "8b default-image-render",
```

Replace the 3b part of `check3` (`:455-459`) with:

```python
    # Case b edits Chart.yaml in a temp COPY only. run.sh exports TMPDIR, so the copy lands under
    # the gate's own mktemp directory. SMA-688: both sides render with every tag cleared, so the
    # row still proves the appVersion fallback now that values.yaml pins each tag.
    with tempfile.TemporaryDirectory(prefix="helm-render-3b-") as tmp:
        bumped = _bumped_app_version(chart, Path(tmp) / "chart")
        before = parse_docs(helm_template(chart, both, CLEARED_TAGS))
        rows.append(compare_templates("3b", before, parse_docs(helm_template(bumped, both, CLEARED_TAGS))))
```

After `check7` (`:620`), add row 8:

```python
# --------------------------------------------------------------------------- check 8


def chain_registry(path=CHAINS_TOML):
    """key -> entry, from the chain registry. An unreadable registry is rc 2."""
    try:
        data = tomllib.loads(Path(path).read_text(encoding="utf-8"))
    except (OSError, UnicodeDecodeError, tomllib.TOMLDecodeError) as exc:
        raise InfraError(f"cannot read the chain registry {path}: {exc}") from exc
    chains = data.get("chain")
    if not isinstance(chains, dict) or not chains:
        raise InfraError(f"{path} has no [chain.<key>] table")
    for key, entry in chains.items():
        if not isinstance(entry, dict) or not all(
                isinstance(entry.get(f), str) and entry.get(f) for f in ("kind", "version_file", "ghcr")):
            raise InfraError(f"{path}: [chain.{key}] needs string kind, version_file and ghcr values")
    return chains


def chain_version(entry, root=REPO_ROOT):
    """The version in a chain's version file: `[package] version` (cargo) or the top-level
    "version" (npm). An unreadable file or a missing version is rc 2: a source file this module
    cannot parse."""
    path = Path(root) / entry["version_file"]
    try:
        text = path.read_text(encoding="utf-8")
        if entry["kind"] == "cargo":
            version = (tomllib.loads(text).get("package") or {}).get("version")
        elif entry["kind"] == "npm":
            doc = json.loads(text)
            version = doc.get("version") if isinstance(doc, dict) else None
        else:
            raise InfraError(f"{path}: unknown chain kind {entry['kind']!r}")
    except (OSError, UnicodeDecodeError, tomllib.TOMLDecodeError, ValueError) as exc:
        raise InfraError(f"cannot read the version in {path}: {exc}") from exc
    if not isinstance(version, str):
        raise InfraError(f"{path} carries no string version")
    return version


def image_blocks(values, path="$"):
    """Every mapping under an `image` key of a values tree: (dotted path, repository, tag)."""
    out = []
    if isinstance(values, dict):
        for key, value in values.items():
            here = f"{path}.{key}"
            if key == "image" and isinstance(value, dict):
                out.append((here, value.get("repository"), value.get("tag")))
            else:
                out += image_blocks(value, here)
    elif isinstance(values, list):
        for i, value in enumerate(values):
            out += image_blocks(value, f"{path}[{i}]")
    return out


def check8a(values, registry, versions):
    """Row 8a: each chain has exactly ONE image block, and its tag equals the chain's version."""

    def body():
        problems = []
        blocks = image_blocks(values)
        known = {entry["ghcr"] for entry in registry.values()}
        for where, repo, _tag in blocks:
            if repo not in known:
                problems.append(f"{where}: repository {repo!r} is named by no chain in ci/images/chains.toml")
        for key, entry in registry.items():
            # Whole-string equality. `repo/iam` is a prefix of `repo/iam-console`.
            mine = [b for b in blocks if b[1] == entry["ghcr"]]
            if len(mine) != 1:
                problems.append(f"chain {key}: {len(mine)} image blocks name {entry['ghcr']}, expected exactly one")
                continue
            where, _repo, tag = mine[0]
            version = versions[key]
            if version == "0.0.0":
                problems.append(f"chain {key}: {entry['version_file']} is at 0.0.0, which is never released, so no image carries that tag")
            elif not tag:
                problems.append(f"{where}.tag is empty. It falls back to the chart appVersion, not to {version}")
            elif tag != version:
                problems.append(f"{where}.tag is {tag!r}, but {entry['version_file']} is at {version!r}. Update the tag in the same PR as the version bump")
        return problems

    return _row("8a default-image-tags", body)


def check8b(values, docs, stub_values=STUB_VALUES):
    """Row 8b: the iam+gateway render holds RENDERED_IMAGES images, and each one equals
    <repository>:<tag> of its values.yaml block. STUB_VALUES must set no image key, or the row
    would read the stub, not the default."""

    def body():
        problems = []
        leaked = [k for k, _v in stub_values if ".image." in f".{k}."]
        if leaked:
            problems.append(f"STUB_VALUES sets {leaked}; row 8b must render the values.yaml defaults")
        tags = {repo: tag for _where, repo, tag in image_blocks(values)}
        images = set()
        for dep in _of_kind(docs, "Deployment"):
            pod = _get(dep, "spec", "template", "spec")
            for container in (pod.get("containers") or []) + (pod.get("initContainers") or []):
                images.add(str(container.get("image")))
        if len(images) != RENDERED_IMAGES:
            problems.append(f"the render holds {len(images)} distinct images {sorted(images)}, expected {RENDERED_IMAGES}")
        for image in sorted(images):
            repo, _sep, tag = image.rpartition(":")
            if repo not in tags:
                problems.append(f"{image}: no values.yaml image block names {repo}")
            elif tag != tags[repo]:
                problems.append(f"{image}: the rendered tag is {tag!r}, but the values.yaml default is {tags[repo]!r}")
        return problems

    return _row("8b default-image-render", body)
```

Replace the tail of `run_checks` (`:666-677`) with:

```python
    rows = [check1a(chart_slugs(helpers), slugs, state_ts, capability_ts)]
    both_docs = None
    for label, enabled in SUBSETS:
        raw = helm_template(chart, enabled)
        docs = parse_docs(raw)
        rows += check1(label, docs, enabled, paths, slugs)
        if enabled == ("iam",):
            rows.append(check2(docs, raw))
        rows += check4(label, docs)
        if label == "iam+gateway":
            both_docs = docs
    rows += check3(chart)
    rows.append(check7(chart))
    # Row 8 (SMA-688 D7): the chart's default image tags against the chain registry.
    if both_docs is None:
        raise InfraError("SUBSETS holds no iam+gateway render; row 8b needs it")
    values = _chart_values(chart)
    registry = chain_registry()
    rows.append(check8a(values, registry, {key: chain_version(entry) for key, entry in registry.items()}))
    rows.append(check8b(values, both_docs))
    _check_row_inventory([r.row for r in rows])
    return rows
```

Change `main`'s parser description (`:1024`) to `"repo:helm-render checks 1, 1a, 2, 3, 4, 7 and 8"`.

`moon.yml`, `repo:helm-render` `inputs:` (`:1001-1009`). Replace the list with:

```yaml
    inputs:
      - '.prototools'
      - '.proto/plugins/helm.toml'
      - 'charts/**/*'
      - 'ci/helm-render/**/*'
      # SMA-688: row 8a reads the chain registry and each chain's version file. Without these
      # five, a version bump that forgets the chart tag serves a cached PASS.
      - 'ci/images/chains.toml'
      - 'ci/kind/values/**/*'
      - 'contracts/proto/paigasus/common/v1/service_info.proto'
      - 'rs/crates/services/paigasus-gateway/Cargo.toml'
      - 'rs/crates/services/paigasus-iam/Cargo.toml'
      - 'ts/apps/gateway-console/package.json'
      - 'ts/apps/iam-console/package.json'
      - 'ts/packages/paigasus-discovery/src/core/state.ts'
      - 'ts/packages/paigasus-proto/src/capability.ts'
```

and add one sentence to the task's `# INPUTS.` comment (`:986-989`): `ci/images/chains.toml and the four version files are what row 8a reads (SMA-688).`

`ci/affected-graph/ci_targets.py`, replace the `"helm-render"` entry of `SELF_TASK_EXPECTED_GLOBS` (`:385-400`) with:

```python
    # SMA-513 PR 2b. Globs first, then literals, in check_gate_inputs' comparison order (globs
    # sorted, then files sorted — `.proto/plugins/helm.toml` sorts before `.prototools` because
    # `/` < `t`). The two helm files are the byte pin the golden files depend on: drop them and a
    # helm bump serves a cached PASS. The proto and the two TypeScript files are what check 1a
    # reads; drop one and a new capability slug or a changed derivation line no longer re-keys it.
    # SMA-688: the chain registry and the four version files are what row 8a reads.
    "helm-render": (
        "charts/**/*",
        "ci/helm-render/**/*",
        # SMA-513 PR 3: row 7 renders the kind job's values files.
        "ci/kind/values/**/*",
        ".proto/plugins/helm.toml",
        ".prototools",
        "ci/images/chains.toml",
        "contracts/proto/paigasus/common/v1/service_info.proto",
        "rs/crates/services/paigasus-gateway/Cargo.toml",
        "rs/crates/services/paigasus-iam/Cargo.toml",
        "ts/apps/gateway-console/package.json",
        "ts/apps/iam-console/package.json",
        "ts/packages/paigasus-discovery/src/core/state.ts",
        "ts/packages/paigasus-proto/src/capability.ts",
    ),
```

Re-baseline the golden files and check the diff:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" PROTO_REPORTER=text
/bin/bash charts/paigasus/tests/render.sh --update
git diff --stat -- charts/paigasus/tests/golden
git diff -U0 -- charts/paigasus/tests/golden | grep '^[-+] '
```
Expected: two files changed; exactly ten `-`/`+` lines, each of the form `image: "ghcr.io/smk1085/paigasus-<name>:0.0.0"` or `…:0.1.0"`: two pairs in `iam-only.yaml`, three pairs in `iam-and-gateway.yaml`. Any other changed line is a defect: stop and diagnose (spec § 7.5).

- [ ] **Step 4: Run, expect PASS**

Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" PROTO_REPORTER=text
/bin/bash ci/helm-render/run.sh --self-test; echo "rc=$?"
/bin/bash ci/helm-render/run.sh --negative-control; echo "rc=$?"
/bin/bash ci/helm-render/run.sh; echo "rc=$?"
python3 ci/affected-graph/ci_targets.py --self-test; echo "rc=$?"
python3 ci/affected-graph/ci_targets.py; echo "rc=$?"
uv run --locked --project py ruff check --config py/pyproject.toml ci/helm-render/ ci/affected-graph/
```
Expected: the self-test prints `== helm_render.py self-test passed ==` and `== helm-render self-test passed ==`, `rc=0`. The negative control prints six `negative-control OK` lines and `rc=0`. The real run prints `PASS  [8a default-image-tags]`, `PASS  [8b default-image-render]`, `PASS  [3b]`, `== helm-render: all 23 rows passed ==`, seven `PASS  [6 …]` lines and `rc=0`. Both `ci_targets.py` runs give `rc=0` (the second one reads `moon query`; it proves the new inputs equal the twin). ruff prints `All checks passed!`.

The delete-the-feature check for row 8a on the real chart:
```bash
sed -i.bak 's/^        tag: "0.1.0"$/        tag: "0.0.9"/' charts/paigasus/values.yaml
/bin/bash ci/helm-render/run.sh 2>&1 | sed -n '/\[8a/p'
mv charts/paigasus/values.yaml.bak charts/paigasus/values.yaml
touch charts/paigasus/values.yaml
git diff --stat -- charts/paigasus/values.yaml
```
Expected: `FAIL  [8a default-image-tags]: …tag is '0.0.9', but … is at '0.1.0'…`, then the restore leaves the same diff as before the check.

- [ ] **Step 5: Commit**

```bash
git add charts/paigasus/values.yaml ci/helm-render/helm_render.py moon.yml ci/affected-graph/ci_targets.py charts/paigasus/tests/golden/iam-only.yaml charts/paigasus/tests/golden/iam-and-gateway.yaml
git commit -m "$(cat <<'EOF'
feat(repo): pin the chart image tags to the image versions (SMA-688)

values.yaml sets tag 0.1.0 on all four images, so a default install
pulls published images. repo:helm-render row 8a holds each tag equal to
the version file that chains.toml names, and row 8b checks the three
rendered images. Row 3b now renders both sides of the appVersion bump
with the tags cleared. The helm-render task keys on the registry and
the four version files, with its SELF_TASK_EXPECTED_GLOBS twin. The
golden files change on the image lines only.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 10: Documentation

**Files:**
- Modify: `.github/CLAUDE.md` (the chain bullet at "Each service image releases through **its own chain**" and the bullet "A service version is set **by hand**")
- Modify: `charts/CLAUDE.md` (append one bullet)
- Modify: `ci/release-plan/README.md` (`## Modes` `:91-146`; `## The --github-output arm …` `:148-163`)
- Modify: `ci/helm-render/README.md` (`## Checks` table `:28-41`)
- Modify: `charts/paigasus/README.md` (new section before `## The golden files` `:137`)
- Modify: `docs/ops/RUNBOOK-chart.md` (`:14-15`)
- Modify: `docs/ops/RUNBOOK-containers.md` (`### If the release plan itself cannot be read` `:614-621`; new `## Release a console image` at the end)
- Test: `ts:fmt` is not involved (no file under `ts/`); `repo:actionlint` check 12 (no new mention of the moon CI report file).

**Interfaces:**
- Consumes: the behaviour of Tasks 1-9, and one measurement in Step 1.
- Produces: text only.

- [ ] **Step 1: Measure the Docker Hub read of a missing repository**

Spec § 8 says the plan measures which step fails first when the Docker Hub repository does not exist. The `decide` step logs in, then reads `${HUB_IMAGE}:${VERSION}` through `digest_or_none` (`release.yml:1349-1365`, `:1413-1415`), which accepts only `MANIFEST_UNKNOWN`, `NAME_UNKNOWN` or `unexpected status code 404` as "absent". Measure the anonymous answer for a repository that does not exist:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" PROTO_REPORTER=text
crane digest docker.io/smaschek/paigasus-sma688-missing-probe:0.1.0; echo "rc=$?"
```
Record the output. Read it this way:
- It holds `UNAUTHORIZED` (a 401): the anonymous read cannot tell "missing" from "private". The existing runbook text (`RUNBOOK-containers.md:529-532`) already says the step treats a 401 as fatal. Write the runbook text of Step 2 with the "decide step fails" wording.
- It holds `NAME_UNKNOWN`, `MANIFEST_UNKNOWN` or `404`: `digest_or_none` reads "absent", the job takes the `push-new` path, pushes the two `:<sha>-<arch>` tags and the `:<sha>` index to GHCR, and fails at the first Docker Hub write (`docker buildx imagetools create --tag "${HUB_IMAGE}:${GITHUB_SHA}"`, `release.yml:1594`). Write the runbook text with the "fails in the Docker Hub copy step, after the GHCR `:<sha>` tags" wording.

The authenticated answer (the job logs in first) is not measured here, because no Docker Hub token is available locally. Say so in the runbook text.

- [ ] **Step 2: Write the text**

`.github/CLAUDE.md`, the chain bullet. Replace its first two sentences (from `- Each service image releases through **its own chain**` to `…for \`iam\` and \`gateway\`.`) with:

```markdown
- Each image releases through **its own chain** in `release.yml`: `images-build-<key>` →
  `approve-images-<key>` → `publish-images-<key>` → `tag-<key>`, for `iam`, `gateway`,
  `iam-console` and `gateway-console` (SMA-688). `ci/images/chains.toml` is the one registry of
  the chain keys, their kind, version file, changelog and image names; `release_plan.py`,
  `release_decision.py`, `helm_render.py` and `release_guard.py` read it, and V16 asserts that
  `release.yml` and `CHAIN_APPROVALS` agree with it. `iam` is a string prefix of `iam-console`:
  a chain selects its jobs and artifacts by exact name, never by a prefix or a glob (V17).
```

`.github/CLAUDE.md`, the version bullet. Replace its first sentence (`- A service version is set **by hand**, in a normal pull request, with a \`CHANGELOG.md\` section.`) with:

```markdown
- A service or console version is set **by hand**, in a normal pull request, with a
  `CHANGELOG.md` section, and update the tag in `charts/paigasus/values.yaml`; row 8 fails
  otherwise. A console's version source is the `version` field of `ts/apps/<app>/package.json`
  (SMA-688 D1).
```

`charts/CLAUDE.md`, append:

```markdown
- **Each default image tag is pinned to its image version.** `repo:helm-render` row 8a compares
  every `image.tag` in `values.yaml` with the version file that `ci/images/chains.toml` names
  (SMA-688). A version bump updates the tag in the same PR. An empty tag falls back to
  `appVersion` (`0.0.0`), which names no published image.
```

`ci/helm-render/README.md`, `## Checks` table. Change the `3a…3c` row's last cell to `\`spec.template\` of a Deployment differs, stays equal or survives against the table in spec § 5 check 3. Row \`3b\` renders both sides of the \`appVersion\` bump with every \`image.tag\` cleared (SMA-688): the "bump \`appVersion\` to roll every pod" contract holds only when the tags are empty`, and add two rows after the `7 kind-values` row:

```markdown
| `8a default-image-tags` | The default tags track the image versions (SMA-688) | For a chain of `ci/images/chains.toml`, `values.yaml` does not hold exactly one `image` block with its `ghcr` repository, or that block's `tag` is empty or differs from the version in the chain's version file, or that version is `0.0.0`; or an `image` block names a repository that no chain names |
| `8b default-image-render` | The rendered images use those tags (SMA-688) | `STUB_VALUES` sets an `image` key, or the `iam+gateway` render does not hold exactly three images, each equal to `<repository>:<tag>` of its `values.yaml` block |
```

`charts/paigasus/README.md`, add before `## The golden files`:

```markdown
## The default image tags

Each `image.tag` in `values.yaml` is pinned to the published version of that image. The version
lives in the file that `ci/images/chains.toml` names for the image: a service's `Cargo.toml` or a
console's `package.json`. `repo:helm-render` row 8a fails when a tag and its version differ, so a
version bump must update the tag in the same pull request. The tags no longer default to
`appVersion`. An empty tag still falls back to `.Chart.AppVersion`, which is `0.0.0` and names no
published image.
```

`docs/ops/RUNBOOK-chart.md:14-15`. Replace the two image rows with:

```markdown
| `zones.<id>.console.image.{repository,tag}` | — | The console image. `tag` is pinned to the published console version (for example `0.1.0`). An empty `tag` falls back to the chart `appVersion` (`0.0.0`), which names no published image |
| `zones.iam.backend.image.{repository,tag}` | — | The IAM image. `tag` is pinned to the published IAM version, like the console tags |
```

`ci/release-plan/README.md`, `## Modes`. After the `--self-test` bullet, add a bullet:

```markdown
- `--keys` — prints the chain keys of `ci/images/chains.toml`, one on each line, in file order,
  and exits `0` (SMA-688). A registry that cannot be read prints no key and exits `3`. The
  `--github-output` arm reads this list to name the outputs it writes. `run.sh` has no `--keys`
  mode; call `release_plan.py --keys <repo_root>` directly.
```

In the `--negative-control` bullet, change `eight rows` to `thirteen rows`; change the row-5 sentence `and asserts the wrapper exits \`0\` and writes exactly one matching verdict line` to `and asserts the wrapper exits \`0\` and writes exactly one line for the verdict and for each of the eight chain outputs (\`^<key>=\`, with the \`=\`: \`skip_iam\` is a string prefix of \`skip_iam-console\`)`; change `holding symlinks to \`bash\`, \`dirname\`, \`grep\` and \`tail\` and nothing else` to `holding symlinks to \`bash\`, \`dirname\`, \`grep\`, \`sed\` and \`tail\` and nothing else`; change the row-6 sentence `and then asserts the wrapper still exits \`0\` and writes \`nothing_to_release=false\`` to `and then asserts the wrapper still exits \`0\` and writes all nine outputs fail-safe: without \`uv\` it cannot run \`--keys\`, so it reads the keys from \`ci/images/chains.toml\` with \`sed\` (SMA-688)`; and append after the row-8 text:

```markdown
  **Row 9** replaces the checker in a copy of this directory with a stub that names four keys and
  prints one of them, and asserts the fail-safe branch writes all nine outputs. **Rows 10 to 13**
  use a stub whose `--keys` answer the wrapper must not trust. Its decision run prints a complete
  decision with every `skip_` set to `true`, so a wrapper that trusts it fails the row. In row 10
  the answer is empty, and in row 11 it holds `IAM_X`. Both rows copy the real `chains.toml`, and
  assert the sed read: exit `0` and the nine fail-safe lines. In rows 12 and 13 `--keys` fails,
  and `chains.toml` is missing (row 12) or empty (row 13). They assert exit `2` with
  `nothing_to_release=false` as the only line.
```

In the `--github-output` bullet, change `it is the one mode that never fails its caller` to `it is the one mode that fails its caller only when no source names a chain key`.

Replace the section `## The \`--github-output\` arm inverts the usual contract, deliberately` body (`:150-163`) with:

```markdown
Every other mode follows this repo's usual three exit codes: `0` pass, `1` the repo is wrong,
`2` infrastructure failed. `--github-output` exits `0` on every run that can name the chain
keys.

A failed `plan` job would **skip** its dependents rather than build them — GitHub applies an
implicit `success()` to a job-level `if:` with no status function named — so a broken decision
that exited non-zero would stop the release entirely rather than fail safe. `--github-output`
therefore catches every failure mode of the checker's decision, writes
`nothing_to_release=false`, `skip_<key>=false` and an empty `version_<key>` for every key to
`$GITHUB_OUTPUT`, prints a `::warning::` annotation naming the failure, and exits `0`. The build
proceeds; nothing is silently skipped.

**The chain keys (SMA-688, spec § 4.3).** The keys come from `release_plan.py --keys`. When that
call fails, prints no key, or prints a line outside `[a-z][a-z0-9-]*`, the arm reads the keys from
`ci/images/chains.toml` with `sed`, which needs no toolchain. It tests each key against the same
shape. It then takes the fail-safe branch above for every key, without a run of the decision,
and exits `0`. A missing `uv` on the runner is one cause, so the SMA-603 C1 contract stays true.
`--assert` fails a pull request when this `sed` read and tomllib find different keys (for
example, a chain header with a trailing comment), so the fallback always names every chain.

**One exit `2`.** When the `sed` read also finds no key, `chains.toml` is missing, unreadable or
empty. The arm then cannot name the chain outputs, and a chain output that nobody writes runs that
chain with an empty version. So the arm writes `nothing_to_release=false` only, prints an
`::error::` annotation, and exits `2`. The `plan` job fails, and every chain and the kernel
release skip until a fix.

`--self-test`, `--negative-control`, and `--assert` keep the normal contract. CI runs those
three as the actual gate; `--github-output` is exercised by the negative control's rows 5, 6
and 9 to 13.
```

`docs/ops/RUNBOOK-containers.md`, replace the body of `### If the release plan itself cannot be read` (`:616-621`) with:

```markdown
`ci/release-plan/run.sh` has two failure paths (SMA-688).

- **The decision or `--keys` cannot be read, but the registry can.** When `release_plan.py
  --keys` fails (for example, `uv` is missing), the wrapper reads the keys from
  `ci/images/chains.toml` with `sed`, and the `plan` log shows a `::warning::release-plan --keys
  gave no usable key list` line. In both cases the fail-safe branch writes
  `skip_<key>=false` for every key of `ci/images/chains.toml` (so every chain RUNS — the fail-safe
  direction) and writes each `version_<key>` as an explicit EMPTY string. Every chain job that got
  past its gate then hard-fails at the label compare (`the archive carries version , but plan says
  .`), because `plan`'s version output is empty. Read that specific failure as "the release plan
  could not be read" — check the `plan` job's own log — not as a build problem in
  `images-build-<key>`.
- **No source names a key.** `release_plan.py --keys` failed or printed no usable key, and the
  `sed` read of `ci/images/chains.toml` found no key either: the file is missing, unreadable or
  empty. The wrapper cannot name the chain outputs. It writes `nothing_to_release=false`, prints
  `::error::release-plan could not name the chain keys`, and fails the `plan` job. Every chain and
  the kernel release then skip. Fix `ci/images/chains.toml`, and re-run the workflow.
```

Append at the end of the file:

```markdown
## Release a console image

A maintainer sets a console version by hand, in the `version` field of
`ts/apps/<app>/package.json`. release-plz does not process the console apps (SMA-688, spec D1).
The chain key is `iam-console` or `gateway-console`, and the release name is `paigasus-<key>`.

Before you release a console for the first time, create the public Docker Hub repository
`smaschek/paigasus-<key>`. The `release-images` and `release-approval` environments and the
`DOCKERHUB_TOKEN` need no change.

1. Open a pull request. In it:
   - set `version` in `ts/apps/<app>/package.json`;
   - add a `## [<version>] - <date>` section to `ts/apps/<app>/CHANGELOG.md`;
   - set the same version as `tag` of `zones.<zone>.console.image` in
     `charts/paigasus/values.yaml`, and re-baseline the golden files with
     `charts/paigasus/tests/render.sh --update`.
   `repo:actionlint` check 11 fails the pull request when the changelog section is missing.
   `repo:helm-render` row 8a fails it when the chart tag differs from the version.
2. Merge it. The `plan` job selects the console, because its version has no tag. From the merge
   until the publish, the chart default names a tag that is not yet published.
3. Approve the newest pending run. Reject older pending runs, if any. One approval releases every
   chain that waits in the same run, because all approval jobs use the `release-approval`
   environment.
4. `publish-images-<key>` pushes, copies, signs and attests the image, exactly as for a service.
   `tag-<key>` then makes `paigasus-<key>-v<version>`.
5. After the first push, set the GHCR package `paigasus-<key>` to public and link it to the
   repository.
6. Check each default chart image with an anonymous pull:
   `crane manifest ghcr.io/smk1085/paigasus-<key>:<version>`.
```

Then append exactly ONE of the two paragraphs below, the one that matches the Step 1 measurement.

Variant A, when Step 1 printed `UNAUTHORIZED`:

```markdown
If the Docker Hub repository does not exist when you approve, the `decide` step of
`publish-images-<key>` fails before any registry write. The step reads Docker Hub after its
login and treats every answer except "no such tag" as fatal. An anonymous read of a missing
repository answers 401 (measured for SMA-688). Create the repository and re-run the failed job.
The authenticated answer was not measured.
```

Variant B, when Step 1 printed `NAME_UNKNOWN`, `MANIFEST_UNKNOWN` or `404`:

```markdown
If the Docker Hub repository does not exist when you approve, the job pushes the `:<sha>` tags to
GHCR and then fails at the Docker Hub copy. No `:<version>` tag exists yet in either registry.
Create the repository and re-run the failed job. The re-run takes the `push-new` path with the
same build artifacts, so with the same digest. An anonymous read of a missing repository answers
"no such tag" (measured for SMA-688); the authenticated answer was not measured.
```

- [ ] **Step 3: Run the checks**

Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" PROTO_REPORTER=text
grep -c "the authenticated answer was not measured\|The authenticated answer was not measured" docs/ops/RUNBOOK-containers.md
grep -rln "the check-12 corpus token" .github/CLAUDE.md charts/CLAUDE.md ci/release-plan/README.md ci/helm-render/README.md charts/paigasus/README.md docs/ops/RUNBOOK-chart.md docs/ops/RUNBOOK-containers.md; echo "corpus-token-grep rc=$?"
```
Expected: the first grep prints `1` (exactly one variant was appended), and `corpus-token-grep rc=1` (no changed file names the moon CI report, so check 12 needs no marker).

- [ ] **Step 4: Read the text once more**

Read each changed paragraph against ASD-STE100: one idea for each sentence, active voice, at most 25 words for a description and 20 for an instruction. Fix any sentence that breaks the rules.

- [ ] **Step 5: Commit**

```bash
git add .github/CLAUDE.md charts/CLAUDE.md ci/release-plan/README.md ci/helm-render/README.md charts/paigasus/README.md docs/ops/RUNBOOK-chart.md docs/ops/RUNBOOK-containers.md
git commit -m "$(cat <<'EOF'
docs(docs): document the console image release (SMA-688)

The chain registry, the console version source, the keys branch of the
release plan wrapper, rows 8a and 8b and the new row 3b, the pinned
chart tags, and the console release runbook with the measured Docker
Hub behaviour.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 11: Final verification

**Files:** none changed. If a step fails, fix the cause in the task that owns the file, as a new commit.

**Interfaces:**
- Consumes: every earlier task.
- Produces: the evidence list for the PR description.

- [ ] **Step 1: The Python and self-test layer**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" PROTO_REPORTER=text
uv run --locked --project py ruff check --config py/pyproject.toml ci/
uv run --locked --project ci/release-plan --python '>=3.12' python3 ci/release-plan/release_plan.py --keys .
bash ci/release-plan/run.sh --self-test && bash ci/release-plan/run.sh --negative-control && bash ci/release-plan/run.sh --assert; echo "release-plan rc=$?"
uv run --no-project --python '>=3.12' python3 ci/images/release_decision.py --self-test; echo "decision rc=$?"
uv run --locked --project py python3 ci/actionlint/release_guard.py --self-test; echo "guard self-test rc=$?"
uv run --locked --project py python3 ci/actionlint/release_guard.py .github/workflows/release.yml; echo "guard real rc=$?"
/bin/bash ci/helm-render/run.sh --self-test && /bin/bash ci/helm-render/run.sh --negative-control && /bin/bash ci/helm-render/run.sh; echo "helm-render rc=$?"
actionlint -shellcheck= .github/workflows/release.yml .github/workflows/images.yml; echo "actionlint rc=$?"
```
Read: every `rc=` is `0`. `--keys` prints the four keys in order. ruff prints `All checks passed!`.

- [ ] **Step 2: The full gate graph, under system bash 3.2**

`repo:affected-smoke` needs `/bin/bash` 3.2, and Moon takes `bash` from `PATH`, where Homebrew 5.3.15 comes first. Put a directory that holds only a `bash` link first on `PATH`; do not put `/bin` first (it changes `python3` too).
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" PROTO_REPORTER=text
git fetch origin main
b32="$(mktemp -d)" && ln -s /bin/bash "$b32/bash"
PATH="$b32:$PATH" moon ci :build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking \
  :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free \
  :redis-connect-single-site :iam-docker-policy-single-site :error-code-single-site \
  :http-extractor-envelope :input-liveness :promtool :observability-drift \
  :nats-permissions :release-parity :release-parity-py :release-parity-ts \
  :publish-metadata :version-lockstep :workflow-credentials :pyo3-stub-drift :ruff-ci \
  :next-public-free :helm-render :test-e2e \
  --base origin/main \
  --include-relations; echo "moon ci rc=$?"
```
Read: `repo:affected-smoke`, `repo:helm-render`, `repo:input-liveness` and every build, test, lint and fmt target must pass. Under bash 3.2 these gates are EXPECTED to fail and are read in Step 3 instead: `repo:ruff-ci`, `repo:next-public-free`, `repo:publish-metadata`, `repo:version-lockstep`, `repo:nats-permissions` (a wall of `mapfile: command not found` rows, or an empty stdout with `declare: -A: invalid option`), and `repo:actionlint` (Step 4). Any other red is real: diagnose it with the root `CLAUDE.md` procedure between the `moon-diagnosis` markers. Copy the moon cache state files before any re-run (its Step 0).

- [ ] **Step 3: The bash-4+ gates, under Homebrew bash**

With the normal `PATH` (Homebrew bash 5.3.15 first):
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" PROTO_REPORTER=text
moon run repo:ruff-ci repo:next-public-free repo:publish-metadata repo:version-lockstep repo:nats-permissions --force; echo "bash4 gates rc=$?"
```
Read: `rc=0`. `repo:nats-permissions` needs Docker. If one of these hangs at 0% CPU, that is the 512-byte pipe on this Mac (root `CLAUDE.md`, SMA-612), not a finding; record it, and rely on CI for that gate.

- [ ] **Step 4: `repo:actionlint`, with the pipe preflight**

The Bash tool runs zsh, so do not read `PIPESTATUS`. Write the output to a file and take the status directly:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" PROTO_REPORTER=text
al_out="$(mktemp)"
/opt/homebrew/bin/bash ci/actionlint/run.sh > "$al_out" 2>&1; echo "actionlint rc=$?"
sed -n '/pipe capacity/p;/FAIL/p;/passed/p;/ABORT/p' "$al_out"
```
Read the preflight line first:
- `pipe capacity 65536 bytes (floor 8192)`: the full gate runs here. `rc=0` with no `FAIL` row is the local verdict.
- `rc=2` with a `small` message: this host is in the 512-byte state and has no local full-gate verdict. Then run `/bin/bash ci/actionlint/run.sh --self-test`. Its only accepted failures are the two false `cargo-lock-step` rows. Any other failing row is real. Check 10 and check 11 over the real files were already run directly in Step 1.

- [ ] **Step 5: The affectedness evidence for spec § 7.4**

Parse one target per `tasks[project][task]`, never by grepping `"target"` keys (root `CLAUDE.md`: that counts scheduled upstreams as selections).
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" PROTO_REPORTER=text
ev="$(mktemp -d)"
cat > "$ev/targets.py" <<'PY'
import json, sys
d = json.load(sys.stdin)
print("\n".join(sorted(f"{p}:{t}" for p, tasks in (d.get("tasks") or {}).items() for t in tasks)))
PY
for f in ci/images/chains.toml ts/apps/iam-console/package.json ts/apps/gateway-console/package.json \
         rs/crates/services/paigasus-iam/Cargo.toml rs/crates/services/paigasus-gateway/Cargo.toml; do
  printf '== %s\n' "$f"
  printf '%s\n' "$f" | moon query tasks --affected | python3 "$ev/targets.py" | sed -n '/^repo:helm-render$/p;/^repo:actionlint$/p'
done
git diff --name-only origin/main...HEAD > "$ev/changed.txt"
moon query tasks --affected < "$ev/changed.txt" | python3 "$ev/targets.py" > "$ev/affected.txt"
sed -n '/^repo:/p' "$ev/affected.txt"
```
If the sandbox refuses the `git diff` line, put it in a script file and run `/bin/bash <script>` (Global Constraints).
Read: each of the five files selects `repo:helm-render` and `repo:actionlint`. `ci/images/chains.toml` selects `repo:helm-render` only because of the new input, which proves the input works. The branch diff selects at least `repo:helm-render`, `repo:actionlint`, `repo:affected-smoke` and `repo:input-liveness`. Record the lists for the PR description.

- [ ] **Step 6: Record what only CI and the live run prove**

Write these into the PR description, one line each:
- `images.yml` on the PR runs the console release sequence on amd64 (spec R1). After the push, run `gh workflow run images.yml --ref feature/sma-688-console-image-release` for the arm64 leg, and read both `M8 image=paigasus-<key>` lines.
- The changed anchors on the service chains run live first at the next release (spec R3).
- The rollout of spec § 8 and the AC 2, AC 3 and AC 6 checks of spec § 9 run after the merge.

No commit in this task.
