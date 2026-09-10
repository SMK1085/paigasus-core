// SPDX-License-Identifier: Apache-2.0
//
// The resolver that ships TODAY. It derives identity from the verified ID-token claims and
// reports the grant set as UNAVAILABLE.
//
// IAM's Introspect is what mints a principal PRN and returns memberships and role grants, and it
// is not reachable from TypeScript yet: contracts/buf.gen.yaml runs only bufbuild/es for TS
// (descriptors, no client), @paigasus/proto exports only common/v1, and @paigasus/sdk is a stub.
// SMA-508 lands the transport; IntrospectPrincipalResolver then slots in behind this same port
// with no change to the session shape or the store.
//
// grantsAvailable: false is NOT the same as "no grants". can() must treat it as unknown and fail
// OPEN, or a console gating navigation on it renders with no navigation at all.
import type { PrincipalResolver, ResolvedPrincipal } from '../ports/principal-resolver.js';

export const claimsPrincipalResolver: PrincipalResolver = {
  resolve({ idTokenClaims }): Promise<ResolvedPrincipal> {
    return Promise.resolve({
      principalPrn: null,
      issuer: idTokenClaims.iss,
      subject: idTokenClaims.sub,
      memberships: [],
      roleGrants: [],
      grantsAvailable: false,
    });
  },
};
