// SPDX-License-Identifier: Apache-2.0
import { createServer, type Server } from 'node:http';
import type { AddressInfo } from 'node:net';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { httpsRequest } from '../../support/https-client';
import { startTlsTerminator } from '../../support/tls-terminator';
import { testTls, type TlsMaterial } from '../../support/tls';

describe('the TLS terminator', () => {
  let tls: TlsMaterial;
  let upstream: Server;
  let terminator: { origin: string; close(): Promise<void> };

  beforeAll(async () => {
    tls = testTls();
    upstream = createServer((req, res) => {
      const chunks: Buffer[] = [];
      req.on('data', (chunk: Buffer) => chunks.push(chunk));
      req.on('end', () => {
        res.setHeader('set-cookie', ['a=1; Path=/', 'b=2; Path=/']);
        res.writeHead(201, { 'content-type': 'application/json' });
        res.end(
          JSON.stringify({
            method: req.method,
            url: req.url,
            host: req.headers.host,
            proto: req.headers['x-forwarded-proto'],
            forwardedHost: req.headers['x-forwarded-host'],
            body: Buffer.concat(chunks).toString('utf8'),
          }),
        );
      });
    });
    await new Promise<void>((resolve) => upstream.listen(0, '127.0.0.1', resolve));
    const { port } = upstream.address() as AddressInfo;
    terminator = await startTlsTerminator({ target: `http://127.0.0.1:${String(port)}`, tls });
  });
  afterAll(async () => {
    await terminator.close();
    await new Promise<void>((resolve) => upstream.close(() => resolve()));
  });

  it('keeps Host, sets the forwarded headers, and forwards the method, path and body', async () => {
    const res = await httpsRequest(`${terminator.origin}/iam/orgs?x=1`, tls, { method: 'POST', headers: { 'content-type': 'text/plain' }, body: 'hello' });
    expect(res.status).toBe(201);
    const seen = JSON.parse(res.body) as Record<string, string>;
    const publicHost = new URL(terminator.origin).host;
    expect(seen).toEqual({ method: 'POST', url: '/iam/orgs?x=1', host: publicHost, proto: 'https', forwardedHost: publicHost, body: 'hello' });
  });

  it('passes every Set-Cookie header through', async () => {
    const res = await httpsRequest(`${terminator.origin}/`, tls);
    expect(res.headers['set-cookie']).toEqual(['a=1; Path=/', 'b=2; Path=/']);
  });
});
