// SPDX-License-Identifier: Apache-2.0
import { connection } from 'next/server';
import { getPublicConfig } from './runtime-config';

// `await connection()` opts this page out of prerendering. Without it, Next would evaluate the
// page during `next build`, the runtime read would throw, and the build would fail — which is the
// guard working as designed. This is the pattern every config-reading page follows.
//
// getPublicConfig(), not getRuntimeConfig(). This page renders only the zone and the zone map,
// which are exactly the client-safe slice, and routing it through the sanitizer gives that
// projection a PRODUCTION call site. Without one, the § 9.2 sanitizer tests would exercise a
// function nothing calls, and the standalone runtime smoke would not cover it at all.
export default async function Page() {
  await connection();
  const config = getPublicConfig();
  return (
    <main>
      <h1>Paigasus console</h1>
      <p data-testid="zone">{config.zone}</p>
      <pre data-testid="zones">{JSON.stringify(config.zones)}</pre>
    </main>
  );
}
