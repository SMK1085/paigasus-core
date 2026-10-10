// SPDX-License-Identifier: Apache-2.0
//
// SMA-725 spec § 6.1. The six single-field PRN accessors carry a `@deprecated` tag that names
// `prnParse`, and a consumer sees it through BOTH package entries. The test asks the TypeScript
// language service, as an editor does, so it proves what a caller sees, not what a file contains.
//
// The probe files exist only in the language-service host. They sit INSIDE this package, so
// package self-reference (`name` + `exports`) resolves `@paigasus/kernel`: ts/node_modules has no
// copy of this package. The compiler options are written here, not read from a tsconfig, so this
// test needs no tsconfig in the Moon task inputs.
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import ts from 'typescript';
import { describe, expect, it } from 'vitest';

const KERNEL_ROOT = fileURLToPath(new URL('..', import.meta.url));
const ACCESSORS = ['prnErrorKind', 'prnService', 'prnRegion', 'prnOrg', 'prnResourceType', 'prnResourceId'] as const;
/** TypeScript diagnostic 6385: "'{0}' is deprecated." */
const DEPRECATED = 6385;
/** A cold language service can go past vitest's 5 s default in CI (SMA-725 spec § 6.1). */
const TIMEOUT = 30_000;

const ENTRIES = [
  { specifier: '@paigasus/kernel', probe: 'tests/__probe_wasm__.ts', source: 'src/wasm.ts' },
  { specifier: '@paigasus/kernel/napi', probe: 'tests/__probe_napi__.ts', source: 'src/index.ts' },
] as const;

const OPTIONS: ts.CompilerOptions = {
  module: ts.ModuleKind.ESNext,
  moduleResolution: ts.ModuleResolutionKind.Bundler,
  target: ts.ScriptTarget.ES2022,
  strict: true,
  noEmit: true,
  skipLibCheck: true,
};

/** Import statements only: one per accessor, then `prnParse`. A use site would add more 6385s. */
function probeSource(specifier: string): string {
  return [...ACCESSORS, 'prnParse'].map((name) => `import { ${name} } from '${specifier}';`).join('\n') + '\n';
}

const files = new Map<string, string>(ENTRIES.map((e) => [path.join(KERNEL_ROOT, e.probe), probeSource(e.specifier)]));

const host: ts.LanguageServiceHost = {
  getScriptFileNames: () => [...files.keys()],
  getScriptVersion: () => '1',
  getScriptSnapshot: (fileName) => {
    const text = files.get(fileName) ?? ts.sys.readFile(fileName);
    return text === undefined ? undefined : ts.ScriptSnapshot.fromString(text);
  },
  getCurrentDirectory: () => KERNEL_ROOT,
  getCompilationSettings: () => OPTIONS,
  getDefaultLibFileName: (options) => ts.getDefaultLibFilePath(options),
  fileExists: (fileName) => files.has(fileName) || ts.sys.fileExists(fileName),
  readFile: (fileName) => files.get(fileName) ?? ts.sys.readFile(fileName),
  readDirectory: (...args) => ts.sys.readDirectory(...args),
  directoryExists: (dir) => ts.sys.directoryExists(dir),
  getDirectories: (dir) => ts.sys.getDirectories(dir),
  realpath: (p) => (ts.sys.realpath ? ts.sys.realpath(p) : p),
};

const service = ts.createLanguageService(host, ts.createDocumentRegistry());

/** The `@deprecated` tag text of a symbol, following an alias to its target, or `undefined`. */
function deprecatedText(symbol: ts.Symbol, checker: ts.TypeChecker): string | undefined {
  const chain = symbol.flags & ts.SymbolFlags.Alias ? [symbol, checker.getAliasedSymbol(symbol)] : [symbol];
  for (const s of chain) {
    const tag = s.getJsDocTags(checker).find((t) => t.name === 'deprecated');
    if (tag !== undefined) return ts.displayPartsToString(tag.text);
  }
  return undefined;
}

/** Map every export of an entry file that carries `@deprecated` to its tag text. */
function deprecatedExports(sourceRel: string): Map<string, string> {
  const program = service.getProgram();
  if (program === undefined) throw new Error('the language service has no program');
  const checker = program.getTypeChecker();
  const sourceFile = program.getSourceFile(path.join(KERNEL_ROOT, sourceRel));
  if (sourceFile === undefined) throw new Error(`${sourceRel} is not in the probe program`);
  const moduleSymbol = checker.getSymbolAtLocation(sourceFile);
  if (moduleSymbol === undefined) throw new Error(`${sourceRel} has no module symbol`);
  const result = new Map<string, string>();
  for (const exported of checker.getExportsOfModule(moduleSymbol)) {
    const text = deprecatedText(exported, checker);
    if (text !== undefined) result.set(exported.getName(), text);
  }
  return result;
}

describe.each(ENTRIES)('the deprecated accessors through $specifier', ({ probe, source }) => {
  const probePath = path.join(KERNEL_ROOT, probe);
  const probeText = files.get(probePath) ?? '';

  it(
    'resolves the probe with no semantic diagnostic (a failed resolution gives 2307 and no 6385)',
    () => {
      const messages = service.getSemanticDiagnostics(probePath).map((d) => `${d.code}: ${ts.flattenDiagnosticMessageText(d.messageText, '\n')}`);
      expect(messages).toEqual([]);
    },
    TIMEOUT,
  );

  it(
    'reports 6385 exactly once at each accessor import specifier, and never at prnParse',
    () => {
      const starts = service
        .getSuggestionDiagnostics(probePath)
        .filter((d) => d.code === DEPRECATED)
        .map((d) => d.start)
        .sort((a, b) => (a ?? 0) - (b ?? 0));
      const expected = ACCESSORS.map((name) => probeText.indexOf(`{ ${name} }`) + 2).sort((a, b) => a - b);
      expect(starts).toEqual(expected);
      expect(starts).not.toContain(probeText.indexOf('{ prnParse }') + 2);
    },
    TIMEOUT,
  );

  it(
    `deprecates exactly the six accessors in ${source}, each with a text that names prnParse`,
    () => {
      const deprecated = deprecatedExports(source);
      expect([...deprecated.keys()].sort()).toEqual([...ACCESSORS].sort());
      for (const [name, text] of deprecated) {
        expect(text, `${name} @deprecated text`).toContain('prnParse');
      }
    },
    TIMEOUT,
  );
});

describe('the two entries', () => {
  it(
    'carry the same @deprecated text for each accessor',
    () => {
      const wasm = deprecatedExports('src/wasm.ts');
      const napi = deprecatedExports('src/index.ts');
      for (const name of ACCESSORS) {
        expect(napi.get(name), name).toBe(wasm.get(name));
      }
    },
    TIMEOUT,
  );
});
