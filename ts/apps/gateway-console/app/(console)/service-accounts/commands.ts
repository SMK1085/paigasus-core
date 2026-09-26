// SPDX-License-Identifier: Apache-2.0
//
// The commands of the gateway settings (SMA-636 spec § 5). Pure and dependency-injected: the
// actions pass in the clients, the clock and the logger. They take NO mayI: IAM decides (§ 5.1).
//
// D6. A grant's scope and a key's scope are ALWAYS the account's owner node, read from IAM on the
// server: from CreateServiceAccount's response in the create flow, and from GetServiceAccount in
// the other two. No input below carries a scope, so a form field cannot choose one.
import 'server-only';
import { z } from 'zod';
import type { PaigasusError } from '@paigasus/sdk/errors/types';
import { callIam, nameField, neverReachedIam, prnField, toActionResult, type ActionResult, type ConsoleLogger, type IamClients, type IamResult } from '@paigasus/console-core';
import { GATEWAY_ROLE } from '../gateway-role';
import { expiresAtFor } from './keys';
import { EXPIRY_CHOICES, type CreateState, type IssueKeyState } from './view';

type ServiceAccounts = IamClients['serviceAccounts'];
type Authz = IamClients['authz'];
type AccountMessage = NonNullable<Awaited<ReturnType<ServiceAccounts['getServiceAccount']>>['serviceAccount']>;

/** `ownerPrn` is under client control, and that is safe: IAM checks CreateServiceAccount at that node (§ 5.1). */
export const createServiceAccountForm = z.object({ ownerPrn: prnField, name: nameField });
export type CreateServiceAccountInput = z.infer<typeof createServiceAccountForm>;

export const serviceAccountForm = z.object({ saPrn: prnField });
export type ServiceAccountInput = z.infer<typeof serviceAccountForm>;

export const issueApiKeyForm = z.object({ saPrn: prnField, expiry: z.enum(EXPIRY_CHOICES) });
export type IssueApiKeyInput = z.infer<typeof issueApiKeyForm>;

/** A key id is IAM's; this bound only limits the request. `saPrn` only keeps the panel selected (§ 5.5). */
export const revokeApiKeyForm = z.object({ saPrn: prnField, keyId: z.string().trim().min(1).max(128) });
export type RevokeApiKeyInput = z.infer<typeof revokeApiKeyForm>;

/**
 * IAM answered a successful call with no account. That is version skew, not a state IAM produces.
 * It is reported as not-found, and no grant and no key follow. `neverReachedIam` only fills the
 * fields that carry no IAM data.
 */
function missingAccount(): PaigasusError {
  return neverReachedIam({ presentation: 'not-found', message: 'IAM returned no service account.', transport: { kind: 'grpc', code: 5, codeName: 'NotFound' } });
}

/** GetServiceAccount: the account as IAM stores it, for the owner PRN that D6 takes from IAM. */
async function storedAccount(serviceAccounts: Pick<ServiceAccounts, 'getServiceAccount'>, saPrn: string): Promise<IamResult<AccountMessage>> {
  const got = await callIam(() => serviceAccounts.getServiceAccount({ prn: saPrn }));
  if (!got.ok) return got;
  const account = got.value.serviceAccount;
  return account === undefined ? { ok: false, error: missingAccount() } : { ok: true, value: account };
}

/**
 * § 5.2. Two RPCs, not one atomic RPC (D3): CreateServiceAccount, then — when IAM offers role
 * administration (D13) — GrantRole(account, gateway_user, owner). A failed grant leaves an account
 * that cannot call models; the answer is `partial`, and the screen offers the repair control.
 */
