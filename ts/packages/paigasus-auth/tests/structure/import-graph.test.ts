// SPDX-License-Identifier: Apache-2.0
//
// Gap 2 (task 12, escalated from task 11's review): task 11 found that the client boundary's
// eslint deny group listed './runtime' and './config' — specifiers that never matched anything,
// because this codebase then wrote relative imports with `.js` (`./runtime.js`; src/ is extensionless since SMA-511). Those two entries
// were the SOLE nominal defence against src/client.ts reaching src/runtime.ts, the composition
// root that pulls in openid-client, the Redis adapter, and the claims resolver. The eslint rule
// has since been fixed, but the episode showed src/client.ts had no STRUCTURAL backstop the way
// src/middleware.ts already has (tests/middleware.test.ts's AC 4 walk) — a lint rule alone can
// regress silently the same way this one did, and this file closes that gap.
//
// It reuses tests/support/import-graph.ts's walker (built for tests/middleware.test.ts in task 10,
// hardened in its fix round) rather than duplicating a second one that could drift from it.
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import { collectImportGraph, filesWithDynamicImportOrRequire } from '../support/import-graph.js';

const HERE = dirname(fileURLToPath(import.meta.url));
const SRC = resolve(HERE, '../../src');

// Mirrors eslint.mjs's `paigasus/boundaries/auth-client` deny group's directory entries
// (packages/paigasus-next-config/src/eslint.mjs): adapters/, core/, ports/, http/, next/.
function reachesBannedDirectory(files: Set<string>): string[] {
  const forbidden = /[/\\](adapters|core|ports|http|next)[/\\]/;
  return [...files].filter((f) => forbidden.test(f));
}

describe('client import graph (AC 5)', () => {
  it('reaches no server directory, no openid-client, and no redis', () => {
    const graph = collectImportGraph(resolve(SRC, 'client.ts'));

    expect(reachesBannedDirectory(graph.files)).toEqual([]);
    expect(graph.packages.has('openid-client')).toBe(false);
    expect(graph.packages.has('redis')).toBe(false);
    // Backstop, mirroring tests/middleware.test.ts's AC 4 guard: no file the static declaration
    // walk reached may itself contain a dynamic import()/require() call, which that walk cannot
    // see on its own — see tests/support/import-graph.ts's doc comment for why.
    expect(filesWithDynamicImportOrRequire(graph.files)).toEqual([]);
  });

  it('reaches exactly one non-react module: ./session-view.js', () => {
    const entry = resolve(SRC, 'client.ts');
    const sessionView = resolve(SRC, 'session-view.ts');
    const graph = collectImportGraph(entry);

    // src/client.ts's own doc comment says it "imports NOTHING beyond react and the SessionView
    // type" — this is the assertion that keeps that comment honest, with strict set equality
    // rather than a "does not contain" check.
    //
    // DECISION on type-only imports: `SessionView` is imported with `import type`, and
    // tests/support/import-graph.ts's walker does NOT special-case `isTypeOnly` — it treats a
    // type-only specifier as a real edge, the same as a value import. That is deliberate here,
    // not an oversight: the eslint `paigasus/boundaries/auth-client` rule this test mirrors also
    // treats a type-only reach across a banned directory as a violation (see src/session-view.ts's
    // own comment on why `SessionView` had to move OUT of src/core/session.ts for exactly that
    // reason — a type import still requires the source file to name a path under a banned
    // directory, even though the import itself is erased at runtime). Since src/session-view.ts is
    // a leaf module with no imports of its own, walking its type-only edge does not risk a false
    // positive here — it just makes this assertion exact instead of vacuous.
    expect(graph.files).toEqual(new Set([entry, sessionView]));
    expect(graph.packages).toEqual(new Set(['react']));
  });

  // POSITIVE CONTROL (guard-the-guard). Without this, a walker whose resolver silently returns
  // nothing would report an empty package set for src/client.ts too, and both assertions above
  // would pass vacuously. src/server.ts is known, by construction, to reach openid-client (via
  // src/runtime.ts, which re-exports createOidcClient from src/adapters/oidc.ts) — this re-asserts
  // that fact for THIS file's own walker import, rather than relying on
  // tests/middleware.test.ts's copy of the same control to keep proving it.
  it('positive control: the same walker reaches openid-client from src/server.ts', () => {
    const graph = collectImportGraph(resolve(SRC, 'server.ts'));

    expect(graph.packages.has('openid-client')).toBe(true);
  });
});

// I1 (final fix wave). `import 'server-only'` at the top of src/server.ts (design doc § 4.3 layer
// 2) had ZERO coverage: both vitest configs alias `server-only` to an empty stub and
// tests/e2e/e2e-loader.mjs intercepts it, so no test, lint rule, or typecheck reads that
// statement — deleting it left every gate green while making @paigasus/auth/server importable
// from a client component, shipping the Redis adapter into a browser bundle.
//
// This does not newly RUN the statement (that would need the real `server-only` package under a
// non-`react-server` condition, which the client-boundary graph above already forbids reaching).
// It asserts the STATEMENT IS THERE, the same way the rest of this file asserts the shape of an
// import graph rather than executing one — `collectImportGraph`'s `packages` set records every
// bare specifier `server.ts`'s own module graph reaches, `server-only` included, since it is a
// real `import 'server-only'` declaration at the top of the entry file itself.
describe('server entry point carries the server-only guard (I1)', () => {
  it("src/server.ts's import graph reaches the bare 'server-only' specifier", () => {
    const graph = collectImportGraph(resolve(SRC, 'server.ts'));

    expect(graph.packages.has('server-only')).toBe(true);
  });
});
