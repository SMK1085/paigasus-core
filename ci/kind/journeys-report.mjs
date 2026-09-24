// SPDX-License-Identifier: Apache-2.0
//
// SMA-514 spec § 7.2: the checks on the `journeys` Playwright project that run in
// `ci/kind/run.sh specs journeys`.
//
//   node ci/kind/journeys-report.mjs sources <tests/cluster/journeys>   before the run
//   node ci/kind/journeys-report.mjs report <report.json>               after the run
//
// `sources` asserts that each spec file calls test.step with exactly the titles below, in order.
// `report` reads the JSON report and asserts: 0 skipped, 0 unexpected, 0 flaky, no top-level
// error, exactly 2 tests, both in project `journeys`, each with expectedStatus "passed", a last
// result "passed", and every one of its step titles. The counters alone are not enough (measured
// on Playwright 1.63): a test marked test.fail() that fails counts as `expected`, and a test that
// returns early passes every counter with a shorter step list.
//
// Field names: Playwright 1.63 JSONReport (node_modules/playwright/types/testReporter.d.ts). The
// report keeps only `test.step` steps and omits `steps` when a result has none.
//
// Controller ruling F3: when every check in `report` mode passes, print each test's annotations
// (type and description), one per line, after the pass summary. `chart.yml` uploads no artifact
// on a green run, so this job-log line is the only surviving record of a journey's own notes
// (for example scenario 1's logout hops). Annotations never change the exit code.
//
// Exit codes: 0 pass | 3 a check failed on a well-formed input | 2 a missing, unreadable or
// malformed input, or a bad command line. Not 1: node exits 1 on an uncaught error, and a crash
// must never read as a failed journey. run.sh maps 3 to its rc 1 and every other non-zero code to
// its rc 2 (decision D6).
import { readFileSync } from 'node:fs';
import { basename, join } from 'node:path';
import { pathToFileURL } from 'node:url';

export const RC_OK = 0;
export const RC_INFRA = 2;
export const RC_ASSERT = 3;

/** The two journeys files and their test.step titles, in order. The spec files must match. */
export const EXPECTED_STEPS = Object.freeze({
  'auth-roundtrip.spec.ts': Object.freeze([
    'cold visit: /iam/orgs goes through /iam/auth/login to the IdP form',
    'login: the callback returns to /iam/orgs with a session',
    'controls: the sid replays in both zones, and the IdP holds an SSO session',
    'logout: the shell form ends at the IdP and returns to /iam/ with no session cookie',
    'the old sid is refused by both zones',
    'the IdP session is gone: a new page shows the IdP form',
  ]),
  'zone-round-trip.spec.ts': Object.freeze([
    'record every request of the context',
    'log in at /iam/orgs',
    'wait until the Gateway nav entry is a link',
    'RSC check after hydration, before any click',
    'IAM to gateway: a hard navigation with the same session',
    'gateway to IAM: a hard navigation with the same session',
    'no RSC request leaves its zone',
  ]),
});

export class MalformedInput extends Error {}

function messageOf(error) {
  return error instanceof Error ? error.message : String(error);
}

export function parseReport(text) {
  let doc;
  try {
    doc = JSON.parse(text);
  } catch (error) {
    throw new MalformedInput(`the report is not JSON: ${messageOf(error)}`);
  }
  if (typeof doc !== 'object' || doc === null || !Array.isArray(doc.suites) || typeof doc.stats !== 'object' || doc.stats === null) {
    throw new MalformedInput('the report has no `suites` array or no `stats` object');
  }
  for (const key of ['expected', 'unexpected', 'flaky', 'skipped']) {
    if (typeof doc.stats[key] !== 'number') throw new MalformedInput(`stats.${key} is not a number`);
  }
  return doc;
}

function collectSpecs(suites, out) {
  for (const suite of suites) {
    if (typeof suite !== 'object' || suite === null) throw new MalformedInput('a suite is not an object');
    for (const spec of suite.specs ?? []) {
      if (typeof spec !== 'object' || spec === null || !Array.isArray(spec.tests)) throw new MalformedInput('a spec has no `tests` array');
      out.push(spec);
    }
    collectSpecs(suite.suites ?? [], out);
  }
  return out;
}

function collectStepTitles(steps, out) {
  for (const step of steps ?? []) {
    out.add(step.title);
    collectStepTitles(step.steps, out);
  }
  return out;
}

