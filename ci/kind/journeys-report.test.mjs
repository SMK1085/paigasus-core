// SPDX-License-Identifier: Apache-2.0
//
// Unit tests for journeys-report.mjs (SMA-514 spec § 7.2, Review Focus 1 and 2). Run:
//   node --test ci/kind/journeys-report.test.mjs
// `ci/kind/run.sh specs journeys` runs this file before its Playwright run. No Moon task and no
// required CI check runs it.
//
// The named fixtures in fixtures/journeys-report/ each change ONE thing against pass.json, in the
// shape measured on Playwright 1.63 (plan "Measured facts"). The rows below derive more variants
// in memory, so each input fails for exactly one reason and the assertion names that reason.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';
import { EXPECTED_STEPS } from './journeys-report.mjs';

const HERE = dirname(fileURLToPath(import.meta.url));
const CHECKER = join(HERE, 'journeys-report.mjs');
const FIXTURES = join(HERE, 'fixtures', 'journeys-report');

function run(...args) {
  const r = spawnSync(process.execPath, [CHECKER, ...args], { encoding: 'utf8' });
  return { status: r.status, output: `${r.stdout}${r.stderr}` };
}

function pass() {
  return JSON.parse(readFileSync(join(FIXTURES, 'pass.json'), 'utf8'));
}

function tempFile(name, text) {
  const file = join(mkdtempSync(join(tmpdir(), 'journeys-report-')), name);
  writeFileSync(file, text);
  return file;
}

function variant(mutate) {
  const doc = pass();
  mutate(doc);
  return tempFile('report.json', JSON.stringify(doc));
}

function expectFail(file, fragment) {
  const r = run('report', file);
  assert.equal(r.status, 3, r.output);
  assert.ok(r.output.includes(fragment), `want "${fragment}" in:\n${r.output}`);
}

test('pass.json passes', () => {
  const r = run('report', join(FIXTURES, 'pass.json'));
  assert.equal(r.status, 0, r.output);
});

test('pass.json prints its annotation after the pass summary (controller ruling F3)', () => {
  const r = run('report', join(FIXTURES, 'pass.json'));
  assert.equal(r.status, 0, r.output);
  assert.ok(
    r.output.includes('note') && r.output.includes('SMA-514 scenario 1: logout hops across both zones and the IdP'),
    `want the J1 annotation type and description in:\n${r.output}`,
  );
  // A description holding a newline must still print as exactly one output line: type and
  // description are JSON.stringify'd, so a real newline character becomes the two characters `\n`.
  const file = variant((d) => {
    d.suites[0].specs[0].tests[0].annotations[0].description = 'first line\nsecond line';
  });
  const withNewline = run('report', file);
  assert.equal(withNewline.status, 0, withNewline.output);
  const annotationLines = withNewline.output.split('\n').filter((line) => line.includes('annotation [auth-roundtrip.spec.ts]'));
  assert.equal(annotationLines.length, 1, `want exactly one annotation line in:\n${withNewline.output}`);
  assert.ok(annotationLines[0].includes('first line\\nsecond line'), `want the escaped newline in:\n${withNewline.output}`);
});

test('fail-expected.json: test.fail() counts as expected, the checker refuses it', () => {
  const doc = JSON.parse(readFileSync(join(FIXTURES, 'fail-expected.json'), 'utf8'));
  assert.equal(doc.stats.expected, 2, 'the fixture must keep the counter clean, as Playwright does');
  expectFail(join(FIXTURES, 'fail-expected.json'), 'expectedStatus is "failed"');
});

test('skipped.json fails on the counter and on the test', () => {
  expectFail(join(FIXTURES, 'skipped.json'), 'stats.skipped is 1');
  expectFail(join(FIXTURES, 'skipped.json'), 'expectedStatus is "skipped"');
});

test('early-return.json: every counter passes, the missing steps do not', () => {
  expectFail(join(FIXTURES, 'early-return.json'), '4 step(s) never ran');
});

test('a result with no `steps` key fails (Playwright omits the key when no step ran)', () => {
  expectFail(variant((d) => { delete d.suites[1].specs[0].tests[0].results[0].steps; }), '7 step(s) never ran');
});

