// SPDX-License-Identifier: Apache-2.0
//
// The ingress stand-in for the e2e tier (spec § 9.4). The browser speaks HTTPS to this server; it
// forwards each request over plain HTTP to the standalone Next server.
//
// It keeps `Host` UNCHANGED and adds `X-Forwarded-Proto: https` and `X-Forwarded-Host`. That is
// the deployment contract of spec § 10: Next's Server Action origin check compares `Origin` with
// the forwarded host, and the auth routes build absolute URLs from the public origin.
import { request as httpRequest, type IncomingHttpHeaders, type IncomingMessage, type ServerResponse } from 'node:http';
import { createServer } from 'node:https';
import type { AddressInfo } from 'node:net';
import type { TlsMaterial } from './tls';

/** Hop-by-hop headers (RFC 9110 § 7.6.1). A proxy must not forward them. */
const HOP_BY_HOP = new Set(['connection', 'keep-alive', 'proxy-connection', 'te', 'trailer', 'upgrade']);

function forwardable(headers: IncomingHttpHeaders): IncomingHttpHeaders {
  const out: IncomingHttpHeaders = {};
  for (const [name, value] of Object.entries(headers)) {
    if (!HOP_BY_HOP.has(name) && value !== undefined) out[name] = value;
  }
  return out;
}

export async function startTlsTerminator(opts: { target: string; tls: TlsMaterial }): Promise<{ origin: string; close(): Promise<void> }> {
  const target = new URL(opts.target);

  function forward(req: IncomingMessage, res: ServerResponse): void {
    const host = req.headers.host ?? '';
    const upstream = httpRequest(
      {
        hostname: target.hostname,
        port: target.port,
        method: req.method,
        path: req.url,
        headers: { ...forwardable(req.headers), host, 'x-forwarded-proto': 'https', 'x-forwarded-host': host, 'x-forwarded-port': host.split(':')[1] ?? '443' },
      },
      (upstreamRes) => {
        res.writeHead(upstreamRes.statusCode ?? 502, forwardable(upstreamRes.headers));
        upstreamRes.pipe(res);
      },
    );
    upstream.on('error', () => {
      if (!res.headersSent) res.writeHead(502, { 'content-type': 'text/plain' });
      res.end('tls-terminator: the upstream did not answer');
    });
    req.pipe(upstream);
  }

  const server = createServer({ cert: opts.tls.cert, key: opts.tls.key }, forward);
  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve));
  const { port } = server.address() as AddressInfo;
  return {
    origin: `https://127.0.0.1:${String(port)}`,
    close: () =>
      new Promise<void>((resolve, reject) => {
        server.closeAllConnections();
        server.close((err) => (err ? reject(err) : resolve()));
      }),
  };
}
