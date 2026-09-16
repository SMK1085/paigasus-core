// SPDX-License-Identifier: Apache-2.0
//
// The path router's own test (SMA-512 PR4 task 1). Two trivial node:http servers stand in for the
// IAM zone and the gateway zone; one terminator, given `routes` instead of `target`, fronts both.
// iam-console's tests/integration/doubles/tls-terminator.test.ts covers the existing single-`target`
// form (ruling D23: that form and its three call sites stay unchanged); this file covers the new
// `routes` form that task 3's two-zone harness will consume.
//
// The nested-path case is the one that matters for the tier: static chunks live at
// /gateway/_next/static/…, and a router that matched only the exact prefix would send every asset
// to the fallback instead of the gateway zone.
import { createServer, type Server } from 'node:http';
import { request as httpsRequest } from 'node:https';
import type { AddressInfo } from 'node:net';
import { connect as tlsConnect } from 'node:tls';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { startTlsTerminator, testTls, type TlsMaterial } from '@paigasus/console-core/testing';

type Upstream = { server: Server; url: string };
type Response = { status: number; body: string };
/** The shape a startEcho() upstream's JSON body parses into. `JSON.parse` returns `any`, so a case
 * that reads a field off the parsed body (rather than handing the whole value to `toMatchObject`)
 * needs this to avoid an unsafe member access. */
type Echo = { server: string; url: string };

/** A minimal HTTPS client that trusts the test certificate, mirroring iam-console's own. */
function get(url: string, tls: TlsMaterial): Promise<Response> {
  return new Promise((resolve, reject) => {
    const req = httpsRequest(url, { ca: tls.cert }, (res) => {
      const chunks: Buffer[] = [];
      res.on('data', (chunk: Buffer) => chunks.push(chunk));
      res.on('end', () => resolve({ status: res.statusCode ?? 0, body: Buffer.concat(chunks).toString('utf8') }));
      res.on('error', reject);
    });
    req.on('error', reject);
    req.end();
  });
}

/** A trivial upstream that echoes which server answered, the requested path, and two forwarded headers. */
function startEcho(name: string): Promise<Upstream> {
  return new Promise((resolve) => {
    const server = createServer((req, res) => {
      res.writeHead(200, { 'content-type': 'application/json' });
      res.end(JSON.stringify({ server: name, url: req.url, host: req.headers.host, proto: req.headers['x-forwarded-proto'] }));
    });
    server.listen(0, '127.0.0.1', () => {
      const { port } = server.address() as AddressInfo;
      resolve({ server, url: `http://127.0.0.1:${String(port)}` });
    });
  });
}

function closeUpstream(upstream: Upstream): Promise<void> {
  return new Promise((resolve) => upstream.server.close(() => resolve()));
}

/**
 * Writes a request line over a raw TLS socket, bypassing `node:https`'s own client, which would
 * refuse to send a malformed request line before it ever reaches the terminator. This is what lets
 * a test drive a request line Node's HTTP parser accepts but `new URL` rejects.
 */
function sendRawRequest(origin: string, tls: TlsMaterial, requestLine: string): Promise<Response> {
  return new Promise((resolve, reject) => {
    const url = new URL(origin);
    const socket = tlsConnect({ host: url.hostname, port: Number(url.port), ca: tls.cert }, () => {
      socket.write(`${requestLine}\r\nHost: ${url.host}\r\nConnection: close\r\n\r\n`);
    });
    const chunks: Buffer[] = [];
    socket.on('data', (chunk: Buffer) => chunks.push(chunk));
    socket.on('end', () => {
      const raw = Buffer.concat(chunks).toString('utf8');
      const [head, ...bodyParts] = raw.split('\r\n\r\n');
      const statusLine = head?.split('\r\n')[0] ?? '';
      resolve({ status: Number(statusLine.split(' ')[1] ?? 0), body: bodyParts.join('\r\n\r\n') });
    });
    socket.on('error', reject);
  });
}

