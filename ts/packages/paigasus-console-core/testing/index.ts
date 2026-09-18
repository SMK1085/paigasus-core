// SPDX-License-Identifier: Apache-2.0
//
// The testing surface. OUTSIDE src/ and deliberately NOT server-only guarded: vitest and Playwright
// harnesses import it outside a Next server. The src/ rule — every file imports 'server-only' — has
// ONE exception: src/global.d.ts declares a type only and imports nothing, so it carries no
// 'server-only' import either, harmlessly, since a .d.ts emits no runtime code. What actually keeps
// a client bundle safe is the package's `exports` map (only '.' and './testing' are reachable, so no
// deep import can reach an unguarded module) together with src/index.ts's own 'server-only' import.
// The per-file rule is defence in depth on top of that, not the guard itself.
export { buildDevEnv, DEV_ZONES, type DevEnvInput } from './dev-env';
export { DEV_GATEWAY_DESCRIPTOR, DEV_IAM_DESCRIPTOR, devWorld } from './dev-world';
export { GATEWAY_CORRELATION_HEADER, startFakeGateway, type FakeGateway, type FakeGatewayCall, type GatewayDescriptorBody } from './fake-gateway';
export {
  denial,
  errorInfoOf,
  FAKE_IAM_ISSUER,
  IAM_ERROR_DOMAIN,
  startFakeIam,
  type FakeIam,
  type FakeIamCall,
  type FakeIamContext,
  type FakeIamHandlers,
  type FakeIamMethod,
  type ServiceDescriptorBody,
} from './fake-iam';
export { startFakeIdp, type FakeIdp } from './fake-idp';
export { testTls, type TlsMaterial } from './tls';
export { startTlsTerminator, type TerminatorRoute } from './tls-terminator';
