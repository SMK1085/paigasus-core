// SPDX-License-Identifier: Apache-2.0
//
// "Can this service account call models?" (SMA-636 spec § 4.3, D8). It asks IAM's IsAuthorized
// about the ACCOUNT, not about the current user. IAM allows that question only to a caller that
// holds ListRoleGrants at the resource, which every *_admin role holds (spec § 3.2).
//
// IT FAILS CLOSED. Any failed call — `forbidden` included — answers `unknown`, never `yes`. It is
// NOT mayI(), which fails open and asks about the current user.
//
// A status other than `active` answers `archived` with NO call: no Cedar policy reads the
// principal's status, so IAM would answer `allowed` for an archived account that still holds
// gateway_user (spec § 3.2).
import 'server-only';
import { callIam, type IamClients } from '@paigasus/console-core';
import type { ModelCallState } from './view';

export async function modelCallState(authz: Pick<IamClients['authz'], 'isAuthorized'>, saPrn: string, ownerPrn: string, status: string): Promise<ModelCallState> {
  if (status !== 'active') return 'archived';
  const answer = await callIam(() => authz.isAuthorized({ principalPrn: saPrn, action: 'InvokeModel', resourcePrn: ownerPrn }));
  if (!answer.ok) return 'unknown';
  return answer.value.allowed ? 'yes' : 'no';
}
