// SPDX-License-Identifier: Apache-2.0
//
// Spec § 5.3: every exported Server Action obtains its client through iamClients(), so
// requireSession() runs for EVERY action. The (console) layout does not guard Server Actions —
// an action is a POST to the page URL, and Next does not render the layout for it (spec § 3.3).
// No actions.ts names mayI (spec § 6.3): a hidden button is cosmetic, and IAM decides. That check
// reads identifiers, not text, so a comment that names mayI() is not a violation.
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
  '(console)/orgs/actions.ts': ['attachMembershipAction', 'createOrganizationAction', 'detachMembershipAction'],
  '(console)/orgs/[org]/actions.ts': ['createTeamAction'],
  '(console)/orgs/[org]/teams/[team]/actions.ts': ['createProjectAction'],
};

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

function callsIamClients(node: ts.Node): boolean {
  let found = false;
  const visit = (child: ts.Node): void => {
    if (found) return;
    if (ts.isCallExpression(child) && ts.isIdentifier(child.expression) && child.expression.text === 'iamClients') {
      found = true;
      return;
    }
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
    else if (!callsIamClients(value.body)) violations.push(`${file}: export ${value.name} does not call iamClients()`);
  }
  // createIamClients? covers both lib/iam.ts' createIamClients and the SDK's createIamClient.
  if (/\b(?:iamClientsForToken|createIamClients?)\b/.test(text)) violations.push(`${file}: builds an IAM client without a session`);
  if (namesIdentifier(source, 'mayI')) violations.push(`${file}: consults mayI()`);
  return { names: values.map((value) => value.name).sort(), violations };
}

describe('every Server Action gets its client through iamClients() (spec § 5.3)', () => {
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
    const header = "'use server';\nimport { iamClients } from '../lib/iam';\n";
    it.each([
      ['a function without the call', `${header}export async function a() { return 1; }`, 'does not call iamClients()'],
      ['an arrow without the call', `${header}export const a = async () => 1;`, 'does not call iamClients()'],
      ['a re-export', `${header}export { a } from './other';`, 'is not a function whose body this test can read'],
      ['a default export of a value', `${header}export default iamClients;`, 'is not a function whose body this test can read'],
      ['a missing directive', 'export async function a() { await iamClients(); }', "the first statement is not 'use server'"],
      ['a session-less client', `${header}import { iamClientsForToken } from '../lib/iam';\nexport async function a() { await iamClients(); iamClientsForToken('t'); }`, 'without a session'],
      // No import line: the check reads the text, so the call `createIamClients(` alone must trip it.
      ['a session-less client builder', `${header}export async function a() { await iamClients(); createIamClients({ baseUrl: 'x', token: 't' }); }`, 'without a session'],
      ['an action that consults mayI', `${header}import { mayI } from '../lib/authorize';\nexport async function a() { await iamClients(); const may = await mayI(); return may; }`, 'consults mayI()'],
    ])('%s', (_label, source, message) => {
      expect(checkActionsSource('probe.ts', source).violations.join('\n')).toContain(message);
    });

    it('accepts a correct action, and a comment that names mayI()', () => {
      expect(
        checkActionsSource('probe.ts', `${header}// No action consults mayI().\nexport async function a(_p: unknown, f: FormData) { const c = await iamClients(); return c; }`).violations,
      ).toEqual([]);
    });
  });
});
