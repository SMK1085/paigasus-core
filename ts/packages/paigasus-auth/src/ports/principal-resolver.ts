// SPDX-License-Identifier: Apache-2.0
//
// The port speaks DOMAIN types, not proto types. RoleGrantRef and Membership exist as protobuf
// messages in @paigasus/proto, and importing them would make @paigasus/auth a dependent of that
// package — contradicting the § 6 dependency graph (apps → {sdk, auth/server}; sdk → proto) and
// putting paigasus-auth-ts into ci/affected-graph's strict-equality contracts->proto set.
//
// When IntrospectPrincipalResolver lands in SMA-508, the ADAPTER maps proto to these types.

/** A role grant, flattened from IAM's RoleGrantRef { scope_prn, role_key }. */
export interface RoleGrantRef {
  scopePrn: string;
  roleKey: string;
}

/** A membership, flattened from IAM's Membership { id, principal_prn, node_prn }. */
export interface Membership {
  id: string;
  principalPrn: string;
  nodePrn: string;
}

export interface ResolvedPrincipal {
  /** null until SMA-508 wires Introspect — IAM mints the PRN, the claims cannot. */
  principalPrn: string | null;
  issuer: string;
  subject: string;
  memberships: Membership[];
  roleGrants: RoleGrantRef[];
  /**
   * False while the claims-based resolver is in use. Consumers MUST treat an unavailable grant
   * set as "unknown", never as "denied" — see can() in src/client.ts.
   */
  grantsAvailable: boolean;
}

export interface PrincipalResolver {
  resolve(input: { accessToken: string; idTokenClaims: IdTokenClaims }): Promise<ResolvedPrincipal>;
}

export interface IdTokenClaims {
  iss: string;
  sub: string;
  email?: string;
  name?: string;
  [claim: string]: unknown;
}
