// SPDX-License-Identifier: Apache-2.0
//
// Dry-run a single package through semantic-release via its JS API and print the
// computed next version to stdout (empty string if no release is due). Used by
// the SMA-406 parity adapter. The JS API returns the structured next release, so
// we never scrape the human-readable CLI log. semantic-release's own logs are
// routed to stderr so stdout carries ONLY the version.
import semanticRelease from 'semantic-release';

const cwd = process.argv[2];
if (!cwd) {
  process.stderr.write('usage: semantic-release-next-version.mjs <package-dir>\n');
  process.exit(2);
}

try {
  const result = await semanticRelease({ dryRun: true, ci: false }, { cwd, stdout: process.stderr, stderr: process.stderr });
  // `result` is `false` when no release is due (e.g. no qualifying commit).
  process.stdout.write(result ? result.nextRelease.version : '');
} catch (err) {
  process.stderr.write(`\nsemantic-release JS API failed in ${cwd}: ${err?.message ?? err}\n`);
  process.exit(1);
}
