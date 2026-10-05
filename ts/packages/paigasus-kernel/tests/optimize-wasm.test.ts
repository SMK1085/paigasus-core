// SPDX-License-Identifier: Apache-2.0
//
// Unit tests for scripts/optimize-wasm.mjs (SMA-435 spec § 8). No test downloads anything.
//
// Each command-line test builds a fixture tree in a temporary directory: a COPY of the script at
// <tmp>/ts/packages/paigasus-kernel/scripts/, a <tmp>/rs/crates/bindings/paigasus-wasm/.prototools,
// and two stubs first on PATH. The script finds its crate directory from its own location, so the
// copy reads the fixture pin and not the real one. That copy is the only seam. Production has none.
//
//   stub `proto`     logs each call (argv, cwd, two env values); `bin wasm-opt` prints the stub path
//   stub `wasm-opt`  `--version` prints a configurable version; otherwise it logs its argv, copies a
//                    configurable file (default: its input) to the `-o` path, then exits STUB_RC
//
// The input is a tiny wasm module, built from the bytes below, with a `name` section like a raw
// wasm-pack release output. `inspect` reads a binary with V8 in a child process, which is an
// independent check of the section helpers under test.
import { Buffer } from 'node:buffer';
import { spawnSync } from 'node:child_process';
import { chmodSync, copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { delimiter, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { afterEach, describe, expect, it } from 'vitest';
import { appendCustomSection, customSectionCount, customSectionPayloads, MARKER_SECTION, pinnedMajor, wasmOptDisabled } from '../scripts/optimize-wasm.mjs';

const SCRIPT = fileURLToPath(new URL('../scripts/optimize-wasm.mjs', import.meta.url));
const MARKER_133 = 'binaryen=133;flags=-O';
const PLUGINS = '[plugins]\nwasm-opt = "file://../../../../.proto/plugins/binaryen.toml"\n';

// A minimal valid module: `(func (result i32) i32.const 42)`, exported as `sum`.
const HEADER = [0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00]; // "\0asm", version 1
const TYPE = [0x01, 0x05, 0x01, 0x60, 0x00, 0x01, 0x7f]; // id 1, 5 bytes: one type, () -> i32
const FUNCTION = [0x03, 0x02, 0x01, 0x00]; // id 3, 2 bytes: one function of type 0
const EXPORT = [0x07, 0x07, 0x01, 0x03, 0x73, 0x75, 0x6d, 0x00, 0x00]; // id 7, 7 bytes: "sum" -> function 0
const CODE = [0x0a, 0x06, 0x01, 0x04, 0x00, 0x41, 0x2a, 0x0b]; // id 10, 6 bytes: one body, no locals, i32.const 42, end
// id 0, 13 bytes: the name "name" (4 bytes), then subsection 1 (function names), 6 bytes: one entry, 0 -> "sum"
const NAME = [0x00, 0x0d, 0x04, 0x6e, 0x61, 0x6d, 0x65, 0x01, 0x06, 0x01, 0x00, 0x03, 0x73, 0x75, 0x6d];

function wasm(...parts: number[][]): Buffer {
  return Buffer.from(parts.flat());
}

const RAW = wasm(HEADER, TYPE, FUNCTION, EXPORT, CODE, NAME); // what wasm-pack writes
const OPTIMIZED = wasm(HEADER, TYPE, FUNCTION, EXPORT, CODE); // what `wasm-opt -O` writes
const NO_EXPORT = wasm(HEADER, TYPE, FUNCTION, CODE);

interface Inspection {
  names: number;
  markers: string[];
  exports: string[];
}

const INSPECT = [
  "const fs = require('node:fs');",
  'const m = new WebAssembly.Module(fs.readFileSync(0));',
  "const markers = WebAssembly.Module.customSections(m, 'paigasus.wasm-opt').map((b) => Buffer.from(b).toString('utf8'));",
  "process.stdout.write(JSON.stringify({ names: WebAssembly.Module.customSections(m, 'name').length, markers, exports: WebAssembly.Module.exports(m).map((e) => e.name) }));",
].join('\n');

// V8's own reading of a binary, in a child process.
function inspect(bytes: Uint8Array): Inspection {
  const result = spawnSync(process.execPath, ['-e', INSPECT], { input: bytes, encoding: 'utf8' });
  if (result.status !== 0) throw new Error(`V8 rejected the binary: ${result.stderr}`);
  return JSON.parse(result.stdout) as Inspection;
}

// CommonJS on purpose: the fixture tree has no package.json, so Node runs these extensionless
// files as CommonJS.
const STUB_PROTO = [
  "const fs = require('node:fs');",
  'const args = process.argv.slice(2);',
  'const call = { args, cwd: process.cwd(), pin: process.env.PROTO_WASM_OPT_VERSION ?? null, reporter: process.env.PROTO_REPORTER ?? null };',
  "fs.appendFileSync(process.env.STUB_LOG, JSON.stringify(call) + '\\n');",
  "if (args[0] === '--version') { process.stdout.write('proto 0.61.1\\n'); process.exit(0); }",
  "if (args[0] === 'install') process.exit(Number(process.env.STUB_INSTALL_RC ?? '0'));",
  "if (args.join(' ') === '--reporter text bin wasm-opt') {",
  '  if (process.env.STUB_NDJSON === \'1\') process.stdout.write(\'{"type":"message","message":"Detected an AI agent environment"}\\n\');',
  "  process.stdout.write((process.env.STUB_BIN ?? process.env.STUB_WASM_OPT) + '\\n');",
  '  process.exit(0);',
  '}',
  "process.stderr.write('stub proto: unexpected arguments ' + JSON.stringify(args) + '\\n');",
  'process.exit(64);',
].join('\n');

const STUB_WASM_OPT = [
  "const fs = require('node:fs');",
  'const args = process.argv.slice(2);',
  "if (args[0] === '--version') {",
  "  const v = process.env.STUB_VERSION ?? '133';",
  "  process.stdout.write('wasm-opt version ' + v + ' (version_' + v + ')\\n');",
  '  process.exit(0);',
  '}',
  'fs.writeFileSync(process.env.STUB_ARGV, JSON.stringify(args));',
  "const o = args.indexOf('-o');",
  'if (o >= 0) fs.copyFileSync(process.env.STUB_OUTPUT ?? args[0], args[o + 1]);',
  "const rc = Number(process.env.STUB_RC ?? '0');",
  "if (rc !== 0) process.stderr.write('stub wasm-opt: failed on purpose\\n');",
  'process.exit(rc);',
].join('\n');

interface Fixture {
  root: string;
  script: string;
  crate: string;
  out: string;
  input: string;
  temp: string;
  stubs: string;
  log: string;
  argv: string;
}

interface Run {
  status: number | null;
  stdout: string;
  stderr: string;
}

interface ProtoCall {
  args: string[];
  cwd: string;
  pin: string | null;
  reporter: string | null;
}

const roots: string[] = [];
afterEach(() => {
  for (const root of roots.splice(0)) rmSync(root, { recursive: true, force: true });
});

function fixture(pin: string = 'wasm-opt = "133.0.0"\n', input: Buffer = RAW): Fixture {
  // realpath: on macOS the temporary directory sits behind the /var -> /private/var symlink, and
  // the argv and cwd assertions compare path strings.
  const root = realpathSync(mkdtempSync(join(tmpdir(), 'optimize-wasm-')));
  roots.push(root);
  const scripts = join(root, 'ts/packages/paigasus-kernel/scripts');
  const crate = join(root, 'rs/crates/bindings/paigasus-wasm');
  const out = join(crate, '.wasmpack-regen-out');
  const stubs = join(root, 'stubs');
  for (const dir of [scripts, out, stubs]) mkdirSync(dir, { recursive: true });
  const script = join(scripts, 'optimize-wasm.mjs');
  copyFileSync(SCRIPT, script);
  writeFileSync(join(crate, '.prototools'), `${pin}\n${PLUGINS}`);
  writeFileSync(join(out, 'paigasus_wasm_bg.wasm'), input);
  for (const [name, body] of [
    ['proto', STUB_PROTO],
    ['wasm-opt', STUB_WASM_OPT],
  ] as const) {
    writeFileSync(join(stubs, name), `#!${process.execPath}\n${body}\n`);
    chmodSync(join(stubs, name), 0o755);
  }
  return {
    root,
    script,
    crate,
    out,
    input: join(out, 'paigasus_wasm_bg.wasm'),
    temp: join(out, '.optimize-wasm.tmp'),
    stubs,
    log: join(root, 'proto-calls.jsonl'),
    argv: join(root, 'wasm-opt-argv.json'),
  };
}

function file(f: Fixture, name: string, bytes: Buffer): string {
  const path = join(f.root, name);
  writeFileSync(path, bytes);
  return path;
}

function run(f: Fixture, args: string[], env: Record<string, string> = {}, options: { cwd?: string; script?: string } = {}): Run {
  const result = spawnSync(process.execPath, [options.script ?? f.script, ...args], {
    cwd: options.cwd ?? f.root,
    encoding: 'utf8',
    env: { ...process.env, PATH: `${f.stubs}${delimiter}${process.env.PATH ?? ''}`, STUB_LOG: f.log, STUB_ARGV: f.argv, STUB_WASM_OPT: join(f.stubs, 'wasm-opt'), ...env },
  });
  return { status: result.status, stdout: result.stdout, stderr: result.stderr };
}

function protoCalls(f: Fixture): ProtoCall[] {
  if (!existsSync(f.log)) return [];
  return readFileSync(f.log, 'utf8')
    .trim()
    .split('\n')
    .map((line) => JSON.parse(line) as ProtoCall);
}

function wasmOptArgv(f: Fixture): string[] {
  return JSON.parse(readFileSync(f.argv, 'utf8')) as string[];
}

function optimizedOutput(f: Fixture): Record<string, string> {
  return { STUB_OUTPUT: file(f, 'optimized.wasm', OPTIMIZED) };
}

function expectOptimized(f: Fixture, r: Run): void {
  expect(r.status, r.stderr).toBe(0);
  expect(inspect(readFileSync(f.input))).toEqual({ names: 0, markers: [MARKER_133], exports: ['sum'] });
  expect(existsSync(f.temp)).toBe(false);
}

function expectUntouched(f: Fixture, input: Buffer = RAW): void {
  expect(readFileSync(f.input).equals(input)).toBe(true);
  expect(existsSync(f.temp)).toBe(false);
}

const RELEASE = 'package.metadata.wasm-pack.profile.release';

function cargoToml(body: string): string {
  return `[package]\nname = "x"\n\n[${RELEASE}]\n${body}\n`;
}

const AFTER_STEP_7: { label: string; env: (f: Fixture) => Record<string, string>; message: RegExp }[] = [
  { label: 'wasm-opt exits non-zero', env: (f) => ({ ...optimizedOutput(f), STUB_RC: '3' }), message: /exited 3/ },
  { label: 'the output keeps a name section', env: () => ({}), message: /name section/ },
  { label: 'the output is not valid wasm', env: (f) => ({ STUB_OUTPUT: file(f, 'garbage.wasm', Buffer.from('not wasm at all')) }), message: /not a valid wasm module/ },
  { label: 'the output drops an export', env: (f) => ({ STUB_OUTPUT: file(f, 'no-export.wasm', NO_EXPORT) }), message: /exports/ },
];

describe('optimize-wasm.mjs', { timeout: 60_000 }, () => {
  describe('the fixture bytes', () => {
    it('are modules that V8 accepts, and only RAW has a name section', () => {
      expect(inspect(RAW)).toEqual({ names: 1, markers: [], exports: ['sum'] });
      expect(inspect(OPTIMIZED)).toEqual({ names: 0, markers: [], exports: ['sum'] });
      expect(inspect(NO_EXPORT).exports).toEqual([]);
    });
  });

  describe('the section helpers', () => {
    it('count custom sections as V8 does', () => {
      expect(customSectionCount(RAW, 'name')).toBe(1);
      expect(customSectionCount(OPTIMIZED, 'name')).toBe(0);
    });

    it('round-trip a payload longer than 127 bytes (a two-byte LEB128 size)', () => {
      const payload = 'x'.repeat(200);
      const out = appendCustomSection(OPTIMIZED, 'test.section', payload);
      expect(out.readUInt8(OPTIMIZED.length)).toBe(0);
      expect(out.readUInt8(OPTIMIZED.length + 1) & 0x80).toBe(0x80);
      expect(customSectionPayloads(out, 'test.section')).toEqual([payload]);
      expect(inspect(out).exports).toEqual(['sum']);
    });

    it('reject bytes that are not wasm', () => {
      expect(() => customSectionCount(Buffer.from('not wasm'), 'name')).toThrow(/magic/);
    });

    it('read the pin and ignore the plugin line', () => {
      expect(pinnedMajor(`wasm-opt = "133.0.0"\n${PLUGINS}`)).toBe('133');
      expect(pinnedMajor(PLUGINS)).toBeNull();
      expect(pinnedMajor('wasm-opt = "133.1.0"\n')).toBeNull();
    });
  });

  describe('the check 7 reader', () => {
    it.each(['wasm-opt=false', 'wasm-opt = false # note', '  wasm-opt = false'])('accepts %j', (line) => {
      expect(wasmOptDisabled(cargoToml(line), RELEASE)).toBe(true);
    });

    it.each(["wasm-opt = ['-O']", "wasm-opt = ['-O', '-g']", 'wasm-opt = true', ''])('rejects %j', (line) => {
      expect(wasmOptDisabled(cargoToml(line), RELEASE)).toBe(false);
    });

    it('rejects a wasm-opt = false line that sits in another section', () => {
      expect(wasmOptDisabled(`[${RELEASE}]\n\n[package.metadata.wasm-pack.profile.dev]\nwasm-opt = false\n`, RELEASE)).toBe(false);
    });
  });

  describe('optimize', () => {
    it('optimizes a raw input with the pinned wasm-opt, from the crate directory', () => {
      const f = fixture();
      const r = run(f, ['optimize', f.out], optimizedOutput(f));
      expectOptimized(f, r);
      expect(wasmOptArgv(f)).toEqual([f.input, '-O', '-o', f.temp]);
      expect(r.stdout).toMatch(/wasm-opt version 133/);
      const calls = protoCalls(f);
      expect(calls.map((c) => c.args.join(' '))).toEqual(expect.arrayContaining(['install wasm-opt', '--reporter text bin wasm-opt']));
      for (const call of calls) {
        expect(call.cwd).toBe(f.crate);
        expect(call.reporter).toBe('text');
      }
    });

    it('takes the last line of `proto bin`, after an NDJSON preamble (SMA-609)', () => {
      const f = fixture();
      expectOptimized(f, run(f, ['optimize', f.out], { ...optimizedOutput(f), STUB_NDJSON: '1' }));
    });

    it('hides PROTO_WASM_OPT_VERSION from proto, which would beat the pin', () => {
      const f = fixture();
      expectOptimized(f, run(f, ['optimize', f.out], { ...optimizedOutput(f), PROTO_WASM_OPT_VERSION: '1.0.0' }));
      const calls = protoCalls(f);
      expect(calls.length).toBeGreaterThan(0);
      for (const call of calls) expect(call.pin).toBeNull();
    });

    it.each(['wasm-opt="133.0.0"\n', 'wasm-opt = "133.0.0"   \n', 'wasm-opt\t=\t"133.0.0"\r\n'])('reads the pin %j', (pin) => {
      const f = fixture(pin);
      expectOptimized(f, run(f, ['optimize', f.out], optimizedOutput(f)));
    });

    it('exits 2 when the nested .prototools has no pin', () => {
      const f = fixture('');
      const r = run(f, ['optimize', f.out], optimizedOutput(f));
      expect(r.status, r.stderr).toBe(2);
      expect(r.stderr).toMatch(/wasm-opt = "<N>\.0\.0"/);
      expectUntouched(f);
    });

    it('exits 1 on another wasm-opt version and names both', () => {
      const f = fixture();
      const r = run(f, ['optimize', f.out], { ...optimizedOutput(f), STUB_VERSION: '132' });
      expect(r.status, r.stderr).toBe(1);
      expect(r.stderr).toMatch(/version_132/);
      expect(r.stderr).toMatch(/version_133/);
      expect(existsSync(f.argv)).toBe(false);
      expectUntouched(f);
    });

    it('exits 2 when proto install fails', () => {
      const f = fixture();
      const r = run(f, ['optimize', f.out], { ...optimizedOutput(f), STUB_INSTALL_RC: '1' });
      expect(r.status, r.stderr).toBe(2);
      expectUntouched(f);
    });

    it.each(['a path that does not exist', 'a directory'])('exits 2 when proto bin prints %s', (kind) => {
      const f = fixture();
      const bin = kind === 'a directory' ? f.root : join(f.root, 'no-such-wasm-opt');
      const r = run(f, ['optimize', f.out], { ...optimizedOutput(f), STUB_BIN: bin });
      expect(r.status, r.stderr).toBe(2);
      expectUntouched(f);
    });

    it('exits 1 on an input with no name section', () => {
      const f = fixture(undefined, OPTIMIZED);
      const r = run(f, ['optimize', f.out], optimizedOutput(f));
      expect(r.status, r.stderr).toBe(1);
      expect(r.stderr).toMatch(/Cargo\.toml/);
      expectUntouched(f, OPTIMIZED);
    });

    it('exits 1 on an input that already has the marker', () => {
      const marked = appendCustomSection(RAW, MARKER_SECTION, MARKER_133);
      const f = fixture(undefined, marked);
      const r = run(f, ['optimize', f.out], optimizedOutput(f));
      expect(r.status, r.stderr).toBe(1);
      expect(r.stderr).toMatch(/optimized it before/);
      expectUntouched(f, marked);
    });

    it.each(AFTER_STEP_7)('$label: exit 1, the input is unchanged, no temporary file remains', ({ env, message }) => {
      const f = fixture();
      const r = run(f, ['optimize', f.out], env(f));
      expect(r.status, r.stderr).toBe(1);
      expect(r.stderr).toMatch(message);
      expectUntouched(f);
    });

    // Review Focus 1.
    it('runs when it is started through a symlinked path', () => {
      // Node resolves import.meta.url through symlinks but keeps process.argv[1] as given. A naive
      // entry-point check then never runs main(), and the script exits 0 having done nothing.
      const f = fixture();
      const link = join(f.root, 'link');
      symlinkSync(f.root, link);
      const r = run(f, ['optimize', f.out], optimizedOutput(f), { script: join(link, 'ts/packages/paigasus-kernel/scripts/optimize-wasm.mjs') });
      expectOptimized(f, r);
    });

    // Review Focus 2.
    it('replaces a temporary file that a killed run left behind', () => {
      const f = fixture();
      writeFileSync(f.temp, 'left over');
      expectOptimized(f, run(f, ['optimize', f.out], optimizedOutput(f)));
    });

    // Review Focus 3.
    it('resolves a relative out-dir against the working directory', () => {
      const f = fixture();
      const r = run(f, ['optimize', '.wasmpack-regen-out'], optimizedOutput(f), { cwd: f.crate });
      expectOptimized(f, r);
      expect(wasmOptArgv(f)[0]).toBe(f.input);
    });

    // Review Focus 4.
    it('exits 2 when the out-dir holds no binary', () => {
      const f = fixture();
      rmSync(f.input);
      const r = run(f, ['optimize', f.out], optimizedOutput(f));
      expect(r.status, r.stderr).toBe(2);
      expect(existsSync(f.temp)).toBe(false);
    });

    // Review Focus 5.
    it.each<[string[]]>([[[]], [['optimize']], [['optimise', 'OUT']], [['optimize', 'OUT', 'extra']]])('refuses the arguments %j with exit 2', (args) => {
      const f = fixture();
      const r = run(
        f,
        args.map((a) => (a === 'OUT' ? f.out : a)),
      );
      expect(r.status, r.stderr).toBe(2);
      expect(r.stderr).toMatch(/usage/);
      expectUntouched(f);
    });
  });

  describe('verify', () => {
    it('passes on a binary with the pinned marker and no name section, and calls no proto', () => {
      const f = fixture(undefined, appendCustomSection(OPTIMIZED, MARKER_SECTION, MARKER_133));
      const r = run(f, ['verify', f.out]);
      expect(r.status, r.stderr).toBe(0);
      expect(protoCalls(f)).toEqual([]);
    });

    it.each<[string, Buffer]>([
      ['another payload', appendCustomSection(OPTIMIZED, MARKER_SECTION, 'binaryen=132;flags=-O')],
      ['a second marker', appendCustomSection(appendCustomSection(OPTIMIZED, MARKER_SECTION, MARKER_133), MARKER_SECTION, MARKER_133)],
      ['a name section', appendCustomSection(RAW, MARKER_SECTION, MARKER_133)],
      ['no marker', OPTIMIZED],
    ])('exits 1 on %s', (_label, bytes) => {
      const f = fixture(undefined, bytes);
      const r = run(f, ['verify', f.out]);
      expect(r.status, r.stderr).toBe(1);
    });
  });
});
