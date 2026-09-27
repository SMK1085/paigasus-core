// SPDX-License-Identifier: Apache-2.0
//
// SMA-656 T9: a local HTTP server whose discovery document fails in one chosen way, so
// tests/adapters/oidc.test.ts can measure the `reason` that the REAL adapter gives. startOidcFixture
// (jwks.ts) has no hook for these cases, and a mode there would widen every caller of that fixture.
//
// OPEN HANDLES. The `hang` mode accepts a request and never answers. close() therefore destroys
// every socket the server holds BEFORE it calls server.close(): server.close() alone waits for the
// open connections, and it would wait for ever on a hung one. Every caller closes the fixture in an
// afterEach, so a failed assertion does not leave a server or a socket open.
import { createServer, type Server } from 'node:http';
import type { AddressInfo, Socket } from 'node:net';

export type DiscoveryFailureMode = 'status-404' | 'status-503' | 'not-json' | 'wrong-issuer' | 'issuer-not-a-url' | 'hang';

export interface DiscoveryFailureFixture {
  /** The issuer to give createOidcClient. Plain http on 127.0.0.1, so it needs allowInsecureRequests. */
  readonly issuer: string;
  /** How many HTTP requests the server received. A row asserts on it, so it cannot pass without reaching the server. */
  readonly requests: number;
  close(): Promise<void>;
}

export async function startDiscoveryFailureFixture(mode: DiscoveryFailureMode): Promise<DiscoveryFailureFixture> {
  const sockets = new Set<Socket>();
  let issuer = '';
  let requests = 0;

  const server: Server = createServer((_req, res) => {
    requests += 1;
    const metadata = (issuerValue: string): void => {
      res.writeHead(200, { 'content-type': 'application/json' });
      res.end(JSON.stringify({ issuer: issuerValue, authorization_endpoint: `${issuer}/authorize`, token_endpoint: `${issuer}/token`, jwks_uri: `${issuer}/jwks` }));
    };
    switch (mode) {
      case 'status-404':
        // A wrong realm: the path does not exist.
        res.writeHead(404, { 'content-type': 'text/plain' });
        res.end('not found');
        return;
      case 'status-503':
        // An ingress in front of an IdP that is down.
        res.writeHead(503, { 'content-type': 'text/plain' });
        res.end('service unavailable');
        return;
      case 'not-json':
        // An HTML page with a 200 in place of the document.
        res.writeHead(200, { 'content-type': 'text/html' });
        res.end('<!doctype html><title>sign in</title>');
        return;
      case 'wrong-issuer':
        // A valid URL that is not the configured issuer.
        metadata(`${issuer}/other-realm`);
        return;
      case 'issuer-not-a-url':
        metadata('not a url');
        return;
      case 'hang':
        // Never answer. close() destroys the socket.
        return;
    }
  });
  server.on('connection', (socket) => {
    sockets.add(socket);
    socket.on('close', () => sockets.delete(socket));
  });

  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve));
  const { port } = server.address() as AddressInfo;
  issuer = `http://127.0.0.1:${String(port)}`;

  return {
    issuer,
    get requests(): number {
      return requests;
    },
    close(): Promise<void> {
      for (const socket of sockets) socket.destroy();
      return new Promise<void>((resolve, reject) => {
        server.close((err) => (err ? reject(err) : resolve()));
      });
    },
  };
}

/**
 * An issuer on a port that nothing listens on: the port was bound, then released, so a connect gets
 * ECONNREFUSED. Not `127.0.0.1:1`: port 1 is on the Fetch "bad port" list, so undici refuses it
 * BEFORE any connect, with a cause that has no `code` (Review Focus 5).
 */
export async function closedPortIssuer(): Promise<string> {
  const server = createServer();
  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve));
  const { port } = server.address() as AddressInfo;
  await new Promise<void>((resolve, reject) => {
    server.close((err) => (err ? reject(err) : resolve()));
  });
  return `http://127.0.0.1:${String(port)}`;
}
