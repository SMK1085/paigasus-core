// SPDX-License-Identifier: Apache-2.0
//
// The testing surface. OUTSIDE src/ and deliberately NOT server-only guarded: vitest and Playwright
// harnesses import it outside a Next server. That placement is what lets the src/ rule — every file
// imports 'server-only' — hold with no exception.
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
export { startTlsTerminator } from './tls-terminator';
