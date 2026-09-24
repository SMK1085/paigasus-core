// SPDX-License-Identifier: Apache-2.0
//
// The pure half of the playground's gateway child (SMA-635, Review Focus 5). NO import at all, so
// tests/unit/gateway-env.test.ts can load it under vitest: harness.ts calls @playwright/test's
// `test.extend` at module scope, which must not run there.

export type GatewayLogLine = { readonly fields?: Readonly<Record<string, unknown>> };

/** The parent's env minus RUST_LOG and every GATEWAY_* variable, plus `values`. */
export function gatewayEnv(values: Readonly<Record<string, string>>, parent: NodeJS.ProcessEnv = process.env): NodeJS.ProcessEnv {
  const env: NodeJS.ProcessEnv = { NODE_ENV: 'production' };
  for (const [key, value] of Object.entries(parent)) {
    if (key === 'NODE_ENV' || key === 'RUST_LOG' || key.startsWith('GATEWAY_')) continue;
    env[key] = value;
  }
  return { ...env, ...values };
}

/** Every complete JSON line of the gateway's output. A partial last line is skipped. */
export function parseGatewayLog(output: string): GatewayLogLine[] {
  const lines: GatewayLogLine[] = [];
  for (const line of output.split('\n')) {
    if (!line.startsWith('{')) continue;
    try {
      lines.push(JSON.parse(line) as GatewayLogLine);
    } catch {
      // A line the gateway has not finished writing.
    }
  }
  return lines;
}