test('a step with a caught error on a passing test fails, even though the test passed', () => {
  expectFail(
    variant((d) => {
      d.suites[0].specs[0].tests[0].results[0].steps[0].error = { message: 'boom' };
    }),
    'a caught step error',
  );
});

test('a failed test fails', () => {
  expectFail(
    variant((d) => {
      const t = d.suites[0].specs[0].tests[0];
      t.status = 'unexpected';
      t.results[0].status = 'failed';
      d.stats.expected = 1;
      d.stats.unexpected = 1;
    }),
    'the last result is "failed"',
  );
});

test('a flaky test (a retry passed) fails on the counter', () => {
  expectFail(
    variant((d) => {
      const t = d.suites[0].specs[0].tests[0];
      t.status = 'flaky';
      t.results = [{ ...t.results[0], status: 'failed' }, { ...t.results[0], retry: 1 }];
      d.stats.expected = 1;
      d.stats.flaky = 1;
    }),
    'stats.flaky is 1',
  );
});

test('a third test fails', () => {
  expectFail(variant((d) => { d.suites[0].specs.push(structuredClone(d.suites[0].specs[0])); }), 'want exactly 2');
});

test('a test from another project fails', () => {
  expectFail(variant((d) => { d.suites[0].specs[0].tests[0].projectName = 'phase-a'; }), 'want "journeys"');
});

test('a top-level error fails', () => {
  expectFail(variant((d) => { d.errors = [{ message: 'Error: worker crashed' }]; }), 'top-level error');
});

test('a test inside a describe block is still found', () => {
  const file = variant((d) => {
    const specs = d.suites[0].specs;
    d.suites[0].specs = [];
    d.suites[0].suites = [{ title: 'group', file: 'journeys/auth-roundtrip.spec.ts', line: 3, column: 1, specs }];
  });
  assert.equal(run('report', file).status, 0);
});

test('a missing, unparseable or shapeless report is rc 2; so is a bad command line', () => {
  assert.equal(run('report', join(FIXTURES, 'no-such-file.json')).status, 2);
  assert.equal(run('report', join(FIXTURES, 'not-json.txt')).status, 2);
  assert.equal(run('report', tempFile('report.json', '{}')).status, 2);
  assert.equal(run('report', tempFile('report.json', '{"suites":[],"stats":{"expected":"2"}}')).status, 2);
  assert.equal(run().status, 2);
  assert.equal(run('bogus', 'x').status, 2);
  assert.equal(run('report', 'a', 'b').status, 2);
});

function sourcesDir(overrides = {}) {
  const dir = mkdtempSync(join(tmpdir(), 'journeys-sources-'));
  mkdirSync(dir, { recursive: true });
  for (const [file, titles] of Object.entries(EXPECTED_STEPS)) {
    const list = Object.hasOwn(overrides, file) ? overrides[file] : titles;
    if (list === null) continue;
    const body = list.map((t) => `  await test.step(\n    '${t}',\n    async () => {},\n  );`).join('\n');
    writeFileSync(join(dir, file), `// SPDX-License-Identifier: Apache-2.0\ntest('x', async () => {\n${body}\n});\n`);
  }
  return dir;
}

test('sources: the expected titles pass, also when Prettier wraps the call', () => {
  const r = run('sources', sourcesDir());
  assert.equal(r.status, 0, r.output);
});

test('sources: a renamed, a missing and an extra step title fail', () => {
  const titles = EXPECTED_STEPS['auth-roundtrip.spec.ts'];
  for (const drift of [
    [...titles.slice(0, 5), 'renamed'],
    titles.slice(0, 5),
    [...titles, 'extra'],
  ]) {
    const r = run('sources', sourcesDir({ 'auth-roundtrip.spec.ts': drift }));
    assert.equal(r.status, 3, r.output);
    assert.match(r.output, /auth-roundtrip\.spec\.ts: test\.step titles are/);
  }
});

test('sources: a missing spec file is rc 2', () => {
  assert.equal(run('sources', sourcesDir({ 'zone-round-trip.spec.ts': null })).status, 2);
});

test('the real journeys spec files hold exactly the expected step titles', () => {
  const r = run('sources', join(HERE, '..', '..', 'ts', 'apps', 'iam-console', 'tests', 'cluster', 'journeys'));
  assert.equal(r.status, 0, r.output);
});
