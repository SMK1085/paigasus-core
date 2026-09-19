// SPDX-License-Identifier: Apache-2.0
//
// COPY of ts/apps/iam-console/tests/unit/actions-structure.test.ts (SMA-636 spec § 7.4), with this
// zone's EXPECTED list. Every exported Server Action obtains its client through
// iamClientsForAction(), so a session read runs for EVERY action. The (console) layout does not
// guard Server Actions: an action is a POST to the page URL, and Next does not render the layout
// for it.
//
// The redirecting page accessor is BANNED here: it redirects through requireSession(), and a
// Server Action's redirect carries no basePath, so the browser leaves the /gateway zone
// (@paigasus/console-core's src/runtime.ts, on iamClientsForAction's doc comment).
//
// No actions.ts names mayI (SMA-636 spec § 5.1): a hidden button is cosmetic, and IAM decides. That
// check reads identifiers, not text, so a comment that names mayI() is not a violation.
//
// EXPECTED is a strict-equality list. A new action is a review point: add it here. The negative
// cases at the bottom prove the checker can fail, so a green here is not vacuous.
import { readFileSync, readdirSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import ts from 'typescript';
import { describe, expect, it } from 'vitest';

const APP_DIR = fileURLToPath(new URL('../../app', import.meta.url));

const EXPECTED: Readonly<Record<string, readonly string[]>> = {
  '(console)/service-accounts/actions.ts': ['allowModelCallsAction', 'archiveServiceAccountAction', 'createServiceAccountAction', 'issueApiKeyAction', 'revokeApiKeyAction'],
};

// SMA-630 spec § 4.4, D2. A Server Action never navigates. redirect() and permanentRedirect() from
// an action carry no basePath and leave the zone; forbidden(), unauthorized() and notFound() would
// replace the inline form error that D2 requires. The check reads identifiers: a presentation
// STRING such as 'forbidden' is not one. It also matches a property NAME (`x.forbidden`), which no
// action uses.
const NAVIGATION_HELPERS = ['redirect', 'permanentRedirect', 'forbidden', 'unauthorized', 'notFound'] as const;

function findActionFiles(dir: string, prefix = ''): string[] {
  const found: string[] = [];
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    if (entry.name === 'node_modules' || entry.name === '.next') continue;
    const relative = prefix === '' ? entry.name : `${prefix}/${entry.name}`;
    if (entry.isDirectory()) found.push(...findActionFiles(path.join(dir, entry.name), relative));
    else if (entry.name === 'actions.ts') found.push(relative);
  }
  return found.sort();
}

type Exported = { readonly name: string; readonly body: ts.Node | undefined };

function isExported(node: ts.Node): boolean {
  return ts.canHaveModifiers(node) && (ts.getModifiers(node) ?? []).some((modifier) => modifier.kind === ts.SyntaxKind.ExportKeyword);
}

function exportedValues(source: ts.SourceFile): Exported[] {
  const out: Exported[] = [];
  for (const statement of source.statements) {
    if (ts.isFunctionDeclaration(statement) && isExported(statement)) {
      out.push({ name: statement.name?.text ?? 'default', body: statement.body });
    } else if (ts.isVariableStatement(statement) && isExported(statement)) {
      for (const declaration of statement.declarationList.declarations) {
        const init = declaration.initializer;
        const body = init !== undefined && (ts.isArrowFunction(init) || ts.isFunctionExpression(init)) ? init.body : undefined;
        out.push({ name: declaration.name.getText(source), body });
      }
    } else if (ts.isExportAssignment(statement)) {
      out.push({ name: 'default', body: undefined });
    } else if (ts.isExportDeclaration(statement) && !statement.isTypeOnly) {
      // `export { x } from './y'` hides the body where this test cannot read it.
      out.push({ name: `re-export ${statement.getText(source)}`, body: undefined });
    }
  }
  return out;
}

/**
 * Walks the action's OWN body only. A call inside a nested, uninvoked function scope (a helper
 * declared but never called) must NOT count — see the negative control below.
 */
function callsActionClients(node: ts.Node): boolean {
  let found = false;
  const visit = (child: ts.Node): void => {
    if (found) return;
    if (ts.isCallExpression(child) && ts.isIdentifier(child.expression) && child.expression.text === 'iamClientsForAction') {
      found = true;
      return;
    }
    if (ts.isFunctionLike(child)) return; // stop at a nested function scope; do not descend into it
    ts.forEachChild(child, visit);
  };
  visit(node);
  return found;
}

/** True when any identifier in the tree is `name`: an import, a call, a reference. Comments are not nodes. */
function namesIdentifier(node: ts.Node, name: string): boolean {
  let found = false;
  const visit = (child: ts.Node): void => {
    if (found) return;
    if (ts.isIdentifier(child) && child.text === name) {
      found = true;
      return;
    }
    ts.forEachChild(child, visit);
  };
  visit(node);
  return found;
}

