// SPDX-License-Identifier: Apache-2.0
//
// The ingress stand-in for the e2e tier (spec § 9.4). The browser speaks HTTPS to this server; it
// forwards each request over plain HTTP to a standalone Next server — one, given `target`, or one
// of several chosen by path prefix, given `routes` (SMA-512 PR4 task 1). The two forms are
// mutually exclusive (ruling D23): the existing `target` form and its three call sites are
// unchanged, and `routes` is what task 3's two-zone harness uses to front both zones with one
// terminator.
//
// It keeps `Host` UNCHANGED and adds `X-Forwarded-Proto: https` and `X-Forwarded-Host`. That is
// the deployment contract of spec § 10: Next's Server Action origin check compares `Origin` with
// the forwarded host, and the auth routes build absolute URLs from the public origin.
import { request as httpRequest, type IncomingHttpHeaders, type IncomingMessage, type ServerResponse } from 'node:http';
import { createServer } from 'node:https';
import type { AddressInfo } from 'node:net';
import type { TlsMaterial } from './tls';

/** Hop-by-hop headers (RFC 9110 § 7.6.1). A proxy must not forward them. */
const HOP_BY_HOP = new Set(['connection', 'keep-alive', 'proxy-connection', 'te', 'trailer', 'transfer-encoding', 'upgrade']);

function forwardable(headers: IncomingHttpHeaders): IncomingHttpHeaders {
  const out: IncomingHttpHeaders = {};
  for (const [name, value] of Object.entries(headers)) {
    if (!HOP_BY_HOP.has(name) && value !== undefined) out[name] = value;
  }
  return out;
}

/** One upstream, selected when its request path starts with `prefix`. */
export type TerminatorRoute = { readonly prefix: string; readonly target: string };

/** `/gateway` matches `/gateway`, `/gateway/` and `/gateway/anything`, but not `/gatewayx`. */
function matches(pathname: string, prefix: string): boolean {
  if (prefix === '/') return true;
  return pathname === prefix || pathname.startsWith(`${prefix}/`);
}

// A bare zone root carrying a query string — `/gateway?_rsc=1`, the shape Next's RSC prefetch
// produces — is neither `=== prefix` nor `startsWith(prefix + '/')` if matched against the raw
// `req.url`. Strip the query string (and any fragment) before matching; the base URL is a dummy,
// discarded once `.pathname` is read, and never sent anywhere.
function pathnameOf(url: string | undefined): string {
  return new URL(url ?? '/', 'http://tls-terminator.internal').pathname;
}

export async function startTlsTerminator(opts: { tls: TlsMaterial; target?: string; routes?: readonly TerminatorRoute[] }): Promise<{ origin: string; close(): Promise<void> }> {
  // Mutually exclusive, and an error rather than a precedence rule: a caller that passes both has a
  // wrong mental model of which upstream serves a path, and silently preferring one would hide it.
  if ((opts.target === undefined) === (opts.routes === undefined)) {
    throw new Error('tls-terminator: pass exactly one of `target` (one upstream) or `routes` (path-routed upstreams)');
  }
  if (opts.routes !== undefined && opts.routes.length === 0) {
    throw new Error('tls-terminator: `routes` must not be empty');
  }
  // LONGEST PREFIX FIRST, so a future '/iam/admin' route wins over '/iam' regardless of array order.
  const routes: readonly TerminatorRoute[] = opts.routes === undefined ? [{ prefix: '/', target: opts.target as string }] : [...opts.routes].sort((a, b) => b.prefix.length - a.prefix.length);
  const configuredPrefixes = routes.map((route) => route.prefix).join(', ');

  function forward(req: IncomingMessage, res: ServerResponse): void {
    const pathname = pathnameOf(req.url);
    const route = routes.find((candidate) => matches(pathname, candidate.prefix));
    if (route === undefined) {
      res.writeHead(502, { 'content-type': 'text/plain' });
      res.end(`tls-terminator: no route matches ${pathname} (configured prefixes: ${configuredPrefixes})`);
      return;
    }
    const target = new URL(route.target);
    const host = req.headers.host ?? '';
    const upstream = httpRequest(
      {
        hostname: target.hostname,
        port: target.port,
        method: req.method,
        // The ORIGINAL req.url, query string and all — only the ROUTING decision above uses the
        // stripped pathname; the upstream still needs `_rsc=1` and every other query parameter.
        path: req.url,
        headers: { ...forwardable(req.headers), host, 'x-forwarded-proto': 'https', 'x-forwarded-host': host, 'x-forwarded-port': host.split(':')[1] ?? '443' },
      },
      (upstreamRes) => {
        res.writeHead(upstreamRes.statusCode ?? 502, forwardable(upstreamRes.headers));
        upstreamRes.pipe(res);
      },
    );
    upstream.on('error', () => {
      if (res.headersSent) return;
      res.writeHead(502, { 'content-type': 'text/plain' });
      res.end('tls-terminator: the upstream did not answer');
    });
    // The downstream (browser-facing) response closed before the upstream finished, or after it did
    // — either way the upstream request has nothing left to do. Destroying it is a no-op once the
    // upstream has already completed normally.
    res.on('close', () => upstream.destroy());
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