export async function createServiceAccount(
  deps: {
    readonly serviceAccounts: Pick<ServiceAccounts, 'createServiceAccount'>;
    readonly authz: Pick<Authz, 'grantRole'>;
    readonly cedar: boolean;
    readonly logger: Pick<ConsoleLogger, 'appEvent'>;
  },
  input: CreateServiceAccountInput,
): Promise<Exclude<CreateState, null>> {
  const created = await callIam(() => deps.serviceAccounts.createServiceAccount({ ownerPrn: input.ownerPrn, name: input.name }));
  if (!created.ok) return { kind: 'failed', error: created.error };
  const account = created.value.serviceAccount;
  if (account === undefined) return { kind: 'failed', error: missingAccount() };
  if (!deps.cedar) return { kind: 'created', saPrn: account.prn, granted: false };
  const granted = await callIam(() => deps.authz.grantRole({ principalPrn: account.prn, roleKey: GATEWAY_ROLE, scopePrn: account.ownerPrn }));
  if (granted.ok) return { kind: 'created', saPrn: account.prn, granted: true };
  // Scalars only, and never IAM's message (the logger's redaction contract).
  deps.logger.appEvent('gateway.sa.grant_failed', { presentation: granted.error.presentation, reason: granted.error.rawReason, correlation_id: granted.error.correlationId });
  return { kind: 'partial', saPrn: account.prn, error: granted.error };
}

/** § 5.3: GetServiceAccount, then GrantRole(account, gateway_user, owner_prn). */
export async function allowModelCalls(
  deps: { readonly serviceAccounts: Pick<ServiceAccounts, 'getServiceAccount'>; readonly authz: Pick<Authz, 'grantRole'> },
  input: ServiceAccountInput,
): Promise<ActionResult> {
  const account = await storedAccount(deps.serviceAccounts, input.saPrn);
  if (!account.ok) return { ok: false, error: account.error };
  return toActionResult(await callIam(() => deps.authz.grantRole({ principalPrn: account.value.prn, roleKey: GATEWAY_ROLE, scopePrn: account.value.ownerPrn })));
}

/**
 * § 5.4: GetServiceAccount, then IssueApiKey(account, scope = owner_prn, expires_at). The scope
 * lists stay empty (IAM stores them and does not enforce them in v1). The token is in the result
 * ONLY: this function logs nothing, and callIam logs only a FAILED call, which carries no token.
 */
export async function issueApiKey(
  deps: { readonly serviceAccounts: Pick<ServiceAccounts, 'getServiceAccount' | 'issueApiKey'>; readonly now: () => number },
  input: IssueApiKeyInput,
): Promise<Exclude<IssueKeyState, null>> {
  const account = await storedAccount(deps.serviceAccounts, input.saPrn);
  if (!account.ok) return { ok: false, error: account.error };
  const expiresAt = expiresAtFor(input.expiry, deps.now());
  const issued = await callIam(() =>
    deps.serviceAccounts.issueApiKey({ serviceAccountPrn: account.value.prn, scopePrn: account.value.ownerPrn, ...(expiresAt === undefined ? {} : { expiresAt }), scopeActions: [], scopeRoles: [] }),
  );
  if (!issued.ok) return { ok: false, error: issued.error };
  return { ok: true, token: issued.value.token, prefix: issued.value.apiKey?.prefix ?? '' };
}

/** § 5.5. IAM authorizes the revoke at the owner node of the key's own account. */
export async function revokeApiKey(deps: { readonly serviceAccounts: Pick<ServiceAccounts, 'revokeApiKey'> }, input: RevokeApiKeyInput): Promise<ActionResult> {
  return toActionResult(await callIam(() => deps.serviceAccounts.revokeApiKey({ id: input.keyId })));
}

/** § 5.6. IAM disables the principal and evicts its keys from its API-key cache. No restore RPC exists. */
export async function archiveServiceAccount(deps: { readonly serviceAccounts: Pick<ServiceAccounts, 'archiveServiceAccount'> }, input: ServiceAccountInput): Promise<ActionResult> {
  return toActionResult(await callIam(() => deps.serviceAccounts.archiveServiceAccount({ prn: input.saPrn })));
}
