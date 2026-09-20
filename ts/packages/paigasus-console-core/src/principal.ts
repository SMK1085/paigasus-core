// SPDX-License-Identifier: Apache-2.0
//
// Who the current user is, according to IAM, NOW. The pages use this, never the login snapshot
// in the session record: a degraded login must not stay degraded for the session, and a
// membership change must appear on the next render.
//
// ONE CALL (SMA-632). WhoAmI is bearer-enforced — it is deliberately absent from `is_exempt`
// (rs/crates/services/paigasus-iam/src/adapters/grpc/authn.rs:139-141) — so IAM resolves the
// bearer with Provisioning::Enabled and JIT-provisions the caller before the handler runs.
// Provisioning is no longer a side effect of GetServiceInfo, and there is nothing left to retry.
//
// `identity-not-provisioned` can still come back: enforcement is necessary but not sufficient,
// because `resolve` also checks the issuer's JIT flag (application/authenticate_token.rs:107-109).
// A retry would not help — the issuer's configuration is what refused — so it is reported.
import 'server-only';
import type { IamClients } from './iam-clients';
import { callIam, type IamResult } from './errors';
import { principalPrnOf } from './principal-prn';

/**
 * `prn` is `null` when IAM answered but named no principal (review, defect 1). That is NOT the
 * login-time degrade in principal-resolver.ts, which discards the whole answer: here the
 * memberships IAM did send stay usable, and only the two things that need a name change — mayI()
 * cannot ask IAM about an unnamed principal (authorize.ts) and myScopes() cannot list its role
 * grants (scopes.ts). Both say so in the log rather than passing `''` to IAM.
 */
export type Principal = { prn: string | null; memberships: readonly { nodePrn: string }[] };

export async function whoAmI(clients: Pick<IamClients, 'authn'>, opts: { timeoutMs?: number } = {}): Promise<IamResult<Principal>> {
  const callOptions = opts.timeoutMs === undefined ? {} : { timeoutMs: opts.timeoutMs };
  const answer = await callIam(() => clients.authn.whoAmI({}, callOptions));
  if (!answer.ok) return answer;
  return { ok: true, value: { prn: principalPrnOf(answer.value.principalPrn), memberships: answer.value.memberships.map((m) => ({ nodePrn: m.nodePrn })) } };
}
