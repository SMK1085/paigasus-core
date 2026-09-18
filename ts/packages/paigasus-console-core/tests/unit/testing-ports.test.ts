// SPDX-License-Identifier: Apache-2.0
import { createServer } from 'node:http';
import type { AddressInfo } from 'node:net';
import { afterAll, describe, expect, it } from 'vitest';
import { startFakeIdp, startTlsTerminator, testTls } from '../../testing/index';

const tls = testTls();

/** A plain HTTP server on a free port, so a terminator has something to route to. */
async function upstream(): Promise<{ url: string; close: () => Promise<void> }> {
  const server = createServer((_req, res) => {
    res.writeHead(200, { 'content-type': 'text/plain' });
    res.end('upstream');
  });
  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve));
  const { port } = server.address() as AddressInfo;
  return {
    url: `http://127.0.0.1:${String(port)}`,
    close: () => new Promise<void>((resolve, reject) => server.close((err) => (err ? reject(err) : resolve()))),
  };
}

/** A port that was free a moment ago. Binding it again is what the in-use cases need. */
async function takenPort(): Promise<{ port: number; close: () => Promise<void> }> {
  const server = createServer();
  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve));
  const { port } = server.address() as AddressInfo;
  return { port, close: () => new Promise<void>((resolve, reject) => server.close((err) => (err ? reject(err) : resolve()))) };
}

describe('the terminator and the fake IdP accept a fixed port', () => {
  const cleanup: (() => Promise<void>)[] = [];
  afterAll(async () => {
    for (const close of cleanup.reverse()) await close();
  });

  it('keeps a random port when none is given', async () => {
    const back = await upstream();
    cleanup.push(back.close);
    const first = await startTlsTerminator({ tls, target: back.url });
    cleanup.push(first.close);
    const second = await startTlsTerminator({ tls, target: back.url });
    cleanup.push(second.close);
    expect(first.origin).not.toEqual(second.origin);
  });

  it('binds the port it is given', async () => {
    const back = await upstream();
    cleanup.push(back.close);
    const free = await takenPort();
    await free.close(); // now very likely free again
    const terminator = await startTlsTerminator({ tls, target: back.url, port: free.port });
    cleanup.push(terminator.close);
    expect(terminator.origin).toEqual(`https://127.0.0.1:${String(free.port)}`);
  });

  it('REJECTS with the port in the message when the port is taken', async () => {
    const back = await upstream();
    cleanup.push(back.close);
    const held = await takenPort();
    cleanup.push(held.close);
    await expect(startTlsTerminator({ tls, target: back.url, port: held.port })).rejects.toThrow(String(held.port));
  });

  it('gives the fake IdP the port, and its issuer carries it', async () => {
    const free = await takenPort();
    await free.close();
    const idp = await startFakeIdp({ cert: tls, port: free.port });
    cleanup.push(idp.close);
    expect(idp.issuer).toEqual(`https://127.0.0.1:${String(free.port)}`);
  });

  it('REJECTS the fake IdP with the port in the message when the port is taken', async () => {
    const held = await takenPort();
    cleanup.push(held.close);
    await expect(startFakeIdp({ cert: tls, port: held.port })).rejects.toThrow(String(held.port));
  });
});
