<!-- moon-diagnosis:ok -->
<!-- The marker above is for check 12 of repo:actionlint: Task 11 names the ciReport token when it
points the implementer at CLAUDE.md's moon-diagnosis block, so this file needs the marker. The
reference is the corrected procedure — read operations[] on the task-execution entry — not the
broken action-level exitCode advice. -->

# SMA-634 — Node consumers load the kernel through its wasm entry — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development
> (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `@paigasus/kernel` loadable by every Node consumer in this monorepo through its wasm
entry, and remove the hand-written PRN grammar from `@paigasus/console-core`.

**Architecture:** The kernel's `.` export resolves to the wasm entry under every condition, and the
napi entry moves to `./napi`. `paigasus_wasm_bg.wasm` and its four glue files become committed
artifacts, written only by a new `paigasus-kernel-ts:generate-wasm` task. Four host-independent
checks hold those artifacts to the Rust source. `console-core/src/prn-tenancy.ts` becomes an IAM
tenancy adapter over the kernel.

**Tech Stack:** Moon 2.5.3, pnpm 11.3.0, node 24.16.0, vitest 5.0.1, Next 16.3.5 (Turbopack),
wasm-pack 0.15.0, rustc 1.95.0, wasm-bindgen 0.2.127, Docker BuildKit.

**Spec:** `docs/superpowers/specs/2026-09-22-sma-634-kernel-napi-consumers-design.md`
**Measurements:** `docs/superpowers/specs/2026-09-22-sma-634-measurements.md`

---

## Global Constraints

Every task's requirements include this section.

- **Worktree and branch.** Work in
  `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-634-kernel-napi`, on branch
  `feature/sma-634-kernel-napi-node-modules`. Run `git branch --show-current` first. Stop if it
  differs. Do not touch the main checkout.
- **Shell prefix.** Start every shell with
  `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"; export PROTO_REPORTER=text`. The Bash
  tool's PATH does not hold the proto-managed CLIs, and `proto` prints NDJSON in an agent session.
- **Moon targets, never a bare pnpm filter.** `pnpm --filter <pkg> test` exits 0 with no output when
  the package declares no `test` script, so a delegated run reports a green from a command that ran
  nothing. Use `moon run <project>:<task>`.
- **macOS has no `timeout`.** Start a long command in the background and poll it. Do not use a bash
  here-string over 512 bytes: a new pipe on this host holds 512 bytes and the write blocks.
- **The bash split.** `repo:affected-smoke` needs system `/bin/bash` (3.2.57). `repo:ruff-ci`,
  `repo:next-public-free` and `repo:publish-metadata` need `/opt/homebrew/bin/bash` (4+).
  `repo:actionlint` needs bash 5 and a healthy pipe; read its preflight line before you read its
  verdict. Run such a gate directly: `<bash-binary> ci/<gate>/run.sh`.
- **Commits.** Conventional, lowercase subject, workspace scope, `(SMA-634)` at the end. No body
  line may start with `Word:` — `footer-leading-blank` fails then. End with a blank line and
  `Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>`. Never `--no-verify`. **Never amend a
  commit you did not make**: add a new commit instead.
- **New files.** Every source file opens with `// SPDX-License-Identifier: Apache-2.0` (`#` for
  Python, shell and YAML). No path component may be a Windows reserved device name (`CON`, `PRN`,
  `AUX`, `NUL`, `COM1`–`COM9`, `LPT1`–`LPT9`), directories included. No new directory named
  `build/`: the root `.gitignore` ignores any such directory anywhere in the tree.
- **The five artifacts** (one build writes them together):
  `rs/crates/bindings/paigasus-wasm/paigasus_wasm_bg.wasm`, `…/paigasus_wasm_bg.js`,
  `…/paigasus_wasm.js`, `…/paigasus_wasm.d.ts`, `…/paigasus_wasm_bg.wasm.d.ts`.
- **The wasm-pack command**, from inside `rs/crates/bindings/paigasus-wasm`:
  `wasm-pack build . --target bundler --release --no-pack --out-dir <dir> --out-name paigasus_wasm -- --locked`
- **The binary bytes depend on the host** (spec F12). No check compares them. Only one host
  regenerates the artifacts for a change.
- **The pnpm refresh trap** (spec F14). pnpm hard-links a `file:` dependency. An unlink and replace
  (`git checkout`, a branch switch, a delete) breaks the link, and a plain `pnpm install` does not
  repair it. Run `rm -rf ts/node_modules && pnpm -C ts install` after such an operation.

---

## File structure

| File | Create or modify | Responsibility |
| -- | -- | -- |
| `ts/packages/paigasus-kernel/tests/wasm-probe.mjs` | create | A child-process probe. It reads a wasm module's interface, and it replays the parity corpora through a glue and binary pair, under plain `node`. |
| `ts/packages/paigasus-kernel/tests/committed-wasm.test.ts` | create | The drift gate: checks 1, 2 and 3. It spawns the probe. |
| `ts/packages/paigasus-kernel/scripts/generate-wasm.mjs` | create | The `--pre` guards and the `--post` copy of the regeneration task. |
| `ts/packages/paigasus-kernel/moon.yml` | modify | The new `generate-wasm` task; `build` stops writing the artifacts; `test` keys on them. |
| `ts/packages/paigasus-kernel/package.json` | modify | `.` becomes the wasm entry, `./napi` the napi entry. |
| `ts/packages/paigasus-kernel/vitest.config.ts`, `tsconfig.json` | modify | The alias and the condition that existed for the old map. |
| `ts/packages/paigasus-kernel/tests/{sum,uuid7,prn-canonical,prn-fields,cedar}.test.ts` | modify | The import line only: `@paigasus/kernel/napi`. |
| `rs/crates/bindings/paigasus-wasm/.gitignore` | modify | The `!/paigasus_wasm_bg.wasm` negation and the header. |
| `.gitattributes` | create | The binary attribute and the generated marks. |
| `ts/moon.yml` | modify | `packages/*/scripts/**/*` in the `sources` group. |
| `ci/affected-graph/cargo_moon_parity.py` | modify | The `ALLOW_UNLOCKED_CARGO` entry for `generate-wasm`. |
| `ts/packages/paigasus-console-core/src/prn-tenancy.ts` | modify | The IAM tenancy rule over the kernel grammar. |
| `ts/packages/paigasus-console-core/testing/installed-wasm.ts` | create | Check 4, shared by the three vitest setups. |
| `ts/packages/paigasus-console-core/testing/index.ts` | modify | It exports check 4. |
| `ts/packages/paigasus-console-core/tests/unit/prn-tenancy-delegation.test.ts` | create | It proves that the kernel does the parsing. |
| `ts/packages/paigasus-console-core/tests/unit/prn-tenancy.test.ts` | modify | The text check that bans the old grammar calls. |
| `ts/packages/{paigasus-console-core}/tests/support/setup.ts`, `ts/apps/{iam,gateway}-console/tests/support/setup.ts` | modify | They call check 4. |
| `ts/packages/paigasus-console-core/package.json`, `moon.yml` | modify | The kernel dependency and the new inputs. |
| `ts/apps/iam-console/moon.yml`, `ts/apps/gateway-console/moon.yml` | modify | The new inputs, the `.wasm` chunk check, the `dependsOn` comment. |
| `ts/Dockerfile`, `ci/images/run.sh`, `.github/workflows/images.yml` | modify | The `bindings` build context, the chunk check, the `(console)` probe, the filter line. |
| `ts/CLAUDE.md`, `.github/workflows/{prebuild,ci}.yml`, `docs/ops/RUNBOOK-containers.md`, `rs/CLAUDE.md` | modify | The records and the comment drift. |

---

## Task order, and the red-first rule

Task 1 records the red. Task 2 builds the regeneration task but commits **no** artifact, so the red
survives. Task 3 writes the gate, watches it fail on the real defect, and then commits the
artifacts that make it pass. **The branch is red between Task 3's two commits on purpose.** Both
commits belong to this PR. Nothing is fixed before its red is recorded.

---

### Task 1: Record the red — the committed wasm pair does not load

**Files:**
- Create: `ts/packages/paigasus-kernel/tests/wasm-probe.mjs`
- Read only: `rs/crates/bindings/paigasus-wasm/paigasus_wasm_bg.js`

**Interfaces:**
- Consumes: nothing.
- Produces: `tests/wasm-probe.mjs`, a CLI with two modes.
  `node tests/wasm-probe.mjs --interfaces <dir>` prints
  `{"imports": string[], "exports": string[]}` on stdout and exits 0.
  `node tests/wasm-probe.mjs --corpus <dir>` prints `{"checked": {"sum": 67, …}}` and exits 0, or
  prints the failures on stderr and exits 1. `<dir>` holds `paigasus_wasm.js`,
  `paigasus_wasm_bg.js` and `paigasus_wasm_bg.wasm`. Task 3's gate spawns both modes.

**A subagent may touch only the file above.**

- [ ] **Step 1: Write the probe**

Create `ts/packages/paigasus-kernel/tests/wasm-probe.mjs`:

```js
// SPDX-License-Identifier: Apache-2.0
//
// A child-process probe for a wasm glue and binary pair (SMA-634 spec § 5.4). It runs under plain
// `node`, NOT under vitest: the kernel's `node` vitest project has no wasm plugin and its
// `server.deps.external` rule names `.node` files only, so an import through vitest would meet
// Vite's own wasm handling instead of the loader a console server uses (spec F11).
// tests/committed-wasm.test.ts spawns it.
//
//   node tests/wasm-probe.mjs --interfaces <dir>   the module's import and export lists, as JSON
//   node tests/wasm-probe.mjs --corpus <dir>       replay all five parity corpora through <dir>
//
// <dir> holds paigasus_wasm.js, paigasus_wasm_bg.js and paigasus_wasm_bg.wasm. The glue imports
// its binary through a RELATIVE specifier, so the pair must be co-located: a mixed pair is only
// testable by copying one file over the other, which is what the negative controls do.
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';

// tests -> paigasus-kernel -> packages -> ts -> repo root: four `../`, the same walk tests/corpus.ts
// makes.
const VECTORS = new URL('../../../../rs/crates/libs/paigasus-kernel-parity/vectors/', import.meta.url);

function vectors(name) {
  const rows = JSON.parse(readFileSync(new URL(`${name}.json`, VECTORS), 'utf8'));
  if (!Array.isArray(rows) || rows.length === 0) {
    throw new Error(`the ${name} corpus is empty or not an array — a replay over it would assert nothing`);
  }
  return rows;
}

function dirUrl(argument) {
  if (argument === undefined) throw new Error('a directory argument is required');
  return pathToFileURL(`${resolve(argument)}/`);
}

function interfaces(dir) {
  const module = new WebAssembly.Module(readFileSync(new URL('paigasus_wasm_bg.wasm', dir)));
  return {
    imports: WebAssembly.Module.imports(module)
      .map((entry) => `${entry.module}.${entry.name}:${entry.kind}`)
      .sort(),
    exports: WebAssembly.Module.exports(module)
      .map((entry) => `${entry.name}:${entry.kind}`)
      .sort(),
  };
}

async function corpus(dir) {
  // A LinkError here is the defect this probe exists for: the glue and the binary are from
  // different builds (spec F13).
  const api = await import(new URL('paigasus_wasm.js', dir).href);
  const failures = [];
  const same = (label, actual, expected) => {
    if (actual !== expected) failures.push(`${label}: ${JSON.stringify(actual)} !== ${JSON.stringify(expected)}`);
  };

  const sum = vectors('sum');
  for (const row of sum) same(`sum(${row.a}, ${row.b})`, api.sum(row.a, row.b), row.expected);

  const uuid7 = vectors('uuid7');
  for (const row of uuid7) {
    same(`mintUuid7(${row.unix_ms}, ${row.rand_hex})`, api.mintUuid7(row.unix_ms, row.rand_hex), row.expected_uuid);
  }

  const canonical = vectors('prn_canonical');
  for (const row of canonical) {
    same(`prnErrorKind(${row.input})`, api.prnErrorKind(row.input), row.error_kind);
    if (row.error_kind === '') same(`prnCanonicalize(${row.input})`, api.prnCanonicalize(row.input), row.canonical);
  }

  const cedar = vectors('prn_cedar');
  for (const row of cedar) {
    same(`prnCedarEntityType(${row.prn})`, api.prnCedarEntityType(row.prn), row.entity_type);
    same(`prnCedarEntityId(${row.prn})`, api.prnCedarEntityId(row.prn), row.entity_id);
  }

  const fields = vectors('prn_fields');
  for (const row of fields) {
    same(`prnService(${row.prn})`, api.prnService(row.prn), row.service);
    same(`prnRegion(${row.prn})`, api.prnRegion(row.prn), row.region);
    same(`prnOrg(${row.prn})`, api.prnOrg(row.prn), row.org);
    same(`prnResourceType(${row.prn})`, api.prnResourceType(row.prn), row.resource_type);
    same(`prnResourceId(${row.prn})`, api.prnResourceId(row.prn), row.resource_id);
    same(
      `prnBuild(${row.prn})`,
      api.prnBuild(row.service, row.region, row.org, row.resource_type, row.resource_id),
      row.prn,
    );
  }

  if (failures.length > 0) {
    const error = new Error(`${failures.length} corpus failures`);
    error.failures = failures;
    throw error;
  }
  return {
    checked: {
      sum: sum.length,
      uuid7: uuid7.length,
      prn_canonical: canonical.length,
      prn_cedar: cedar.length,
      prn_fields: fields.length,
    },
  };
}

const [mode, target] = process.argv.slice(2);
try {
  const dir = dirUrl(target);
  const result = mode === '--interfaces' ? interfaces(dir) : mode === '--corpus' ? await corpus(dir) : null;
  if (result === null) throw new Error(`unknown mode ${JSON.stringify(mode)} — use --interfaces or --corpus`);
  process.stdout.write(`${JSON.stringify(result)}\n`);
} catch (error) {
  process.stderr.write(`${error.name}: ${error.message}\n`);
  for (const failure of error.failures ?? []) process.stderr.write(`  ${failure}\n`);
  process.exit(1);
}
```

- [ ] **Step 2: Build a fresh binary beside the committed glue, without overwriting the glue**

The `test` task builds into `.wasmpack-test-out` and never writes the crate directory. The `build`
task does write it, so **do not run `build` here**: it would repair the glue and destroy the
evidence.

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"; export PROTO_REPORTER=text
moon run paigasus-kernel-ts:test --force
cp rs/crates/bindings/paigasus-wasm/.wasmpack-test-out/paigasus_wasm_bg.wasm \
   rs/crates/bindings/paigasus-wasm/paigasus_wasm_bg.wasm
```

Expected: the task passes, and the copy writes a gitignored file, so `git status --short` stays
empty.

- [ ] **Step 3: Run the probe and record the red**

```bash
node ts/packages/paigasus-kernel/tests/wasm-probe.mjs --corpus rs/crates/bindings/paigasus-wasm
echo "rc=$?"
```

Expected: `rc=1`, and on stderr

```
LinkError: WebAssembly.Instance(): Import #0 "./paigasus_wasm_bg.js" "__wbg_Error_408e67f47ca7b58b": function import requires a callable
```

The committed glue exports `__wbg_Error_92b29b0548f8b746` (commit `0b2e346a`). Record the two
symbol names and the exit code in the PR body. If the probe instead exits 0, **stop and report**:
the glue was repaired by an earlier `build` run, and the red must be re-created from a clean
checkout of the glue (`git checkout -- rs/crates/bindings/paigasus-wasm/paigasus_wasm_bg.js`).

- [ ] **Step 4: Confirm the interface mode works**

```bash
node ts/packages/paigasus-kernel/tests/wasm-probe.mjs --interfaces rs/crates/bindings/paigasus-wasm
```

Expected: exit 0 and a JSON object with two imports and 21 exports (spec F17).

- [ ] **Step 5: Commit**

```bash
git add ts/packages/paigasus-kernel/tests/wasm-probe.mjs
git commit -m "test(ts): a node probe for a committed wasm glue and binary pair (SMA-634)

The probe reads a module's interface and replays the five parity corpora. It runs
under plain node, which is the loader a console server uses. Against the committed
glue and a current binary it exits 1 with a LinkError, which is the defect the drift
gate of this change catches.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 2: The regeneration task, `paigasus-kernel-ts:generate-wasm`

**Files:**
- Create: `ts/packages/paigasus-kernel/scripts/generate-wasm.mjs`
- Modify: `ts/packages/paigasus-kernel/moon.yml` (add the task), `ts/moon.yml` (the `sources` group)

**Interfaces:**
- Consumes: nothing from Task 1.
- Produces: the Moon target `paigasus-kernel-ts:generate-wasm`. It writes the five artifacts into
  `rs/crates/bindings/paigasus-wasm/`. Task 3 and every later task call it as
  `moon run paigasus-kernel-ts:generate-wasm`.

**A subagent may touch only the three files above.** It must not commit any file under
`rs/crates/bindings/paigasus-wasm/`.

- [ ] **Step 1: Write the script**

Create `ts/packages/paigasus-kernel/scripts/generate-wasm.mjs`:

```js
// SPDX-License-Identifier: Apache-2.0
//
// The guards and the copy of `paigasus-kernel-ts:generate-wasm` (SMA-634 spec § 5.3). The wasm-pack
// call itself stays in moon.yml's `script:` block, because ci/affected-graph/cargo_moon_parity.py
// derives an FFI task from the resolved invocation text (FFI_MARKERS holds `wasm-pack`). A wrapper
// that hides the call would make the task invisible to the A5, A7 and A8 assertions.
//
//   node scripts/generate-wasm.mjs --pre     the guards, and the mtime bump cargo needs
//   node scripts/generate-wasm.mjs --post    the home-path rejection, the copy, the cleanup
//
// The task has no "unchanged" early exit. The binary bytes depend on the host (spec F12), so there
// is nothing a rebuild could compare itself against.
import { execFileSync } from 'node:child_process';
import { copyFileSync, readFileSync, rmSync, utimesSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

// scripts -> paigasus-kernel -> packages -> ts -> repo root: four `../`.
const ROOT = new URL('../../../../', import.meta.url);
const CRATE = new URL('rs/crates/bindings/paigasus-wasm/', ROOT);
const OUT = new URL('.wasmpack-out/', CRATE);
const GLUE = ['paigasus_wasm.js', 'paigasus_wasm_bg.js', 'paigasus_wasm.d.ts', 'paigasus_wasm_bg.wasm.d.ts'];
const BINARY = 'paigasus_wasm_bg.wasm';
const TOUCH = ['rs/crates/libs/paigasus-kernel/src/lib.rs', 'rs/crates/bindings/paigasus-wasm/src/lib.rs'];

function fail(message) {
  process.stderr.write(`generate-wasm: ${message}\n`);
  process.exit(1);
}

function pinnedWasmPackVersion() {
  // .prototools holds TWO wasm-pack lines: the version pin, and a `file://` plugin path. Match the
  // bare semantic version only.
  const text = readFileSync(new URL('.prototools', ROOT), 'utf8');
  const match = /^wasm-pack\s*=\s*"(\d+\.\d+\.\d+)"$/m.exec(text);
  if (match === null) fail('no `wasm-pack = "<version>"` pin found in .prototools');
  return match[1];
}

