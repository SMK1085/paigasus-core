// SPDX-License-Identifier: Apache-2.0
//
// The ONE check that a standalone tree is staged for an e2e tier (SMA-655). The standalone output
// has no .next/static of its own (measured, SMA-510), so the Moon `build` task copies it in
// (moon.yml). This module only READS. No e2e tier may write into a build tree: iam-console's tree
// serves both this app's tier and gateway-console's two-zone tier, Moon runs the two at the same
// time, and when each tier staged the tree for itself, one tier's delete wiped it under the other.
// tests/unit/e2e-read-only.test.ts reds any fs write under tests/e2e/.
import { existsSync, readdirSync, readFileSync, statSync } from 'node:fs';
import path from 'node:path';

export class StagedBuildError extends Error {
  override readonly name = 'StagedBuildError';
}

function readBuildId(file: string): string {
  return existsSync(file) ? readFileSync(file, 'utf8').trim() : '';
}

/** Every regular file under `root`, as paths relative to it, with `/` separators. */
function listFiles(root: string, dir: string = root, out: string[] = []): string[] {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) listFiles(root, full, out);
    else if (entry.isFile()) out.push(path.relative(root, full).split(path.sep).join('/'));
  }
  return out;
}

/**
 * Throws a StagedBuildError unless `standaloneDir` holds this build's static tree: the same
 * non-empty BUILD_ID as `appDir`, a `static/<BUILD_ID>` directory, and every file under
 * `appDir/.next/static` with the same size. Extra staged files pass, because a Moon cache-hit
 * restore merges into the existing tree instead of replacing it.
 */
export function assertStagedBuild(appDir: string, standaloneDir: string, buildTask: string): void {
  const fail = (reason: string): never => {
    throw new StagedBuildError(
      `${reason}. The \`build\` task stages the standalone tree: run \`moon run ${buildTask} --force\`. ` +
        '`--force` is needed because a bare `next build` deletes the staged tree without staging it ' +
        'again, and Moon then sees an unchanged hash and skips the build.',
    );
  };
  const buildId = readBuildId(path.join(appDir, '.next', 'BUILD_ID'));
  if (buildId === '') fail(`${path.join(appDir, '.next', 'BUILD_ID')}: BUILD_ID is missing or empty`);
  const stagedId = readBuildId(path.join(standaloneDir, '.next', 'BUILD_ID'));
  if (stagedId !== buildId) fail(`the standalone tree's BUILD_ID is '${stagedId}', not '${buildId}'`);
  const sourceStatic = path.join(appDir, '.next', 'static');
  const stagedStatic = path.join(standaloneDir, '.next', 'static');
  const buildDir = path.join(stagedStatic, buildId);
  if (!existsSync(buildDir) || !statSync(buildDir).isDirectory()) fail(`${buildDir.split(path.sep).join('/')} is not a directory`);
  const sources = existsSync(sourceStatic) ? listFiles(sourceStatic) : [];
  if (sources.length === 0) fail(`no files under ${sourceStatic}`);
  const problems: string[] = [];
  for (const rel of sources) {
    const staged = path.join(stagedStatic, rel);
    if (!existsSync(staged)) problems.push(`missing: ${rel}`);
    else if (statSync(staged).size !== statSync(path.join(sourceStatic, rel)).size) problems.push(`size differs: ${rel}`);
  }
  if (problems.length > 0) fail(`the staged static tree does not match ${sourceStatic} (${problems.length}): ${problems.slice(0, 5).join(', ')}`);
}
