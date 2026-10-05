// SPDX-License-Identifier: Apache-2.0
//
// Optimize the wasm kernel with the proto-pinned binaryen `wasm-opt`, and verify the result
// (SMA-435 spec § 5.4).
//
//   node scripts/optimize-wasm.mjs optimize <out-dir>   optimize <out-dir>/paigasus_wasm_bg.wasm in place
//   node scripts/optimize-wasm.mjs verify <dir>         check <dir>/paigasus_wasm_bg.wasm only
//
// wasm-pack keeps `wasm-opt = false` (rs/crates/bindings/paigasus-wasm/Cargo.toml), because its
// own optimizer downloads a binaryen release that no file in this repository pins (SMA-427 L3).
// This script runs the binaryen that rs/crates/bindings/paigasus-wasm/.prototools pins. It has two
// callers: the `generate-wasm` task in moon.yml (the committed binary) and the `assemble` job of
// .github/workflows/prebuild.yml (the published binary). The flag list lives here only, so the
// two binaries cannot get different flags.
//
// After wasm-opt, the script appends its own custom section `paigasus.wasm-opt` with the payload
// `binaryen=<major>;flags=-O`. `verify` and checks 5 and 6 of tests/committed-wasm.test.ts read it.
//
// Exit 1 is a finding and exit 2 an infrastructure error, as in the repository's gates.
//
// The crate directory comes from this file's own location, as in generate-wasm.mjs. There is no
// environment variable and no flag for it, so nothing can point the script at another pin. The
// unit tests (tests/optimize-wasm.test.ts) copy this file into a fixture tree instead.
import { Buffer } from 'node:buffer';
import { spawnSync } from 'node:child_process';
import { accessSync, constants, existsSync, readFileSync, realpathSync, renameSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

// scripts -> paigasus-kernel -> packages -> ts -> repo root: four `../`, as in generate-wasm.mjs.
const ROOT = new URL('../../../../', import.meta.url);
const CRATE = new URL('rs/crates/bindings/paigasus-wasm/', ROOT);
const CRATE_DIR = fileURLToPath(CRATE);
const PROTOTOOLS = new URL('.prototools', CRATE);
const BINARY = 'paigasus_wasm_bg.wasm';
// Not `paigasus_wasm*`: prebuild.yml stages `.wasmpack-release-out/paigasus_wasm*` for upload, and
// a temporary file must never match that glob.
const TEMP = '.optimize-wasm.tmp';
// The only flags. `-O` needs no `--enable-*`: wasm-opt reads the target_features section (spec F8).
const FLAGS = ['-O'];
const WASM_HEADER = [0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];

/** The custom section that this script appends after wasm-opt. */
export const MARKER_SECTION = 'paigasus.wasm-opt';

// A failure with its exit code. The helpers throw it too, so the drift gate gets the message.
class ScriptError extends Error {
  /**
   * @param {1 | 2} code
   * @param {string} message
   */
  constructor(code, message) {
    super(message);
    this.name = 'ScriptError';
    this.code = code;
  }
}

/** @param {string} message */
const finding = (message) => new ScriptError(1, message);
/** @param {string} message */
const infra = (message) => new ScriptError(2, message);

/**
 * Read an unsigned LEB128 number of at most 32 bits.
 * @param {Uint8Array} bytes
 * @param {number} offset
 * @returns {[number, number]} the value, and the offset after it
 */
function readU32(bytes, offset) {
  let value = 0;
  let position = offset;
  for (let shift = 0; shift < 35; shift += 7) {
    const byte = bytes[position];
    if (byte === undefined) throw finding(`a LEB128 number at byte ${offset} is cut off: the file is not a complete wasm binary`);
    position += 1;
    value += (byte & 0x7f) * 2 ** shift;
    if ((byte & 0x80) === 0) return [value, position];
  }
  throw finding(`a LEB128 number at byte ${offset} is longer than 5 bytes: the file is not a valid wasm binary`);
}

/**
 * @param {number} value
 * @returns {number[]}
 */
function encodeU32(value) {
  const out = [];
  let rest = value;
  do {
    const low = rest & 0x7f;
    rest = Math.floor(rest / 128);
    out.push(rest === 0 ? low : low | 0x80);
  } while (rest !== 0);
  return out;
}

/**
 * The custom sections of a wasm binary, in file order. A section is id 0, a LEB128 size, a LEB128
 * name length, the name, and the payload. Throws a finding when the bytes are not a well-formed
 * sequence of sections.
 * @param {Uint8Array} bytes
 * @returns {{ name: string, payload: Buffer }[]}
 */
function customSections(bytes) {
  const buffer = Buffer.from(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  if (buffer.length < WASM_HEADER.length || WASM_HEADER.some((value, index) => buffer[index] !== value)) {
    throw finding('the file does not start with the wasm magic number and version 1');
  }
  const found = [];
  let position = WASM_HEADER.length;
  while (position < buffer.length) {
    const id = buffer[position];
    const [size, start] = readU32(buffer, position + 1);
    const end = start + size;
    if (end > buffer.length) throw finding(`section ${id} at byte ${position} ends after the end of the file`);
    if (id === 0) {
      const [nameLength, nameStart] = readU32(buffer, start);
      if (nameStart + nameLength > end) throw finding(`the custom section at byte ${position} has a name longer than the section`);
      found.push({ name: buffer.toString('utf8', nameStart, nameStart + nameLength), payload: buffer.subarray(nameStart + nameLength, end) });
    }
    position = end;
  }
  return found;
}

/**
 * @param {Uint8Array} bytes
 * @param {string} name
 * @returns {number} how many custom sections have this name
 */
export function customSectionCount(bytes, name) {
  return customSections(bytes).filter((section) => section.name === name).length;
}

/**
 * @param {Uint8Array} bytes
 * @param {string} name
 * @returns {string[]} the UTF-8 payloads of the custom sections with this name, in file order
 */
export function customSectionPayloads(bytes, name) {
  return customSections(bytes)
    .filter((section) => section.name === name)
    .map((section) => section.payload.toString('utf8'));
}

/**
 * @param {Uint8Array} bytes
 * @param {string} name
 * @param {string} payload
 * @returns {Buffer} a new buffer: the bytes, then one custom section. Imports, exports and glue do not change.
 */
export function appendCustomSection(bytes, name, payload) {
  const nameBytes = Buffer.from(name, 'utf8');
  const content = Buffer.concat([Buffer.from(encodeU32(nameBytes.length)), nameBytes, Buffer.from(payload, 'utf8')]);
  return Buffer.concat([Buffer.from(bytes), Buffer.from([0]), Buffer.from(encodeU32(content.length)), content]);
}

/**
 * The binaryen major version that a .prototools text pins, or null. binaryen tags are
 * `version_<N>`, so the proto version is always <N>.0.0. The `[plugins]` line of the same file
 * holds a `file://` path and does not match.
 * @param {string} text
 * @returns {string | null}
 */
export function pinnedMajor(text) {
  const match = /^wasm-opt[ \t]*=[ \t]*"(\d+)\.0\.0"[ \t\r]*$/m.exec(text);
  return match === null ? null : (match[1] ?? null);
}

/** @returns {string} */
function readPin() {
  let text;
  try {
    text = readFileSync(PROTOTOOLS, 'utf8');
  } catch (error) {
    throw infra(`cannot read ${fileURLToPath(PROTOTOOLS)}: ${error instanceof Error ? error.message : String(error)}`);
  }
  const major = pinnedMajor(text);
  if (major === null) throw infra(`${fileURLToPath(PROTOTOOLS)} has no \`wasm-opt = "<N>.0.0"\` line. binaryen tags are version_<N>, so the pin is always <N>.0.0.`);
  return major;
}

/** @param {string} major */
function markerFor(major) {
  return `binaryen=${major};flags=${FLAGS.join(' ')}`;
}

/**
 * The marker payload that the nested pin demands, for example `binaryen=133;flags=-O`.
 * @returns {string}
 */
export function expectedMarker() {
  return markerFor(readPin());
}

/**
 * True when the TOML text sets `wasm-opt = false` in the table `[section]`, and no other value
 * there. A small line reader, because the lockfile has no TOML parser (spec F17). It accepts spaces
 * around `=` and a trailing comment. A `wasm-opt` key in another table does not count.
 * @param {string} toml
 * @param {string} section for example 'package.metadata.wasm-pack.profile.release'
 * @returns {boolean}
 */
export function wasmOptDisabled(toml, section) {
  let current = '';
  const values = [];
  for (const raw of toml.split(/\r?\n/)) {
    const line = raw.trim();
    const header = /^\[\[?\s*([^\]]+?)\s*\]\]?\s*(?:#.*)?$/.exec(line);
    if (header !== null) {
      current = header[1] ?? '';
      continue;
    }
    if (current !== section) continue;
    const entry = /^wasm-opt\s*=\s*(.*?)\s*(?:#.*)?$/.exec(line);
    if (entry !== null) values.push(entry[1]);
  }
  return values.length > 0 && values.every((value) => value === 'false');
}

// The child environment of every proto call. PROTO_REPORTER=text: proto otherwise prints NDJSON in
// an agent environment and still exits 0 (SMA-609). PROTO_WASM_OPT_VERSION is deleted: proto reads
// it before any .prototools file, so a stale export would beat the pin.
function protoEnv() {
  const env = { ...process.env, PROTO_REPORTER: 'text' };
  delete env.PROTO_WASM_OPT_VERSION;
  return env;
}

// Every proto call runs in the crate directory, so the nested pin applies (spec F6).
/** @param {string[]} args */
function proto(args) {
  const result = spawnSync('proto', args, { cwd: CRATE_DIR, env: protoEnv(), encoding: 'utf8' });
  if (result.error !== undefined) throw infra(`cannot run \`proto ${args.join(' ')}\`: ${result.error.message}. Put the proto shims and bin directories on PATH.`);
  return result;
}

/** @param {string} text */
function lastLine(text) {
  return (
    text
      .split(/\r?\n/)
      .map((line) => line.trim())
      .filter((line) => line !== '')
      .at(-1) ?? ''
  );
}

/** @param {string} path */
function isExecutableFile(path) {
  if (path === '') return false;
  try {
    accessSync(path, constants.X_OK);
    return statSync(path).isFile();
  } catch {
    return false;
  }
}

// Steps 3 to 5 of spec § 5.4.2: install, resolve, compare the version.
/** @param {string} major */
function resolveWasmOpt(major) {
  const install = proto(['install', 'wasm-opt']);
  if (install.status !== 0) {
    throw infra(`\`proto install wasm-opt\` exited ${install.status} in ${CRATE_DIR}. The first run needs network access to github.com.\n${install.stdout}${install.stderr}`);
  }
  const bin = proto(['--reporter', 'text', 'bin', 'wasm-opt']);
  // The LAST line: proto can print an NDJSON preamble before the path (SMA-609).
  const path = lastLine(bin.stdout);
  if (bin.status !== 0 || !isExecutableFile(path)) {
    throw infra(`\`proto bin wasm-opt\` did not give an executable file (exit ${bin.status}, got ${JSON.stringify(path)}). If that looks like JSON, proto's agent-mode output leaked (SMA-609).`);
  }
  const version = spawnSync(path, ['--version'], { encoding: 'utf8' });
  if (version.error !== undefined) throw infra(`cannot run ${path} --version: ${version.error.message}`);
  const actual = lastLine(version.stdout);
  const expected = `wasm-opt version ${major} (version_${major})`;
  if (version.status !== 0 || actual !== expected) {
    throw finding(
      `${path} reports ${JSON.stringify(actual)}, and ${fileURLToPath(PROTOTOOLS)} pins ${JSON.stringify(expected)}. Run \`proto install wasm-opt\` in ${CRATE_DIR} and check that no PROTO_WASM_OPT_VERSION or global pin is set.`,
    );
  }
  return path;
}

/**
 * @param {Uint8Array} bytes
 * @param {string} label
 * @returns {string[]} the sorted `name:kind` export list
 */
function exportNames(bytes, label) {
  let module;
  try {
    module = new WebAssembly.Module(bytes);
  } catch (error) {
    throw finding(`${label} is not a valid wasm module: ${error instanceof Error ? error.message : String(error)}`);
  }
  return WebAssembly.Module.exports(module)
    .map((entry) => `${entry.name}:${entry.kind}`)
    .sort();
}

/** @param {string} outDir an absolute path */
function optimize(outDir) {
  const major = readPin();
  const wasmOpt = resolveWasmOpt(major);
  const input = resolve(outDir, BINARY);
  const temp = resolve(outDir, TEMP);
  if (!existsSync(input)) throw infra(`${input} does not exist. Run wasm-pack into ${outDir} first.`);
  const before = readFileSync(input);

  // Step 6, the optimize-once guard. `-O` is deterministic but not idempotent (spec F10).
  if (customSectionCount(before, MARKER_SECTION) > 0) {
    throw finding(`${input} already has a ${MARKER_SECTION} section, so this script optimized it before. Optimize a fresh wasm-pack output only, never two times.`);
  }
  if (customSectionCount(before, 'name') === 0) {
    throw finding(
      `${input} has no \`name\` section, so it is not a raw wasm-pack release output. There are three causes: (1) wasm-pack optimized it itself: check that rs/crates/bindings/paigasus-wasm/Cargo.toml keeps \`wasm-opt = false\`; (2) another tool optimized it; (3) a rustc, wasm-bindgen or \`strip\` change removed the section (SMA-435 spec R2). For cause 3, remove the \`name\` half of this guard. The ${MARKER_SECTION} marker stays the real guard.`,
    );
  }
  const inputExports = exportNames(before, input);

  // The proto version is for the log only: Moon can run another proto (spec S2). Read it before
  // the rename, so that no failure path can follow the rename (spec § 5.4.2 step 10).
  const protoVersion = lastLine(proto(['--version']).stdout) || 'proto version unknown';

  let done = false;
  // A temporary file from a killed run must not survive into this one.
  rmSync(temp, { force: true });
  try {
    // Step 7.
    const run = spawnSync(wasmOpt, [input, ...FLAGS, '-o', temp], { encoding: 'utf8' });
    if (run.error !== undefined) throw infra(`cannot run ${wasmOpt}: ${run.error.message}`);
    if (run.status !== 0) throw finding(`wasm-opt exited ${run.status} on ${input}:\n${run.stderr}`);
    if (!existsSync(temp)) throw finding(`wasm-opt exited 0 but wrote no ${temp}`);
    const after = readFileSync(temp);

    // Step 8.
    const outputExports = exportNames(after, 'the wasm-opt output');
    const names = customSectionCount(after, 'name');
    if (names !== 0) throw finding(`the wasm-opt output still has ${names} name section(s). \`-O\` removes it; check the flags in this script.`);
    if (customSectionCount(after, MARKER_SECTION) !== 0) throw finding(`the wasm-opt output already has a ${MARKER_SECTION} section.`);
    if (outputExports.join(',') !== inputExports.join(',')) {
      throw finding(`the wasm-opt output exports [${outputExports.join(', ')}] and the input exports [${inputExports.join(', ')}]. wasm-opt must keep every export.`);
    }

    // Step 9. A rename is correct here: the input is in a scratch out-dir, and no pnpm hard link
    // points at it (the SMA-634 F14 rule applies to the crate copy only).
    const marked = appendCustomSection(after, MARKER_SECTION, markerFor(major));
    writeFileSync(temp, marked);
    renameSync(temp, input);
    done = true;

    process.stdout.write(`optimize-wasm: ${input}: ${before.length} -> ${marked.length} bytes, wasm-opt version ${major}, flags ${FLAGS.join(' ')}, ${protoVersion}\n`);
  } finally {
    // Step 10: on every failure path the input is unchanged and no temporary file remains.
    if (!done) rmSync(temp, { force: true });
  }
}

/**
 * Check an optimized binary: exactly one marker section with the payload that the pin demands, and
 * no `name` section. Throws an Error with the reason on a finding. `verify` and the drift gate
 * (tests/committed-wasm.test.ts) share this function, so they cannot disagree.
 * @param {Uint8Array} bytes
 * @param {string} file the name to use in the message
 * @returns {string} the marker payload
 */
export function assertOptimized(bytes, file) {
  const expected = expectedMarker();
  const payloads = customSectionPayloads(bytes, MARKER_SECTION);
  if (payloads.length !== 1) {
    throw finding(`${file} has ${payloads.length} ${MARKER_SECTION} sections, not 1. Run \`node scripts/optimize-wasm.mjs optimize\` on a fresh wasm-pack output exactly once.`);
  }
  if (payloads[0] !== expected) {
    throw finding(`${file} carries ${JSON.stringify(payloads[0])}, and ${fileURLToPath(PROTOTOOLS)} demands ${JSON.stringify(expected)}.`);
  }
  const names = customSectionCount(bytes, 'name');
  if (names !== 0) throw finding(`${file} has ${names} name section(s). An optimized binary has none.`);
  return expected;
}

/** @param {string} dir an absolute path */
function verify(dir) {
  const file = resolve(dir, BINARY);
  if (!existsSync(file)) throw infra(`${file} does not exist`);
  const expected = assertOptimized(readFileSync(file), file);
  process.stdout.write(`optimize-wasm: ${file} carries ${expected}\n`);
}

// realpath on both sides: Node resolves import.meta.url through symlinks, but process.argv[1] keeps
// the path as given (macOS /var is a symlink to /private/var). A string comparison would then
// never run main(), and the script would exit 0 having done nothing.
function isEntryPoint() {
  const entry = process.argv[1];
  if (entry === undefined) return false;
  try {
    return realpathSync(entry) === realpathSync(fileURLToPath(import.meta.url));
  } catch {
    return false;
  }
}

function main() {
  const args = process.argv.slice(2);
  const [mode, target] = args;
  try {
    if (args.length !== 2 || target === undefined || (mode !== 'optimize' && mode !== 'verify')) {
      throw infra(`usage: node scripts/optimize-wasm.mjs optimize <out-dir> | verify <dir> (got ${JSON.stringify(args)})`);
    }
    if (mode === 'optimize') optimize(resolve(target));
    else verify(resolve(target));
  } catch (error) {
    if (error instanceof ScriptError) {
      process.stderr.write(`optimize-wasm: ${error.message}\n`);
      process.exit(error.code);
    }
    process.stderr.write(`optimize-wasm: unexpected error (infrastructure): ${error instanceof Error ? (error.stack ?? error.message) : String(error)}\n`);
    process.exit(2);
  }
}

if (isEntryPoint()) main();
