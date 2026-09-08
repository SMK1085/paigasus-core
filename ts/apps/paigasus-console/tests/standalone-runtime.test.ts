// SPDX-License-Identifier: Apache-2.0
//
// AC1's second half. Task 5's build asserts the standalone ARTIFACT exists; this asserts the
// SERVER reads deployment configuration at runtime. Two boots of the SAME binary with different
// PAIGASUS_ZONES must produce different responses. If Next had inlined the value, or the
// standalone trace had dropped @paigasus/next-config, both boots would agree — and every unit
// test in this repo would still pass.
import { spawn, type ChildProcess } from 'node:child_process';
import { createServer } from 'node:net';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

const SERVER_ENTRY = fileURLToPath(new URL('../.next/standalone/apps/paigasus-console/server.js', import.meta.url));

async function freePort(): Promise<number> {
  return await new Promise((resolve, reject) => {
    const srv = createServer();
    srv.once('error', reject);
    srv.listen(0, '127.0.0.1', () => {
      const address = srv.address();
      if (address === null || typeof address === 'string') {
        reject(new Error('could not acquire a port'));
        return;
      }
      const { port } = address;
      srv.close(() => resolve(port));
    });
  });
}

interface HomeResponse {
  status: number;
  body: string;
}

// Returns the status and body directly instead of rejecting on a non-2xx response. The brief's
// original helper only resolved on `res.ok`, so the "deployed prefix disagrees" case — which is
// SUPPOSED to return 500 — could only be observed by looping the full 60s until the deadline and
// rejecting. Returning the response as soon as the server answers, whatever the status, lets that
// case assert on `status` directly and return in well under a second; it is also a more direct
// assertion of the actual contract (the server answers with 500), not a proxy for it (fetch failed
// somehow).
async function fetchHomeWith(zones: Record<string, string>): Promise<HomeResponse> {
  const port = await freePort();
  const child: ChildProcess = spawn(process.execPath, [SERVER_ENTRY], {
    env: {
      ...process.env,
      PORT: String(port),
      HOSTNAME: '127.0.0.1',
      PAIGASUS_ZONE: 'iam',
      PAIGASUS_ZONES: JSON.stringify(zones),
    },
    stdio: 'pipe',
  });
  try {
    const deadline = Date.now() + 60_000;
    for (;;) {
      if (Date.now() > deadline) throw new Error('standalone server did not become ready within 60s');
      try {
        const res = await fetch(`http://127.0.0.1:${port}/iam`);
        return { status: res.status, body: await res.text() };
      } catch {
        // not listening yet
      }
      await new Promise((r) => setTimeout(r, 250));
    }
  } finally {
    child.kill('SIGTERM');
  }
}

describe('the standalone server reads configuration at runtime', () => {
  it('serves different zone maps from the SAME binary', async () => {
    const first = await fetchHomeWith({ iam: '/iam', gateway: '/gateway' });
    const second = await fetchHomeWith({ iam: '/iam', reports: '/reports' });

    expect(first.status).toBe(200);
    expect(second.status).toBe(200);
    expect(first.body).toContain('/gateway');
    expect(first.body).not.toContain('/reports');
    expect(second.body).toContain('/reports');
    expect(second.body).not.toContain('/gateway');
    expect(first.body).not.toBe(second.body);
  });

  it('fails loudly when the deployed prefix disagrees with the compiled one', async () => {
    const result = await fetchHomeWith({ iam: '/admin/iam' });
    expect(result.status).toBe(500);
  });
});
