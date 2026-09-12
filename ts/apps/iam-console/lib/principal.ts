// SPDX-License-Identifier: Apache-2.0
//
// Who the current user is, according to IAM, NOW (spec § 4.5). The pages use this, never the
// login snapshot in the session record: a degraded login must not stay degraded for the session,
// and a membership change must appear on the next render.
//
// PROVISIONING. Introspect is exempt from bearer enforcement and runs with Provisioning::Disabled,
// so for an identity IAM has never seen it answers PermissionDenied `identity-not-provisioned`
// (rs/crates/services/paigasus-iam/src/adapters/grpc/authn.rs:139-141;
// application/authenticate_token.rs:104-105). IAM provisions only inside a bearer-enforced RPC
// (authn.rs:182-190). GetServiceInfo is bearer-enforced and checks no Cedar action
// (adapters/grpc/service_info.rs:1-44), so it is the provisioning call.
import 'server-only';
import { cache } from 'react';
import { ErrorReason } from '@paigasus/sdk/errors';
import { callIam, type IamResult } from './errors';
import { iamClients, sessionToken, type IamClients } from './iam';

export type Principal = { prn: string; memberships: readonly { nodePrn: string }[] };

export async function introspectWithProvisioning(
  clients: Pick<IamClients, 'authn' | 'serviceInfo'>,
  token: string,
  opts: { timeoutMs?: number; provisionFirst?: boolean } = {},
): Promise<IamResult<Principal>> {
  const callOptions = opts.timeoutMs === undefined ? {} : { timeoutMs: opts.timeoutMs };
  const provision = () => callIam(() => clients.serviceInfo.getServiceInfo({}, callOptions));
  const introspect = () => callIam(() => clients.authn.introspect({ token }, callOptions));

  if (opts.provisionFirst === true) {
    const provisioned = await provision();
    if (!provisioned.ok) return provisioned;
  }
  let answer = await introspect();
  if (!answer.ok && opts.provisionFirst !== true && answer.error.reason === ErrorReason.IDENTITY_NOT_PROVISIONED) {
    // ONE retry, after one provisioning call. A second `identity-not-provisioned` is returned as
    // the error it is; looping would hide a provisioning failure behind a hang.
    const provisioned = await provision();
    if (!provisioned.ok) return provisioned;
    answer = await introspect();
  }
  if (!answer.ok) return answer;
  return { ok: true, value: { prn: answer.value.principalPrn, memberships: answer.value.memberships.map((m) => ({ nodePrn: m.nodePrn })) } };
}

/** Per request: a LIVE Introspect, with one provisioning retry on `identity-not-provisioned`. */
export const currentPrincipal: () => Promise<IamResult<Principal>> = cache(async () => introspectWithProvisioning(await iamClients(), await sessionToken(), { provisionFirst: false }));
