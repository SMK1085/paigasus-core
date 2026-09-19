// SPDX-License-Identifier: Apache-2.0
//
// The five commands of the gateway settings (SMA-636 spec § 5, § 7.1) against the fake IAM. A
// command takes no mayI: IAM decides. D6: a grant's scope and a key's scope come from IAM — from
// CreateServiceAccount's response, or from GetServiceAccount — never from an input.
import { afterAll, beforeAll, describe, expect, it, vi } from 'vitest';
import { Code, ConnectError } from '@connectrpc/connect';
import { ErrorReason } from '@paigasus/sdk/errors/types';
import { disposeTransports } from '@paigasus/sdk/iam';
import { organizationPrn, projectPrn } from '@paigasus/console-core';
import { denial, startFakeIam, type FakeIam } from '@paigasus/console-core/testing';
import { GATEWAY_ROLE, allowModelCalls, archiveServiceAccount, createServiceAccount, issueApiKey, revokeApiKey } from '../../app/(console)/service-accounts/commands';
import { serviceAccountPrn } from '../../app/(console)/service-accounts/service-account-id';
import { IDS, callsSince, clientsFor } from './support';

const OWNER = organizationPrn(IDS.orgA);
/** The same node as OWNER, as a client could send it: IAM answers with the canonical lower-case PRN. */
const OWNER_AS_SENT = `prn:pgs:iam:::organization/${IDS.orgA.toUpperCase()}`;
const PROJECT = projectPrn(IDS.orgA, IDS.projectA1);
const SA = serviceAccountPrn(IDS.saA);
const NOW = Date.UTC(2026, 8, 18, 12, 0, 0);
const DAY = 86_400_000;
const TOKEN = 'pgs_int_0123456789abcdef0123456789abcdef';

let iam: FakeIam;

beforeAll(async () => {
  iam = await startFakeIam();
});

afterAll(async () => {
  disposeTransports();
  await iam.close();
});

function storedAccount(ownerPrn: string) {
  return { serviceAccount: { prn: SA, ownerPrn, name: 'ci-bot', status: 'active' } };
}

