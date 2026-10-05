// SPDX-License-Identifier: Apache-2.0
// SMA-536 — the installed-typings preflight.
//
// `tsc` resolves @paigasus/node-bindings and @paigasus/wasm through ts/node_modules. pnpm installs
// a `file:` dependency as hard links. A rename-write of a committed file (napi, generate-wasm,
// version-lockstep --write, most editors, BSD `sed -i`) breaks the link, and the installed copy
// then stays old. pnpm does not repair it (ts/CLAUDE.md).
//
// This script runs first in every `tsc` task whose package.json closure holds a `file:` binding.
// It compares, byte for byte, the installed copy of each binding's package.json and of each
// `.d.ts` in the binding's `files` list with the committed file. When `tsc` runs after it, `tsc`
// reads bytes equal to the committed files, so an input on a committed file is a real cache key.
// ci/affected-graph/cargo_moon_parity.py A12 asserts the inputs and the order.
//
// Exit 0: every compared file is equal.
// Exit 1: a file differs, or the installed copy is missing. The message names each file and the
//         repair command.
// Exit 2: an infrastructure error (an unparseable package.json, a `file:` path that does not
//         exist, a listed `.d.ts` that is not committed). It never exits 0 on an error.
//
// usage: node ts/scripts/check-installed-bindings.mjs [--ts-root <dir>]
//   With no argument it checks the ts/ directory that holds this script, so one invocation works
//   from any task. --ts-root exists for the tests.
import { existsSync, readFileSync, readdirSync, realpathSync, statSync } from 'node:fs';
import { dirname, join, relative, resolve } from 'node:path';
import process from 'node:process';

const REPAIR = 'rm -rf ts/node_modules && pnpm -C ts install';
const DEP_FIELDS = ['dependencies', 'devDependencies', 'peerDependencies'];
const PACKAGE_PARENTS = ['packages', 'apps'];

class InfraError extends Error {}

function readJson(path) {
  let text;
  try {
    text = readFileSync(path, 'utf8');
  } catch (err) {
    throw new InfraError(`cannot read ${path}: ${err.message}`);
  }
  try {
    return JSON.parse(text);
  } catch (err) {
    throw new InfraError(`${path} is not valid JSON: ${err.message}`);
  }
}

function isPlainObject(value) {
  return value !== null && typeof value === 'object' && !Array.isArray(value);
}

function tsRootFrom(argv) {
  const at = argv.indexOf('--ts-root');
  if (at === -1) return resolve(import.meta.dirname, '..');
  const value = argv[at + 1];
  if (value === undefined || value.startsWith('--')) throw new InfraError('--ts-root needs a directory');
  return resolve(value);
}

// Every `file:` dependency that a ts/packages/* or ts/apps/* package.json declares.
function fileDependencies(tsRoot) {
  const found = [];
  for (const parent of PACKAGE_PARENTS) {
    const parentDir = join(tsRoot, parent);
    if (!existsSync(parentDir)) continue;
    for (const entry of readdirSync(parentDir, { withFileTypes: true })) {
      if (!entry.isDirectory()) continue;
      const declaringDir = join(parentDir, entry.name);
      const manifestPath = join(declaringDir, 'package.json');
      if (!existsSync(manifestPath)) continue;
      const manifest = readJson(manifestPath);
      if (!isPlainObject(manifest)) throw new InfraError(`${manifestPath} is not a JSON object`);
      for (const field of DEP_FIELDS) {
        const deps = manifest[field] ?? {};
        if (!isPlainObject(deps)) throw new InfraError(`${manifestPath}: \`${field}\` is not an object`);
        for (const [name, spec] of Object.entries(deps)) {
          if (typeof spec === 'string' && spec.startsWith('file:')) {
            found.push({ name, declaringDir, committedDir: resolve(declaringDir, spec.slice('file:'.length)) });
          }
        }
      }
    }
  }
  return found;
}

// The files `tsc` reads from a binding: its package.json (`types`, `exports`) and each `.d.ts`
// in its `files` list. The runtime glue, the `.wasm` and the `.node` are out of scope (spec N4).
function comparedFiles(dep) {
  if (!existsSync(dep.committedDir) || !statSync(dep.committedDir).isDirectory()) {
    throw new InfraError(`${dep.name}: the \`file:\` path ${dep.committedDir} does not exist`);
  }
  const manifestPath = join(dep.committedDir, 'package.json');
  const manifest = readJson(manifestPath);
  if (!isPlainObject(manifest)) throw new InfraError(`${manifestPath} is not a JSON object`);
  const files = manifest.files ?? [];
  if (!Array.isArray(files)) throw new InfraError(`${manifestPath}: \`files\` is not a list`);
  const typings = files.filter((file) => typeof file === 'string' && file.endsWith('.d.ts'));
  for (const file of typings) {
    if (!existsSync(join(dep.committedDir, file))) {
      throw new InfraError(`${manifestPath}: \`files\` lists ${file}, which is not committed in ${dep.committedDir}`);
    }
  }
  return ['package.json', ...typings];
}

function compare(dep, repoRoot) {
  const show = (path) => relative(repoRoot, path);
  const files = comparedFiles(dep);
  const link = join(dep.declaringDir, 'node_modules', dep.name);
  let installedDir;
  try {
    installedDir = realpathSync(link);
  } catch {
    return [`${dep.name}: no installed copy at ${show(link)}`];
  }
  const problems = [];
  for (const file of files) {
    const committed = join(dep.committedDir, file);
    const installed = join(installedDir, file);
    if (!existsSync(installed)) {
      problems.push(`${dep.name}: the installed copy has no ${file} (expected ${show(installed)})`);
    } else if (!readFileSync(committed).equals(readFileSync(installed))) {
      problems.push(`${dep.name}: the installed ${file} differs from the committed ${show(committed)}`);
    }
  }
  return problems;
}

function main() {
  const tsRoot = tsRootFrom(process.argv.slice(2));
  const repoRoot = dirname(tsRoot);
  const deps = fileDependencies(tsRoot);
  const problems = deps.flatMap((dep) => compare(dep, repoRoot));
  if (problems.length > 0) {
    const lines = [
      'installed-bindings: the installed binding typings are not the committed files, so `tsc` would read stale typings.',
      ...problems.map((problem) => `  - ${problem}`),
      '',
      `Repair: ${REPAIR}`,
      'pnpm installs a `file:` dependency as hard links. A rename-write of a committed file breaks the link, and pnpm does not repair it (ts/CLAUDE.md).',
    ];
    process.stderr.write(`${lines.join('\n')}\n`);
    return 1;
  }
  process.stdout.write(`installed-bindings: checked ${deps.length} file: dependencies; the installed typings equal the committed files\n`);
  return 0;
}

try {
  process.exitCode = main();
} catch (err) {
  const kind = err instanceof InfraError ? 'infrastructure error' : 'unexpected error';
  process.stderr.write(`installed-bindings: ${kind}: ${err.message}\n`);
  process.exitCode = 2;
}
