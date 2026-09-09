// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from 'vitest';
import { ErrorDomain, ErrorDomainSchema, ErrorInfoSchema, ErrorReason, ErrorReasonSchema, asWireDomain, asWireReason, fromWireDomain, fromWireReason } from './index.js';
import { AuditService, AuthnService, AuthorizationService, OutboxService, ServiceAccountService, TenancyService, UserService } from './iam.js';

// These assertions are the package's public-surface contract. @paigasus/sdk's
// eslint boundary rule permits it to import @paigasus/proto and nothing else in
// the @paigasus/* namespace, so anything the SDK needs must be reachable here.
// Narrowing this surface breaks a consumer that cannot route around it.
describe('the root barrel', () => {
  it('exposes the error registry and its schemas', () => {
    expect(ErrorReason.SLUG_CONFLICT).toBe(1);
    expect(ErrorDomain.IAM).toBe(1);
    expect(ErrorReasonSchema.typeName).toBe('paigasus.common.v1.ErrorReason');
    expect(ErrorDomainSchema.typeName).toBe('paigasus.common.v1.ErrorDomain');
  });

  it('exposes the google.rpc.ErrorInfo schema Connect-ES needs for findDetails', () => {
    expect(ErrorInfoSchema.typeName).toBe('google.rpc.ErrorInfo');
  });

  it('exposes both codec directions', () => {
    expect(asWireReason(ErrorReason.SLUG_CONFLICT)).toBe('slug-conflict');
    expect(fromWireReason('slug-conflict')).toBe(ErrorReason.SLUG_CONFLICT);
    expect(asWireDomain(ErrorDomain.GATEWAY)).toBe('gateway.paigasus.io');
    expect(fromWireDomain('gateway.paigasus.io')).toBe(ErrorDomain.GATEWAY);
  });
});

describe('the ./iam subpath', () => {
  it('exposes all seven IAM services', () => {
    const services = [TenancyService, AuthnService, AuthorizationService, ServiceAccountService, AuditService, UserService, OutboxService];
    expect(services.map((s) => s.typeName)).toEqual([
      'paigasus.iam.v1.TenancyService',
      'paigasus.iam.v1.AuthnService',
      'paigasus.iam.v1.AuthorizationService',
      'paigasus.iam.v1.ServiceAccountService',
      'paigasus.iam.v1.AuditService',
      'paigasus.iam.v1.UserService',
      'paigasus.iam.v1.OutboxService',
    ]);
  });

  it('is a separate module from the root barrel, which is what avoids the ServiceInfo collision', async () => {
    // iam.proto:22-33 keeps a DEPRECATED ServiceInfo message that buf forbids
    // deleting. The live one is paigasus.common.v1.ServiceInfo, re-exported
    // from the root. BOTH generated modules export a runtime `ServiceInfoSchema`
    // AND a `ServiceInfo` type, so re-exporting both from one module is a
    // duplicate-export error. They must never meet.
    //
    // Asserted on the SCHEMA, not the message type: `ServiceInfo` is a type and
    // is erased at runtime, so an `in` check against the namespace object would
    // read false and prove nothing.
    const iam = await import('./iam.js');
    const root = await import('./index.js');
    expect(iam.ServiceInfoSchema.typeName).toBe('paigasus.iam.v1.ServiceInfo');
    expect(root.ServiceInfoSchema.typeName).toBe('paigasus.common.v1.ServiceInfo');
  });
});