describe('createServiceAccount (§ 5.2)', () => {
  it('creates, then grants gateway_user at the owner PRN of IAM’s RESPONSE', async () => {
    iam.setHandlers({
      'serviceAccounts.createServiceAccount': (req) => ({ serviceAccount: { prn: SA, ownerPrn: req.ownerPrn.toLowerCase(), name: req.name, status: 'active' } }),
      'authz.grantRole': (req) => ({ grant: { id: 'g-1', principalPrn: req.principalPrn, roleKey: req.roleKey, scopePrn: req.scopePrn } }),
    });
    const calls = callsSince(iam);
    const clients = clientsFor(iam);
    const appEvent = vi.fn();

    const result = await createServiceAccount({ serviceAccounts: clients.serviceAccounts, authz: clients.authz, cedar: true, logger: { appEvent } }, { ownerPrn: OWNER_AS_SENT, name: 'ci-bot' });

    expect(result).toEqual({ kind: 'created', saPrn: SA, granted: true });
    expect(calls('serviceAccounts.createServiceAccount')[0]?.request).toMatchObject({ ownerPrn: OWNER_AS_SENT, name: 'ci-bot' });
    const grants = calls('authz.grantRole');
    expect(grants).toHaveLength(1);
    expect(grants[0]?.request).toMatchObject({ principalPrn: SA, roleKey: 'gateway_user', scopePrn: OWNER });
    expect(GATEWAY_ROLE).toBe('gateway_user');
    expect(appEvent).not.toHaveBeenCalled();
  });

  it('answers partial and logs gateway.sa.grant_failed, without IAM’s message, when the grant fails', async () => {
    iam.setHandlers({
      'serviceAccounts.createServiceAccount': () => storedAccount(OWNER),
      'authz.grantRole': () => {
        throw new ConnectError('grant exploded with internal detail', Code.Internal);
      },
    });
    const clients = clientsFor(iam);
    const appEvent = vi.fn();

    const result = await createServiceAccount({ serviceAccounts: clients.serviceAccounts, authz: clients.authz, cedar: true, logger: { appEvent } }, { ownerPrn: OWNER, name: 'ci-bot' });

    expect(result.kind).toBe('partial');
    if (result.kind !== 'partial') throw new Error('expected partial');
    expect(result.saPrn).toBe(SA);
    expect(result.error.presentation).toBe('generic');
    expect(appEvent).toHaveBeenCalledTimes(1);
    expect(appEvent).toHaveBeenCalledWith('gateway.sa.grant_failed', expect.objectContaining({ presentation: 'generic' }));
    expect(JSON.stringify(appEvent.mock.calls)).not.toContain('internal detail');
  });

  it('answers failed with the name-conflict reason, and makes no grant', async () => {
    iam.setHandlers({
      'serviceAccounts.createServiceAccount': () => {
        throw denial({ code: Code.AlreadyExists, reason: 'service-account-name-conflict' });
      },
    });
    const calls = callsSince(iam);
    const clients = clientsFor(iam);

    const result = await createServiceAccount({ serviceAccounts: clients.serviceAccounts, authz: clients.authz, cedar: true, logger: { appEvent: vi.fn() } }, { ownerPrn: OWNER, name: 'ci-bot' });

    expect(result.kind).toBe('failed');
    if (result.kind !== 'failed') throw new Error('expected failed');
    expect(result.error.presentation).toBe('conflict');
    expect(result.error.reason).toBe(ErrorReason.SERVICE_ACCOUNT_NAME_CONFLICT);
    expect(calls('authz.grantRole')).toHaveLength(0);
  });

  it('makes no grant when IAM does not offer role administration (iam.authz.cedar absent)', async () => {
    iam.setHandlers({ 'serviceAccounts.createServiceAccount': () => storedAccount(OWNER) });
    const calls = callsSince(iam);
    const clients = clientsFor(iam);

    const result = await createServiceAccount({ serviceAccounts: clients.serviceAccounts, authz: clients.authz, cedar: false, logger: { appEvent: vi.fn() } }, { ownerPrn: OWNER, name: 'ci-bot' });

    expect(result).toEqual({ kind: 'created', saPrn: SA, granted: false });
    expect(calls('authz.grantRole')).toHaveLength(0);
  });
});

describe('allowModelCalls (§ 5.3)', () => {
  it('grants gateway_user at the owner that GetServiceAccount names', async () => {
    iam.setHandlers({
      'serviceAccounts.getServiceAccount': () => storedAccount(PROJECT),
      'authz.grantRole': (req) => ({ grant: { id: 'g-2', principalPrn: req.principalPrn, roleKey: req.roleKey, scopePrn: req.scopePrn } }),
    });
    const calls = callsSince(iam);
    const clients = clientsFor(iam);

    expect(await allowModelCalls({ serviceAccounts: clients.serviceAccounts, authz: clients.authz }, { saPrn: SA })).toEqual({ ok: true });
    expect(calls('serviceAccounts.getServiceAccount')[0]?.request).toMatchObject({ prn: SA });
    expect(calls('authz.grantRole')[0]?.request).toMatchObject({ principalPrn: SA, roleKey: 'gateway_user', scopePrn: PROJECT });
  });

  it('makes no grant when IAM refuses to show the account', async () => {
    iam.setHandlers({
      'serviceAccounts.getServiceAccount': () => {
        throw denial();
      },
    });
    const calls = callsSince(iam);
    const clients = clientsFor(iam);

    const result = await allowModelCalls({ serviceAccounts: clients.serviceAccounts, authz: clients.authz }, { saPrn: SA });

    expect(result.ok).toBe(false);
    if (result.ok) throw new Error('expected a refusal');
    expect(result.error.presentation).toBe('forbidden');
    expect(calls('authz.grantRole')).toHaveLength(0);
  });

  it('returns a duplicate grant as IAM answers it: an internal error (spec § 3.2)', async () => {
    iam.setHandlers({
      'serviceAccounts.getServiceAccount': () => storedAccount(OWNER),
      'authz.grantRole': () => {
        throw new ConnectError('duplicate', Code.Internal);
      },
    });
    const clients = clientsFor(iam);

    const result = await allowModelCalls({ serviceAccounts: clients.serviceAccounts, authz: clients.authz }, { saPrn: SA });

    expect(result.ok).toBe(false);
    if (result.ok) throw new Error('expected a refusal');
    expect(result.error.presentation).toBe('generic');
  });
});