function checkActionsSource(file: string, text: string): { names: string[]; violations: string[] } {
  const source = ts.createSourceFile(file, text, ts.ScriptTarget.ES2022, true, ts.ScriptKind.TS);
  const violations: string[] = [];
  const [first] = source.statements;
  if (first === undefined || !ts.isExpressionStatement(first) || !ts.isStringLiteral(first.expression) || first.expression.text !== 'use server') {
    violations.push(`${file}: the first statement is not 'use server'`);
  }
  const values = exportedValues(source);
  for (const value of values) {
    if (value.body === undefined) violations.push(`${file}: export ${value.name} is not a function whose body this test can read`);
    else if (!callsActionClients(value.body)) violations.push(`${file}: export ${value.name} does not call iamClientsForAction()`);
  }
  // createIamClients? covers both @paigasus/console-core's iam-clients.ts' createIamClients and the SDK's createIamClient.
  if (/\b(?:iamClientsForToken|createIamClients?)\b/.test(text)) violations.push(`${file}: builds an IAM client without a session`);
  // `iamClients` is the PAGE accessor. It redirects, and a Server Action's redirect leaves the zone.
  if (namesIdentifier(source, 'iamClients')) violations.push(`${file}: uses the redirecting iamClients()`);
  if (namesIdentifier(source, 'mayI')) violations.push(`${file}: consults mayI()`);
  for (const helper of NAVIGATION_HELPERS) {
    if (namesIdentifier(source, helper)) violations.push(`${file}: names the navigation helper ${helper}()`);
  }
  return { names: values.map((value) => value.name).sort(), violations };
}

describe('every Server Action gets its client through iamClientsForAction() (spec § 5.3)', () => {
  const files = findActionFiles(APP_DIR);

  it('finds exactly the expected actions.ts files and exports', () => {
    expect(files).toEqual(Object.keys(EXPECTED).sort());
    for (const file of files) {
      expect(checkActionsSource(file, readFileSync(path.join(APP_DIR, file), 'utf8')).names).toEqual([...(EXPECTED[file] ?? [])].sort());
    }
  });

  it('reports no violation in any actions.ts', () => {
    const violations = files.flatMap((file) => checkActionsSource(file, readFileSync(path.join(APP_DIR, file), 'utf8')).violations);
    expect(violations).toEqual([]);
  });

  describe('the checker fails on each broken shape (negative controls)', () => {
    const header = "'use server';\nimport { iamClientsForAction } from '../lib/console';\n";
    it.each([
      ['a function without the call', `${header}export async function a() { return 1; }`, 'does not call iamClientsForAction()'],
      ['an arrow without the call', `${header}export const a = async () => 1;`, 'does not call iamClientsForAction()'],
      // The only call sits inside a nested, uninvoked function scope. The walk must not descend into it.
      ['a call hidden in an uninvoked nested function', `${header}export async function a() { function unused() { iamClientsForAction(); } return 1; }`, 'does not call iamClientsForAction()'],
      ['a re-export', `${header}export { a } from './other';`, 'is not a function whose body this test can read'],
      ['a default export of a value', `${header}export default iamClientsForAction;`, 'is not a function whose body this test can read'],
      ['a missing directive', 'export async function a() { await iamClientsForAction(); }', "the first statement is not 'use server'"],
      [
        'a session-less client',
        `${header}import { iamClientsForToken } from '../lib/console';\nexport async function a() { await iamClientsForAction(); iamClientsForToken('t'); }`,
        'without a session',
      ],
      // No import line: the check reads the text, so the call `createIamClients(` alone must trip it.
      ['a session-less client builder', `${header}export async function a() { await iamClientsForAction(); createIamClients({ baseUrl: 'x', token: 't' }); }`, 'without a session'],
      // The redirecting page accessor. This is the control on the final-review fix (see the header).
      [
        'the redirecting page accessor',
        `${header}import { iamClients } from '../lib/console';\nexport async function a() { await iamClientsForAction(); await iamClients(); }`,
        'redirecting iamClients()',
      ],
      [
        'an action that consults mayI',
        `${header}import { mayI } from '../lib/console';\nexport async function a() { await iamClientsForAction(); const may = await mayI(); return may; }`,
        'consults mayI()',
      ],
      [
        'an action that redirects',
        `${header}import { redirect } from 'next/navigation';\nexport async function a() { await iamClientsForAction(); redirect('/orgs'); }`,
        'navigation helper redirect()',
      ],
      [
        'an action that renders the 403 view',
        `${header}import { forbidden } from 'next/navigation';\nexport async function a() { await iamClientsForAction(); forbidden(); }`,
        'navigation helper forbidden()',
      ],
      [
        'an action that renders the 404 view',
        `${header}import { notFound } from 'next/navigation';\nexport async function a() { await iamClientsForAction(); notFound(); }`,
        'navigation helper notFound()',
      ],
      [
        'an action that redirects permanently',
        `${header}import { permanentRedirect } from 'next/navigation';\nexport async function a() { await iamClientsForAction(); permanentRedirect('/orgs'); }`,
        'navigation helper permanentRedirect()',
      ],
      [
        'an action that renders the 401 view',
        `${header}import { unauthorized } from 'next/navigation';\nexport async function a() { await iamClientsForAction(); unauthorized(); }`,
        'navigation helper unauthorized()',
      ],
    ])('%s', (_label, source, message) => {
      expect(checkActionsSource('probe.ts', source).violations.join('\n')).toContain(message);
    });

    it('accepts a correct action, and a comment that names mayI()', () => {
      expect(
        checkActionsSource('probe.ts', `${header}// No action consults mayI().\nexport async function a(_p: unknown, f: FormData) { const c = await iamClientsForAction(); return c; }`).violations,
      ).toEqual([]);
    });

    it('accepts a presentation string and a comment that name forbidden', () => {
      expect(
        checkActionsSource(
          'probe.ts',
          `${header}// Never call forbidden() here.\nexport async function a() { const c = await iamClientsForAction(); return c.ok ? 'fine' : c.error.presentation === 'forbidden'; }`,
        ).violations,
      ).toEqual([]);
    });
  });
});
