// SPDX-License-Identifier: Apache-2.0
//
// AC 3's runtime half (spec § 9.5). The build asserts the standalone ARTIFACT exists; this asserts
// the SERVER reads deployment configuration at runtime. Two boots of the SAME binary with different
// PAIGASUS_ZONES must answer different zone maps on `GET /iam/healthz`, which is public and runs the
// FULL configuration parse. If Next had inlined a value, or the standalone trace had dropped a
// package, both boots would agree — and every unit test in this repo would still pass.
//
// Each boot gets a COMPLETE, valid environment, so a failure can only come from what a case
// changes. The mismatch case asserts the mismatch MESSAGE in the server output, not only a 500: a
// missing unrelated variable also answers 500, and must not pass it.
import { spawn, type ChildProcess } from 'node:child_process';
import { createServer } from 'node:net';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

const SERVER_ENTRY = fileURLToPath(new URL('../.next/standalone/apps/iam-console/server.js', import.meta.url));

const COMPLETE_ENV: Readonly<Record<string, string>> = {
  PAIGASUS_ZONE: 'iam',
  PAIGASUS_OIDC_ISSUER: 'https://idp.example.test',
  PAIGASUS_OIDC_CLIENT_ID: 'console',
  PAIGASUS_OIDC_CLIENT_SECRET: 'console-secret',
  PAIGASUS_PUBLIC_ORIGIN: 'https://console.example.test',
  PAIGASUS_SESSION_STORE: 'memory',
  PAIGASUS_SERVICES: '{"iam":"http://127.0.0.1:9"}',
  PAIGASUS_IAM_GRPC_URL: 'http://127.0.0.1:9',
};

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

type Answer = { status: number; body: string; output: string };

// The child is OBSERVED, not merely polled (the reasons are recorded in this file's history): an
// early death is reported as a death, both streams are drained so a chatty child cannot block, and
// a deadline still covers a process that starts and never listens. `waitForOutput` keeps the server
// alive until its log contains that text, because Next writes a route error to stderr around the
// time it sends the 500.
async function healthzWith(zones: Record<string, string>, waitForOutput?: string): Promise<Answer> {
  const port = await freePort();
  const child: ChildProcess = spawn(process.execPath, [SERVER_ENTRY], {
    env: { ...process.env, ...COMPLETE_ENV, PORT: String(port), HOSTNAME: '127.0.0.1', PAIGASUS_ZONES: JSON.stringify(zones) },
    stdio: 'pipe',
  });

  let output = '';
  const capture = (chunk: Buffer | string): void => {
    output += String(chunk);
  };
  child.stdout?.on('data', capture);
  child.stderr?.on('data', capture);

  let died: string | undefined;
  child.once('error', (err: Error) => {
    died = `the server process failed to start: ${err.message}`;
  });
  child.once('exit', (code, signal) => {
    died = `the server process exited before answering (code ${String(code)}, signal ${String(signal)})`;
  });

  const withOutput = (message: string): Error => new Error(`${message}\n--- server output ---\n${output.trim() === '' ? '(none captured)' : output}`);

  try {
    const deadline = Date.now() + 60_000;
    for (;;) {
      if (died !== undefined) throw withOutput(died);
      if (Date.now() > deadline) throw withOutput('standalone server did not become ready within 60s');
      let res: Response | undefined;
      try {
        res = await fetch(`http://127.0.0.1:${String(port)}/iam/healthz`);
      } catch {
        // not listening yet
      }
      if (res !== undefined) {
        const answer = { status: res.status, body: await res.text() };
        const logDeadline = Date.now() + 5_000;
        while (waitForOutput !== undefined && !output.includes(waitForOutput) && Date.now() < logDeadline) {
          await new Promise((r) => setTimeout(r, 100));
        }
        return { ...answer, output };
      }
      await new Promise((r) => setTimeout(r, 250));
    }
  } finally {
    if (child.exitCode === null && child.signalCode === null) child.kill('SIGTERM');
  }
}

describe('the standalone server reads configuration at runtime', () => {
  it('serves different zone maps from the SAME binary', async () => {
    const first = await healthzWith({ iam: '/iam', gateway: '/gateway' });
    const second = await healthzWith({ iam: '/iam', reports: '/reports' });

    expect(first.status).toBe(200);
    expect(second.status).toBe(200);
    expect(JSON.parse(first.body)).toEqual({ zone: 'iam', zones: { iam: '/iam', gateway: '/gateway' } });
    expect(JSON.parse(second.body)).toEqual({ zone: 'iam', zones: { iam: '/iam', reports: '/reports' } });
  });

  it('fails loudly, with the mismatch message, when the deployed prefix disagrees with the compiled one', async () => {
    const message = 'Base path mismatch for zone "iam"';
    const result = await healthzWith({ iam: '/admin/iam' }, message);
    expect(result.status).toBe(500);
    expect(result.output).toContain(message);
  });
});
