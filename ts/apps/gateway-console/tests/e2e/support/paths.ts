// SPDX-License-Identifier: Apache-2.0
import path from 'node:path';
import { fileURLToPath } from 'node:url';

export const APP_DIR = fileURLToPath(new URL('../../..', import.meta.url));
// createNextConfig pins outputFileTracingRoot to ts/, so the entry point lands under the app's path
// relative to ts/ (the same path the `build` task asserts).
export const STANDALONE_APP_DIR = path.join(APP_DIR, '.next', 'standalone', 'apps', 'gateway-console');
// The ts/ workspace root. Kept separate from APP_DIR, which names the app's OWN root: composing
// '..'/'..' on top of APP_DIR to reach a package under ts/packages/ would smuggle a second, unrelated
// kind of knowledge (where ts/ is) through a constant whose name promises something narrower.
export const TS_ROOT = fileURLToPath(new URL('../../../../..', import.meta.url));

// The two-zone tier (SMA-512 PR4 task 3) also spawns the `iam-console` standalone server, so its
// path is derived from TS_ROOT, not by composing '..' onto APP_DIR (which names THIS app's root).
export const IAM_CONSOLE_APP_DIR = path.join(TS_ROOT, 'apps', 'iam-console');
export const IAM_CONSOLE_STANDALONE_DIR = path.join(IAM_CONSOLE_APP_DIR, '.next', 'standalone', 'apps', 'iam-console');

// The repository root (SMA-635): the playground project runs rs/target/debug/paigasus-gateway.
export const REPO_ROOT = fileURLToPath(new URL('../../../../../..', import.meta.url));
