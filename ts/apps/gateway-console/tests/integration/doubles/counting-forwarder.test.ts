// SPDX-License-Identifier: Apache-2.0
//
// The counting forwarder's own test (SMA-512 PR4 task 2). It proxies to a trivial upstream and
// asserts the two counters — connections() (TCP) and requests() (HTTP) — diverge under keep-alive,
// that both read as snapshots a caller can subtract, that headers/method/body reach the upstream
// unchanged, and that a streamed response is piped through rather than buffered whole.
import { Agent, createServer, request as httpRequest, type IncomingMessage, type Server, type ServerResponse } from 'node:http';
import type { AddressInfo } from 'node:net';
import { afterEach, describe, expect, it } from 'vitest';
import { startCountingForwarder, type CountingForwarder } from '../../e2e/support/counting-forwarder';

type Upstream = { server: Server; url: string };
type Response = { status: number; headers: IncomingMessage['headers']; body: string };

function startEcho(handler?: (req: IncomingMessage, res: ServerResponse) => void): Promise<Upstream> {
  return new Promise((resolve) => {
    const server = createServer(
      handler ??
        ((req, res) => {
          const chunks: Buffer[] = [];
          req.on('data', (chunk: Buffer) => chunks.push(chunk));
          req.on('end', () => {
            res.writeHead(200, { 'content-type': 'application/json' });
            res.end(JSON.stringify({ method: req.method, url: req.url, headers: req.headers, body: Buffer.concat(chunks).toString('utf8') }));
          });
        }),
    );
    server.listen(0, '127.0.0.1', () => {
      const { port } = server.address() as AddressInfo;
      resolve({ server, url: `http://127.0.0.1:${String(port)}` });
    });
  });
}

function closeUpstream(upstream: Upstream): Promise<void> {
  return new Promise((resolve) => upstream.server.close(() => resolve()));
}

function get(url: string, opts: { method?: string; headers?: Record<string, string>; body?: string; agent?: Agent } = {}): Promise<Response> {
  return new Promise((resolve, reject) => {
    const req = httpRequest(url, { method: opts.method ?? 'GET', headers: opts.headers, agent: opts.agent }, (res) => {
      const chunks: Buffer[] = [];
      res.on('data', (chunk: Buffer) => chunks.push(chunk));
      res.on('end', () => resolve({ status: res.statusCode ?? 0, headers: res.headers, body: Buffer.concat(chunks).toString('utf8') }));
      res.on('error', reject);
    });
    req.on('error', reject);
    req.end(opts.body);
  });
}

describe('the counting forwarder', () => {
  let upstream: Upstream | undefined;
  let forwarder: CountingForwarder | undefined;

  afterEach(async () => {
    if (forwarder !== undefined) await forwarder.close();
    if (upstream !== undefined) await closeUpstream(upstream);
    forwarder = undefined;
    upstream = undefined;
  });

  it('forwards a request to the upstream and the response body arrives intact', async () => {
    upstream = await startEcho();
    forwarder = await startCountingForwarder({ target: upstream.url });

    const res = await get(`${forwarder.url}/x`);
    expect(res.status).toBe(200);
    expect(JSON.parse(res.body)).toMatchObject({ method: 'GET', url: '/x' });
  });

  it('forwards the method, headers and body unchanged', async () => {
    upstream = await startEcho();
    forwarder = await startCountingForwarder({ target: upstream.url });

    const res = await get(`${forwarder.url}/orgs`, { method: 'PUT', headers: { 'x-test-header': 'value123' }, body: 'the-request-body' });
    const body = JSON.parse(res.body) as { method: string; headers: Record<string, string>; body: string };
    expect(body.method).toBe('PUT');
    expect(body.headers['x-test-header']).toBe('value123');
    expect(body.body).toBe('the-request-body');
  });

  it('counts connections() and requests() as snapshots a caller can subtract as a delta', async () => {
    upstream = await startEcho();
    forwarder = await startCountingForwarder({ target: upstream.url });

    const connectionsBefore = forwarder.connections();
    const requestsBefore = forwarder.requests();

    await get(`${forwarder.url}/a`);
    await get(`${forwarder.url}/b`);

    // Reading again does not mutate anything: the snapshot from before the two requests is
    // untouched, so a caller may hold on to it and subtract at any later point.
    expect(forwarder.connections()).toBeGreaterThanOrEqual(connectionsBefore);
    const connectionsDelta = forwarder.connections() - connectionsBefore;
    const requestsDelta = forwarder.requests() - requestsBefore;
    expect(requestsDelta).toBe(2);
    expect(connectionsDelta).toBeGreaterThanOrEqual(1);
  });

  it('counts one connection but two requests under keep-alive, because they differ', async () => {
    upstream = await startEcho();
    forwarder = await startCountingForwarder({ target: upstream.url });
    const agent = new Agent({ keepAlive: true, maxSockets: 1 });

    try {
      const connectionsBefore = forwarder.connections();
      const requestsBefore = forwarder.requests();

      await get(`${forwarder.url}/a`, { agent });
      await get(`${forwarder.url}/b`, { agent });

      expect(forwarder.requests() - requestsBefore).toBe(2);
      expect(forwarder.connections() - connectionsBefore).toBe(1);
    } finally {
      agent.destroy();
    }
  });

  it('pipes a streamed response through without buffering the whole body first', async () => {
    let releaseSecondChunk: (() => void) | undefined;
    const secondChunkGate = new Promise<void>((resolve) => {
      releaseSecondChunk = resolve;
    });

    upstream = await startEcho((_req, res) => {
      res.writeHead(200, { 'content-type': 'text/plain' });
      res.write('first-chunk');
      // Only send the second chunk once the client has already observed the first one — a
      // forwarder that buffered the whole response before answering could never satisfy this.
      void secondChunkGate.then(() => {
        res.write('second-chunk');
        res.end();
      });
    });
    forwarder = await startCountingForwarder({ target: upstream.url });

    const received: string[] = [];
    await new Promise<void>((resolve, reject) => {
      const req = httpRequest(`${forwarder?.url}/stream`, (res) => {
        res.on('data', (chunk: Buffer) => {
          received.push(chunk.toString('utf8'));
          if (received.length === 1) releaseSecondChunk?.();
        });
        res.on('end', resolve);
        res.on('error', reject);
      });
      req.on('error', reject);
      req.end();
    });

    expect(received).toEqual(['first-chunk', 'second-chunk']);
  });
});
