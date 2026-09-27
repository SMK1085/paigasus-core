// SPDX-License-Identifier: Apache-2.0
//
// SMA-705 D8. logDiscoveryFailed is the one emitter of `oidc.discovery_failed`. The login and the
// callback route call it (SMA-656, tests/http/discovery-failed.test.ts covers them through the
// routes), and the readiness route calls it with the stage 'readiness'.
import { describe, expect, it } from 'vitest';
import { OidcDiscoveryFailed } from '../../src/core/errors.js';
import { logDiscoveryFailed } from '../../src/http/discovery-log.js';
import { IDP_SENTINELS, SENTINEL_IDP_URL, expectEventsClean, harness } from '../support/store-failure.js';

/** No store call happens in these rows. */
const noStoreFailure = (): Error => new Error('no store call in the SMA-705 rows');

describe('logDiscoveryFailed (SMA-705 D8)', () => {
  it.each(['login', 'callback', 'readiness'] as const)('logs one event with the %s stage and the closed reason', (stage) => {
    const h = harness([], noStoreFailure);
    logDiscoveryFailed(h.runtime, stage, new OidcDiscoveryFailed(`oidc discovery failed at ${SENTINEL_IDP_URL}`, 'tls'));
    expect(h.events).toEqual([['oidc.discovery_failed', { zone: 'iam', stage, reason: 'tls' }]]);
    expectEventsClean(h.events, IDP_SENTINELS);
  });

  it('logs the reason other for an error that is not an OidcDiscoveryFailed', () => {
    const h = harness([], noStoreFailure);
    logDiscoveryFailed(h.runtime, 'readiness', new Error(`fetch failed at ${SENTINEL_IDP_URL}`));
    expect(h.events).toEqual([['oidc.discovery_failed', { zone: 'iam', stage: 'readiness', reason: 'other' }]]);
    expectEventsClean(h.events, IDP_SENTINELS);
  });
});