function pre() {
  // cargo's rustflags sources are mutually exclusive. An exported value would silently replace the
  // --config remap the moon.yml script passes, and the binary would then hold absolute host paths.
  for (const name of ['RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS']) {
    if (process.env[name] !== undefined && process.env[name] !== '') {
      fail(`${name} is set. cargo reads only one rustflags source, so it would drop the path remap. Run \`unset ${name}\` and start again.`);
    }
  }

  const pinned = pinnedWasmPackVersion();
  const reported = execFileSync('wasm-pack', ['--version'], { encoding: 'utf8' }).trim();
  if (reported !== `wasm-pack ${pinned}`) {
    fail(`wasm-pack reports "${reported}", and .prototools pins ${pinned}. Run \`proto install wasm-pack\` and start again.`);
  }

  // cargo's freshness is mtime-based. After a git operation a source can be older than an artifact
  // in rs/target, and cargo then reports "up to date" and links a stale binary. The `build` and
  // `test` tasks carry the same bump for the measured sum(2, 3) → 6 failure.
  const now = new Date();
  for (const path of TOUCH) utimesSync(fileURLToPath(new URL(path, ROOT)), now, now);

  process.stdout.write(`generate-wasm: wasm-pack ${pinned}, sources touched\n`);
}

function post() {
  const binary = readFileSync(new URL(BINARY, OUT));
  // CARGO_HOME can sit outside the home directory, so both roots are searched, not $HOME.
  const text = binary.toString('latin1');
  for (const marker of ['/Users/', '/home/']) {
    if (text.includes(marker)) {
      fail(`the new ${BINARY} holds the absolute path marker "${marker}". The --config remap did not take effect, and the binary must not be committed. Check the rustflags of the moon.yml script.`);
    }
  }

  // An in-place overwrite, so pnpm's hard-linked installed copy changes with it (spec F14). A
  // delete and replace would break the link and leave node_modules stale until it is rebuilt.
  for (const name of GLUE) copyFileSync(new URL(name, OUT), new URL(name, CRATE));
  writeFileSync(new URL(BINARY, CRATE), binary);
  rmSync(OUT, { recursive: true, force: true });

  process.stdout.write(`generate-wasm: wrote ${GLUE.length + 1} files into rs/crates/bindings/paigasus-wasm/\n`);
  process.stdout.write('generate-wasm: commit all five, and run `rm -rf ts/node_modules && pnpm -C ts install` if a git operation replaced them\n');
}

const mode = process.argv[2];
if (mode === '--pre') pre();
else if (mode === '--post') post();
else fail(`unknown mode ${JSON.stringify(mode)} — use --pre or --post`);
```

- [ ] **Step 2: Add the Moon task**

In `ts/packages/paigasus-kernel/moon.yml`, after the `test` task, add:

```yaml
  # SMA-634. The ONLY writer of the five committed wasm artifacts
  # (rs/crates/bindings/paigasus-wasm/paigasus_wasm*). `build` and `test` never write them any more:
  # `build` stopped calling wasm-pack, and `test` builds into its own .wasmpack-test-out.
  #
  # Run it after an edit to the Rust kernel or the wasm binding, then commit all five files.
  # tests/committed-wasm.test.ts reds until the committed files agree with the source.
  #
  # ONE host regenerates them. The binary bytes differ per host (spec F12), so a second host
  # produces a diff that says nothing. The four glue files are byte-identical on macOS, Linux arm64
  # and Linux amd64 (spec F15), which is what makes the gate valid in CI.
  #
  # The literal `wasm-pack` call stays HERE, not inside the script file:
  # ci/affected-graph/cargo_moon_parity.py derives an FFI task from the resolved invocation
  # (FFI_MARKERS), and an invocation behind a wrapper would drop this task out of A5, A7 and A8.
  # The path remap travels as `--config`, never as RUSTFLAGS: cargo reads one rustflags source
  # only, and a `--config` value also survives a path that holds a space.
  #
  # runInCI: false — CI never regenerates; it only compares. cache: false — the task writes tracked
  # files, and a cached replay would write nothing.
  generate-wasm:
    script: |
      set -euo pipefail
      node scripts/generate-wasm.mjs --pre
      ( cd ../../../rs/crates/bindings/paigasus-wasm && wasm-pack build . --target bundler --release --no-pack --out-dir .wasmpack-out --out-name paigasus_wasm -- --config 'target.wasm32-unknown-unknown.rustflags=["--remap-path-prefix=/Users=/redacted","--remap-path-prefix=/home=/redacted"]' --locked )
      node scripts/generate-wasm.mjs --post
    deps: ['^:build']
    # A5 demands the five workspace inputs on every FFI task, and A7 the sources and manifests of
    # the dependsOn closure (paigasus-kernel-rs, paigasus-node-bindings-rs, paigasus-wasm-rs), plus
    # the napi crate's build.rs. They are declared although the task is `cache: false`: the
    # assertions read the declaration, not the cache.
    inputs:
      - 'scripts/**/*'
      - 'package.json'
      - '/rs/crates/libs/paigasus-kernel/src/**/*'
      - '/rs/crates/libs/paigasus-kernel/Cargo.toml'
      - '/rs/crates/bindings/paigasus-node-bindings/build.rs'
      - '/rs/crates/bindings/paigasus-node-bindings/src/**/*'
      - '/rs/crates/bindings/paigasus-node-bindings/Cargo.toml'
      - '/rs/crates/bindings/paigasus-wasm/src/**/*'
      - '/rs/crates/bindings/paigasus-wasm/Cargo.toml'
      - '/rs/Cargo.lock'
      - '/rs/Cargo.toml'
      - '/rs/rust-toolchain.toml'
      - '/.prototools'
      - '/rs/.cargo/config.toml'
    options:
      runInCI: false
      cache: false
```

- [ ] **Step 3: Cover the new directory with `ts:lint` and `ts:fmt`**

`ts/moon.yml`'s `sources` group lists `packages/*/src/**/*` and `packages/*/testing/**/*`, and its
`tests` group lists `packages/*/tests/**/*`. A file under `packages/*/scripts/` is in neither, so an
edit there would serve a cached lint and format pass. Add one line to the `sources` group, after the
`packages/*/testing/**/*` entry:

```yaml
    # SMA-634: paigasus-kernel/scripts/generate-wasm.mjs. Without this, ts:lint and ts:fmt do not
    # key on packages/*/scripts and an edit there serves a cached pass.
    - 'packages/*/scripts/**/*'
