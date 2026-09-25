// SPDX-License-Identifier: Apache-2.0
//
// SMA-514 spec § 4.4: `ci/kind/run.sh stub up` checks the gateway stub from inside the cluster.
// The stub-check pod (ci/kind/manifests/stub-check.yaml) runs curl, which prints the body and
// then two marker lines from --write-out:
//
//   <body>
//   STUB-STATUS <http code>
//   STUB-CONTENT-TYPE <content type>
//
// The discovery probe (ts/packages/paigasus-discovery/src/probe.ts) needs status 200 with no
// redirect, the content-type essence application/json, and a JSON object with string `service`
// and `version`. A stub that fails any of these leaves the gateway zone `degraded`.
//
//   node ci/kind/stub-check.mjs <stub-check.log>
//
// Exit codes: 0 pass | 3 the stub answers wrongly | 2 an unreadable log or a bad command line.
// Not 1: node exits 1 on an uncaught error. run.sh maps EVERY non-zero code to its rc 2, because
// the stub is infrastructure (spec § 4.4).
import { readFileSync } from 'node:fs';
import { pathToFileURL } from 'node:url';

const STATUS_MARK = 'STUB-STATUS ';
const TYPE_MARK = 'STUB-CONTENT-TYPE ';

function markedValue(lines, mark) {
  const line = lines.find((l) => l.startsWith(mark));
  return line === undefined ? '' : line.slice(mark.length).trim();
}

export function checkStubLog(text) {
  const lines = text.split(/\r?\n/);
  const status = markedValue(lines, STATUS_MARK);
  const type = markedValue(lines, TYPE_MARK);
  const body = lines.find((l) => l.startsWith('{'));
  const problems = [];
  if (status !== '200') problems.push(`status is ${JSON.stringify(status)}, want "200"`);
  const essence = (type.split(';')[0] ?? '').trim().toLowerCase();
  if (essence !== 'application/json') problems.push(`content type is ${JSON.stringify(type)}, want the essence application/json`);
  let doc = null;
  if (body === undefined) {
    problems.push('no JSON body');
  } else {
    try {
      doc = JSON.parse(body);
    } catch {
      problems.push(`the body is not JSON: ${body}`);
    }
  }
  if (doc !== null && (typeof doc !== 'object' || typeof doc.service !== 'string' || typeof doc.version !== 'string')) {
    problems.push(`service and version must be strings, got ${body}`);
  }
  const summary = problems.length === 0 ? `stub OK: 200 ${essence}, service ${doc.service}, version ${doc.version}` : '';
  return { problems, summary };
}

export function main(argv) {
  if (argv.length !== 1) {
    console.error('usage: stub-check.mjs <stub-check.log>');
    return 2;
  }
  let text;
  try {
    text = readFileSync(argv[0], 'utf8');
  } catch (error) {
    console.error(`stub-check: cannot read ${argv[0]}: ${error instanceof Error ? error.message : String(error)}`);
    return 2;
  }
  const { problems, summary } = checkStubLog(text);
  if (problems.length > 0) {
    console.log(`stub-check: ${problems.join('; ')}`);
    return 3;
  }
  console.log(`  ${summary}`);
  return 0;
}

if (process.argv[1] !== undefined && import.meta.url === pathToFileURL(process.argv[1]).href) {
  process.exitCode = main(process.argv.slice(2));
}
