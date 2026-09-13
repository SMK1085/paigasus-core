// SPDX-License-Identifier: Apache-2.0
//
// A self-signed certificate for the fake IdP and the TLS terminator. Based on
// ts/packages/paigasus-auth/tests/e2e/tls-fixture.ts, and COPIED rather than imported: one
// package's tests must not import another package's tests (spec § 9.1).
//
// Vitest runs test files in parallel processes, and Playwright runs parallel workers, so two
// processes can call testTls() at the same moment. The layout under the root is:
//   - `pair-<pid>-<random>/cert.pem` and `key.pem`: one pair per directory. A process generates
//     into a NEW directory that no other process knows about, and publishes it only when openssl
//     has written both files.
//   - `current`: a pointer file that holds the NAME of the published pair directory. A process
//     publishes by writing a scratch pointer and then one `renameSync` over `current`. The rename
//     replaces the file atomically, so a reader sees the old name or the new name, never a mix.
//
// What the code guarantees:
//   - testTls() never returns a half-written pair. It reads the pointer ONCE and then reads both
//     files from the directory that the pointer named.
//   - No code deletes, moves or rewrites a pair directory after it is published. An expired pair
//     stays where it is; only the pointer moves to the new pair. So `certPath` and `keyPath` hold
//     the same pair as the returned `cert` and `key`, even after a peer publishes a newer pair.
//     This matters for the e2e tier: it passes `certPath` to the standalone server as
//     NODE_EXTRA_CA_CERTS, which Node reads once, at process start.
//   - When two processes generate at the same moment, both publish, and the LAST rename wins the
//     pointer. Each process still returns its own complete pair. Each current caller uses one
//     TlsMaterial for both the server and the client that trusts it, so two pairs do no harm.
// What it does not guarantee: one pair per day (a race costs one extra pair), and a clean root.
// Old pair directories stay under os.tmpdir() until the OS cleans that directory.
import { execFileSync } from 'node:child_process';
import { randomBytes } from 'node:crypto';
import { existsSync, lstatSync, mkdirSync, mkdtempSync, readFileSync, renameSync, rmSync, writeFileSync } from 'node:fs';
import os from 'node:os';
import path from 'node:path';

export type TlsMaterial = { certPath: string; keyPath: string; cert: string; key: string };

/** The root every caller uses. Only the self-test passes its own root. */
const DEFAULT_TLS_ROOT = path.join(os.tmpdir(), 'paigasus-iam-console-tls');

/** A published pair is reused only while it stays valid for this many more seconds. */
const MIN_VALID_SECONDS = 300;

const PAIR_PREFIX = 'pair-';

function pointerPath(root: string): string {
  return path.join(root, 'current');
}

/** The pair directory that the pointer names, or null when there is no usable pointer. */
function publishedPair(root: string): string | null {
  let name: string;
  try {
    name = readFileSync(pointerPath(root), 'utf8');
  } catch {
    return null;
  }
  // This module writes a bare directory name. Any other content is not a pointer it wrote.
  if (!name.startsWith(PAIR_PREFIX) || path.basename(name) !== name) return null;
  return path.join(root, name);
}

/**
 * `-checkend`: reuse a pair only while it stays valid for `minValidSeconds` more seconds. The cert
 * lives one day, so a plain existence check would reuse an expired cert on day two and fail with
 * an opaque TLS error.
 */
function stillValid(dir: string, minValidSeconds: number): boolean {
  const certPath = path.join(dir, 'cert.pem');
  if (!existsSync(certPath) || !existsSync(path.join(dir, 'key.pem'))) return false;
  try {
    execFileSync('openssl', ['x509', '-in', certPath, '-checkend', String(minValidSeconds), '-noout']);
    return true;
  } catch {
    return false;
  }
}

/** Writes a complete pair into a NEW directory and returns it. The directory is not published yet. */
function generate(root: string): string {
  const dir = mkdtempSync(path.join(root, `${PAIR_PREFIX}${String(process.pid)}-`));
  try {
    execFileSync(
      'openssl',
      [
        'req',
        '-x509',
        '-newkey',
        'rsa:2048',
        '-nodes',
        '-keyout',
        path.join(dir, 'key.pem'),
        '-out',
        path.join(dir, 'cert.pem'),
        '-days',
        '1',
        '-subj',
        '/CN=localhost',
        '-addext',
        'subjectAltName=DNS:localhost,IP:127.0.0.1',
      ],
      { stdio: 'ignore' },
    );
    if (!stillValid(dir, MIN_VALID_SECONDS)) throw new Error(`testTls: openssl wrote no valid certificate in ${dir}`);
    return dir;
  } catch (error) {
    // The directory is not published, so no other process knows its name. Removing it is safe.
    rmSync(dir, { recursive: true, force: true });
    throw error;
  }
}

/**
 * Points `current` at `dir`: a scratch pointer, then ONE atomic rename over the real one.
 *
 * The shape check comes first. MEASURED on macOS: a `current` that is a DIRECTORY — an older
 * layout left in `os.tmpdir()`, which outlives a branch — makes `renameSync` fail with a bare
 * `EISDIR: illegal operation on a directory`, which names neither this module nor the remedy. This
 * module writes only a regular file there, so anything else is foreign state. It REPORTS that state
 * rather than removing it: a foreign `current` may hold files another process is using, and
 * deleting it would break the one property the layout exists to give (this module never removes
 * what it did not publish).
 */
function publish(root: string, dir: string): void {
  const pointer = pointerPath(root);
  const existing = lstatSync(pointer, { throwIfNoEntry: false });
  if (existing !== undefined && !existing.isFile()) {
    throw new Error(`testTls: ${pointer} is not a regular file, so the pointer cannot be published. Remove ${root} by hand and run again.`);
  }
  const scratch = path.join(root, `current-${String(process.pid)}-${randomBytes(8).toString('hex')}.tmp`);
  writeFileSync(scratch, path.basename(dir));
  try {
    renameSync(scratch, pointer);
  } catch (error) {
    // A losing racer must not leave its scratch pointer behind: the root would fill up with one
    // file per failed publish.
    rmSync(scratch, { force: true });
    throw error;
  }
}

function load(dir: string): TlsMaterial {
  const certPath = path.join(dir, 'cert.pem');
  const keyPath = path.join(dir, 'key.pem');
  return { certPath, keyPath, cert: readFileSync(certPath, 'utf8'), key: readFileSync(keyPath, 'utf8') };
}

/**
 * The test certificate pair. It reuses the published pair while that pair stays valid for
 * `minValidSeconds` more seconds (default 300). Otherwise it generates a pair, publishes it and
 * returns it. `root` and `minValidSeconds` are for the self-test; other callers pass nothing.
 */
export function testTls(opts: { root?: string; minValidSeconds?: number } = {}): TlsMaterial {
  const root = opts.root ?? DEFAULT_TLS_ROOT;
  const minValidSeconds = opts.minValidSeconds ?? MIN_VALID_SECONDS;
  mkdirSync(root, { recursive: true });
  const current = publishedPair(root);
  if (current !== null && stillValid(current, minValidSeconds)) return load(current);
  const fresh = generate(root);
  publish(root, fresh);
  return load(fresh);
}
