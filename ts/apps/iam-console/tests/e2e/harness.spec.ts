// SPDX-License-Identifier: Apache-2.0
//
// The harness itself: the standalone server answers through the TLS terminator on an https origin.
// Every scenario in this directory depends on this, so it fails first and loudly.
import { expect, test } from './support/harness';

test('harness: /iam/healthz answers through the TLS terminator with the one configured zone', async ({ request, harness }) => {
  expect(new URL(harness.origin).protocol).toBe('https:');
  const response = await request.get(harness.url('/iam/healthz'));
  expect(response.status()).toBe(200);
  expect(await response.json()).toMatchObject({ zone: 'iam', zones: { iam: '/iam' } });
});

// SMA-705 T16. The positive half of /iam/readyz through the real build. The TLS fake IdP answers
// discovery, so the probe must reach 200 within the bound. An earlier spec of this run can have
// logged in, which discovers too. Either way the route must answer 200.
test('harness: /iam/readyz reaches 200 ready once discovery succeeded (SMA-705 T16)', async ({ request, harness }) => {
  const url = harness.url('/iam/readyz');
  let last = 0;
  await expect
    .poll(
      async () => {
        last = (await request.get(url, { maxRedirects: 0 })).status();
        return last;
      },
      { timeout: 15_000, intervals: [250], message: 'GET /iam/readyz never answered 200' },
    )
    .toBe(200)
    .catch((error: unknown) => {
      throw new Error(`the last status was ${String(last)}\n--- server output ---\n${harness.serverOutput()}`, { cause: error });
    });
  const response = await request.get(url, { maxRedirects: 0 });
  expect(response.status()).toBe(200);
  expect(response.headers()['cache-control']).toBe('no-store');
  expect(await response.json()).toEqual({ status: 'ready' });
  expect(harness.serverOutput()).not.toContain('"stage":"readiness"');
  expect(harness.serverOutput()).not.toContain('readiness.runtime_failed');
});
