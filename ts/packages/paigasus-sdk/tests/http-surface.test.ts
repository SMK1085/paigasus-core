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
// and for the two gRPC names only. `createGrpcTransport` and `Http2SessionManager` are allowed
// because they are gRPC transport plumbing. `Http2SessionManager` ALSO exposes raw HTTP/2 methods
// (`request`, `connect`) on the object it builds. Allowing the name does not allow those calls:
// rule 3 below closes that path by property name, everywhere outside src/chat.ts.
const CONNECT_NODE = '@connectrpc/connect-node';
const CONNECT_NODE_FILE = 'src/transport.ts';
const CONNECT_NODE_IMPORTS = new Set(['createGrpcTransport', 'Http2SessionManager']);

// Rule 3. `Http2SessionManager.request(method, path, headers, options)` sends a raw HTTP/2
// request, and `.connect()` opens a raw session — both bypass the generated Connect-ES clients
// entirely. MEASURED: no property access named either word exists in src/ today. A property
// access by either name, outside src/chat.ts, is a violation regardless of which object it is
// called on — the check is by NAME, not by declared type, so it also fires on an unrelated
// object's same-named method. That is a stated limit (spec § 4.5), not a defect: the allowlist is
// small and the false positive costs one rename.
const CONNECT_NODE_PROPERTY_ESCAPES = new Set(['request', 'connect']);

// Rule 2. Banned outside src/chat.ts, except for the three names src/chat.ts itself needs
// (CHAT_ALLOWED_IDENTIFIERS below). `globalThis`, `global` and `process` close the obvious
// escapes (`globalThis['fe' + 'tch']`, `process.getBuiltinModule('node:https')`). `require` and
// `module` close two more: the specifier check in `checkSpecifier` only fires when the callee is
// the bare identifier `require`, so an aliased call (`const r = require; r(...)`, `(require)(...)`)
// or `module.require(...)` reaches a module outside the allowlist with zero violations otherwise.
// `Headers` is NOT here: src/errors/map-error.ts uses it to read response metadata.
const BANNED_IDENTIFIERS = new Set(['fetch', 'XMLHttpRequest', 'WebSocket', 'EventSource', 'Request', 'Response', 'RequestInit', 'globalThis', 'global', 'process', 'require', 'module']);

// MEASURED (final review, SMA-575): src/chat.ts uses exactly these three banned names, and
// nothing else on the list. The exemption is per-NAME, not per-file: every other banned
// identifier, and rule 3 above, applies to chat.ts too.
const CHAT_ALLOWED_IDENTIFIERS = new Set(['fetch', 'globalThis', 'Response']);

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
    } else if (ts.isImportEqualsDeclaration(node) && ts.isExternalModuleReference(node.moduleReference)) {
      checkSpecifier(node.moduleReference.expression);
    }
    if (ts.isIdentifier(node) && BANNED_IDENTIFIERS.has(node.text) && !(relPath === CHAT_FILE && CHAT_ALLOWED_IDENTIFIERS.has(node.text))) {
      report(node, `network identifier outside ${CHAT_FILE}: ${node.text}`);
    }
    if (ts.isPropertyAccessExpression(node) && relPath !== CHAT_FILE && CONNECT_NODE_PROPERTY_ESCAPES.has(node.name.text)) {
      report(node, `raw HTTP/2 session escape: ${node.name.text}`);
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
    // `require` is now on BANNED_IDENTIFIERS too, so this yields both the identifier violation and
    // the specifier violation.
    ["const a = require('axios');", 'src/x.ts', 2],
    // A parenthesised callee is not `ts.isIdentifier(node.expression)`, so `checkSpecifier` never
    // runs for the call — only the `require` identifier itself is caught (rule 2).
    ["const h = (require)('node:https');", 'src/x.ts', 1],
    // An aliased call: `r(...)` is a call on an Identifier named `r`, not `require`, so
    // `checkSpecifier` never runs. Only the `require` identifier in the alias assignment is caught.
    ["const r = require; export const h = r('node:https');", 'src/x.ts', 1],
    // MEASURED (AST dump): a PropertyAccessExpression callee is not `ts.isIdentifier`, so
    // `checkSpecifier` never runs for the call. But `module.require` is itself a
    // PropertyAccessExpression with TWO separate Identifier children — "module" and "require" —
    // and `ts.forEachChild` visits both, so BOTH are caught by rule 2. Two violations, not one.
    ["const h = module.require('node:https');", 'src/x.ts', 2],
    // MEASURED: for `import h = require('node:https')`, ts.ExternalModuleReference's `expression`
    // is the string literal itself; there is no separate `require` Identifier node in this AST
    // shape (unlike the call-expression form). So only the specifier violation fires.
    ["import h = require('node:https');", 'src/x.ts', 1],
    ["import { createNodeHttpClient } from '@connectrpc/connect-node';", 'src/transport.ts', 1],
    ["import { createGrpcTransport } from '@connectrpc/connect-node';", 'src/x.ts', 1],
    ["export { createGrpcTransport } from '@connectrpc/connect-node';", 'src/transport.ts', 1],
    ["import { x } from '../../paigasus-auth/src/x';", 'src/x.ts', 1],
    ['// fetch is mentioned here\n/** See {@link fetch}. @param fetch unused */\nexport const a = 1;', 'src/x.ts', 0],
    ['const r = await fetch(u);', 'src/chat.ts', 0],
    ["import 'node:https';", 'src/chat.ts', 1],
    // MEASURED (final review, SMA-575). CHAT_ALLOWED_IDENTIFIERS is per-name, not per-file: every
    // OTHER banned identifier still fires in src/chat.ts.
    ["const h = process.getBuiltinModule('node:https');", 'src/chat.ts', 1],
    ['const s = new WebSocket(u);', 'src/chat.ts', 1],
    ["import { fetch } from './x';", 'src/errors/chat.ts', 1],
    ["import { createGrpcTransport, Http2SessionManager } from '@connectrpc/connect-node';", 'src/transport.ts', 0],
    // Rule 3, MEASURED. `Http2SessionManager` itself is not a banned identifier — only its raw
    // `request`/`connect` methods are, and only by property name.
    ["const s = new Http2SessionManager(u); export const r = s.request('POST', '/v1/users', {}, {});", 'src/transport.ts', 1],
    ['export const c = (m: Http2SessionManager) => m.connect();', 'src/transport.ts', 1],
    // Rule 1, MEASURED. A default or namespace import of @connectrpc/connect-node is not the
    // narrow named-import shape the allowlist carves out, so both fall through to "not on the
    // allowlist" even in src/transport.ts.
    ["import cn, { createGrpcTransport } from '@connectrpc/connect-node';", 'src/transport.ts', 1],
    ["import * as cn from '@connectrpc/connect-node';", 'src/transport.ts', 1],
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

    // Finding 5, final review (SMA-575). chat.ts reads `globalThis.fetch` once, into `fetchImpl`
    // (the seam above), and never calls `fetch` or `globalThis.fetch` directly anywhere else.
    it('never calls fetch directly', () => {
      let directCalls = 0;
      const visit = (node: ts.Node): void => {
        if (ts.isCallExpression(node)) {
          const callee = node.expression;
          if (ts.isIdentifier(callee) && callee.text === 'fetch') directCalls += 1;
          else if (ts.isPropertyAccessExpression(callee) && ts.isIdentifier(callee.expression) && callee.expression.text === 'globalThis' && callee.name.text === 'fetch') directCalls += 1;
        }
        ts.forEachChild(node, visit);
      };
      visit(chat);
      expect(directCalls).toBe(0);
    });
  });
});
