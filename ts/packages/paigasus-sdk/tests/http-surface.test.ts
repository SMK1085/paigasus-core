// SPDX-License-Identifier: Apache-2.0
//
// SMA-575 AC 2 (spec § 4). The ONLY hand-written HTTP client in @paigasus/sdk is src/chat.ts, and
// it serves only the gateway's POST /v1/chat/completions. Everything else reaches a service through
// a generated Connect-ES client (ADR-0018 decision 5, Amendments A1 and A2).
//
// A vitest test, not an ESLint rule: nobody can `eslint-disable` an assertion, and the negative
// controls below are plain calls of a pure function. It PARSES with the TypeScript compiler API
// and visits with `ts.forEachChild`, which skips comments and JSDoc, so prose that names `fetch`
// does not count.
//
// RULE: no test in this package writes into `src/`. This file walks `src/`, and vitest runs test
// files in parallel (SMA-575 spec § 2).
import { readdirSync, readFileSync, statSync } from 'node:fs';
import { dirname, posix, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';
import ts from 'typescript';
import { describe, expect, it } from 'vitest';

const PKG_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const SRC_DIR = 'src';
const CHAT_FILE = 'src/chat.ts';

// Rule 1. Bare specifiers any src/ file may import. Strict: a new dependency is a deliberate edit.
const ALLOWED_SPECIFIERS = new Set(['server-only', '@connectrpc/connect', '@bufbuild/protobuf', '@paigasus/proto', '@paigasus/proto/iam']);

// Rule 1, narrowed. @connectrpc/connect-node also exports a generic HTTP client
// (`createNodeHttpClient`) and the Connect and gRPC-web transports, so it is allowed in ONE file
// and for the two gRPC names only.
const CONNECT_NODE = '@connectrpc/connect-node';
const CONNECT_NODE_FILE = 'src/transport.ts';
const CONNECT_NODE_IMPORTS = new Set(['createGrpcTransport', 'Http2SessionManager']);

// Rule 2. Banned outside src/chat.ts. `globalThis`, `global` and `process` close the obvious
// escapes (`globalThis['fe' + 'tch']`, `process.getBuiltinModule('node:https')`). `Headers` is
// NOT here: src/errors/map-error.ts uses it to read response metadata.
const BANNED_IDENTIFIERS = new Set(['fetch', 'XMLHttpRequest', 'WebSocket', 'EventSource', 'Request', 'Response', 'RequestInit', 'globalThis', 'global', 'process']);

type Violation = { readonly relPath: string; readonly line: number; readonly reason: string };

/** `relPath` is package-relative with `/` separators, e.g. `src/errors/map-error.ts`. */
function findViolations(relPath: string, source: string): Violation[] {
  const file = ts.createSourceFile(relPath, source, ts.ScriptTarget.ESNext, true, ts.ScriptKind.TS);
  const violations: Violation[] = [];
  const report = (node: ts.Node, reason: string): void => {
    violations.push({ relPath, line: file.getLineAndCharacterOfPosition(node.getStart(file)).line + 1, reason });
  };

  const checkSpecifier = (specifier: ts.Node, declaration?: ts.ImportDeclaration | ts.ExportDeclaration): void => {
    // A template literal or a variable hides the module from review: always a violation.
    if (!ts.isStringLiteral(specifier)) {
      report(specifier, 'module specifier is not a plain string literal');
      return;
    }
    const text = specifier.text;
    if (text.startsWith('./') || text.startsWith('../')) {
      const target = posix.normalize(posix.join(posix.dirname(relPath), text));
      if (!target.startsWith(`${SRC_DIR}/`)) report(specifier, `relative import leaves src/: ${text}`);
      return;
    }
    if (ALLOWED_SPECIFIERS.has(text)) return;
    if (text === CONNECT_NODE && relPath === CONNECT_NODE_FILE && declaration !== undefined && ts.isImportDeclaration(declaration)) {
      const clause = declaration.importClause;
      const bindings = clause?.namedBindings;
      if (clause !== undefined && clause.name === undefined && bindings !== undefined && ts.isNamedImports(bindings)) {
        const names = bindings.elements.map((e) => (e.propertyName ?? e.name).text);
        if (names.length > 0 && names.every((n) => CONNECT_NODE_IMPORTS.has(n))) return;
      }
    }
    report(specifier, `module not on the allowlist: ${text}`);
  };

  const visit = (node: ts.Node): void => {
    if ((ts.isImportDeclaration(node) || ts.isExportDeclaration(node)) && node.moduleSpecifier !== undefined) {
      checkSpecifier(node.moduleSpecifier, node);
    } else if (ts.isCallExpression(node) && (node.expression.kind === ts.SyntaxKind.ImportKeyword || (ts.isIdentifier(node.expression) && node.expression.text === 'require'))) {
      const [first] = node.arguments;
      if (first === undefined) report(node, 'import()/require() without an argument');
      else checkSpecifier(first);
    }
    if (ts.isIdentifier(node) && relPath !== CHAT_FILE && BANNED_IDENTIFIERS.has(node.text)) {
      report(node, `network identifier outside ${CHAT_FILE}: ${node.text}`);
    }
    ts.forEachChild(node, visit);
  };
  visit(file);
  return violations;
}

describe('SMA-575 AC 2 — findViolations negative controls', () => {
  // [fixture, relPath, expected violation count]. Code strings in an ARRAY, never in comments, so
  // stripping comments cannot make a control inert.
  const CASES: readonly (readonly [string, string, number])[] = [
    ['const r = fetch(url);', 'src/x.ts', 1],
    ['const f = globalThis.fetch;', 'src/x.ts', 2],
    ['type F = typeof fetch;', 'src/x.ts', 1],
    ["const f = globalThis['fe' + 'tch'];", 'src/x.ts', 1],
    ["const h = process.getBuiltinModule('node:https');", 'src/x.ts', 1],
    ['const s = new WebSocket(u);', 'src/x.ts', 1],
    ['export const read = (r: Response) => r;', 'src/x.ts', 1],
    ["import 'node:https';", 'src/x.ts', 1],
    ["import type { Dispatcher } from 'undici';", 'src/x.ts', 1],
    ["export * from 'undici';", 'src/x.ts', 1],
    ["export type { Dispatcher } from 'undici';", 'src/x.ts', 1],
    ["const m = await import('node:http');", 'src/x.ts', 1],
    ['const m = await import(`undici`);', 'src/x.ts', 1],
    ['const m = await import(name);', 'src/x.ts', 1],
    // An ALLOWED module behind a template literal: only the non-literal rule can catch this one.
    ['const m = await import(`server-only`);', 'src/x.ts', 1],
    ["const a = require('axios');", 'src/x.ts', 1],
    ["import { createNodeHttpClient } from '@connectrpc/connect-node';", 'src/transport.ts', 1],
    ["import { createGrpcTransport } from '@connectrpc/connect-node';", 'src/x.ts', 1],
    ["export { createGrpcTransport } from '@connectrpc/connect-node';", 'src/transport.ts', 1],
    ["import { x } from '../../paigasus-auth/src/x';", 'src/x.ts', 1],
    ['// fetch is mentioned here\n/** See {@link fetch}. @param fetch unused */\nexport const a = 1;', 'src/x.ts', 0],
    ['const r = await fetch(u);', 'src/chat.ts', 0],
    ["import 'node:https';", 'src/chat.ts', 1],
    ["import { fetch } from './x';", 'src/errors/chat.ts', 1],
    ["import { createGrpcTransport, Http2SessionManager } from '@connectrpc/connect-node';", 'src/transport.ts', 0],
    [
      [
        "import './server-guard';",
        "import { createClient } from '@connectrpc/connect';",
        "import type { DescService } from '@bufbuild/protobuf';",
        "import { ErrorReason } from '@paigasus/proto';",
        "import { OutboxService } from '@paigasus/proto/iam';",
        "import { mapError } from './errors/map-error';",
        "export { y } from '../src/y';",
        "import 'server-only';",
      ].join('\n'),
      'src/x.ts',
      0,
    ],
  ];

  it.each(CASES)('fixture %j at %s yields %i violation(s)', (source, relPath, expected) => {
    expect(findViolations(relPath, source)).toHaveLength(expected);
  });
});

function listSourceFiles(): string[] {
  // readdirSync with recursive: true — no glob dependency; Node 24 supports it natively.
  return readdirSync(resolve(PKG_ROOT, SRC_DIR), { recursive: true, encoding: 'utf8' })
    .map((entry) => `${SRC_DIR}/${entry.split(sep).join('/')}`)
    .filter((relPath) => statSync(resolve(PKG_ROOT, relPath)).isFile())
    .sort();
}

function literalTexts(node: ts.Node, out: string[] = []): string[] {
  if (ts.isStringLiteral(node) || ts.isNoSubstitutionTemplateLiteral(node)) out.push(node.text);
  else if (ts.isTemplateExpression(node)) {
    out.push(node.head.text);
    for (const span of node.templateSpans) out.push(span.literal.text);
  }
  ts.forEachChild(node, (child) => {
    literalTexts(child, out);
  });
  return out;
}

describe('SMA-575 AC 2 — the real src/ tree', () => {
  const files = listSourceFiles();

  it('walks at least the ten files that exist today', () => {
    expect(files.length).toBeGreaterThanOrEqual(10);
  });

  it('holds only .ts files, so nothing hides from the walk', () => {
    expect(files.filter((f) => !f.endsWith('.ts'))).toEqual([]);
  });

  it('has every package entry point under ./src/', () => {
    const pkg = JSON.parse(readFileSync(resolve(PKG_ROOT, 'package.json'), 'utf8')) as { exports: Record<string, string> };
    expect(Object.values(pkg.exports).filter((target) => !target.startsWith(`./${SRC_DIR}/`))).toEqual([]);
  });

  it('has no violation in any file', () => {
    const violations = files.filter((f) => f.endsWith('.ts')).flatMap((f) => findViolations(f, readFileSync(resolve(PKG_ROOT, f), 'utf8')));
    expect(violations).toEqual([]);
  });

  describe('the chat exception is live and narrow', () => {
    const chat = ts.createSourceFile(CHAT_FILE, readFileSync(resolve(PKG_ROOT, CHAT_FILE), 'utf8'), ts.ScriptTarget.ESNext, true, ts.ScriptKind.TS);

    it('makes exactly one call through the fetch seam', () => {
      let seamCalls = 0;
      const visit = (node: ts.Node): void => {
        if (ts.isCallExpression(node) && ts.isIdentifier(node.expression) && node.expression.text === 'fetchImpl') seamCalls += 1;
        ts.forEachChild(node, visit);
      };
      visit(chat);
      expect(seamCalls).toBe(1);
    });

    it('names no gateway path other than /v1/chat/completions', () => {
      expect(literalTexts(chat).filter((text) => text.includes('/v1/'))).toEqual(['/v1/chat/completions']);
    });
  });
});