```

- [ ] **Step 4: Run the task, and measure M1 (two runs on this host)**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"; export PROTO_REPORTER=text
moon run paigasus-kernel-ts:generate-wasm
shasum -a 256 rs/crates/bindings/paigasus-wasm/paigasus_wasm* > /tmp/sma634-run1.txt
moon run paigasus-kernel-ts:generate-wasm
shasum -a 256 rs/crates/bindings/paigasus-wasm/paigasus_wasm* > /tmp/sma634-run2.txt
diff /tmp/sma634-run1.txt /tmp/sma634-run2.txt && echo "M1 PASS: two runs agree"
```

Expected: both runs exit 0, and the two hash lists are identical — the four glue files **and** the
binary, because one host is deterministic (spec F12). **If the binary differs between two runs on
one host, stop and report.** That contradicts the measurement this design rests on.

- [ ] **Step 5: Verify the guards**

```bash
RUSTFLAGS=-C panic=abort moon run paigasus-kernel-ts:generate-wasm; echo "rc=$?"
grep -c "/Users/" rs/crates/bindings/paigasus-wasm/paigasus_wasm_bg.wasm || echo "no home path in the binary"
```

Expected: the first command exits non-zero with the `RUSTFLAGS is set` message. The second prints
`no home path in the binary` (`grep -c` exits 1 on no match). If `grep` reports a match, the remap
did not take effect: **stop and report**, and do not commit the binary.

- [ ] **Step 6: Restore the glue, so Task 3 still has its red**

`generate-wasm` has just written fresh glue over the committed glue. Task 3 must watch the gate fail
against the **stale committed** glue, so restore it. These four files are generated, and the fix of
this task is the script, not the glue, so a checkout of them discards no work.

```bash
git checkout -- rs/crates/bindings/paigasus-wasm/paigasus_wasm.js \
  rs/crates/bindings/paigasus-wasm/paigasus_wasm_bg.js \
  rs/crates/bindings/paigasus-wasm/paigasus_wasm.d.ts \
  rs/crates/bindings/paigasus-wasm/paigasus_wasm_bg.wasm.d.ts
git status --short
```

Expected: only `ts/packages/paigasus-kernel/moon.yml`, `ts/packages/paigasus-kernel/scripts/generate-wasm.mjs`
and `ts/moon.yml` appear.

- [ ] **Step 7: Commit**

