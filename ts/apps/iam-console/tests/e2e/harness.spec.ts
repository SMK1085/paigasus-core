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
