// SPDX-License-Identifier: Apache-2.0
//
// Review Focus 5 (SMA-635). The e2e gateway child must see ONLY the GATEWAY_* values the harness
// gives it: an inherited RUST_LOG can silence the `chat completion proxied` line that row R22
// reads, and an inherited GATEWAY_* can reconfigure the gateway under test.
import { describe, expect, it } from 'vitest';
// gateway-env.ts has no import at all: harness.ts calls @playwright/test's `test.extend` at module
// scope, which must not run under vitest.
import { assertDefaultCargoTargetDir, gatewayEnv, parseGatewayLog } from '../e2e/support/gateway-env';

describe('gatewayEnv', () => {
  it('drops every inherited GATEWAY_* variable, RUST_LOG, NODE_ENV and every proxy variable, and keeps the rest', () => {
    const env = gatewayEnv(
      { GATEWAY_LOG_LEVEL: 'info' },
      {
        PATH: '/bin',
        RUST_LOG: 'warn',
        GATEWAY_STREAM_ENABLED: 'false',
        GATEWAY_UPSTREAM__OPENAI__API_KEY: 'sk-leak',
        NODE_ENV: 'test',
        HOME: '/h',
        HTTP_PROXY: 'http://proxy.test:8080',
        HTTPS_PROXY: 'http://proxy.test:8080',
        http_proxy: 'http://proxy.test:8080',
        https_proxy: 'http://proxy.test:8080',
        ALL_PROXY: 'http://proxy.test:8080',
        all_proxy: 'http://proxy.test:8080',
        NO_PROXY: 'localhost',
        no_proxy: 'localhost',
      },
    );
    expect(env['PATH']).toBe('/bin');
    expect(env['HOME']).toBe('/h');
    expect(env['RUST_LOG']).toBeUndefined();
    expect(env['GATEWAY_STREAM_ENABLED']).toBeUndefined();
    expect(env['GATEWAY_UPSTREAM__OPENAI__API_KEY']).toBeUndefined();
    expect(env['GATEWAY_LOG_LEVEL']).toBe('info');
    expect(env['NODE_ENV']).toBeUndefined();
    expect(env['HTTP_PROXY']).toBeUndefined();
    expect(env['HTTPS_PROXY']).toBeUndefined();
    expect(env['http_proxy']).toBeUndefined();
    expect(env['https_proxy']).toBeUndefined();
    expect(env['ALL_PROXY']).toBeUndefined();
    expect(env['all_proxy']).toBeUndefined();
    expect(env['NO_PROXY']).toBeUndefined();
    expect(env['no_proxy']).toBeUndefined();
  });

  it('does not inject NODE_ENV, even when the parent has none at all', () => {
    const env = gatewayEnv({}, { PATH: '/bin' });
    expect('NODE_ENV' in env).toBe(false);
  });
});

describe('parseGatewayLog', () => {
  it('reads JSON lines and skips everything else', () => {
    const lines = parseGatewayLog('{"fields":{"message":"chat completion proxied","auth":"oidc"}}\nplain text\n{"fields":{"mess');
    expect(lines).toEqual([{ fields: { message: 'chat completion proxied', auth: 'oidc' } }]);
  });
});

describe('assertDefaultCargoTargetDir', () => {
  it('throws, naming the stale-binary risk, when CARGO_TARGET_DIR is set', () => {
    expect(() => assertDefaultCargoTargetDir({ NODE_ENV: 'test', CARGO_TARGET_DIR: '/tmp/other-target' })).toThrow(/rs\/target\/debug\/paigasus-gateway/);
  });

  it('does nothing when CARGO_TARGET_DIR is unset', () => {
    expect(() => assertDefaultCargoTargetDir({ NODE_ENV: 'test' })).not.toThrow();
  });
});