describe('issueApiKey (§ 5.4)', () => {
  it.each([
    ['default', null],
    ['30', 30],
    ['90', 90],
    ['365', 365],
  ] as const)('sends expiry %s from the injected clock, the owner scope from IAM, and empty scope lists', async (expiry, days) => {
    iam.setHandlers({
      'serviceAccounts.getServiceAccount': () => storedAccount(PROJECT),
      'serviceAccounts.issueApiKey': (req) => ({ apiKey: { id: 'k-1', serviceAccountPrn: req.serviceAccountPrn, scopePrn: req.scopePrn, prefix: 'pgs_int_0123' }, token: TOKEN }),
    });
    const calls = callsSince(iam);
    const clients = clientsFor(iam);

    const result = await issueApiKey({ serviceAccounts: clients.serviceAccounts, now: () => NOW }, { saPrn: SA, expiry });

    expect(result).toEqual({ ok: true, token: TOKEN, prefix: 'pgs_int_0123' });
    const request = calls('serviceAccounts.issueApiKey')[0]?.request as { expiresAt?: unknown; scopeActions: string[]; scopeRoles: string[] };
    expect(request).toMatchObject({ serviceAccountPrn: SA, scopePrn: PROJECT, scopeActions: [], scopeRoles: [] });
    if (days === null) expect(request.expiresAt).toBeUndefined();
    else expect(request.expiresAt).toMatchObject({ seconds: BigInt(Math.floor((NOW + days * DAY) / 1000)), nanos: 0 });
  });

  it('issues nothing when GetServiceAccount fails', async () => {
    iam.setHandlers({
      'serviceAccounts.getServiceAccount': () => {
        throw denial({ code: Code.NotFound, reason: 'not-found' });
      },
    });
    const calls = callsSince(iam);
    const clients = clientsFor(iam);

    const result = await issueApiKey({ serviceAccounts: clients.serviceAccounts, now: () => NOW }, { saPrn: SA, expiry: '30' });

    expect(result.ok).toBe(false);
    expect(calls('serviceAccounts.issueApiKey')).toHaveLength(0);
  });
});

describe('revokeApiKey and archiveServiceAccount (§ 5.5, § 5.6)', () => {
  it('revokes the key by id', async () => {
    iam.setHandlers({ 'serviceAccounts.revokeApiKey': () => ({}) });
    const calls = callsSince(iam);

    expect(await revokeApiKey({ serviceAccounts: clientsFor(iam).serviceAccounts }, { saPrn: SA, keyId: 'k-1' })).toEqual({ ok: true });
    expect(calls('serviceAccounts.revokeApiKey')[0]?.request).toMatchObject({ id: 'k-1' });
  });

  it('returns IAM’s refusal of a revoke', async () => {
    iam.setHandlers({
      'serviceAccounts.revokeApiKey': () => {
        throw denial();
      },
    });

    const result = await revokeApiKey({ serviceAccounts: clientsFor(iam).serviceAccounts }, { saPrn: SA, keyId: 'k-1' });

    expect(result.ok).toBe(false);
  });

  it('archives the account by PRN', async () => {
    iam.setHandlers({ 'serviceAccounts.archiveServiceAccount': () => ({}) });
    const calls = callsSince(iam);

    expect(await archiveServiceAccount({ serviceAccounts: clientsFor(iam).serviceAccounts }, { saPrn: SA })).toEqual({ ok: true });
    expect(calls('serviceAccounts.archiveServiceAccount')[0]?.request).toMatchObject({ prn: SA });
  });
});
