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

// Compute the next version as a LOCAL run against the package's own git branch.
// Inside CI (e.g. GitHub Actions) semantic-release's env-ci would otherwise read the
// ambient CI branch (GITHUB_HEAD_REF/GITHUB_REF) instead of this repo's branch and
// classify it as a non-release branch -> no release. `ci:false` skips CI *verification*
// but env-ci still reads the ambient branch, so strip the CI markers from the env too.
const env = { ...process.env };
for (const key of ['CI', 'CONTINUOUS_INTEGRATION', 'GITHUB_ACTIONS', 'GITHUB_REF', 'GITHUB_HEAD_REF', 'GITHUB_REF_NAME', 'GITHUB_BASE_REF', 'GITHUB_EVENT_NAME']) {
  delete env[key];
}

try {
  const result = await semanticRelease({ dryRun: true, ci: false }, { cwd, env, stdout: process.stderr, stderr: process.stderr });
  // `result` is `false` when no release is due (e.g. no qualifying commit).
  process.stdout.write(result ? result.nextRelease.version : '');
} catch (err) {
  process.stderr.write(`\nsemantic-release JS API failed in ${cwd}: ${err?.message ?? err}\n`);
  process.exit(1);
}
