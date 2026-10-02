// SPDX-License-Identifier: Apache-2.0
//
// SMA-640 spec § 6.2: the terminator's forward() removes the fields that a `Connection` header
// nominates (RFC 9110 § 7.6.1) and the two proxy-auth fields, in both directions. The upgrade path
// is in terminator-upgrade.test.ts.
//
// This file has its OWN echo upstream. The iam-console terminator test's upstream echoes a fixed
// field set and checks it with toEqual, so it cannot carry these cases (spec § 6.2).
import { createServer, type IncomingHttpHeaders, type Server } from 'node:http';
import { request as httpsRequest } from 'node:https';
import type { AddressInfo } from 'node:net';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { startTlsTerminator, testTls } from '../../testing/index';

const tls = testTls();
let upstream: Server;
let terminator: { origin: string; close(): Promise<void> };

type Reply = { status: number; headers: IncomingHttpHeaders; body: string };

function send(path: string, headers: Record<string, string>): Promise<Reply> {
  return new Promise((resolve, reject) => {
    const req = httpsRequest(new URL(path, terminator.origin), { headers, ca: tls.cert }, (res) => {
      const chunks: Buffer[] = [];
      res.on('data', (chunk: Buffer) => chunks.push(chunk));
      res.on('end', () => resolve({ status: res.statusCode ?? 0, headers: res.headers, body: Buffer.concat(chunks).toString('utf8') }));
      res.on('error', reject);
    });
    req.on('error', reject);
    req.end();
  });
}

beforeAll(async () => {
  upstream = createServer((req, res) => {
    if (req.url === '/nominating-response') {
      // A node:http server sends a caller-set Connection value verbatim (measured, spec § 6.6).
      res.setHeader('set-cookie', ['a=1; Path=/', 'b=2; Path=/']);
      res.writeHead(200, { connection: 'X-Resp', 'x-resp': '1', 'x-resp-kept': '1', 'proxy-authenticate': 'Basic realm="x"', 'content-type': 'text/plain' });
      res.end('ok');
      return;
    }
    res.writeHead(200, { 'content-type': 'application/json' });
    res.end(JSON.stringify(req.headers));
  });
  await new Promise<void>((resolve) => upstream.listen(0, '127.0.0.1', resolve));
  const { port } = upstream.address() as AddressInfo;
  terminator = await startTlsTerminator({ tls, target: `http://127.0.0.1:${String(port)}` });
});

afterAll(async () => {
  await terminator.close();
  await new Promise<void>((resolve) => {
    upstream.closeAllConnections();
    upstream.close(() => resolve());
  });
});

describe('the terminator removes Connection-nominated fields', () => {
  it('in the request direction, and removes proxy-authorization', async () => {
    const reply = await send('/echo', { connection: 'x-internal', 'x-internal': '1', 'x-kept': '1', 'proxy-authorization': 'Basic eA==' });
    const seen = JSON.parse(reply.body) as Record<string, string>;
    expect(seen['x-kept']).toBe('1');
    expect(seen).not.toHaveProperty('x-internal');
    expect(seen).not.toHaveProperty('proxy-authorization');
  });

  it('in the response direction, removes proxy-authenticate, and keeps every set-cookie value', async () => {
    const reply = await send('/nominating-response', {});
    expect(reply.status).toBe(200);
    expect(reply.headers['x-resp-kept']).toBe('1');
    expect(reply.headers).not.toHaveProperty('x-resp');
    expect(reply.headers).not.toHaveProperty('proxy-authenticate');
    expect(reply.headers['set-cookie']).toEqual(['a=1; Path=/', 'b=2; Path=/']);
  });

  // Review Focus 4, decision D3. forward() sets host and x-forwarded-* AFTER the filtered spread,
  // so a nomination of them has no effect here. A refactor that filters the merged object would
  // drop them and reds this row.
  it("keeps the terminator's own host and x-forwarded-host when the client nominates them", async () => {
    const reply = await send('/echo', { connection: 'Host, X-Forwarded-Host' });
    const seen = JSON.parse(reply.body) as Record<string, string>;
    const publicHost = new URL(terminator.origin).host;
    expect(seen.host).toBe(publicHost);
    expect(seen['x-forwarded-host']).toBe(publicHost);
    expect(seen['x-forwarded-proto']).toBe('https');
  });
});
