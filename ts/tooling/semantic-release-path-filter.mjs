// SPDX-License-Identifier: Apache-2.0
//
// In-repo semantic-release path-filter (SMA-406). Replaces the abandoned
// `semantic-release-monorepo` plugin: restricts the analyzed commits to those
// touching the current package's directory (cwd) before delegating
// classification to `@semantic-release/commit-analyzer`.
//
// IMPORTANT: this must be the ONLY `analyzeCommits` provider in the `plugins`
// array. semantic-release runs `analyzeCommits` for every plugin and takes the
// max release type, so also listing `@semantic-release/commit-analyzer`
// separately would classify the UNFILTERED commit set and defeat the filter.
import { execFileSync } from 'node:child_process';
import * as commitAnalyzer from '@semantic-release/commit-analyzer';

export async function analyzeCommits(pluginConfig, context) {
  const { cwd, commits } = context;
  // semantic-release commit objects carry no file list, so ask git which commits
  // touched this package dir (cwd) and intersect with the since-last-release set.
  const touched = new Set(execFileSync('git', ['log', '--format=%H', '--', '.'], { cwd, encoding: 'utf8' }).split('\n').filter(Boolean));
  const filtered = commits.filter((commit) => touched.has(commit.hash));
  return commitAnalyzer.analyzeCommits(pluginConfig, { ...context, commits: filtered });
}
