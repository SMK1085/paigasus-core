// SPDX-License-Identifier: Apache-2.0
//
// lib/correlation.ts: the two request headers proxy.ts sets, read back defensively.
import { describe, expect, it } from 'vitest';
import { CORRELATION_HEADER, REQUEST_PATH_HEADER, requestCorrelationId, requestPath } from '../../lib/correlation';
import { setRequestHeaders } from '../support/next-headers';

describe('the request correlation helpers', () => {
  it('uses the header name IAM adopts', () => {
    expect(CORRELATION_HEADER).toBe('paigasus-correlation-id');
  });

  it('returns the id proxy.ts set', async () => {
    setRequestHeaders({ [CORRELATION_HEADER]: '0198f2c1-8888-7000-8000-000000000042' });
    expect(await requestCorrelationId()).toBe('0198f2c1-8888-7000-8000-000000000042');
  });

  it('treats an absent or non-UUID value as no id', async () => {
    expect(await requestCorrelationId()).toBeNull();
    setRequestHeaders({ [CORRELATION_HEADER]: '<script>' });
    expect(await requestCorrelationId()).toBeNull();
  });

  it('returns the request path only when it is an absolute path', async () => {
    setRequestHeaders({ [REQUEST_PATH_HEADER]: '/iam/orgs' });
    expect(await requestPath()).toBe('/iam/orgs');
    setRequestHeaders({ [REQUEST_PATH_HEADER]: 'https://elsewhere.test/' });
    expect(await requestPath()).toBeNull();
  });
});
