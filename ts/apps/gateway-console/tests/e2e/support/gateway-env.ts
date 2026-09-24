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

/**
 * Throws when `CARGO_TARGET_DIR` overrides cargo's default target directory. The playground
 * harness always runs `rs/target/debug/paigasus-gateway`; with `CARGO_TARGET_DIR` set, a rebuild
 * writes the binary somewhere else, and the harness would run a stale binary with no warning.
 */
export function assertDefaultCargoTargetDir(parent: NodeJS.ProcessEnv = process.env): void {
  if (parent.CARGO_TARGET_DIR !== undefined) {
    throw new Error(
      'CARGO_TARGET_DIR is set. The playground harness always runs rs/target/debug/paigasus-gateway. With CARGO_TARGET_DIR set, a rebuild writes the binary elsewhere, and this harness would run a stale rs/target/debug/paigasus-gateway. Unset CARGO_TARGET_DIR before running the e2e tier.',
    );
  }
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