```bash
git add ts/packages/paigasus-kernel/scripts/generate-wasm.mjs ts/packages/paigasus-kernel/moon.yml ts/moon.yml
git commit -m "build(ts): a generate-wasm task that writes the committed wasm artifacts (SMA-634)

The task is the only writer of the five artifacts. It guards against a set rustflags
variable, a wasm-pack version that is not the pinned one, and an absolute home path in
the binary, and it bumps the two Rust source mtimes that cargo freshness needs. The
literal wasm-pack call stays in moon.yml so the affected-graph gate still derives it as
an FFI task.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 3: The drift gate, the committed artifacts, and the four negative controls

**Files:**
- Create: `ts/packages/paigasus-kernel/tests/committed-wasm.test.ts`, `.gitattributes`
- Modify: `ts/packages/paigasus-kernel/vitest.config.ts` (the `node` project's `include`),
  `ts/packages/paigasus-kernel/moon.yml` (the `test` inputs),
  `rs/crates/bindings/paigasus-wasm/.gitignore`
- Commit: the five artifacts

**Interfaces:**
- Consumes: `tests/wasm-probe.mjs` (Task 1) and `paigasus-kernel-ts:generate-wasm` (Task 2).
- Produces: checks 1, 2 and 3 inside `paigasus-kernel-ts:test`. Check 4 is Task 7's.

**A subagent may touch the files above and the five artifacts.**

- [ ] **Step 1: Write the gate**

Create `ts/packages/paigasus-kernel/tests/committed-wasm.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The drift gate for the five committed wasm artifacts (SMA-634 spec § 5.4). Three checks, all
// host-independent, so they hold in CI although the binary bytes differ per host (spec F12):
//
//   1. the committed glue equals the glue of the fresh build this task already makes;
//   2. the committed binary has the same import and export lists as the fresh one;
//   3. the committed glue and binary instantiate together and replay all five parity corpora.
//
// Check 4 (the pnpm-installed copy) is NOT here: Moon's hasher ignores node_modules, so a cached
// pass would replay while the installed copy is another branch's. It lives in the setupFiles of the
// console-core and app vitest configs.
//
// Checks 2 and 3 run in a CHILD node process (tests/wasm-probe.mjs). This project has no wasm
// plugin, so an import through vitest would not be the loader a console server uses (spec F11).
import { execFileSync } from 'node:child_process';
import { existsSync, readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

const CRATE = new URL('../../../../rs/crates/bindings/paigasus-wasm/', import.meta.url);
const FRESH = new URL('.wasmpack-test-out/', CRATE);
const PROBE = fileURLToPath(new URL('./wasm-probe.mjs', import.meta.url));
const GLUE = ['paigasus_wasm.js', 'paigasus_wasm_bg.js', 'paigasus_wasm.d.ts', 'paigasus_wasm_bg.wasm.d.ts'];

const REGENERATE =
  'Run `moon run paigasus-kernel-ts:generate-wasm` and commit all five files under rs/crates/bindings/paigasus-wasm/ (paigasus_wasm_bg.wasm and the four glue files).';

function probe(mode: string, dir: URL): unknown {
  // A failure must name the probe's own stderr: a LinkError or a corpus row is the finding.
  try {
    return JSON.parse(execFileSync(process.execPath, [PROBE, mode, fileURLToPath(dir)], { encoding: 'utf8' }));
  } catch (error) {
    const detail = (error as { stderr?: string }).stderr ?? String(error);
    throw new Error(`wasm-probe.mjs ${mode} failed for ${fileURLToPath(dir)}:\n${detail}\n${REGENERATE}`);
  }
}

describe('the committed wasm artifacts agree with the Rust source', () => {
  it('has the fresh build this task makes', () => {
    // Without this the three checks below would compare against an absent directory and report a
    // confusing failure. The `test` task's own script builds it.
    expect(existsSync(fileURLToPath(new URL('paigasus_wasm_bg.wasm', FRESH))), `${fileURLToPath(FRESH)} is missing — run \`moon run paigasus-kernel-ts:test\`, which builds it.`).toBe(true);
  });

  it.each(GLUE)('check 1: the committed %s equals the fresh build', (name) => {
    const committed = readFileSync(new URL(name, CRATE));
    const fresh = readFileSync(new URL(name, FRESH));
    // A byte comparison, reported as text: the glue is JavaScript and TypeScript, so a diff is
    // readable, and the four files are byte-identical on every host (spec F15).
    expect(committed.toString('utf8'), `the committed ${name} is not this source's. ${REGENERATE}`).toBe(fresh.toString('utf8'));
  });

  it('check 2: the committed binary has the fresh interface', () => {
    expect(probe('--interfaces', CRATE), `the committed binary's imports or exports differ from the fresh build's. ${REGENERATE}`).toEqual(probe('--interfaces', FRESH));
  });

  it('check 3: the committed pair instantiates and replays every corpus', () => {
    const result = probe('--corpus', CRATE) as { checked: Record<string, number> };
    // The counts guard against a vacuous pass: an empty corpus file would otherwise replay nothing.
    // wasm-probe.mjs also rejects an empty corpus, so this is the second control on the same thing.
    expect(Object.keys(result.checked).sort()).toEqual(['prn_canonical', 'prn_cedar', 'prn_fields', 'sum', 'uuid7']);
    for (const [name, count] of Object.entries(result.checked)) expect(count, `the ${name} corpus is empty`).toBeGreaterThan(0);
  });
});
```

- [ ] **Step 2: Wire it into the `node` project and the task inputs**

`ts/packages/paigasus-kernel/vitest.config.ts`'s `node` project has an explicit `include` list, so a
file that is not listed never runs. Add the gate:

```ts
          include: ['tests/sum.test.ts', 'tests/uuid7.test.ts', 'tests/prn-canonical.test.ts', 'tests/prn-fields.test.ts', 'tests/cedar.test.ts', 'tests/committed-wasm.test.ts'],
```

In `ts/packages/paigasus-kernel/moon.yml`, add the five artifacts to the `test` task's `inputs`,
after the `/rs/crates/bindings/paigasus-wasm/package.json` line:

```yaml
      # SMA-634: the five committed artifacts the drift gate compares. Named one by one, not as a
      # `paigasus_wasm*` glob, so a new scratch file beside them cannot re-key this task.
      - '/rs/crates/bindings/paigasus-wasm/paigasus_wasm_bg.wasm'
      - '/rs/crates/bindings/paigasus-wasm/paigasus_wasm_bg.js'
      - '/rs/crates/bindings/paigasus-wasm/paigasus_wasm.js'
      - '/rs/crates/bindings/paigasus-wasm/paigasus_wasm.d.ts'
      - '/rs/crates/bindings/paigasus-wasm/paigasus_wasm_bg.wasm.d.ts'
```

- [ ] **Step 3: Run the gate, and record the red**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"; export PROTO_REPORTER=text
moon run paigasus-kernel-ts:test --force; echo "rc=$?"
```

Expected: `rc` is non-zero. Check 1 fails on `paigasus_wasm_bg.js` (the `__wbg_Error_*` line), and
check 3 fails with the `LinkError` of Task 1. Checks 2 and the five napi tests pass. Record the
output. If the run passes, **stop and report**: the committed glue is no longer stale, so this
gate's red-first evidence is gone and Task 1's state must be restored first.

- [ ] **Step 4: Commit the gate on its own**

```bash
git add ts/packages/paigasus-kernel/tests/committed-wasm.test.ts ts/packages/paigasus-kernel/vitest.config.ts ts/packages/paigasus-kernel/moon.yml
git commit -m "test(ts): a drift gate for the committed wasm artifacts (SMA-634)

Three host-independent checks in paigasus-kernel-ts:test. It is RED at this commit, on
the stale committed glue that the next commit repairs. The gate compares the glue, the
import and export lists and the corpus behaviour, never the binary bytes, which differ
per host.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

- [ ] **Step 5: Stop ignoring the binary, and add the git attributes**

In `rs/crates/bindings/paigasus-wasm/.gitignore`, replace the header comment and the `*.wasm` line
with:

```gitignore
# wasm-pack emits the binary and the glue here. SMA-634: ALL FIVE are committed, and
# `moon run paigasus-kernel-ts:generate-wasm` is their only writer. A console loads the committed
# copy, because pnpm links a `file:` dependency once, at install time, before any build task runs.
#
# NOTE: `wasm-pack build` CLEANS its --out-dir each run (it overwrites .gitignore with a bare `*`
# and DELETES package.json, even with --no-pack), so every task builds into a gitignored scratch
# dir and copies out of it: generate-wasm uses .wasmpack-out, the kernel's `test` task uses
# .wasmpack-test-out (the fresh build its drift gate compares against), and prebuild.yml uses
# .wasmpack-release-out.
*.wasm
# ...except the committed binary. The leading slash anchors the negation at the crate root, so a
# same-named file in a future sub-directory stays ignored.
!/paigasus_wasm_bg.wasm
.wasmpack-out/
.wasmpack-test-out/
.wasmpack-release-out/
```

Create `.gitattributes` at the repository root (none exists today):

```gitattributes
# SPDX-License-Identifier: Apache-2.0
#
# SMA-634. The kernel's wasm binary is a tracked binary file, and its glue is generated.
# `binary` keeps git from attempting a text diff or an end-of-line conversion, which also makes the
# Windows checkout safe without git's NUL heuristic — prebuild.yml checks the tree out on
# windows-latest.
#
# A rebase or merge conflict in these five files is NEVER resolved by hand: take either side, run
# `moon run paigasus-kernel-ts:generate-wasm`, and commit its result. The drift gate then proves
# that the five files agree with the Rust source.
rs/crates/bindings/paigasus-wasm/paigasus_wasm_bg.wasm binary
rs/crates/bindings/paigasus-wasm/paigasus_wasm_bg.js linguist-generated=true
rs/crates/bindings/paigasus-wasm/paigasus_wasm.js linguist-generated=true
rs/crates/bindings/paigasus-wasm/paigasus_wasm.d.ts linguist-generated=true
rs/crates/bindings/paigasus-wasm/paigasus_wasm_bg.wasm.d.ts linguist-generated=true
```

- [ ] **Step 6: Regenerate, and make the gate green**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"; export PROTO_REPORTER=text
moon run paigasus-kernel-ts:generate-wasm
git status --short
moon run paigasus-kernel-ts:test --force; echo "rc=$?"
```

Expected: `git status --short` lists the five artifacts (the binary as a new file, since the
negation now tracks it) plus `.gitignore` and `.gitattributes`. The task exits 0, and the gate's
seven cases pass.

- [ ] **Step 7: Commit the artifacts**

```bash
git add rs/crates/bindings/paigasus-wasm/paigasus_wasm_bg.wasm \
  rs/crates/bindings/paigasus-wasm/paigasus_wasm_bg.js \
  rs/crates/bindings/paigasus-wasm/paigasus_wasm.js \
  rs/crates/bindings/paigasus-wasm/paigasus_wasm.d.ts \
  rs/crates/bindings/paigasus-wasm/paigasus_wasm_bg.wasm.d.ts \
  rs/crates/bindings/paigasus-wasm/.gitignore .gitattributes
git commit -m "build(rs): commit the wasm binary beside its glue (SMA-634)

pnpm links a file: dependency once, at install time, so a console can only load an
artifact that exists in the tree. All five files come from one generate-wasm run, which
also repairs the glue that has not matched its binary since 0b2e346a. The drift gate of
the previous commit turns green here.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

- [ ] **Step 8: Negative control 1 — a stale glue must red check 1**

```bash
git show 0b2e346a:rs/crates/bindings/paigasus-wasm/paigasus_wasm_bg.js > rs/crates/bindings/paigasus-wasm/paigasus_wasm_bg.js
moon run paigasus-kernel-ts:test --force; echo "rc=$?"
git checkout -- rs/crates/bindings/paigasus-wasm/paigasus_wasm_bg.js
```

Expected: check 1 fails for `paigasus_wasm_bg.js`, check 3 fails with the `LinkError`, and check 2
passes (the binary is untouched). The restore is a checkout of one generated file, so it discards
nothing hand-written.

**The order of a control matters, and it is not obvious.** `paigasus-kernel-ts:test` rebuilds
`.wasmpack-test-out` from the CURRENT Rust source at the start of its own script. So a control that
leaves the Rust mutation in place while the task runs mutates the fresh build as well, and the
comparison then passes or fails for the wrong reason. Every control below therefore mutates the
source, regenerates the committed artifacts, **restores the source and the glue, and only then runs
the task**. The committed binary alone carries the mutation, which is exactly the drift the check
must see.

- [ ] **Step 9: Negative control 2 — a renamed export must red check 2**

`rs/crates/bindings/paigasus-wasm/src/lib.rs:18` is a self-contained `#[wasm_bindgen]` wrapper
(`pub fn sum(a: i32, b: i32) -> i32`), so the rename compiles. It also changes the glue, which is
why the glue is restored before the run.

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"; export PROTO_REPORTER=text
# 1. Mutate, and write the committed artifacts from the mutated source.
sed -i '' 's/pub fn sum(/pub fn sum_renamed(/' rs/crates/bindings/paigasus-wasm/src/lib.rs
moon run paigasus-kernel-ts:generate-wasm
# 2. Restore the SOURCE and the GLUE. Only the committed BINARY keeps the rename.
git checkout -- rs/crates/bindings/paigasus-wasm/src/lib.rs \
  rs/crates/bindings/paigasus-wasm/paigasus_wasm_bg.js \
  rs/crates/bindings/paigasus-wasm/paigasus_wasm.js \
  rs/crates/bindings/paigasus-wasm/paigasus_wasm.d.ts \
  rs/crates/bindings/paigasus-wasm/paigasus_wasm_bg.wasm.d.ts
# 3. The task now builds a FRESH pair from the original source and compares.
moon run paigasus-kernel-ts:test --force; echo "rc=$?"
# 4. Restore the binary.
moon run paigasus-kernel-ts:generate-wasm
git status --short
```

Expected: check 2 fails and names the export lists (`sum:function` against `sum_renamed:function`).
Check 1 passes: the committed glue is the restored one and the fresh build is from the restored
source. Check 3 fails too, because the committed glue calls an export the committed binary no longer
has — record that, it is the same drift seen from another side. The last `git status --short` is
empty. If it is not, the regenerated files differ from the committed ones and **you must stop and
report**: one host's output is supposed to be deterministic (M1).

If the crate does not compile after the rename, rename another `#[wasm_bindgen]` function that
nothing else in the crate calls, and record which one you used.

- [ ] **Step 10: Negative control 3 — a behaviour change must red check 3 alone**

`rs/crates/libs/paigasus-kernel/src/resource_name.rs:60` maps `PrnError::WrongFieldCount` to the
string `"wrong-field-count"`. A changed string alters no export name and no glue file, so this
control isolates check 3.

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"; export PROTO_REPORTER=text
sed -i '' 's/"wrong-field-count"/"wrong-field-countx"/' rs/crates/libs/paigasus-kernel/src/resource_name.rs
moon run paigasus-kernel-ts:generate-wasm
git checkout -- rs/crates/libs/paigasus-kernel/src/resource_name.rs \
  rs/crates/bindings/paigasus-wasm/paigasus_wasm_bg.js \
  rs/crates/bindings/paigasus-wasm/paigasus_wasm.js \
  rs/crates/bindings/paigasus-wasm/paigasus_wasm.d.ts \
  rs/crates/bindings/paigasus-wasm/paigasus_wasm_bg.wasm.d.ts
moon run paigasus-kernel-ts:test --force; echo "rc=$?"
moon run paigasus-kernel-ts:generate-wasm
git status --short
```

Expected: check 3 fails and names a `prnErrorKind` row of the `prn_canonical` corpus
(`"wrong-field-countx" !== "wrong-field-count"`). Checks 1 and 2 pass: the glue is unchanged, and
the export list does not hold an error string. The final `git status --short` is empty.

Note what this control also shows: a Rust change that the corpus can see is caught even though no
check compares the binary bytes. A change that the corpus cannot see is the residual that spec
§ 5.4 records.

- [ ] **Step 11: Record the control results**

Write the three control outcomes into the PR body: which check reddened, which stayed green, and
that the tree was clean afterwards. Control 4 belongs to Task 7 and is recorded there.

---

### Task 4: The kernel exports — `.` is wasm, `./napi` is napi

**Files:**
- Modify: `ts/packages/paigasus-kernel/package.json`, `vitest.config.ts`, `tsconfig.json`,
  `tests/{sum,uuid7,prn-canonical,prn-fields,cedar}.test.ts`, `moon.yml` (the `build` task),
  `.github/workflows/prebuild.yml`, `.github/workflows/ci.yml` (comments only)

**Interfaces:**
- Consumes: the committed artifacts of Task 3.
- Produces: `import { prnBuild } from '@paigasus/kernel'` resolves to `src/wasm.ts` under every
  condition, and `@paigasus/kernel/napi` resolves to `src/index.ts`. Task 6 imports the first form.

**A subagent may touch only the files above.**

- [ ] **Step 1: Flip the exports map**

In `ts/packages/paigasus-kernel/package.json`:

```json
  "_comment_exports": "`.` is the WASM entry under every condition, and `./napi` is the napi entry (SMA-634, ADR-0022). Measured: a Next server build cannot load the napi binding from an in-monorepo `file:` or `link:` dependency, so a condition-switched map handed every Node consumer an entry that does not load. The napi entry stays reachable on purpose, and only the kernel's own tests name it. This package is `private: true` at 0.0.0 (see _comment_publish), so it has no npm consumers at all; an npm consumer of the napi binding uses @paigasus/node-bindings directly, whose loader-only package plus seven per-platform packages are correct and unchanged. Conditions point at SOURCE (bundler-aware consumers walk TS); switch to ./dist/* when tsup wiring lands, IN LOCKSTEP with flipping `private: false`.",
```

```json
  "exports": {
    ".": "./src/wasm.ts",
    "./napi": "./src/index.ts"
  },
```

- [ ] **Step 2: Point the five napi tests at the napi subpath**

In each of `tests/sum.test.ts`, `tests/uuid7.test.ts`, `tests/prn-canonical.test.ts`,
`tests/prn-fields.test.ts` and `tests/cedar.test.ts`, change `from '@paigasus/kernel'` to
`from '@paigasus/kernel/napi'`. Nothing else in those files changes. The five `*.wasm.test.ts`
files keep `from '@paigasus/kernel'`, which is now the wasm entry by the map rather than by an
alias.

- [ ] **Step 3: Drop the alias that the old map needed**

In `ts/packages/paigasus-kernel/vitest.config.ts`, remove the `kernelWasmEntry` constant and its
`'@paigasus/kernel'` alias from the browser project, and replace the comment above it:

```ts
// The browser project resolves @paigasus/kernel through the package's own exports map, which since
// SMA-634 points `.` at src/wasm.ts under every condition — so no alias is needed for the kernel
// itself. @paigasus/wasm under it is still aliased to this task's fresh scratch glue, because pnpm
// does not refresh its `file:` copy after a rebuild (SMA-420, SMA-427 M4).
//
// The additive condition list stays: dropping module/import/default breaks source-exports `.ts`
// resolution for every @paigasus/* package (SMA-427 M4, CLAUDE.md).
```

The browser project's `resolve` block becomes:

```ts
        resolve: {
          conditions: ['browser', 'module', 'import', 'default'],
          alias: { '@paigasus/wasm': wasmBindingDir },
        },
```

The `node` project keeps its `@paigasus/node-bindings` alias: it is what makes a rebuilt `.node`
visible to `@paigasus/kernel/napi`.

- [ ] **Step 4: Decide `customConditions` by measurement**

`ts/packages/paigasus-kernel/tsconfig.json` sets `customConditions: ["node"]` so that tsc resolves
`.` to the napi entry. The map no longer switches on a condition, and `include` covers both entry
files, so the setting should be removable. Measure it:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"; export PROTO_REPORTER=text
# with the setting removed
moon run paigasus-kernel-ts:build --force; echo "rc=$?"
```

If it passes, keep the removal and replace the comment:

```jsonc
  "compilerOptions": {
    "noEmit": true
    // SMA-634 removed `customConditions: ["node"]`. It existed so tsc resolved the kernel's `.`
    // export to the napi entry; `.` is now the wasm entry unconditionally, and `include` below
    // holds both entry files, so both surfaces are type-checked either way. The wasm and napi
    // surfaces are kept in lockstep by src/binding-parity.types.ts, which queries the two binding
    // packages directly and fails `tsc --noEmit` if a signature drifts.
  },
```

If it fails, keep the setting and write the measured reason (the exact tsc error) into the comment
instead. Either way the comment must match the new map. Record which branch you took.

- [ ] **Step 5: Stop `build` from writing the committed artifacts**

In `ts/packages/paigasus-kernel/moon.yml`'s `build` task: remove the wasm-pack subshell, the
`cp …/.wasmpack-out/paigasus_wasm* …` copy and the `rm -rf …/.wasmpack-out`, so the script is

```yaml
    script: 'touch ../../../rs/crates/libs/paigasus-kernel/src/lib.rs ../../../rs/crates/bindings/paigasus-node-bindings/src/lib.rs ../../../rs/crates/bindings/paigasus-wasm/src/lib.rs && pnpm exec napi build --platform --cwd ../../../rs/crates/bindings/paigasus-node-bindings && pnpm exec tsc -p tsconfig.json --noEmit'
```

Remove `/rs/crates/bindings/paigasus-wasm/paigasus_wasm_bg.wasm` from the task's `outputs`. That
line is now harmful, not merely stale: the file is tracked, and Moon would restore a cached copy
over it.

**Keep every `/rs/crates/bindings/paigasus-wasm/**` input.** The task still calls `napi build`, so
it is still an FFI task, and A7 demands the closure inputs of `paigasus-wasm-rs`. Its `tsc` also
reads the committed `paigasus_wasm.d.ts` through the parity guard.

Replace the wasm paragraphs of the task's comment with:

```yaml
  # SMA-634: this task no longer runs wasm-pack and no longer writes the crate directory. The five
  # wasm artifacts are committed, and `paigasus-kernel-ts:generate-wasm` is their only writer. The
  # wasm inputs stay: `napi build` keeps this an FFI task, so A7 demands the dependsOn closure's
  # sources and manifests, and `tsc` reads the committed paigasus_wasm.d.ts through
  # src/binding-parity.types.ts.
```

- [ ] **Step 6: Fix the two workflow comments that describe the old task**

`.github/workflows/prebuild.yml`, in the "A THIRD scratch dir" comment, replace the sentence
`ts/packages/paigasus-kernel's build task owns .wasmpack-out and its test task owns
.wasmpack-test-out` with:

```yaml
      # ts/packages/paigasus-kernel's generate-wasm task owns .wasmpack-out and its test task owns
      # .wasmpack-test-out (SMA-634); this needs its own name so a concurrent moon ci cannot race it.
```

`.github/workflows/ci.yml`, in the wasm32 comment, replace `kernel-ts's `build` and `test` tasks
both invoke wasm-pack concurrently` with:

```yaml
      # Same race for the wasm32 target (SMA-427): `wasm-pack build` runs `rustup target add
      # wasm32-unknown-unknown` itself ("Checking for the Wasm target…"). Since SMA-634 only
      # kernel-ts's `test` task invokes wasm-pack in CI (`generate-wasm` is runInCI: false), but the
      # serial pre-install stays: it also covers a future second caller, and the components race
      # above is unrelated to wasm.
```

- [ ] **Step 7: Run the kernel tasks**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"; export PROTO_REPORTER=text
moon run paigasus-kernel-ts:build --force; echo "build rc=$?"
moon run paigasus-kernel-ts:test --force; echo "test rc=$?"
git status --short
```

Expected: both exit 0. The five napi tests now import `@paigasus/kernel/napi`, the five wasm tests
resolve through the new map, and the gate still passes. `git status --short` must show no change
under `rs/crates/bindings/paigasus-wasm/`: `build` no longer writes there.

- [ ] **Step 8: Commit**

```bash
git add ts/packages/paigasus-kernel/package.json ts/packages/paigasus-kernel/vitest.config.ts \
  ts/packages/paigasus-kernel/tsconfig.json ts/packages/paigasus-kernel/moon.yml \
  ts/packages/paigasus-kernel/tests .github/workflows/prebuild.yml .github/workflows/ci.yml
git commit -m "feat(ts): resolve @paigasus/kernel to the wasm entry, and napi at ./napi (SMA-634)

A Next server build cannot load the napi binding from an in-monorepo file: dependency,
so a condition-switched map gave every Node consumer an entry that does not load. The
default entry is now platform-neutral, and the napi entry needs an explicit subpath.
The build task stops writing the committed wasm artifacts.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 5: The affected-graph obligations of `generate-wasm`

**Files:**
- Modify: `ci/affected-graph/cargo_moon_parity.py`
- Possibly modify: `ci/affected-graph/run.sh` and `ci/affected-graph/README.md` (only if the gate
  shows a changed case)

**Interfaces:**
- Consumes: the `generate-wasm` task of Task 2.
- Produces: a green `repo:affected-smoke`.

**A subagent may touch only the files above.**

- [ ] **Step 1: Add the A8 entry**

`generate-wasm` reaches cargo through wasm-pack, which cannot guarantee a locked resolution, so A8
demands a reviewed entry. In `ci/affected-graph/cargo_moon_parity.py`, inside `ALLOW_UNLOCKED_CARGO`
and after the `"paigasus-kernel-ts:test"` line:

```python
    "paigasus-kernel-ts:generate-wasm": (
        "as paigasus-kernel-ts:build — it reaches cargo through `wasm-pack build ... -- --locked`, "
        "and wasm-pack makes its OWN unlocked cargo call before that build and repairs the lock "
        "there (measured, SMA-601). SMA-634 made this task the only writer of the committed wasm "
        "artifacts; it is runInCI: false, so CI never runs it, but A8 reads the declaration."
    ),
```

- [ ] **Step 2: Run the gate under system bash 3.2**

`repo:affected-smoke` needs system `/bin/bash` (3.2.57), not the Homebrew bash.

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"; export PROTO_REPORTER=text
/bin/bash ci/affected-graph/run.sh; echo "rc=$?"
```

Expected: `rc=0`. Two failure shapes are known and are not findings by themselves:
- a wall of `expected rc 0` rows, or an empty stdout with `declare: -A: invalid option` on stderr,
  means the wrong bash ran it;
- a `cargo metadata` error naming a `.napi-stage-<random>` path is a defect that SMA-663 fixed, so
  treat a recurrence as new, and look for a glob character in `rs/Cargo.toml`'s `members`.

- [ ] **Step 3: Read the A5, A7 and A8 rows for the new task**

If the gate fails, the message names the target and the missing entries. Three outcomes are
possible, and each has a defined response:
- `paigasus-kernel-ts:generate-wasm inputs omit …` — add the named paths to the task's `inputs` in
  `ts/packages/paigasus-kernel/moon.yml` (they are the A5 workspace files or an A7 closure path).
- an A8 row for the task — check Step 1's entry key spelling against the target name.
- a **project** case row (`kernel->bindings`, `binding-oneway-node`, `binding-oneway-wasm`,
  `kernel->consumer-tasks`) — this is not expected: spec F7 measured that a `workspace:*` dependency
  adds no `dependsOn` edge, and this task adds none either. Re-baseline the case in
  `ci/affected-graph/run.sh` only if the run shows a real change, and write the reason into the case
  comment and into `ci/affected-graph/README.md`. If a case changes without an obvious cause, **stop
  and report** rather than adjusting the expectation to match the output.

- [ ] **Step 4: Confirm the task is in the derived FFI set**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"; export PROTO_REPORTER=text
moon task paigasus-kernel-ts:generate-wasm --json | python3 -c "import json,sys; d=json.load(sys.stdin); print('script holds wasm-pack:', 'wasm-pack' in json.dumps(d)); print('inputs:', len(d['inputs']))"
```

Expected: `script holds wasm-pack: True`. If it is `False`, the literal call moved out of the
`script:` block, and the task has silently left A5, A7 and A8. That is the blind spot the gate's own
comment records, so restore the literal call.

- [ ] **Step 5: Commit**

```bash
git add ci/affected-graph/cargo_moon_parity.py
git commit -m "ci: allow the unlocked cargo call of generate-wasm (SMA-634)

wasm-pack repairs the lock in its own call before the build that -- --locked reaches,
which the two other kernel tasks already record. The new task is runInCI: false, and A8
reads the declaration rather than a run.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 6: `console-core` uses the kernel

**Files:**
- Modify: `ts/packages/paigasus-console-core/package.json`,
  `ts/packages/paigasus-console-core/src/prn-tenancy.ts`,
  `ts/packages/paigasus-console-core/tests/unit/prn-tenancy.test.ts`, `ts/pnpm-lock.yaml`
- Create: `ts/packages/paigasus-console-core/tests/unit/prn-tenancy-delegation.test.ts`

**Interfaces:**
- Consumes: `@paigasus/kernel`'s `.` export (Task 4).
- Produces: `prn-tenancy.ts` with an unchanged public surface — `TenancyKind`, `TenancyRef`,
  `ROOT_PRN: string`, `isUuid(value: string): boolean`,
  `parseTenancyPrn(prn: string): TenancyRef | null`, `organizationPrn(orgId: string): string`,
  `teamPrn(orgId: string, teamId: string): string`,
  `projectPrn(orgId: string, projectId: string): string`. No caller in the two apps changes.

**A subagent may touch only the files above.**

- [ ] **Step 1: Add the dependency and install**

In `ts/packages/paigasus-console-core/package.json`, add to `dependencies`, in alphabetical order
after `@paigasus/discovery`:

```json
    "@paigasus/kernel": "workspace:*",
```

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"; export PROTO_REPORTER=text
pnpm -C ts install
git status --short ts/pnpm-lock.yaml
```

Expected: the lockfile changes.

- [ ] **Step 2: Write the delegation test first**

Create `ts/packages/paigasus-console-core/tests/unit/prn-tenancy-delegation.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// src/prn-tenancy.ts must DELEGATE the PRN grammar to the kernel (SMA-634 spec § 6.3). The corpus
// replay in prn-tenancy.test.ts cannot prove that: the hand-written reader it replaced passed the
// same rows, and so would a text check, because that reader's only regex was the UUID one. This
// file mocks the kernel and asserts that its answers decide the result.
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { prnBuild, prnErrorKind, prnOrg, prnRegion, prnResourceId, prnResourceType, prnService } from '@paigasus/kernel';
import { organizationPrn, parseTenancyPrn, projectPrn, teamPrn } from '../../src/prn-tenancy';

vi.mock('@paigasus/kernel', () => ({
  prnErrorKind: vi.fn(),
  prnService: vi.fn(),
  prnRegion: vi.fn(),
  prnResourceType: vi.fn(),
  prnResourceId: vi.fn(),
  prnOrg: vi.fn(),
  prnBuild: vi.fn(),
}));

const ORG = '0190a100-0000-7000-8000-0000000000aa';
const TEAM = '0190a1b2-0000-7000-8000-000000000001';
// A PRN that is valid and of a tenancy shape, so only the kernel's verdict can reject it.
const TEAM_PRN = `prn:pgs:iam::${ORG}:team/${TEAM}`;

beforeEach(() => {
  vi.mocked(prnErrorKind).mockReturnValue('');
  vi.mocked(prnService).mockReturnValue('iam');
  vi.mocked(prnRegion).mockReturnValue('');
  vi.mocked(prnResourceType).mockReturnValue('team');
  vi.mocked(prnResourceId).mockReturnValue(TEAM);
  vi.mocked(prnOrg).mockReturnValue(ORG);
  vi.mocked(prnBuild).mockReturnValue('built-by-the-kernel');
});

describe('parseTenancyPrn delegates the grammar to the kernel', () => {
  it("returns null when the kernel rejects a PRN that LOOKS valid — a local grammar would accept it", () => {
    vi.mocked(prnErrorKind).mockReturnValue('wrong-field-count');
    expect(parseTenancyPrn(TEAM_PRN)).toBeNull();
    expect(vi.mocked(prnErrorKind)).toHaveBeenCalledWith(TEAM_PRN);
  });

  it("takes the fields from the kernel, not from the string", () => {
    const otherOrg = '0190a100-0000-7000-8000-0000000000bb';
    const otherId = '0190a1b2-0000-7000-8000-000000000002';
    vi.mocked(prnOrg).mockReturnValue(otherOrg);
    vi.mocked(prnResourceId).mockReturnValue(otherId);
    // The argument still spells ORG and TEAM. A reader that split the string would return those.
    expect(parseTenancyPrn(TEAM_PRN)).toEqual({ kind: 'team', orgId: otherOrg, id: otherId });
  });

  it('returns null when the kernel reports another service', () => {
    vi.mocked(prnService).mockReturnValue('gateway');
    expect(parseTenancyPrn(TEAM_PRN)).toBeNull();
  });

  it('returns null when the kernel reports a region', () => {
    vi.mocked(prnRegion).mockReturnValue('us-east-1');
    expect(parseTenancyPrn(TEAM_PRN)).toBeNull();
  });

  it('returns null when the kernel reports a non-tenancy resource type', () => {
    vi.mocked(prnResourceType).mockReturnValue('user');
    expect(parseTenancyPrn(TEAM_PRN)).toBeNull();
  });
});

describe('the builders call prnBuild with the IAM tenancy arguments', () => {
  it('organizationPrn sends an EMPTY org field', () => {
    expect(organizationPrn(ORG)).toBe('built-by-the-kernel');
    expect(vi.mocked(prnBuild)).toHaveBeenCalledWith('iam', '', '', 'organization', ORG);
  });

  it('teamPrn sends the org field', () => {
    expect(teamPrn(ORG, TEAM)).toBe('built-by-the-kernel');
    expect(vi.mocked(prnBuild)).toHaveBeenCalledWith('iam', '', ORG, 'team', TEAM);
  });

  it('projectPrn sends the org field', () => {
    expect(projectPrn(ORG, TEAM)).toBe('built-by-the-kernel');
    expect(vi.mocked(prnBuild)).toHaveBeenCalledWith('iam', '', ORG, 'project', TEAM);
  });

  it('rejects an id that is not a UUID before it reaches the kernel', () => {
    expect(() => organizationPrn('x')).toThrow(TypeError);
    expect(vi.mocked(prnBuild)).not.toHaveBeenCalled();
  });
});
```

- [ ] **Step 3: Run it against the OLD reader and watch it fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"; export PROTO_REPORTER=text
moon run paigasus-console-core-ts:test --force; echo "rc=$?"
```

Expected: the delegation file fails. The current reader ignores the mocked kernel completely, so
the first case returns a `TenancyRef` where `null` is required, and the builder cases fail because
`prnBuild` is never called. This is the proof that the test can tell the two implementations apart.

- [ ] **Step 4: Rewrite `prn-tenancy.ts` as the adapter**

Replace `ts/packages/paigasus-console-core/src/prn-tenancy.ts` with:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// IAM tenancy PRNs for the console, over the kernel's PRN grammar (SMA-634 spec § 6.2, ADR-0022).
//
// NOT an ADR-0005 exception any more. The grammar lives once, in paigasus-kernel
// (rs/crates/libs/paigasus-kernel/src/resource_name.rs), and this module calls it through
// `@paigasus/kernel`, whose `.` export is the platform-neutral wasm entry. What stays here is IAM's
// own tenancy rule (rs/crates/libs/paigasus-iam-core/src/tenancy.rs `check`): service `iam`; an
// organization has NO org field (its org id is its own id); a team and a project have one.
//
// tests/unit/prn-tenancy.test.ts replays the kernel parity corpus through this file, and
// tests/unit/prn-tenancy-delegation.test.ts proves that the kernel, not this file, reads the PRN.
import 'server-only';
import { prnBuild, prnErrorKind, prnOrg, prnRegion, prnResourceId, prnResourceType, prnService } from '@paigasus/kernel';

export type TenancyKind = 'organization' | 'team' | 'project';
export type TenancyRef = { kind: TenancyKind; orgId: string; id: string };

/** The canonical PRN of IAM's synthetic Cedar Root: `root_prn()` in paigasus-iam-core authz/model.rs. */
export const ROOT_PRN = 'prn:pgs:iam:::root/00000000-0000-0000-0000-000000000000';

/**
 * A RESOURCE LIMIT, not grammar. The kernel enforces its own 512-byte rule, but it does so AFTER the
 * string is copied into wasm linear memory, and that memory never shrinks. So an unbounded caller
 * input is bounded here, before the first kernel call. Do not remove this as a duplicate of the
 * kernel's MAX_LEN.
 */
const MAX_LEN = 512;

/**
 * The UUID shape of a URL segment. This is NOT PRN grammar: the kernel exposes no UUID predicate,
 * and the pages use this to reject a bad URL segment before they build a PRN from it.
 */
const UUID = /^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$/;

const TENANCY_KINDS: ReadonlySet<string> = new Set<TenancyKind>(['organization', 'team', 'project']);

export function isUuid(value: string): boolean {
  return UUID.test(value);
}

function requireUuid(label: string, value: string): string {
  if (!isUuid(value)) throw new TypeError(`${label} must be a UUID`);
  return value.toLowerCase();
}

/**
 * The tenancy node a PRN names, or null for any other resource and for an invalid PRN. The kernel
 * decides validity and returns the canonical, lower-case fields.
 *
 * A NON-EMPTY REGION IS REJECTED, well formed or not. `TenancyRef` has no region field and the three
 * builders always emit an empty one, so a regionful PRN read here would be REWRITTEN without its
 * region on the way back out: a silently different resource. IAM's own tenancy PRNs carry no region,
 * so this console loses no valid input. The day IAM regionalises tenancy, this returns null instead
 * of corrupting the value, and `TenancyRef` grows a region field.
 */
export function parseTenancyPrn(prn: string): TenancyRef | null {
  if (prn.length === 0 || prn.length > MAX_LEN) return null;
  // The kernel is the grammar: a non-empty kind is any malformed PRN.
  if (prnErrorKind(prn) !== '') return null;
  if (prnService(prn) !== 'iam' || prnRegion(prn) !== '') return null;
  const type = prnResourceType(prn);
  if (!TENANCY_KINDS.has(type)) return null;
  const kind = type as TenancyKind;
  const id = prnResourceId(prn);
  // prnOrg returns '' for an ABSENT org field; a malformed one is already an error kind above.
  const org = prnOrg(prn);
  if (kind === 'organization') return org === '' ? { kind, orgId: id, id } : null;
  return org === '' ? null : { kind, orgId: org, id };
}

export function organizationPrn(orgId: string): string {
  return prnBuild('iam', '', '', 'organization', requireUuid('orgId', orgId));
}

export function teamPrn(orgId: string, teamId: string): string {
  return prnBuild('iam', '', requireUuid('orgId', orgId), 'team', requireUuid('teamId', teamId));
}

export function projectPrn(orgId: string, projectId: string): string {
  return prnBuild('iam', '', requireUuid('orgId', orgId), 'project', requireUuid('projectId', projectId));
}
```

Note what is deliberate: the adapter does **not** lower-case the kernel's answers. The kernel
returns the canonical form, and the existing case in `prn-tenancy.test.ts` (`lower-cases the ids of
an upper-case PRN, as the kernel canonicalises`) is the control on that. `prnBuild` can throw only
if the kernel grammar changes, and the builders do not catch it.

- [ ] **Step 5: Add the text check to the corpus suite**

In `ts/packages/paigasus-console-core/tests/unit/prn-tenancy.test.ts`, add the import of
`readFileSync`'s helper that the file already has, and a new block at the end:

```ts
describe('the module holds no PRN grammar of its own', () => {
  // A companion to prn-tenancy-delegation.test.ts: that file proves the kernel decides, and this
  // one keeps the mechanical grammar calls of the old reader from coming back.
  const source = read('ts/packages/paigasus-console-core/src/prn-tenancy.ts');

  it('imports the kernel', () => {
    expect(source).toMatch(/from '@paigasus\/kernel'/);
  });

  it.each(['.split(', '.indexOf(', '.slice('])('does not call %s', (call) => {
    expect(source).not.toContain(call);
  });
});
```

The suite's own header comment says it tests the interface and is the same for both
implementations. Correct that sentence: it now tests the adapter, and the delegation file is what
tells the implementations apart.

- [ ] **Step 6: Run the package's tests**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"; export PROTO_REPORTER=text
moon run paigasus-console-core-ts:test --force; echo "rc=$?"
moon run paigasus-console-core-ts:typecheck --force; echo "rc=$?"
```

Expected: both exit 0. The corpus replay, the `ROOT_PRN` block, the 13 negative rows, the `isUuid`
table, the builder round trips and the delegation file all pass. The `ExperimentalWarning` for the
wasm import appears on stderr and is not a failure (spec F11).

If a corpus row fails, do not change the corpus or the expectation: the kernel and the old reader
disagree there, and that disagreement is the finding. **Stop and report it.**

- [ ] **Step 7: Commit**

```bash
git add ts/packages/paigasus-console-core/package.json ts/packages/paigasus-console-core/src/prn-tenancy.ts \
  ts/packages/paigasus-console-core/tests/unit/prn-tenancy.test.ts \
  ts/packages/paigasus-console-core/tests/unit/prn-tenancy-delegation.test.ts ts/pnpm-lock.yaml
git commit -m "feat(ts): read IAM tenancy PRNs through the kernel (SMA-634)

The console kept its own copy of the PRN grammar because the kernel could not load in a
Next build. It loads now, so the file keeps only IAM's tenancy rule, the UUID guard for a
URL segment and a length limit for the FFI boundary. The public surface does not change.
This removes the ADR-0005 exception that SMA-511 recorded.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 7: Check 4 — the installed copy must equal the committed files

**Files:**
- Create: `ts/packages/paigasus-console-core/testing/installed-wasm.ts`
- Modify: `ts/packages/paigasus-console-core/testing/index.ts`,
  `ts/packages/paigasus-console-core/tests/support/setup.ts`,
  `ts/apps/iam-console/tests/support/setup.ts`, `ts/apps/gateway-console/tests/support/setup.ts`

**Interfaces:**
- Consumes: the committed artifacts (Task 3) and the kernel dependency (Task 6).
- Produces: `assertInstalledWasmMatchesCommitted(): void`, exported from `@paigasus/console-core/testing`.
  It throws when the pnpm-installed copy differs from the committed files.

**A subagent may touch only the files above.**

- [ ] **Step 1: Write the helper**

Create `ts/packages/paigasus-console-core/testing/installed-wasm.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// Check 4 of the SMA-634 drift gate (spec § 5.4). pnpm HARD-LINKS a `file:` dependency's files into
// its store at install time. An in-place overwrite (what generate-wasm does) changes both ends at
// once, but an unlink and replace — a `git checkout`, a branch switch, a delete — breaks the link,
// and neither `pnpm install` nor `pnpm install --force` repairs it. Only a new node_modules does
// (measured, spec F14).
//
// It does not live in the kernel's own task: Moon's hasher ignores node_modules, so a cached pass
// would replay while the installed copy is another branch's. It runs in the setupFiles of the
// console-core and app vitest configs, which is where a stale copy changes a result. In CI it
// always passes, because CI installs into a new node_modules.
import { createHash } from 'node:crypto';
import { createRequire } from 'node:module';
import { existsSync, readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

// testing -> paigasus-console-core -> packages -> ts -> repo root: four `../`.
const ROOT = new URL('../../../../', import.meta.url);
const CRATE = new URL('rs/crates/bindings/paigasus-wasm/', ROOT);
// @paigasus/wasm is the KERNEL's dependency, not this package's, so the require is anchored at the
// kernel's own manifest. Its own exports map does not expose ./package.json, so the anchor is the
// file path, not a package specifier.
const KERNEL_MANIFEST = new URL('ts/packages/paigasus-kernel/package.json', ROOT);
const FILES = ['paigasus_wasm_bg.wasm', 'paigasus_wasm_bg.js'];

const REPAIR = 'Run `rm -rf ts/node_modules && pnpm -C ts install`. pnpm does not refresh a file: dependency after a git operation replaces it (SMA-634 spec F14).';

function sha256(path: string): string {
  return createHash('sha256').update(readFileSync(path)).digest('hex');
}

/**
 * Throw when the pnpm-installed @paigasus/wasm copy is not the committed one. A console loads the
 * installed copy, so a stale copy means every test here runs against another commit's kernel.
 */
export function assertInstalledWasmMatchesCommitted(): void {
  const require = createRequire(KERNEL_MANIFEST);
  for (const name of FILES) {
    let installed: string;
    try {
      installed = require.resolve(`@paigasus/wasm/${name}`);
    } catch {
      throw new Error(`@paigasus/wasm/${name} is not installed. ${REPAIR}`);
    }
    const committed = fileURLToPath(new URL(name, CRATE));
    if (!existsSync(committed)) throw new Error(`${committed} is missing from the tree — the committed wasm artifacts are incomplete.`);
    if (sha256(installed) !== sha256(committed)) {
      throw new Error(`the installed ${name} differs from the committed one (${installed}). ${REPAIR}`);
    }
  }
}
```

- [ ] **Step 2: Export it**

Append to `ts/packages/paigasus-console-core/testing/index.ts`:

```ts
export { assertInstalledWasmMatchesCommitted } from './installed-wasm';
```

- [ ] **Step 3: Call it from the three setups**

In each of `ts/packages/paigasus-console-core/tests/support/setup.ts`,
`ts/apps/iam-console/tests/support/setup.ts` and `ts/apps/gateway-console/tests/support/setup.ts`,
add the import and one call at module scope, above the existing `beforeEach`:

```ts
import { assertInstalledWasmMatchesCommitted } from '@paigasus/console-core/testing';

// SMA-634 check 4. At module scope on purpose: a stale installed @paigasus/wasm makes every test in
// this file run against another commit's kernel, so the file must fail at once with the repair
// command rather than report a confusing assertion.
assertInstalledWasmMatchesCommitted();
```

In `ts/packages/paigasus-console-core/tests/support/setup.ts` the import is relative, because the
package cannot import itself by name through its own exports map:

```ts
import { assertInstalledWasmMatchesCommitted } from '../../testing/installed-wasm';
```

- [ ] **Step 4: Prove the check is green on a good tree**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"; export PROTO_REPORTER=text
moon run paigasus-console-core-ts:test --force; echo "rc=$?"
moon run iam-console-ts:test --force; echo "rc=$?"
moon run gateway-console-ts:test --force; echo "rc=$?"
```

Expected: all three exit 0.

- [ ] **Step 5: Negative control 4 — a stale installed copy must red**

The installed copy is hard-linked to the crate file, so a plain overwrite would change both. Break
the link first, exactly as a `git checkout` does, and then write a different byte:

```bash
INSTALLED=$(node -e "const {createRequire}=require('node:module');console.log(createRequire('$PWD/ts/packages/paigasus-kernel/package.json').resolve('@paigasus/wasm/paigasus_wasm_bg.js'))")
cp "$INSTALLED" /tmp/sma634-installed-glue.js
rm "$INSTALLED"
printf '// stale\n' > "$INSTALLED"
moon run paigasus-console-core-ts:test --force; echo "rc=$?"
cp /tmp/sma634-installed-glue.js "$INSTALLED"
```

Expected: every test file of the package fails at setup with `the installed paigasus_wasm_bg.js
differs from the committed one` and the repair command. Afterwards restore properly, because the
hard link is now broken:

```bash
rm -rf ts/node_modules && pnpm -C ts install
moon run paigasus-console-core-ts:test --force; echo "rc=$?"
git status --short
```

Expected: the task passes again and the tree is clean.

- [ ] **Step 6: Commit**

```bash
git add ts/packages/paigasus-console-core/testing/installed-wasm.ts \
  ts/packages/paigasus-console-core/testing/index.ts \
  ts/packages/paigasus-console-core/tests/support/setup.ts \
  ts/apps/iam-console/tests/support/setup.ts ts/apps/gateway-console/tests/support/setup.ts
git commit -m "test(ts): fail a console suite on a stale installed wasm copy (SMA-634)

pnpm hard-links a file: dependency and does not repair the link after a git operation
replaces the file, so a local suite can run against another commit's kernel. The check
sits in the vitest setup, where a stale copy changes a result, not in a Moon task, whose
hasher ignores node_modules.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 8: Moon inputs for the console projects

**Files:**
- Modify: `ts/packages/paigasus-console-core/moon.yml`, `ts/apps/iam-console/moon.yml`,
  `ts/apps/gateway-console/moon.yml`

**Interfaces:**
- Consumes: the kernel dependency (Task 6) and the committed artifacts (Task 3).
- Produces: a kernel or artifact edit selects the console tasks. No `deps` and no `dependsOn` edge,
  so no console task needs a Rust toolchain.

**A subagent may touch only the three files above.**

- [ ] **Step 1: Add the inputs to `console-core`**

In `ts/packages/paigasus-console-core/moon.yml`, add to the `&upstreams` anchor (which `build` and
`typecheck` share) and to the `test` task's own list:

```yaml
      # SMA-634. @paigasus/console-core reads the kernel's source and, through it, the five
      # COMMITTED wasm artifacts. There is deliberately no `deps` on a kernel task and no dependsOn
      # edge: the artifacts are tracked files, so no console task compiles Rust. Inputs are the only
      # thing that confers affectedness on Moon 2.5.3, so they are what makes a kernel edit re-run
      # this package. The five artifact paths are named one by one, not as a `paigasus_wasm*` glob.
      - '/ts/packages/paigasus-kernel/src/**/*'
      - '/ts/packages/paigasus-kernel/package.json'
      - '/rs/crates/bindings/paigasus-wasm/paigasus_wasm_bg.wasm'
      - '/rs/crates/bindings/paigasus-wasm/paigasus_wasm_bg.js'
      - '/rs/crates/bindings/paigasus-wasm/paigasus_wasm.js'
      - '/rs/crates/bindings/paigasus-wasm/paigasus_wasm.d.ts'
      - '/rs/crates/bindings/paigasus-wasm/paigasus_wasm_bg.wasm.d.ts'
      - '/rs/crates/bindings/paigasus-wasm/package.json'
```

`build` runs the same `tsc` as `typecheck` and shares the anchor, so it takes them with it. Add the
same block to `test-e2e`'s `inputs` as well: that tier boots the real package.

- [ ] **Step 2: Add the same inputs to both apps**

In `ts/apps/iam-console/moon.yml` and `ts/apps/gateway-console/moon.yml`, add the identical block to
the `inputs` of `build`, `typecheck`, `test` and `test-e2e`, next to the existing
`/ts/packages/paigasus-console-core/src/**/*` lines. `build`, `test` and `test-e2e` carry
`merge: replace`, so a missing line is not inherited from anywhere.

- [ ] **Step 3: Correct the `dependsOn` comment in both apps**

Both files claim `Every package this app compiles is listed`. The app now compiles
`@paigasus/kernel` through `@paigasus/console-core` without an edge. Replace that sentence with:

```yaml
# Every package this app compiles is listed, EXCEPT @paigasus/kernel: the app reaches it through
# @paigasus/console-core, and it needs no build of its own, because the wasm artifacts it loads are
# committed files (SMA-634). The task `inputs` below name those files, which is what makes a kernel
# change select this app.
```

- [ ] **Step 4: Verify that every new input resolves to a tracked file**

`repo:input-liveness` checks `repo:*` tasks only, so it does **not** cover these. Use the same
method the SMA-630 plan used for this package:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"; export PROTO_REPORTER=text
for target in paigasus-console-core-ts:build paigasus-console-core-ts:typecheck paigasus-console-core-ts:test \
              iam-console-ts:build iam-console-ts:test iam-console-ts:test-e2e \
              gateway-console-ts:build gateway-console-ts:test gateway-console-ts:test-e2e; do
  echo "== $target"
  moon task "$target" --json | python3 -c "
import json, sys
d = json.load(sys.stdin)
want = [
  'ts/packages/paigasus-kernel/package.json',
  'rs/crates/bindings/paigasus-wasm/paigasus_wasm_bg.wasm',
  'rs/crates/bindings/paigasus-wasm/paigasus_wasm_bg.js',
  'rs/crates/bindings/paigasus-wasm/paigasus_wasm.js',
  'rs/crates/bindings/paigasus-wasm/paigasus_wasm.d.ts',
  'rs/crates/bindings/paigasus-wasm/paigasus_wasm_bg.wasm.d.ts',
]
have = set(d.get('inputFiles', []))
missing = [p for p in want if p not in have]
print('MISSING:', missing if missing else 'none')
"
done
```

Expected: `MISSING: none` for every target. An input that resolves to no tracked file does not
appear in `inputFiles`, which is how a typo shows up here.

- [ ] **Step 5: Confirm the affected graph still agrees**

```bash
/bin/bash ci/affected-graph/run.sh; echo "rc=$?"
```

Expected: `rc=0`. If a case changed, follow Task 5 Step 3's rule: re-baseline only with a written
reason, and stop and report if the cause is not obvious.

- [ ] **Step 6: Commit**

```bash
git add ts/packages/paigasus-console-core/moon.yml ts/apps/iam-console/moon.yml ts/apps/gateway-console/moon.yml
git commit -m "build(ts): key the console tasks on the kernel and its committed wasm (SMA-634)

Inputs are the only thing that confers affectedness on Moon 2.5.3, so without them a
kernel change would serve a cached console build that holds the old binary. No task deps
and no project edge: the artifacts are tracked files, so no console task compiles Rust.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 9: The console builds assert a real `.wasm` chunk

**Files:**
- Modify: `ts/apps/iam-console/moon.yml`, `ts/apps/gateway-console/moon.yml` (the `build` scripts)

**Interfaces:**
- Consumes: Task 6's kernel use, so that Turbopack has a wasm module to bundle.
- Produces: `moon ci` fails if Turbopack stops bundling the wasm into the standalone tree.

**A subagent may touch only the two files above.**

- [ ] **Step 1: Add the check to `iam-console`**

In the `build` script, after the `server.js` check and before the `dest=` line:

```bash
      # SMA-634. Turbopack bundles @paigasus/wasm into a server chunk, so the standalone tree must
      # hold a REGULAR .wasm file. `-type f` without `-L`: a symlink here would mean the binary is
      # reached outside the tree, which a container would not carry. No pipe into an early-exit
      # reader (repo:actionlint check 13 scans this script).
      chunks="$(find .next/standalone/apps/iam-console/.next/server/chunks -type f -name '*paigasus_wasm_bg*.wasm' | wc -l | tr -d ' ')"
      if [ "$chunks" -lt 1 ]; then
        echo "iam-console: no paigasus_wasm_bg*.wasm chunk in the standalone tree — the kernel's wasm binding was not bundled, and every (console) page would fail at module evaluation" >&2
        exit 1
      fi
```

- [ ] **Step 2: Add the same check to `gateway-console`**

The same block, with `gateway-console` in both the path and the message.

- [ ] **Step 3: Run both builds**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"; export PROTO_REPORTER=text
moon run iam-console-ts:build --force; echo "rc=$?"
moon run gateway-console-ts:build --force; echo "rc=$?"
```

Expected: both exit 0. If a build fails with `Can't resolve './paigasus_wasm_bg.wasm'`, the
installed copy has no binary: run `rm -rf ts/node_modules && pnpm -C ts install` and try again.

- [ ] **Step 4: Prove the check can fail**

```bash
find ts/apps/iam-console/.next/standalone/apps/iam-console/.next/server/chunks -type f -name '*paigasus_wasm_bg*.wasm' -delete
moon run iam-console-ts:build; echo "rc=$?"
```

Expected: Moon replays a cache hit and exits 0 without running the script, or it rebuilds and
regenerates the chunk. Either way this is not a proof, so force the script to run against the
mutated tree:

```bash
cd ts/apps/iam-console
pnpm exec next build >/dev/null
find .next/standalone/apps/iam-console/.next/server/chunks -type f -name '*paigasus_wasm_bg*.wasm' -delete
chunks="$(find .next/standalone/apps/iam-console/.next/server/chunks -type f -name '*paigasus_wasm_bg*.wasm' | wc -l | tr -d ' ')"
echo "chunks=$chunks (expected 0, so the build check would exit 1)"
cd ../../..
moon run iam-console-ts:build --force
```

Record the `chunks=0` line as the control, and confirm the forced rebuild is green again.

- [ ] **Step 5: Commit**

```bash
git add ts/apps/iam-console/moon.yml ts/apps/gateway-console/moon.yml
git commit -m "build(ts): assert the wasm chunk in each console standalone tree (SMA-634)

A console page fails at module evaluation when the kernel's wasm is not bundled, and the
image workflow is not a required check. The assertion runs in the required build instead.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 10: The console image

**Files:**
- Modify: `ts/Dockerfile`, `ci/images/run.sh`, `.github/workflows/images.yml`,
  `docs/ops/RUNBOOK-containers.md`, `rs/CLAUDE.md`

**Interfaces:**
- Consumes: the committed artifacts and the console changes.
- Produces: an image build whose `pnpm install` can resolve the kernel's two `file:` dependencies,
  and a smoke test that loads the wasm.

**A subagent may touch only the five files above.**

- [ ] **Step 1: Pass the narrow build context**

In `ci/images/run.sh`'s `build_console_one`, add one flag after the `-f "$ROOT/ts/Dockerfile"` line:

```bash
    --build-context "bindings=$ROOT/rs/crates/bindings" \
```

Add to the comment above the function:

```bash
# SMA-634: the console install now reaches @paigasus/kernel, whose two `file:` dependencies live
# under rs/crates/bindings — outside the ts/ build context. They arrive through a NAMED context.
# It is rooted at rs/crates/bindings, not at rs/: a sub-directory context carries no .dockerignore
# of its own, so it needs no edit of rs/.dockerignore (which excludes **/*.wasm) and it cannot
# upload rs/target/. The path is $ROOT-anchored, like every other path here, so the function does
# not depend on the caller's working directory. Without the flag the build fails in a recognisable
# way: `COPY --from=bindings` reads `bindings` as an image reference.
```

- [ ] **Step 2: Copy the two binding directories in the builder stage**

In `ts/Dockerfile`, between `COPY . /build/` and the `pnpm install` line:

```dockerfile
# SMA-634. WORKDIR is /build, which is ts/, so @paigasus/kernel's `file:` specifiers
# (../../../rs/crates/bindings/…) resolve to /rs/crates/bindings/…. The two directories arrive
# through the named context that ci/images/run.sh passes. NO Rust is compiled here: the wasm binary
# and its glue are committed files, and the napi directory is copied only because the kernel
# declares the dependency — the image never loads it.
COPY --from=bindings paigasus-wasm /rs/crates/bindings/paigasus-wasm
COPY --from=bindings paigasus-node-bindings /rs/crates/bindings/paigasus-node-bindings
```

After the existing `server.js` assertion, add the chunk assertion:

```dockerfile
RUN set -eu; \
    chunks="$(find "apps/${APP}/.next/standalone/apps/${APP}/.next/server/chunks" -type f -name '*paigasus_wasm_bg*.wasm' | wc -l | tr -d ' ')"; \
    if [ "$chunks" -lt 1 ]; then \
      echo "no paigasus_wasm_bg*.wasm chunk in the standalone tree — the kernel's wasm binding was not bundled" >&2; exit 1; \
    fi
```

- [ ] **Step 3: Probe a `(console)` route in the smoke test**

The present smoke fetches `${base_path}`, which renders the public page and imports no console code,
so nothing in the image path loads the wasm today. In `ci/images/run.sh`'s `smoke_consoles`, after
the HEALTHCHECK block and before the `html=` block, add:

```bash
    # SMA-634. A (console) route, which imports @paigasus/console-core and so evaluates the kernel's
    # wasm. Module evaluation happens BEFORE the session redirect, so a wasm that cannot load gives
    # 500 here while the public page above stays 200. Any non-500 answer passes: the route redirects
    # to the IdP for an unauthenticated request, and this suite has no session.
    if [ "$bad" -eq 0 ]; then
      console_status="$(curl -s -o /dev/null -w '%{http_code}' --max-time 30 --retry 5 --retry-delay 1 \
        --retry-all-errors "${origin}${base_path}/orgs")" || console_status=""
      case "$console_status" in
        ''|5*)
          echo "::error::${app}: ${base_path}/orgs answered '${console_status:-no response}' — a (console) route must not answer 5xx. The usual cause is the kernel's wasm chunk failing to load, which 500s every (console) page. Read 'docker logs ${name}'." >&2
          docker logs "$name" 2>&1 | tail -30 >&2 || true
          ec=1; bad=1
          ;;
        *) echo "  ${app}: ${base_path}/orgs answers ${console_status} (not 5xx)" ;;
      esac
    fi
