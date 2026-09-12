// SPDX-License-Identifier: Apache-2.0
//
// A minimal Next app for tests/build/client-boundary.test.ts. It builds through the REAL factory,
// so the SDK is transpiled exactly as in the iam-console. ts/ is five levels up.
import path from 'node:path';
import { createNextConfig } from '@paigasus/next-config';

export default createNextConfig({
  zone: 'iam',
  basePath: '/iam',
  outputFileTracingRoot: path.resolve(import.meta.dirname, '../../../../..'),
});
