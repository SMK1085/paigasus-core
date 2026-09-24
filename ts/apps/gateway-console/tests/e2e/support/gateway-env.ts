// SPDX-License-Identifier: Apache-2.0
//
// The pure half of the playground's gateway child (SMA-635, Review Focus 5). NO import at all, so
// tests/unit/gateway-env.test.ts can load it under vitest: harness.ts calls @playwright/test's
// `test.extend` at module scope, which must not run there.

export type GatewayLogLine = { readonly fields?: Readonly<Record<string, unknown>> };

/**
 * Variables stripped from the parent's env before it reaches the gateway child, on top of every
 * GATEWAY_* one. NODE_ENV is a Node-only convention the gateway (a Rust binary) never reads, so
 * it is dropped and NOT re-injected — the child gets no NODE_ENV at all. The proxy variables (both
 * casings) could redirect the gateway's own OpenAI/IAM egress through an unrelated proxy the test
 * host happens to have set.
 */
const STRIPPED_KEYS = new Set(['NODE_ENV', 'RUST_LOG', 'HTTP_PROXY', 'HTTPS_PROXY', 'http_proxy', 'https_proxy', 'ALL_PROXY', 'all_proxy', 'NO_PROXY', 'no_proxy']);

/**
 * The parent's env minus STRIPPED_KEYS and every GATEWAY_* variable, plus `values`. Typed as a
 * plain string record, not `NodeJS.ProcessEnv`: Next's global augmentation makes that interface's
 * `NODE_ENV` a REQUIRED property, which this function deliberately does not set (see
 * STRIPPED_KEYS above). The one caller that must satisfy `child_process.spawn`'s `env` option
 * casts explicitly at that boundary (gateway-process.ts), not here.
 */
export function gatewayEnv(values: Readonly<Record<string, string>>, parent: Readonly<Record<string, string | undefined>> = process.env): Record<string, string> {
  const env: Record<string, string> = {};
  for (const [key, value] of Object.entries(parent)) {
    if (value === undefined || STRIPPED_KEYS.has(key) || key.startsWith('GATEWAY_')) continue;
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
