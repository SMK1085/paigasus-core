// SPDX-License-Identifier: Apache-2.0
//
// A raw upgrade through the terminator. `node:http`'s own 'upgrade' event is the whole protocol
// here: no WebSocket library is involved, so the test asserts the bytes the tunnel must move.
import { createServer, request as httpRequest, type IncomingMessage } from 'node:http';
import { connect as tlsConnect } from 'node:tls';
import type { AddressInfo } from 'node:net';
import type { Duplex } from 'node:stream';
import { afterEach, describe, expect, it } from 'vitest';
import { startTlsTerminator, testTls } from '../../testing/index';

const tls = testTls();
const closers: (() => Promise<void>)[] = [];

afterEach(async () => {
  for (const close of closers.splice(0).reverse()) await close();
});

/**
 * An upstream that accepts an upgrade, answers 101, and echoes every byte it receives back with
 * an `echo:` prefix. `trailer` is written together with the 101, which is the upstream `head`
 * buffer the tunnel must replay.
 */
async function upgradeUpstream(opts: { trailer?: string } = {}): Promise<{ url: string; seen: string[] }> {
  const seen: string[] = [];
  const server = createServer((_req, res) => {
    res.writeHead(426);
    res.end('upgrade required');
  });
  server.on('upgrade', (req, socket: Duplex, head: Buffer) => {
    // MEASURED: an upgraded socket's `allowHalfOpen` is `true` on the server side, and
    // `server.closeAllConnections()` does not reach a socket once it has been handed over via
    // 'upgrade' — the server stops tracking it. So when the peer closes its end (the tunnel test
    // destroys the client socket, which cascades to this connection), this socket sees 'end' but
    // never auto-closes and never emits 'close' on its own. Without this handler, `afterEach`'s
    // `server.close()` call below hangs forever waiting for a connection the server can no longer
    // see. Destroying on 'end' is this fixture's own cleanup responsibility, not the terminator's.
    socket.on('end', () => socket.destroy());
    if (head.length > 0) seen.push(head.toString('utf8'));
    socket.write(`HTTP/1.1 101 Switching Protocols\r\nupgrade: websocket\r\nconnection: Upgrade\r\nsec-websocket-accept: test-accept\r\n\r\n${opts.trailer ?? ''}`);
    socket.on('data', (chunk: Buffer) => {
      seen.push(chunk.toString('utf8'));
      socket.write(`echo:${chunk.toString('utf8')}`);
    });
  });
  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve));
  const { port } = server.address() as AddressInfo;
  closers.push(
    () =>
      new Promise<void>((resolve) => {
        server.closeAllConnections();
        server.close(() => resolve());
      }),
  );
  return { url: `http://127.0.0.1:${String(port)}`, seen };
}

/**
 * Opens a TLS socket to the terminator and sends an upgrade request by hand.
 *
 * The tunnel writes the status line and the UPSTREAM `head` buffer as two separate
 * `clientSocket.write()` calls (`tls-terminator.ts`'s `tunnel()`). MEASURED: over a real TLS
 * connection those two writes can arrive as two separate 'data' events rather than being
 * coalesced into one — a plain, unencrypted socket in the same shape coalesces them reliably,
 * so this is specific to relaying through TLS. Resolving on the FIRST 'data' event would then
 * return only the header block and silently drop the trailing bytes, which is exactly the
 * defect two of the four tests below exist to catch — so this accumulates every chunk and
 * resolves once the socket has been quiet for a short settle window, not on the first byte.
 *
 * When no route matches, the terminator DESTROYS the socket, which can close cleanly without
 * emitting 'data' or 'error' — settling neither would hang the test until the vitest timeout. A
 * dedicated 'close' handler rejects with a named error so that case fails fast and legibly.
 */
function handshake(origin: string, path: string, body = ''): Promise<{ head: string; socket: Duplex }> {
  const url = new URL(origin);
  return new Promise((resolve, reject) => {
    const socket = tlsConnect({ host: url.hostname, port: Number(url.port), ca: tls.cert, servername: '127.0.0.1' }, () => {
      socket.write(`GET ${path} HTTP/1.1\r\nhost: ${url.host}\r\nconnection: Upgrade\r\nupgrade: websocket\r\nsec-websocket-key: dGhlIHNhbXBsZSBub25jZQ==\r\nsec-websocket-version: 13\r\n\r\n${body}`);
    });
    socket.setTimeout(5_000, () => reject(new Error('the handshake timed out')));
    socket.once('error', reject);
    socket.once('close', () => reject(new Error('the handshake socket closed with no response')));
    const chunks: Buffer[] = [];
    let settle: NodeJS.Timeout | undefined;
    const onData = (chunk: Buffer): void => {
      chunks.push(chunk);
      if (settle !== undefined) clearTimeout(settle);
      settle = setTimeout(() => {
        socket.off('data', onData);
        resolve({ head: Buffer.concat(chunks).toString('utf8'), socket });
      }, 50);
    };
    socket.on('data', onData);
  });
}

describe('the terminator tunnels a WebSocket upgrade', () => {
  it('answers 101 and moves bytes both ways', async () => {
    const back = await upgradeUpstream();
    const terminator = await startTlsTerminator({ tls, routes: [{ prefix: '/iam', target: back.url }] });
    closers.push(terminator.close);

    const { head, socket } = await handshake(terminator.origin, '/iam/_next/hmr?id=1');
    expect(head).toContain('101 Switching Protocols');
    expect(head.toLowerCase()).toContain('sec-websocket-accept: test-accept');

    const echoed = new Promise<string>((resolve) => socket.once('data', (chunk: Buffer) => resolve(chunk.toString('utf8'))));
    socket.write('ping');
    await expect(echoed).resolves.toEqual('echo:ping');
    expect(back.seen).toContain('ping');
    socket.destroy();
  });

  it('replays the CLIENT head buffer that arrived with the handshake', async () => {
    const back = await upgradeUpstream();
    const terminator = await startTlsTerminator({ tls, routes: [{ prefix: '/iam', target: back.url }] });
    closers.push(terminator.close);

    // 'early' rides in the same packet as the request line, so it reaches the terminator as the
    // server-side `head` buffer. Dropping it loses the first frame.
    const { socket } = await handshake(terminator.origin, '/iam/_next/hmr', 'early');
    await expect.poll(() => back.seen.join('')).toContain('early');
    socket.destroy();
  });

  it('replays the UPSTREAM head buffer written with the 101', async () => {
    const back = await upgradeUpstream({ trailer: 'first-frame' });
    const terminator = await startTlsTerminator({ tls, routes: [{ prefix: '/iam', target: back.url }] });
    closers.push(terminator.close);

    const { head, socket } = await handshake(terminator.origin, '/iam/_next/hmr');
    expect(head).toContain('first-frame');
    socket.destroy();
  });

  it('destroys the client socket when no route matches, and stays up', async () => {
    const back = await upgradeUpstream();
    const terminator = await startTlsTerminator({ tls, routes: [{ prefix: '/iam', target: back.url }] });
    closers.push(terminator.close);

    await expect(handshake(terminator.origin, '/nowhere/_next/hmr')).rejects.toThrow();

    // The server must still serve a normal request afterwards.
    const { head, socket } = await handshake(terminator.origin, '/iam/_next/hmr');
    expect(head).toContain('101');
    socket.destroy();
  });
});
