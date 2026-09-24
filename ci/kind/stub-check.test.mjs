// SPDX-License-Identifier: Apache-2.0
//
// Unit tests for stub-check.mjs (SMA-514 spec § 4.4, Review Focus 3). Run:
//   node --test ci/kind/stub-check.test.mjs
// `ci/kind/run.sh specs journeys` runs this file before its Playwright run. No Moon task and no
// required CI check runs it.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';
import { checkStubLog } from './stub-check.mjs';

const CHECKER = join(dirname(fileURLToPath(import.meta.url)), 'stub-check.mjs');
const BODY = '{"service":"gateway","version":"0.0.0-kind-stub","capabilities":[]}';

function log({ body = BODY, status = '200', type = 'application/json' } = {}) {
  return `${body}\n\nSTUB-STATUS ${status}\nSTUB-CONTENT-TYPE ${type}\n`;
}

function cli(text) {
  const file = join(mkdtempSync(join(tmpdir(), 'stub-check-')), 'stub-check.log');
  if (text !== null) writeFileSync(file, text);
  const r = spawnSync(process.execPath, [CHECKER, file], { encoding: 'utf8' });
  return { status: r.status, output: `${r.stdout}${r.stderr}` };
}

test('a correct answer passes', () => {
  const r = checkStubLog(log());
  assert.deepEqual(r.problems, []);
  assert.match(r.summary, /service gateway, version 0\.0\.0-kind-stub/);
});

test('a charset parameter and an upper-case type pass (the probe compares the essence)', () => {
  assert.deepEqual(checkStubLog(log({ type: 'application/json; charset=utf-8' })).problems, []);
  assert.deepEqual(checkStubLog(log({ type: 'Application/JSON' })).problems, []);
});

for (const [name, input, fragment] of [
  ['text/plain', { type: 'text/plain' }, 'content type is "text/plain"'],
  ['octet-stream (nginx with no default_type)', { type: 'application/octet-stream' }, 'content type is "application/octet-stream"'],
  ['an empty content type', { type: '' }, 'content type is ""'],
  ['301 (a redirect; the probe uses redirect: error)', { status: '301' }, 'status is "301"'],
  ['404 (the wrong path)', { status: '404' }, 'status is "404"'],
  ['no body', { body: '' }, 'no JSON body'],
  ['a body that is not JSON', { body: '{"service":' }, 'the body is not JSON'],
  ['version is a number', { body: '{"service":"gateway","version":1}' }, 'service and version must be strings'],
  ['service is missing', { body: '{"version":"1"}' }, 'service and version must be strings'],
]) {
  test(`${name} fails`, () => {
    const problems = checkStubLog(log(input)).problems;
    assert.ok(problems.some((p) => p.includes(fragment)), `want "${fragment}" in ${JSON.stringify(problems)}`);
  });
}

test('a log with no marker lines fails on status and content type', () => {
  const problems = checkStubLog(`${BODY}\n`).problems;
  assert.ok(problems.some((p) => p.includes('status is ""')), JSON.stringify(problems));
  assert.ok(problems.some((p) => p.includes('content type is ""')), JSON.stringify(problems));
});

test('CLI: 0 on a correct answer, 3 on a wrong one, 2 on a missing file or a bad command line', () => {
  assert.equal(cli(log()).status, 0);
  const wrong = cli(log({ type: 'text/plain' }));
  assert.equal(wrong.status, 3, wrong.output);
  assert.match(wrong.output, /text\/plain/);
  assert.equal(cli(null).status, 2);
  assert.equal(spawnSync(process.execPath, [CHECKER], { encoding: 'utf8' }).status, 2);
});