```

`${base_path}/orgs` serves both zones: `ts/apps/iam-console/app/(console)/orgs/` and
`ts/apps/gateway-console/app/(console)/orgs/` both exist (verified while this plan was written), so
the probe needs no per-zone branch and no new entry beside `app_for` and `base_path_for`. Confirm
both directories before you write the step; if a zone ever loses `orgs`, add a route lookup next to
`base_path_for` rather than probing a path that 404s — a 404 would pass this check and prove
nothing.

- [ ] **Step 4: Add the one filter line, and the two restatements**

`.github/workflows/images.yml`, in the `pull_request` `paths` list, after
`ts/apps/*/package.json`:

```yaml
      # SMA-634: the kernel's `file:` dependency list. ts/Dockerfile's COPY list is hand-maintained
      # against it, and only the Docker path reads it.
      - 'ts/packages/paigasus-kernel/package.json'
```

Nothing else is added. The RUNBOOK's rule is that a file belongs on the filter when a change to it
can break the image build **but not** the ordinary build. Task 8's inputs already make `moon ci`
re-run `next build` for both consoles on a kernel or artifact change, and
`rs/crates/bindings/paigasus-wasm/**` would also put two cold `--release` service builds on every
wasm PR.

Add the same path to the restated lists in `docs/ops/RUNBOOK-containers.md` (the paragraph at
line 33) and in `rs/CLAUDE.md` (the paragraph at line 184).

- [ ] **Step 5: Build and smoke both console images locally, and measure M6**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"; export PROTO_REPORTER=text
ci/images/run.sh all-consoles 2>&1 | tail -40; echo "rc=$?"
```

Expected: both images build and smoke, the new `(console)` row prints a non-5xx status, and no
Rust toolchain appears in the build log. Read the transfer line of the `bindings` context in the
build output (`transferring context: … B`) and record it (M6). It must hold no `target/` directory:
the context is `rs/crates/bindings`, which has none.

If the build fails with `pull access denied for bindings`, the `--build-context` flag did not
reach `docker buildx build`. If it fails on `ERR_PNPM_FETCH` for `@paigasus/wasm`, the two `COPY`
lines are in the wrong stage or after the install.

- [ ] **Step 6: Commit**

```bash
git add ts/Dockerfile ci/images/run.sh .github/workflows/images.yml docs/ops/RUNBOOK-containers.md rs/CLAUDE.md
git commit -m "build(ts): give the console image the kernel's binding directories (SMA-634)

The console install now reaches @paigasus/kernel, whose file: dependencies live outside
the ts/ context. A named context rooted at rs/crates/bindings carries them, so no
dockerignore edit is needed and rs/target cannot be uploaded. No Rust is compiled. The
smoke test gains a (console) route, because the public page it fetched imports no console
code and so never loaded the wasm.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 11: The records, and the full gate graph before the push

**Files:**
- Modify: `ts/CLAUDE.md`

**Interfaces:**
- Consumes: every task above.
- Produces: a branch that is ready to push, with the measurements recorded.

**A subagent may touch only `ts/CLAUDE.md`.** The verification steps touch no file.

- [ ] **Step 1: Replace the SMA-634 entry in `ts/CLAUDE.md`**

Replace the paragraph that begins `**`@paigasus/kernel` cannot load its napi binding inside a Next
build**` and the `@paigasus/console-core`'s `src/prn-tenancy.ts`` paragraph that follows it with:

```markdown
- **`@paigasus/kernel` resolves to its WASM entry, on the server too** (SMA-634, ADR-0022). A Next
  server build cannot load the napi binding from an in-monorepo `file:` or `link:` dependency:
  pnpm installs a `file:` target once, at install time, by the package's `files` allowlist, and
  Turbopack refuses a `link:` symlink whose target is outside its root. So the kernel's `.` export
  is the wasm entry under every condition, and the napi entry is `@paigasus/kernel/napi`, which only
  the kernel's own tests name. The published npm model of `@paigasus/node-bindings` (a loader-only
  package plus seven per-platform packages) is correct and unchanged.
  `rs/crates/bindings/paigasus-wasm/paigasus_wasm_bg.wasm` and its four glue files are **committed**,
  because pnpm links the crate before any build task runs. `moon run
  paigasus-kernel-ts:generate-wasm` is their only writer: run it after a Rust kernel or wasm-binding
  edit, and commit all five. `paigasus-kernel-ts:test` holds them to the source with four checks —
  the committed glue equals a fresh build, the binary's import and export lists equal a fresh
  build's, the committed pair replays all five parity corpora, and (in the console vitest
  `setupFiles`) the pnpm-installed copy equals the committed files. **No check compares the binary
  bytes**: they differ on macOS, Linux arm64 and Linux amd64, while the glue and the interface do
  not. After a `git checkout`, a rebase or a branch switch that replaces those files, run `rm -rf
  ts/node_modules && pnpm -C ts install`: pnpm hard-links a `file:` dependency and does not repair a
  broken link, not even with `--force`. Only ONE host regenerates the artifacts; a second host makes
  different bytes and a diff that says nothing. A conflict in the five files is resolved by taking
  either side and running `generate-wasm` again.
  `@paigasus/console-core`'s `src/prn-tenancy.ts` is now an IAM tenancy adapter over the kernel, not
  an ADR-0005 exception: it keeps IAM's tenancy rule, the UUID guard for a URL segment and a length
  limit for the FFI boundary. `tests/unit/prn-tenancy-delegation.test.ts` mocks the kernel and fails
  if the file parses a PRN itself. A wasm that cannot load 500s EVERY `(console)` page, not only the
  PRN pages, because `src/index.ts` re-exports the module and `@paigasus/wasm` is side-effectful.
  Node prints an `ExperimentalWarning` for the wasm import in every console vitest run; it is
  harmless.
```

- [ ] **Step 2: Run the full gate graph, as CI does**

Copy the command from the root `CLAUDE.md` `ci-targets` block. It is gated against `ci.yml`'s `T=(…)`
array by `repo:affected-smoke`, so it must stay identical:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"; export PROTO_REPORTER=text
moon ci :build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking \
  :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free \
  :redis-connect-single-site :iam-docker-policy-single-site :error-code-single-site \
  :http-extractor-envelope :input-liveness :promtool :observability-drift \
  :nats-permissions :release-parity :release-parity-py :release-parity-ts \
  :publish-metadata :version-lockstep :workflow-credentials :pyo3-stub-drift :ruff-ci \
  :next-public-free :test-e2e \
  --base origin/main \
  --include-relations
```

One local bash cannot satisfy every gate, so re-run these directly and read those results instead of
the `moon ci` verdict for them:

```bash
/bin/bash ci/affected-graph/run.sh                      # repo:affected-smoke needs bash 3.2
/opt/homebrew/bin/bash ci/ruff/run.sh                   # needs bash 4+
/opt/homebrew/bin/bash ci/next-public/run.sh            # needs bash 4+
/opt/homebrew/bin/bash ci/publish-metadata/run.sh       # needs bash 4+
/opt/homebrew/bin/bash ci/actionlint/run.sh             # needs bash 5 AND a healthy pipe
```

`repo:actionlint` prints its pipe capacity in a preflight line. It gives a local verdict only when
that preflight passes; on a 512-byte-pipe host it exits 2 and there is no local verdict.

For an unattributed failure, follow the diagnosis procedure in the root `CLAUDE.md`
(`moon-diagnosis` block): copy `.moon/cache/ciReport.json` and the task's state directory out of the
repository **before** any re-run, then read the failing action's `operations[]` entry whose
`meta.type` is `task-execution`. A re-run overwrites the evidence, and a passing re-run destroys it
just as thoroughly. Use `moon run <target> --force` when a cache hit hides the task.

- [ ] **Step 3: Record M2 (the cost of the gate)**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"; export PROTO_REPORTER=text
time moon run paigasus-kernel-ts:test --force
```

Record the wall time, and the share the four checks add over the previous suite (compare with a run
on `origin/main`). Put both numbers in the PR body.

- [ ] **Step 4: Commit the record**

```bash
git add ts/CLAUDE.md
git commit -m "docs(ts): record the kernel wasm route and its traps (SMA-634)

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

- [ ] **Step 5: Push, and get the arm64 evidence (M5)**

```bash
git push -u origin feature/sma-634-kernel-napi-node-modules
gh workflow run images.yml --ref feature/sma-634-kernel-napi-node-modules
```

The `pull_request` leg of `images.yml` builds amd64 only. The `workflow_dispatch` run covers arm64,
and it is **required before the merge**: the console image is built for both architectures on
`main`. Wait for both, and record the result of the new `(console)` probe on each.

- [ ] **Step 6: Write the PR body**

It must carry: the measured napi result that removed the first design (a link to the measurements
file), the four gate checks and the three control outcomes from Task 3, control 4 from Task 7, the
`chunks=0` control from Task 9, M1, M2, M6, the amd64 and arm64 image results, and the note that
the napi packaging defect stays open by design with `@paigasus/node-bindings` unchanged.

---

## Self-review

**1. Spec coverage.** Each spec section maps to a task: § 5.1 → Task 4; § 5.2 → Task 3 (steps 5, 7);
§ 5.3 → Task 2 and Task 4 step 5; § 5.4 checks 1–3 and controls 1–3 → Task 3, check 4 and control 4
→ Task 7; § 5.5 → Task 4 step 6 and Task 3 step 5; § 6.1 → Task 6 step 1; § 6.2 → Task 6 step 4;
§ 6.3 → Task 6 steps 2, 3, 5; § 6.4 (no ESLint rule) → nothing to do, deliberately; § 7 → Task 8 and
Task 5; § 8 → Task 9; § 9 → Task 10; § 10 → Task 11 step 1; § 11 → the guards in Tasks 2, 3, 9, 10;
§ 12 M1 → Task 2 step 4, M2 → Task 11 step 3, M3 → Task 3 steps 8–10 and Task 7 step 5, M4 → Task 5
step 2 and Task 8 step 5, M5 → Task 11 step 5, M6 → Task 10 step 5; § 14 files → the file structure
table; § 15 acceptance → Tasks 3, 6, 9, 10, 11.

**2. Placeholders.** None: every code step carries the file's real content or the real replacement
text, and every command is runnable as written.

**3. Type consistency.** `assertInstalledWasmMatchesCommitted` is spelled the same in Task 7's
helper, its export and all three setups. `wasm-probe.mjs`'s two modes (`--interfaces`, `--corpus`)
are spelled the same in Task 1 and in Task 3's gate. `generate-wasm.mjs`'s `--pre` and `--post` match
the moon.yml script. The kernel's `./napi` subpath is spelled the same in Task 4's map and in the
five test imports. `TenancyRef`, `TenancyKind`, `ROOT_PRN`, `isUuid`, `parseTenancyPrn`,
`organizationPrn`, `teamPrn` and `projectPrn` keep their existing signatures, so no caller changes.

**Two corrections to the spec, made while writing this plan.**

- Spec § 12's M4 says that `repo:input-liveness` would fail a declared console input that matches no
  tracked file. It would not: that gate reads `repo:*` tasks only (`ci/affected-graph/task_inputs.py`
  header, and its own description). Task 8 step 4 verifies the console inputs with
  `moon task <target> --json` and its `inputFiles` list instead, which is the method the SMA-630 plan
  used for this same package.
- Spec § 5.3 says `build` drops the `.wasm` from its `outputs`. The plan states why this is required
  rather than cosmetic: the file is tracked from Task 3 on, and Moon restores a declared output from
  its cache, which would overwrite a committed file.

**A trap this plan fixes before an implementer can meet it.** `paigasus-kernel-ts:test` rebuilds the
fresh wasm from the CURRENT Rust source at the start of its own script. A negative control that
leaves a Rust mutation in place while the task runs therefore mutates the comparison's other side
too, and the check then passes or fails for the wrong reason. Controls 2 and 3 restore the source
and the glue before the run, so that only the committed binary carries the mutation. The first
draft of Task 3 had the wrong order and would have reported a false result for check 1.

**What no step can decide in advance.** Task 4 step 4 removes `customConditions: ["node"]` from the
kernel's `tsconfig.json`, and only the `tsc` run shows whether another package's resolution depended
on it. The step gives both branches and demands the measured error in the comment if the setting
stays.
