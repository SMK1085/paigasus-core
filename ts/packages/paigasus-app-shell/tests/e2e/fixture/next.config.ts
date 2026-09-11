// SPDX-License-Identifier: Apache-2.0
//
// The fixture builds through the REAL factory, so its `next build` also proves that
// SOURCE_ONLY_PACKAGES is sufficient for an app that renders @paigasus/app-shell (spec § 10.3).
// createNextConfig forces output: 'standalone'; tests/e2e/global-setup.ts serves that output.
import path from 'node:path';
import { createNextConfig } from '@paigasus/next-config';

// ts/ — the pnpm workspace root, as in the console (five levels up from this directory). The
// standalone entry point lands under this fixture's path RELATIVE to it (measured in the spike).
const workspaceRoot = path.resolve(import.meta.dirname, '../../../../..');

export default createNextConfig({ zone: 'iam', basePath: '/iam', outputFileTracingRoot: workspaceRoot });
