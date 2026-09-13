// SPDX-License-Identifier: Apache-2.0
//
// A minimal HTTPS client that trusts the test certificate. Vitest workers start before any test
// could set NODE_EXTRA_CA_CERTS, so the global `fetch` cannot reach the fake IdP or the TLS
// terminator; `node:https` with an explicit `ca` can.
import { request } from 'node:https';
import type { TlsMaterial } from './tls';

export type HttpsResponse = { status: number; headers: Record<string, string | string[] | undefined>; body: string };

export function httpsRequest(url: string, tls: TlsMaterial, init: { method?: string; headers?: Record<string, string>; body?: string } = {}): Promise<HttpsResponse> {
  return new Promise((resolve, reject) => {
    const req = request(url, { method: init.method ?? 'GET', headers: init.headers ?? {}, ca: tls.cert }, (res) => {
      const chunks: Buffer[] = [];
      res.on('data', (chunk: Buffer) => chunks.push(chunk));
      res.on('end', () => resolve({ status: res.statusCode ?? 0, headers: res.headers, body: Buffer.concat(chunks).toString('utf8') }));
      res.on('error', reject);
    });
    req.on('error', reject);
    if (init.body !== undefined) req.write(init.body);
    req.end();
  });
}
