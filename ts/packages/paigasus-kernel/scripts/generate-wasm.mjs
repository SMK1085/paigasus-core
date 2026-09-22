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
import { copyFileSync, existsSync, readFileSync, rmSync, utimesSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

// scripts -> paigasus-kernel -> packages -> ts -> repo root: four `../`.
const ROOT = new URL('../../../../', import.meta.url);
const CRATE = new URL('rs/crates/bindings/paigasus-wasm/', ROOT);
// Its own scratch dir, distinct from the `build` task's `.wasmpack-out` and the `test` task's
// `.wasmpack-test-out`: wasm-pack wipes its --out-dir at the start of a run, so sharing a name
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
