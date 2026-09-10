// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it, vi } from 'vitest';
import { probeService } from '../src/probe.js';

const OK_BODY = JSON.stringify({ service: 'iam', version: '1.2.3', capabilities: ['iam.audit'] });

function res(body: string, init: { status?: number; contentType?: string | null } = {}): Response {
  const headers = new Headers();
  if (init.contentType !== null) headers.set('content-type', init.contentType ?? 'application/json');
  return new Response(body, { status: init.status ?? 200, headers });
}

function probeWith(fetchImpl: typeof globalThis.fetch, token = 'tok') {
  return probeService({
    baseUrl: 'http://iam:8080',
    token,
    timeoutMs: 1_000,
    maxBytes: 64 * 1024,
    fetch: fetchImpl,
  });
}

describe('probeService', () => {
  it('returns the descriptor on a well-formed 200', async () => {
    const out = await probeWith(() => Promise.resolve(res(OK_BODY)));
    expect(out).toEqual({
      ok: true,
      descriptor: { service: 'iam', version: '1.2.3', capabilities: ['iam.audit'] },
    });
  });

  it('requests /v1/service-info with a bearer token and refuses redirects', async () => {
    const fetchImpl = vi.fn<typeof globalThis.fetch>(() => Promise.resolve(res(OK_BODY)));
    await probeWith(fetchImpl);
    const [url, init] = fetchImpl.mock.calls[0] as [string, RequestInit];
    expect(url).toBe('http://iam:8080/v1/service-info');
    expect(new Headers(init.headers).get('authorization')).toBe('Bearer tok');
    // fetch follows redirects by default; following one would send the user's bearer token
    // wherever a compromised or misconfigured service points.
    expect(init.redirect).toBe('error');
  });

  it('defaults capabilities to an empty array when the field is absent', async () => {
    const out = await probeWith(() => Promise.resolve(res(JSON.stringify({ service: 'iam', version: '1.0.0' }))));
    expect(out).toEqual({ ok: true, descriptor: { service: 'iam', version: '1.0.0', capabilities: [] } });
  });

  it('drops unknown fields rather than failing', async () => {
    const body = JSON.stringify({ service: 'iam', version: '1.0.0', capabilities: [], futureField: 42 });
    const out = await probeWith(() => Promise.resolve(res(body)));
    expect(out).toEqual({ ok: true, descriptor: { service: 'iam', version: '1.0.0', capabilities: [] } });
  });

  it('accepts a content-type in ANY case, with parameters', async () => {
    // An externally supplied header must be compared case-insensitively. Every fixture here
    // varies the case deliberately: a lowercase-only fixture set is how a case-sensitive check
    // survives review.
    for (const ct of ['application/json', 'Application/JSON', 'APPLICATION/JSON; charset=utf-8']) {
      const out = await probeWith(() => Promise.resolve(res(OK_BODY, { contentType: ct })));
      expect(out, ct).toMatchObject({ ok: true });
    }
  });

  it.each([
    [401, 'unauthorized'],
    [403, 'unauthorized'],
    [404, 'not-implemented'],
    [500, 'server-error'],
    [502, 'server-error'],
    [418, 'server-error'],
  ])('maps status %i to %s', async (status, reason) => {
    expect(await probeWith(() => Promise.resolve(res('{}', { status })))).toEqual({ ok: false, reason });
  });

  it('rejects a 200 whose content-type is not JSON', async () => {
    const out = await probeWith(() => Promise.resolve(res('<html>', { contentType: 'text/html' })));
    expect(out).toEqual({ ok: false, reason: 'bad-response' });
  });

  it('rejects a 200 with no content-type at all', async () => {
    expect(await probeWith(() => Promise.resolve(res(OK_BODY, { contentType: null })))).toEqual({
      ok: false,
      reason: 'bad-response',
    });
  });

  it('rejects a 200 whose body is not a descriptor', async () => {
    expect(await probeWith(() => Promise.resolve(res(JSON.stringify({ nope: true }))))).toEqual({
      ok: false,
      reason: 'bad-response',
    });
  });

  it('rejects an oversized body', async () => {
    const huge = JSON.stringify({ service: 'iam', version: 'x'.repeat(200_000), capabilities: [] });
    const out = await probeService({
      baseUrl: 'http://iam:8080',
      token: 'tok',
      timeoutMs: 1_000,
      maxBytes: 1_024,
      fetch: () => Promise.resolve(res(huge)),
    });
    expect(out).toEqual({ ok: false, reason: 'bad-response' });
  });

  it('maps an abort to timeout', async () => {
    const out = await probeWith(() => {
      throw Object.assign(new Error('The operation was aborted'), { name: 'TimeoutError' });
    });
    expect(out).toEqual({ ok: false, reason: 'timeout' });
  });

  it('maps a fetch TypeError to network', async () => {
    const out = await probeWith(() => {
      throw new TypeError('fetch failed');
    });
    expect(out).toEqual({ ok: false, reason: 'network' });
  });

  it('never probes without a token', async () => {
    const fetchImpl = vi.fn<typeof globalThis.fetch>(() => Promise.resolve(res(OK_BODY)));
    expect(await probeWith(fetchImpl, '   ')).toEqual({ ok: false, reason: 'unauthorized' });
    expect(fetchImpl).not.toHaveBeenCalled();
  });
});
