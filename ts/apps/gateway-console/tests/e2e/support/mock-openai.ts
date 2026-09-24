// SPDX-License-Identifier: Apache-2.0
//
// A mock OpenAI upstream that a test controls STEP BY STEP (SMA-635 spec § 7.2). The gateway posts
// `{base_url}/v1/chat/completions` here. Modes:
//   complete          — the whole body at once.
//   stepped           — `first`, then WAIT for release(), then `rest`. A buffering regression
//                       anywhere between here and the page hides `first` until release, and R22 fails.
//   endless           — `chunk` every `everyMs` until the connection closes; waitForClose() sees it.
//   break-mid-record  — `partial` (which ends inside a record), then the socket is destroyed, so
//                       reqwest sees a truncated chunked body.
import { createServer, type IncomingMessage, type ServerResponse } from 'node:http';
import type { Socket } from 'node:net';

export type MockMode =
  | { readonly kind: 'complete'; readonly body: string }
  | { readonly kind: 'stepped'; readonly first: string; readonly rest: string }
  | { readonly kind: 'endless'; readonly chunk: string; readonly everyMs: number }
  | { readonly kind: 'break-mid-record'; readonly partial: string };

export type MockRequest = { readonly authorization: string | null; readonly body: string };

export type MockOpenAi = {
  readonly url: string;
  readonly requests: readonly MockRequest[];
  setMode(mode: MockMode): void;
  release(): void;
  waitForClose(timeoutMs: number): Promise<boolean>;
  close(): Promise<void>;
};

export function delta(text: string): string {
  return `data: ${JSON.stringify({ id: 'chatcmpl-e2e', object: 'chat.completion.chunk', choices: [{ index: 0, delta: { content: text } }] })}\n\n`;
}

export const DONE = 'data: [DONE]\n\n';

export async function startMockOpenAi(): Promise<MockOpenAi> {
  let mode: MockMode = { kind: 'complete', body: `${delta('ok')}${DONE}` };
  const requests: MockRequest[] = [];
  const sockets = new Set<Socket>();
  let gate: (() => void) | null = null;
  let released = false;
  let closed = false;
  const closeWaiters: (() => void)[] = [];

  async function handle(req: IncomingMessage, res: ServerResponse): Promise<void> {
    const chunks: Buffer[] = [];
    for await (const chunk of req) chunks.push(chunk as Buffer);
    requests.push({ authorization: req.headers.authorization ?? null, body: Buffer.concat(chunks).toString('utf8') });
    const current = mode;
    closed = false;
    res.on('close', () => {
      closed = true;
      for (const waiter of closeWaiters.splice(0)) waiter();
    });
    res.writeHead(200, { 'content-type': 'text/event-stream' });
    switch (current.kind) {
      case 'complete':
        res.end(current.body);
        return;
      case 'stepped':
        res.write(current.first);
        await new Promise<void>((resolve) => {
          if (released) {
            released = false;
            resolve();
          } else {
            gate = resolve;
          }
        });
        res.end(current.rest);
        return;
      case 'endless': {
        const timer = setInterval(() => res.write(current.chunk), current.everyMs);
        res.on('close', () => clearInterval(timer));
        res.write(current.chunk);
        return;
      }
      case 'break-mid-record':
        res.write(current.partial);
        setTimeout(() => res.socket?.destroy(), 50);
        return;
    }
  }

  const server = createServer((req, res) => {
    handle(req, res).catch(() => res.destroy());
  });
  server.on('connection', (socket: Socket) => {
    sockets.add(socket);
    socket.once('close', () => sockets.delete(socket));
  });
  const url = await new Promise<string>((resolve, reject) => {
    server.listen(0, '127.0.0.1', () => {
      const address = server.address();
      if (address === null || typeof address === 'string') reject(new Error('mock OpenAI: no port'));
      else resolve(`http://127.0.0.1:${String(address.port)}`);
    });
  });

  return {
    url,
    requests,
    setMode(next) {
      mode = next;
      released = false;
      gate = null;
    },
    release() {
      if (gate === null) {
        released = true;
        return;
      }
      const open = gate;
      gate = null;
      open();
    },
    waitForClose(timeoutMs) {
      if (closed) return Promise.resolve(true);
      return new Promise<boolean>((resolve) => {
        const timer = setTimeout(() => resolve(false), timeoutMs);
        closeWaiters.push(() => {
          clearTimeout(timer);
          resolve(true);
        });
      });
    },
    async close() {
      for (const socket of sockets) socket.destroy();
      await new Promise<void>((resolve) => server.close(() => resolve()));
    },
  };
}
