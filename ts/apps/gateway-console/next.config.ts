// SPDX-License-Identifier: Apache-2.0
import { fileURLToPath } from 'node:url';
import { createNextConfig } from '@paigasus/next-config';

// The AI Gateway zone. `zone` and `basePath` describe the IMAGE, not the deployment: this image IS
// the gateway zone, and Next 16 has no runtime basePath, so the prefix is necessarily compiled in.
// Every deployment-varying value is read at request time instead (spec § 4, § 11).
//
// @paigasus/next-config/runtime cross-checks these two values against PAIGASUS_ZONE and
// PAIGASUS_ZONES at first request and fails closed if they disagree.
export default createNextConfig({
  zone: 'gateway',
  basePath: '/gateway',
  // ts/ — the pnpm workspace root. The inferred value decides where the standalone entry point
  // lands, so it is pinned rather than left to change under a lockfile move.
  outputFileTracingRoot: fileURLToPath(new URL('../..', import.meta.url)),
  // forbidden() and (console)/forbidden.tsx give the 403 view a real HTTP 403 (spec § 7.3).
  // EXPERIMENTAL in Next 16.3.4: the e2e tier asserts the 403 status, so an upgrade that changes
  // the flag fails CI instead of silently rendering the view with status 200.
  extend: { experimental: { authInterrupts: true } },
});
