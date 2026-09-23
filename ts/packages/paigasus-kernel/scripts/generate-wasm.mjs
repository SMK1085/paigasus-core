// SPDX-License-Identifier: Apache-2.0
//
// The guards and the copy of `paigasus-kernel-ts:generate-wasm` (SMA-634 spec § 5.3). The wasm-pack
// call itself stays in moon.yml's `script:` block, because ci/affected-graph/cargo_moon_parity.py
// derives an FFI task from the resolved invocation text (FFI_MARKERS holds `wasm-pack`). A wrapper
// that hides the call would make the task invisible to the A5, A7 and A8 assertions.
//
//   node scripts/generate-wasm.mjs --pre     the guards, and the mtime bump cargo needs
//   node scripts/generate-wasm.mjs --remap   the path-redaction rustflags, as a TOML array
//   node scripts/generate-wasm.mjs --post    the identity rejection, the copy, the cleanup
//
// The task has no "unchanged" early exit. The binary bytes depend on the host (spec F12), so there
// is nothing a rebuild could compare itself against.
import { execFileSync } from 'node:child_process';
import { copyFileSync, existsSync, readFileSync, realpathSync, rmSync, utimesSync, writeFileSync } from 'node:fs';
import { homedir, userInfo } from 'node:os';
import { basename, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

// scripts -> paigasus-kernel -> packages -> ts -> repo root: four `../`.
const ROOT = new URL('../../../../', import.meta.url);
const CRATE = new URL('rs/crates/bindings/paigasus-wasm/', ROOT);
// Its own scratch dir, distinct from the `test` task's `.wasmpack-test-out` (the `build` task runs
// no wasm-pack since SMA-634): wasm-pack wipes its --out-dir at the start of a run, so sharing a name
// with a task that can run concurrently would let a wipe interleave with this task's copy and
// leave a mixed artifact set in the crate directory.
const OUT = new URL('.wasmpack-regen-out/', CRATE);
const GLUE = ['paigasus_wasm.js', 'paigasus_wasm_bg.js', 'paigasus_wasm.d.ts', 'paigasus_wasm_bg.wasm.d.ts'];
const BINARY = 'paigasus_wasm_bg.wasm';
const TOUCH = ['rs/crates/libs/paigasus-kernel/src/lib.rs', 'rs/crates/bindings/paigasus-wasm/src/lib.rs'];

function fail(message) {
  process.stderr.write(`generate-wasm: ${message}\n`);
  process.exit(1);
}

function realpathOrNull(path) {
  try {
    return realpathSync(path);
  } catch {
    return null;
  }
}

// The absolute host paths rustc writes into the binary's panic strings and debug info. The old
// remap was `--remap-path-prefix=/Users=/redacted`, which rewrites the FIRST SEGMENT ONLY: the
// output held `/redacted/smaschek/.rustup/...` and `/redacted/smaschek/.cargo/...`, so the user
// name of whoever regenerated the artifacts shipped in a public Apache-2.0 repository. Each entry
// below maps a whole ROOT instead, so no user name and no machine-specific absolute path survives.
//
// CARGO_HOME and RUSTUP_HOME are honoured, because either can sit outside the home directory. The
// roots are ordered from the least to the most specific, since rustc applies the LAST matching
// remap — but the order is a belt-and-braces measure only: on a default installation both roots sit
// under $HOME, so either precedence removes the user name. `post()` is what proves the output.
function redactions() {
  const home = homedir();
  const cargo = process.env.CARGO_HOME ?? resolve(home, '.cargo');
  const rustup = process.env.RUSTUP_HOME ?? resolve(home, '.rustup');

  const seen = new Set();
  const pairs = [];
  for (const [root, placeholder] of [
    [home, '/redacted-home'],
    [resolve(cargo), '/redacted-cargo'],
    [resolve(rustup), '/redacted-rustup'],
  ]) {
    // A symlinked root reaches rustc resolved, so both spellings are remapped.
    for (const spelling of [root, realpathOrNull(root)]) {
      if (spelling === null || spelling === '' || spelling === '/' || seen.has(spelling)) continue;
      // rustc splits `--remap-path-prefix` on the FIRST `=`, so a root holding one would be parsed
      // into the wrong prefix and the redaction would silently not apply. Fail rather than ship it.
      if (spelling.includes('=')) {
        fail(`the path "${spelling}" holds an "=", which --remap-path-prefix cannot express. Move the directory, or set CARGO_HOME/RUSTUP_HOME to a path without one.`);
      }
      seen.add(spelling);
      pairs.push([spelling, placeholder]);
    }
  }
  return pairs;
}

// The `--config target.wasm32-unknown-unknown.rustflags=<value>` argument the moon.yml script
// passes to cargo. It stays on the `--config` channel and never becomes RUSTFLAGS: cargo reads one
// rustflags source only, so an exported RUSTFLAGS would REPLACE this and ship the raw paths (`pre`
// refuses to run when one is set). A `--config` value also survives a path that holds a space.
// JSON.stringify emits a TOML-compatible array of basic strings and escapes a backslash or a quote.
function remap() {
  const flags = redactions().map(([from, to]) => `--remap-path-prefix=${from}=${to}`);
  if (flags.length === 0) fail('no redaction roots were resolved, so the binary would hold absolute host paths.');
  process.stdout.write(`${JSON.stringify(flags)}\n`);
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
  // Cheap guard before any crate-directory write: confirm every file wasm-pack was meant to
  // produce is actually there. Not a substitute for an atomic rename (rejected — a rename would
  // break pnpm's hard link to the installed copy, spec F14): an I/O failure partway through the
  // copy below can still leave a mixed set, and the remedy there is to re-run the task. This only
  // catches the case where wasm-pack itself produced an incomplete scratch dir.
  for (const name of [...GLUE, BINARY]) {
    if (!existsSync(new URL(name, OUT))) {
      fail(`${name} is missing from the wasm-pack scratch output. wasm-pack did not produce a complete artifact set, and nothing was copied into the crate directory.`);
    }
  }

  const binary = readFileSync(new URL(BINARY, OUT));
  const text = binary.toString('latin1');
  // Two families of marker, and the second is why this guard exists in its present form.
  //
  // The PATH markers catch a remap that did not take effect at all. They are not sufficient: the
  // old `/Users` -> `/redacted` remap produced `/redacted/smaschek/.cargo/...`, which holds neither
  // `/Users/` nor `/home/`, so this guard passed a binary carrying the user name — it searched for
  // the two prefixes AFTER the remap had already rewritten them.
  //
  // The IDENTITY markers close that. They reject the name of the user running the task and the last
  // segment of the home directory, anywhere in the binary, whatever path they appear in.
  const home = homedir();
  const identity = [userInfo().username, basename(home), basename(realpathOrNull(home) ?? '')];
  const markers = new Map();
  for (const value of ['/Users/', '/home/']) markers.set(value, 'an absolute host path');
  // A one-character name would match almost any binary. Two is the shortest that can be meaningful,
  // and a false positive here BLOCKS the regeneration rather than leaking, which is the safe side.
  for (const value of identity) if (value.length >= 2) markers.set(value, 'the identity of the user running this task');

  for (const [marker, kind] of markers) {
    if (text.includes(marker)) {
      fail(
        `the new ${BINARY} holds "${marker}", which is ${kind}. The path redaction did not cover it, and the binary must not be committed into a public repository. Nothing was copied into the crate directory. Widen redactions() in this script — never weaken this guard.`,
      );
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
else if (mode === '--remap') remap();
else if (mode === '--post') post();
else fail(`unknown mode ${JSON.stringify(mode)} — use --pre, --remap or --post`);
