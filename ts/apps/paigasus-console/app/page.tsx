// SPDX-License-Identifier: Apache-2.0
import { connection } from 'next/server';
import { getRuntimeConfig } from './runtime-config';

// `await connection()` opts this page out of prerendering. Without it, Next would evaluate the
// page during `next build`, getRuntimeConfig() would throw, and the build would fail — which is
// the guard working as designed. This is the pattern every config-reading page follows.
export default async function Page() {
  await connection();
  const config = getRuntimeConfig();
  return (
    <main>
      <h1>Paigasus console</h1>
      <p data-testid="zone">{config.PAIGASUS_ZONE}</p>
      <pre data-testid="zones">{JSON.stringify(config.PAIGASUS_ZONES)}</pre>
    </main>
  );
}
