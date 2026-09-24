// SPDX-License-Identifier: Apache-2.0
//
// The REAL paigasus-gateway child of the playground project (SMA-635 spec § 7.2, D9). It is built
// from the commit under test by `paigasus-gateway-rs:e2e-bin` (config.rs:244-248), configured ONLY
// through GATEWAY_* variables, started from a directory that holds no gateway.toml, and started
// with every inherited GATEWAY_* variable and RUST_LOG removed (gateway-env.ts). It logs JSON lines
// on stdout (paigasus-logging), which row R22 reads.
//
// Read-only on disk: `existsSync` is the only fs call (tests/unit/e2e-read-only.test.ts).
import { spawn, type ChildProcess } from 'node:child_process';
import { once } from 'node:events';
import { existsSync } from 'node:fs';
import path from 'node:path';
import { assertDefaultCargoTargetDir, gatewayEnv } from './gateway-env';
import { freePort, stop, waitForHealth } from './harness';
import { REPO_ROOT } from './paths';

export const GATEWAY_BIN = path.join(REPO_ROOT, 'rs', 'target', 'debug', 'paigasus-gateway');
/** The key the gateway sends upstream. The mock records it; a user token must never appear there. */
export const GATEWAY_OPENAI_KEY = 'sk-e2e-openai-key';
const MAX_START_ATTEMPTS = 3;

export type GatewayProcess = { readonly url: string; output(): string; close(): Promise<void> };

export async function startGateway(opts: { readonly iamGrpcUrl: string; readonly openAiUrl: string }): Promise<GatewayProcess> {
  assertDefaultCargoTargetDir();
  if (!existsSync(GATEWAY_BIN)) throw new Error(`${GATEWAY_BIN} does not exist. Run \`moon run paigasus-gateway-rs:e2e-bin\` (gateway-console-ts:test-e2e depends on it).`);
  const cwd = path.dirname(GATEWAY_BIN);
  if (existsSync(path.join(cwd, 'gateway.toml'))) throw new Error(`${cwd} holds a gateway.toml; the e2e gateway must be configured only through GATEWAY_* variables`);
  const failures: string[] = [];
  for (let attempt = 1; attempt <= MAX_START_ATTEMPTS; attempt += 1) {
    const port = await freePort();
    let output = '';
    const child: ChildProcess = spawn(GATEWAY_BIN, [], {
      cwd,
      stdio: ['ignore', 'pipe', 'pipe'],
      // gatewayEnv() returns a plain string record and deliberately carries no NODE_ENV.
      // child_process.spawn's `env` option is typed NodeJS.ProcessEnv, whose NODE_ENV is a
      // REQUIRED property only because Next's global augmentation adds it — the gateway is a Rust
      // binary and never reads it. The double cast documents that gap; the runtime object is
      // unaffected.
      //
      // The cast IS necessary under `tsc -p tsconfig.json` (this app's own config, which includes
      // next-env.d.ts and so the augmented, NODE_ENV-required ProcessEnv). ESLint's projectService
      // resolves this file against tests/tsconfig.json instead (the nearest one, extending the
      // plain workspace base with no Next augmentation), under which NodeJS.ProcessEnv has no
      // required NODE_ENV and the cast looks redundant. Both tsconfigs are real repo config, not a
      // mistake on either side — see tests/tsconfig.json's own comment.
      // eslint-disable-next-line @typescript-eslint/no-unnecessary-type-assertion
      env: gatewayEnv({
        GATEWAY_HTTP_ADDR: `127.0.0.1:${String(port)}`,
        GATEWAY_LOG_LEVEL: 'info',
        GATEWAY_IAM__GRPC_ADDR: opts.iamGrpcUrl,
        GATEWAY_IAM__TLS__MODE: 'loopback_insecure',
        GATEWAY_UPSTREAM__OPENAI__BASE_URL: opts.openAiUrl,
        GATEWAY_UPSTREAM__OPENAI__API_KEY: GATEWAY_OPENAI_KEY,
        GATEWAY_METRICS__ENABLED: 'false',
        GATEWAY_STREAM_ENABLED: 'true',
      }) as unknown as NodeJS.ProcessEnv,
    });
    // A spawn failure (ENOENT, EACCES, ...) emits 'error' asynchronously, after the existsSync
    // check above already passed. With no listener that event is unhandled and crashes the
    // worker. Listening here both stops that crash and turns the failure into the same
    // controlled startup error the health check below throws on.
    const spawnError: Promise<never> = once(child, 'error').then(([error]) => {
      throw new Error(`failed to spawn ${GATEWAY_BIN}: ${(error as Error).message}`, { cause: error });
    });
    child.stdout?.on('data', (chunk: Buffer) => {
      output += chunk.toString('utf8');
    });
    child.stderr?.on('data', (chunk: Buffer) => {
      output += chunk.toString('utf8');
    });
    const url = `http://127.0.0.1:${String(port)}`;
    let state: 'ready' | 'exited';
    try {
      state = await Promise.race([waitForHealth(`${url}/healthz`, child, () => output, 'paigasus-gateway'), spawnError]);
    } catch (error) {
      // waitForHealth throws on a 5xx answer or at its timeout, with the child still running. A
      // spawn failure sets exitCode before 'error' fires, so stop() below is a no-op for that
      // case and a real cleanup for the other two — either way it must run or the child leaks.
      await stop(child);
      throw error;
    }
    if (state === 'ready') return { url, output: () => output, close: () => stop(child) };
    failures.push(`attempt ${String(attempt)}: the gateway exited before it answered\n${output}`);
    await stop(child);
  }
  throw new Error(`paigasus-gateway failed to start after ${String(MAX_START_ATTEMPTS)} attempts:\n${failures.join('\n')}`);
}
