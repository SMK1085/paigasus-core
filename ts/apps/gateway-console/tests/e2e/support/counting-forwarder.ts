// SPDX-License-Identifier: Apache-2.0
//
// A counting forwarder for the gateway zone's isolation proof (spec § 10.5, ruling D22). Task 3's
// harness puts this between the terminator and the standalone `iam-console` server, so acceptance
// criterion 2 — "a cold login at the gateway zone never touches the IAM zone's app" — becomes a
// number a test can read, rather than an assumption.
//
// It counts connections to the `iam-console` APP. The gateway zone also legitimately talks to the
// IAM SERVICE over gRPC — that is a different upstream, for a different purpose, and acceptance
// criterion 2 concerns only the app connection count (spec § 13). Do not fold the two together.
//
// A forwarder, not a bind-and-drop spy: both standalone servers run in this tier, so the
// `iam-console` server already owns its port (spec § 10.5). It counts, then proxies onward with
// `http.request`, the way `@paigasus/console-core/testing`'s TLS terminator does — but plain HTTP
// in and plain HTTP out, with no TLS and no path routing, since this instrument sits inside the
// tier rather than at its edge.
//
// `connections()` counts inbound TCP connections; `requests()` counts HTTP requests. They differ
// under keep-alive: several requests can share one connection. Acceptance criterion 2 wants the
// connection count. Both are SNAPSHOTS — plain numbers, not a live object — so a caller stores one,
// does work, then subtracts. The stack is worker-scoped and Playwright restarts the worker after a
// failed test, so an absolute count is a sum over every earlier test in that worker.
import { createServer, request as httpRequest, type IncomingHttpHeaders, type IncomingMessage, type ServerResponse } from 'node:http';
import type { AddressInfo } from 'node:net';

/** Hop-by-hop headers (RFC 9110 § 7.6.1). A proxy must not forward them. */
const HOP_BY_HOP = new Set(['connection', 'keep-alive', 'proxy-connection', 'te', 'trailer', 'transfer-encoding', 'upgrade']);

function forwardable(headers: IncomingHttpHeaders): IncomingHttpHeaders {
  const out: IncomingHttpHeaders = {};
  for (const [name, value] of Object.entries(headers)) {
    if (!HOP_BY_HOP.has(name) && value !== undefined) out[name] = value;
  }
  return out;
}

export type CountingForwarder = {
  readonly url: string;
  connections(): number;
  requests(): number;
  close(): Promise<void>;
};

export function startCountingForwarder(opts: { target: string }): Promise<CountingForwarder> {
  const target = new URL(opts.target);
  let connectionCount = 0;
  let requestCount = 0;

  const server = createServer((req: IncomingMessage, res: ServerResponse) => {
    requestCount += 1;
    const upstream = httpRequest({ hostname: target.hostname, port: target.port, method: req.method, path: req.url, headers: forwardable(req.headers) }, (upstreamRes) => {
      res.writeHead(upstreamRes.statusCode ?? 502, forwardable(upstreamRes.headers));
      upstreamRes.pipe(res);
    });
    // Never a synchronous throw here: this handler must answer a status code, not propagate.
    upstream.on('error', () => {
      if (res.headersSent) return;
      res.writeHead(502, { 'content-type': 'text/plain' });
      res.end('counting-forwarder: the upstream did not answer');
    });
    res.on('close', () => upstream.destroy());
    req.pipe(upstream);
  });
  server.on('connection', () => {
    connectionCount += 1;
  });

  return new Promise((resolve) => {
    server.listen(0, '127.0.0.1', () => {
      const { port } = server.address() as AddressInfo;
      resolve({
        url: `http://127.0.0.1:${String(port)}`,
        connections: () => connectionCount,
        requests: () => requestCount,
        close: () =>
          new Promise<void>((res, reject) => {
            server.closeAllConnections();
            server.close((err) => (err ? reject(err) : res()));
          }),
      });
    });
  });
}