describe('the TLS terminator, path-routed between two upstreams', () => {
  let tls: TlsMaterial;
  let iam: Upstream;
  let gateway: Upstream;
  let terminator: { origin: string; close(): Promise<void> };

  beforeAll(async () => {
    tls = testTls();
    iam = await startEcho('iam');
    gateway = await startEcho('gateway');
    terminator = await startTlsTerminator({
      tls,
      routes: [
        { prefix: '/iam', target: iam.url },
        { prefix: '/gateway', target: gateway.url },
      ],
    });
  });

  afterAll(async () => {
    await terminator.close();
    await closeUpstream(iam);
    await closeUpstream(gateway);
  });

  it('routes by path prefix, longest prefix first', async () => {
    const iamRes = await get(`${terminator.origin}/iam/x`, tls);
    expect(JSON.parse(iamRes.body)).toMatchObject({ server: 'iam', url: '/iam/x' });

    const gatewayRes = await get(`${terminator.origin}/gateway/x`, tls);
    expect(JSON.parse(gatewayRes.body)).toMatchObject({ server: 'gateway', url: '/gateway/x' });
  });

  it('routes a nested path under a prefix', async () => {
    const res = await get(`${terminator.origin}/gateway/_next/static/x.js`, tls);
    expect(JSON.parse(res.body)).toMatchObject({ server: 'gateway', url: '/gateway/_next/static/x.js' });
  });

  it('routes the longest matching prefix first, even when the array lists the shorter one first', async () => {
    const iamAdmin = await startEcho('iam-admin');
    // Passed in the order that gives the WRONG answer without the longest-prefix-first sort: the
    // short, overlapping prefix ('/iam') listed BEFORE the longer one ('/iam/admin'). If `.find()`
    // ever ran against this array unsorted, '/iam/admin/x' would match '/iam' first and reach the
    // wrong upstream.
    const overlapping = await startTlsTerminator({
      tls,
      routes: [
        { prefix: '/iam', target: iam.url },
        { prefix: '/iam/admin', target: iamAdmin.url },
      ],
    });
    try {
      const adminRes = await get(`${overlapping.origin}/iam/admin/x`, tls);
      expect(JSON.parse(adminRes.body)).toMatchObject({ server: 'iam-admin', url: '/iam/admin/x' });

      const iamRes = await get(`${overlapping.origin}/iam/x`, tls);
      expect(JSON.parse(iamRes.body)).toMatchObject({ server: 'iam', url: '/iam/x' });
    } finally {
      await overlapping.close();
      await closeUpstream(iamAdmin);
    }
  });

  it('routes a bare zone root carrying a query string (an RSC prefetch shape)', async () => {
    const res = await get(`${terminator.origin}/gateway?_rsc=1`, tls);
    const body = JSON.parse(res.body) as Echo;
    expect(body.server).toBe('gateway');
  });

  it('forwards the query string to the upstream unchanged', async () => {
    const res = await get(`${terminator.origin}/gateway/orgs?x=1&y=2`, tls);
    const body = JSON.parse(res.body) as Echo;
    expect(body.url).toBe('/gateway/orgs?x=1&y=2');
  });

  it('keeps Host unchanged and sets X-Forwarded-Proto on a routed request', async () => {
    const res = await get(`${terminator.origin}/iam/x`, tls);
    const body = JSON.parse(res.body) as { host: string; proto: string };
    const publicHost = new URL(terminator.origin).host;
    expect(body.host).toBe(publicHost);
    expect(body.proto).toBe('https');
  });

  it('answers 502 for a path no route matches', async () => {
    const res = await get(`${terminator.origin}/`, tls);
    expect(res.status).toBe(502);
    expect(res.body).toContain('/');
    expect(res.body).toContain('/iam');
    expect(res.body).toContain('/gateway');
  });

  it('answers 502, not a crash, for a request line `new URL` cannot parse, and stays alive for the next request', async () => {
    // Node's HTTP parser accepts this absolute-form request line and hands it to the terminator as
    // req.url; `new URL(req.url, base)` then throws on the unbalanced IPv6 literal.
    const res = await sendRawRequest(terminator.origin, tls, 'GET http://[::1/x HTTP/1.1');
    expect(res.status).toBe(502);
    expect(res.body).toContain('[::1/x');

    // The regression this pins: an uncaught throw in the request handler kills the whole process,
    // so a bare 502 check on the malformed request would still pass against a terminator that then
    // died. A normal request through the SAME instance afterwards is what actually proves it's alive.
    const followUp = await get(`${terminator.origin}/iam/x`, tls);
    expect(JSON.parse(followUp.body)).toMatchObject({ server: 'iam', url: '/iam/x' });
  });
});

describe('the TLS terminator, target/routes validation', () => {
  it('rejects both `target` and `routes` in one call', async () => {
    const tls = testTls();
    await expect(startTlsTerminator({ tls, target: 'http://127.0.0.1:1', routes: [] })).rejects.toThrow(/target|routes/);
  });

  it('rejects an empty routes array', async () => {
    const tls = testTls();
    await expect(startTlsTerminator({ tls, routes: [] })).rejects.toThrow(/routes/);
  });
});
