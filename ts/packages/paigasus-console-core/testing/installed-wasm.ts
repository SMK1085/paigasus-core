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
//
// Path arithmetic is done with `node:path`, not `new URL(<literal>, import.meta.url)`: under a
// `@vitest-environment jsdom` test file (three app suites do this per-file), Vite treats the module
// as client code and its asset-URL plugin statically rewrites that exact `new URL(literal,
// import.meta.url)` shape into a dev-server `http://localhost:.../@fs/...` URL instead of leaving
// it as plain runtime code — `createRequire` then rejects it (measured: it does not repro under
// `environment: 'node'`, only under jsdom). `fileURLToPath(import.meta.url)` itself is unaffected;
// only that specific `new URL(...)` pattern is intercepted.
import { createHash } from 'node:crypto';
import { createRequire } from 'node:module';
import { existsSync, readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const HERE = fileURLToPath(import.meta.url);
// testing -> paigasus-console-core -> packages -> ts -> repo root: four levels up.
const ROOT = join(dirname(HERE), '../../../../');
const CRATE_DIR = join(ROOT, 'rs/crates/bindings/paigasus-wasm');
// @paigasus/wasm is the KERNEL's dependency, not this package's, so the require is anchored at the
// kernel's own manifest. Its own exports map does not expose ./package.json, so the anchor is the
// file path, not a package specifier.
const KERNEL_MANIFEST = join(ROOT, 'ts/packages/paigasus-kernel/package.json');
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
    const committed = join(CRATE_DIR, name);
    if (!existsSync(committed)) throw new Error(`${committed} is missing from the tree — the committed wasm artifacts are incomplete.`);
    if (sha256(installed) !== sha256(committed)) {
      throw new Error(`the installed ${name} differs from the committed one (${installed}). ${REPAIR}`);
    }
  }
}