export function checkReport(doc) {
  const problems = [];
  for (const key of ['skipped', 'unexpected', 'flaky']) {
    if (doc.stats[key] !== 0) problems.push(`stats.${key} is ${String(doc.stats[key])}, want 0`);
  }
  if (Array.isArray(doc.errors) && doc.errors.length > 0) {
    problems.push(`the report holds ${String(doc.errors.length)} top-level error(s): ${doc.errors.map((e) => String(e?.message ?? e)).join(' | ')}`);
  }
  const tests = collectSpecs(doc.suites, []).flatMap((spec) => spec.tests.map((t) => ({ spec, test: t })));
  if (tests.length !== 2) problems.push(`the report holds ${String(tests.length)} test(s), want exactly 2 (SMA-514 AC 3)`);
  const seen = new Set();
  for (const { spec, test } of tests) {
    const file = basename(String(spec.file ?? ''));
    const name = `${file} ${JSON.stringify(spec.title)}`;
    if (!Object.hasOwn(EXPECTED_STEPS, file)) {
      problems.push(`${name}: not one of the journeys files ${Object.keys(EXPECTED_STEPS).join(', ')}`);
      continue;
    }
    if (seen.has(file)) problems.push(`${name}: a second test in this file`);
    seen.add(file);
    if (test.projectName !== 'journeys') problems.push(`${name}: project ${JSON.stringify(test.projectName)}, want "journeys"`);
    if (test.expectedStatus !== 'passed') {
      problems.push(`${name}: expectedStatus is ${JSON.stringify(test.expectedStatus)}, want "passed" (a test.fail(), test.skip() or test.fixme())`);
    }
    const results = Array.isArray(test.results) ? test.results : [];
    const last = results.at(-1);
    if (last === undefined) {
      problems.push(`${name}: no result`);
      continue;
    }
    if (last.status !== 'passed') problems.push(`${name}: the last result is ${JSON.stringify(last.status)}, want "passed"`);
    const titles = collectStepTitles(last.steps, new Set());
    const missing = EXPECTED_STEPS[file].filter((title) => !titles.has(title));
    if (missing.length > 0) {
      problems.push(`${name}: ${String(missing.length)} step(s) never ran: ${missing.map((t) => JSON.stringify(t)).join(', ')} (an early return passes every counter)`);
    }
  }
  for (const file of Object.keys(EXPECTED_STEPS)) {
    if (!seen.has(file)) problems.push(`${file}: no test from this file in the report`);
  }
  return problems;
}

/** Collects each test's own `annotations` (not a result's), for the F3 pass-mode printout. */
function collectAnnotations(doc) {
  const rows = [];
  for (const spec of collectSpecs(doc.suites, [])) {
    for (const test of spec.tests) {
      for (const annotation of test.annotations ?? []) {
        rows.push({ file: basename(String(spec.file ?? '')), title: spec.title, annotation });
      }
    }
  }
  return rows;
}

const STEP_TITLE = /test\.step\(\s*'([^']*)'/g;

export function checkSources(dir) {
  const problems = [];
  for (const [file, want] of Object.entries(EXPECTED_STEPS)) {
    let text;
    try {
      text = readFileSync(join(dir, file), 'utf8');
    } catch (error) {
      throw new MalformedInput(`cannot read ${join(dir, file)}: ${messageOf(error)}`);
    }
    const got = [...text.matchAll(STEP_TITLE)].map((m) => m[1]);
    if (JSON.stringify(got) !== JSON.stringify(want)) {
      problems.push(`${file}: test.step titles are ${JSON.stringify(got)}, want ${JSON.stringify(want)} (EXPECTED_STEPS in ci/kind/journeys-report.mjs)`);
    }
  }
  return problems;
}

export function main(argv) {
  const [mode, target, ...rest] = argv;
  if ((mode !== 'report' && mode !== 'sources') || target === undefined || rest.length > 0) {
    console.error('usage: journeys-report.mjs report <report.json> | sources <journeys dir>');
    return RC_INFRA;
  }
  let problems;
  let doc;
  try {
    if (mode === 'report') {
      let text;
      try {
        text = readFileSync(target, 'utf8');
      } catch (error) {
        throw new MalformedInput(`cannot read the report ${target}: ${messageOf(error)}`);
      }
      doc = parseReport(text);
      problems = checkReport(doc);
    } else {
      problems = checkSources(target);
    }
  } catch (error) {
    if (!(error instanceof MalformedInput)) throw error;
    console.error(`journeys-report: ${error.message}`);
    return RC_INFRA;
  }
  if (problems.length > 0) {
    for (const problem of problems) console.log(`FAIL [journeys ${mode}] ${problem}`);
    return RC_ASSERT;
  }
  console.log(mode === 'report' ? '  ok [journeys report]: 2 tests passed, 0 skipped, 0 flaky, every step ran' : '  ok [journeys sources]: both spec files hold the expected step titles');
  if (mode === 'report') {
    for (const { file, title, annotation } of collectAnnotations(doc)) {
      console.log(`  annotation [${file}] ${JSON.stringify(title)}: ${annotation.type}${annotation.description ? ` - ${annotation.description}` : ''}`);
    }
  }
  return RC_OK;
}

if (process.argv[1] !== undefined && import.meta.url === pathToFileURL(process.argv[1]).href) {
  process.exitCode = main(process.argv.slice(2));
}
