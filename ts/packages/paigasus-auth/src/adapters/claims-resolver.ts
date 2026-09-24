// SPDX-License-Identifier: Apache-2.0
//
// The resolver that ships TODAY. It derives identity from the verified ID-token claims and
// reports the grant set as UNAVAILABLE.
//
// It uses the ID-token claims only, so it is the resolver for a host that has no IAM transport. A
// host with IAM (the consoles) supplies an IAM-backed resolver behind this same port, with no
// change to the session shape or the store.
//
// grantsAvailable: false is NOT the same as "no grants". can() must treat it as unknown and fail
// OPEN, or a console gating navigation on it renders with no navigation at all.
import type { PrincipalResolver, ResolvedPrincipal } from '../ports/principal-resolver';

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
