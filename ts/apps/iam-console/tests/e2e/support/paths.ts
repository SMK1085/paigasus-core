// SPDX-License-Identifier: Apache-2.0
import path from 'node:path';
import { fileURLToPath } from 'node:url';

export const APP_DIR = fileURLToPath(new URL('../../..', import.meta.url));
// createNextConfig pins outputFileTracingRoot to ts/, so the entry point lands under the app's path
// relative to ts/ (the same path the `build` task asserts).
export const STANDALONE_APP_DIR = path.join(APP_DIR, '.next', 'standalone', 'apps', 'iam-console');
