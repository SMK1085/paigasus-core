// SPDX-License-Identifier: Apache-2.0
//
// Helpers for the tier-2 tests (spec § 9.3). The clients talk real gRPC to the fake IAM, so a test
// exercises the SDK's transport and error map, not a hand-built object.
import { AuditService, AuthorizationService, TenancyService, createIamClient } from '@paigasus/sdk/iam';
import type { IamAction, MayI } from '../../lib/authorize';
import type { FakeIam, FakeIamCall, FakeIamMethod } from '../support/fake-iam';

/** Fixed UUIDs. `a` sorts before `b`, so orgA's scopes come first in every list. */
export const IDS = {
  // IAM names a principal `prn:pgs:iam:::principal/<uuid>` (paigasus-iam-core authn.rs), as the fake does.
  principalPrn: 'prn:pgs:iam:::principal/0190a1e5-0000-7000-8000-0000000000e0',
  orgA: '0190a100-0000-7000-8000-00000000000a',
  orgB: '0190a100-0000-7000-8000-00000000000b',
  teamA1: '0190a1b2-0000-7000-8000-0000000000a1',
  teamB1: '0190a1b2-0000-7000-8000-0000000000b1',
  projectA1: '0190a1c3-0000-7000-8000-0000000000a1',
  projectB1: '0190a1c3-0000-7000-8000-0000000000b1',
  membership: '0190a1d4-0000-7000-8000-0000000000c1',
} as const;

export function clientsFor(iam: FakeIam, token = 'tok-integration') {
  const auth = { bearer: token };
  const options = { baseUrl: iam.grpcUrl };
  return {
    tenancy: createIamClient(TenancyService, options, auth),
    authz: createIamClient(AuthorizationService, options, auth),
    audit: createIamClient(AuditService, options, auth),
  };
}

export type ScriptedMayI = MayI & { readonly asked: readonly (readonly [IamAction, string])[] };

/** A MayI that answers from a table (absent = false) and records every question. */
export function scriptedMayI(allowed: Partial<Record<IamAction, boolean>>): ScriptedMayI {
  const asked: (readonly [IamAction, string])[] = [];
  const mayI = (action: IamAction, resourcePrn: string): Promise<boolean> => {
    asked.push([action, resourcePrn]);
    return Promise.resolve(allowed[action] ?? false);
  };
  return Object.assign(mayI, { asked });
}

/** The calls the fake saw from now on, by method. The fake's log is shared by every test in a file. */
export function callsSince(iam: FakeIam): (method: FakeIamMethod | 'http.getServiceInfo') => FakeIamCall[] {
  const start = iam.calls.length;
  return (method) => iam.calls.slice(start).filter((call) => call.method === method);
}
