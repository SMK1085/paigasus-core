// SPDX-License-Identifier: Apache-2.0
//
// The fake gateway (spec § 10.1). It serves `GET /v1/service-info` and NOTHING else — in
// particular no chat route, because decision D11 removed the only consumer and a fake endpoint
// with no product behind it would let a whole tier pass while the product could not make one call.
//
// Reachability is toggled by DESTROYING inbound sockets, not by closing the server: closing frees
// the port, and a later restore would bind a different one, silently invalidating the URL a test
// already handed to the app under test.
import { createServer, type IncomingMessage, type Server, type ServerResponse } from 'node:http';
import type { Socket } from 'node:net';

export const GATEWAY_CORRELATION_HEADER = 'paigasus-correlation-id';

export type FakeGatewayCall = { readonly method: string; readonly path: string; readonly token: string | null; readonly correlationId: string | null };
export type GatewayDescriptorBody = { service: string; version: string; capabilities: string[] };

export type FakeGateway = {
  /** `http://127.0.0.1:<port>` — the value that belongs in PAIGASUS_SERVICES under `gateway`. */
  readonly url: string;
  readonly calls: readonly FakeGatewayCall[];
  /** What `GET /v1/service-info` answers: a descriptor, or an HTTP status to fail with. */
  setServiceInfo(next: GatewayDescriptorBody | { status: number }): void;
  /** false: inbound connections are destroyed, so a probe fails at the transport. */
  setReachable(reachable: boolean): void;
  close(): Promise<void>;
};

const DEFAULT_DESCRIPTOR: GatewayDescriptorBody = { service: 'gateway', version: '0.0.0-fake', capabilities: ['gateway.chat.stream'] };

function bearerOf(value: string | null | undefined): string | null {
  if (value === null || value === undefined) return null;
  const match = /^Bearer (.+)$/.exec(value);
  return match?.[1] ?? null;
}

function listen(server: Server): Promise<string> {
  return new Promise((resolve, reject) => {
    server.listen(0, '127.0.0.1', () => {
      const address = server.address();
      if (address === null || typeof address === 'string') {
        reject(new Error('fake gateway: could not read the listening port'));
        return;
      }
      resolve(`http://127.0.0.1:${String(address.port)}`);
    });
  });
}

export async function startFakeGateway(opts: { descriptor?: GatewayDescriptorBody } = {}): Promise<FakeGateway> {
  let answer: GatewayDescriptorBody | { status: number } = opts.descriptor ?? DEFAULT_DESCRIPTOR;
  let reachable = true;
  const calls: FakeGatewayCall[] = [];
  const sockets = new Set<Socket>();

  function handle(req: IncomingMessage, res: ServerResponse): void {
    const url = new URL(req.url ?? '/', 'http://fake-gateway.invalid');
    const json = (status: number, body: unknown): void => {
      res.writeHead(status, { 'content-type': 'application/json' });
      res.end(JSON.stringify(body));
    };
    const token = bearerOf(req.headers.authorization);
    const header = req.headers[GATEWAY_CORRELATION_HEADER];
    calls.push({ method: req.method ?? 'GET', path: url.pathname, token, correlationId: typeof header === 'string' ? header : null });
    if (req.method !== 'GET' || url.pathname !== '/v1/service-info') {
      json(404, { error: { code: 'not-found', message: 'no such route' } });
      return;
    }
    if (token === null) {
      json(401, { error: { code: 'missing-authorization', message: 'a bearer token is required' } });
      return;
    }
    if ('status' in answer) {
      json(answer.status, { error: { code: 'internal', message: 'the fake gateway is configured to fail' } });
      return;
    }
    json(200, answer);
  }

  const server = createServer(handle);
  server.on('connection', (socket) => {
    if (!reachable) {
      socket.destroy();
      return;
    }
    sockets.add(socket);
    socket.once('close', () => sockets.delete(socket));
  });

  const url = await listen(server);

  return {
    url,
    calls,
    setServiceInfo(next) {
      answer = next;
    },
    setReachable(next) {
      reachable = next;
      if (!next) {
        for (const socket of sockets) socket.destroy();
      }
    },
    async close() {
      for (const socket of sockets) socket.destroy();
      await new Promise<void>((resolve) => server.close(() => resolve()));
    },
  };
}
