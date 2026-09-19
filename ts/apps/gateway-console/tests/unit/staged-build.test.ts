// SPDX-License-Identifier: Apache-2.0
//
// assertStagedBuild (SMA-655). Each case builds a temporary app tree and a temporary standalone
// tree, then damages exactly one thing, so a failure can only come from what the case changes.
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { afterEach, describe, expect, it } from 'vitest';
import { assertStagedBuild, StagedBuildError } from '../e2e/support/staged-build';

const BUILD_ID = 'abc123';
const TASK = 'demo-ts:build';
const roots: string[] = [];

function write(file: string, content: string): void {
  mkdirSync(path.dirname(file), { recursive: true });
  writeFileSync(file, content);
}

/** A complete, correctly staged pair of trees. */
function stagedPair(): { appDir: string; standaloneDir: string } {
  const root = mkdtempSync(path.join(tmpdir(), 'staged-build-'));
  roots.push(root);
  const appDir = path.join(root, 'app');
  const standaloneDir = path.join(appDir, '.next', 'standalone', 'apps', 'demo');
  write(path.join(appDir, '.next', 'BUILD_ID'), BUILD_ID);
  write(path.join(standaloneDir, '.next', 'BUILD_ID'), BUILD_ID);
  for (const tree of [path.join(appDir, '.next', 'static'), path.join(standaloneDir, '.next', 'static')]) {
    write(path.join(tree, BUILD_ID, '_buildManifest.js'), 'manifest');
    write(path.join(tree, 'chunks', 'main.js'), 'main-chunk');
  }
  return { appDir, standaloneDir };
}

function failure(appDir: string, standaloneDir: string): StagedBuildError {
  try {
    assertStagedBuild(appDir, standaloneDir, TASK);
  } catch (error) {
    if (error instanceof StagedBuildError) return error;
    throw error;
  }
  throw new Error('expected assertStagedBuild to throw, and it did not');
}

afterEach(() => {
  for (const root of roots.splice(0)) rmSync(root, { recursive: true, force: true });
});

describe('assertStagedBuild', () => {
  it('accepts a complete staged tree', () => {
    const { appDir, standaloneDir } = stagedPair();
    expect(() => assertStagedBuild(appDir, standaloneDir, TASK)).not.toThrow();
  });

  it('accepts extra files in the staged tree, because a cache-hit restore merges', () => {
    const { appDir, standaloneDir } = stagedPair();
    write(path.join(standaloneDir, '.next', 'static', 'old-build', '_buildManifest.js'), 'stale');
    expect(() => assertStagedBuild(appDir, standaloneDir, TASK)).not.toThrow();
  });

  it('refuses an empty BUILD_ID, which would make every path check name static/ itself', () => {
    const { appDir, standaloneDir } = stagedPair();
    write(path.join(appDir, '.next', 'BUILD_ID'), '  \n');
    expect(failure(appDir, standaloneDir).message).toMatch(/BUILD_ID is missing or empty/);
  });

  it('trims the BUILD_ID, so a trailing newline is not a mismatch', () => {
    const { appDir, standaloneDir } = stagedPair();
    write(path.join(appDir, '.next', 'BUILD_ID'), `${BUILD_ID}\n`);
    expect(() => assertStagedBuild(appDir, standaloneDir, TASK)).not.toThrow();
  });

  it('refuses a standalone tree from a different build', () => {
    const { appDir, standaloneDir } = stagedPair();
    write(path.join(standaloneDir, '.next', 'BUILD_ID'), 'other-build');
    expect(failure(appDir, standaloneDir).message).toMatch(/BUILD_ID is 'other-build', not 'abc123'/);
  });

  it('refuses a staged tree with no directory for this build', () => {
    const { appDir, standaloneDir } = stagedPair();
    rmSync(path.join(standaloneDir, '.next', 'static', BUILD_ID), { recursive: true });
    expect(failure(appDir, standaloneDir).message).toMatch(/static\/abc123 is not a directory/);
  });

  it('refuses a staged tree with a missing file, and names it', () => {
    const { appDir, standaloneDir } = stagedPair();
    rmSync(path.join(standaloneDir, '.next', 'static', 'chunks', 'main.js'));
    expect(failure(appDir, standaloneDir).message).toMatch(/missing: chunks\/main\.js/);
  });

  it('refuses a staged file whose size differs, and names it', () => {
    const { appDir, standaloneDir } = stagedPair();
    write(path.join(standaloneDir, '.next', 'static', 'chunks', 'main.js'), 'short');
    expect(failure(appDir, standaloneDir).message).toMatch(/size differs: chunks\/main\.js/);
  });

  it('refuses an app tree with no static files at all', () => {
    const { appDir, standaloneDir } = stagedPair();
    rmSync(path.join(appDir, '.next', 'static'), { recursive: true });
    expect(failure(appDir, standaloneDir).message).toMatch(/no files under/);
  });

  it('names the forced build in every failure', () => {
    const { appDir, standaloneDir } = stagedPair();
    write(path.join(standaloneDir, '.next', 'BUILD_ID'), 'other-build');
    expect(failure(appDir, standaloneDir).message).toContain('moon run demo-ts:build --force');
  });
});
