// SPDX-License-Identifier: Apache-2.0
//
// The service-accounts section loader (SMA-636 spec § 4.2, § 7.1) against the fake IAM. mayI() is
// scripted, so the fake's isAuthorized log holds only the questions about a SERVICE ACCOUNT
// (modelCallState).
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { Code, ConnectError } from '@connectrpc/connect';
import type { ServiceState } from '@paigasus/discovery/types';
import { disposeTransports } from '@paigasus/sdk/iam';
import { ApiKeyStatus } from '@paigasus/sdk/iam/types';
import { organizationPrn, projectPrn, type IamAction } from '@paigasus/console-core';
import { denial, startFakeIam, type FakeIam, type FakeIamHandlers } from '@paigasus/console-core/testing';
import type { NodeLifecycle } from '../../app/(console)/node-status';
import { loadServiceAccountSection, type SectionDeps, type SectionParams } from '../../app/(console)/service-accounts/load';
import { serviceAccountPrn } from '../../app/(console)/service-accounts/service-account-id';
import type { SectionOk, SelectedView } from '../../app/(console)/service-accounts/view';
import { IDS, callsSince, clientsFor, scriptedMayI } from './support';

const OWNER = organizationPrn(IDS.orgA);
const OTHER_OWNER = organizationPrn(IDS.orgB);
const PROJECT = projectPrn(IDS.orgA, IDS.projectA1);
const SA = serviceAccountPrn(IDS.saA);
const NOW = Date.UTC(2026, 8, 18, 12, 0, 0);
const DAY = 86_400_000;
const ACTIVE: NodeLifecycle = { own: 'active', effective: 'active' };
const ALL: Partial<Record<IamAction, boolean>> = { CreateServiceAccount: true, IssueApiKey: true, RevokeApiKey: true, ArchiveServiceAccount: true, GrantRole: true };

const at = (ms: number) => ({ seconds: BigInt(Math.floor(ms / 1000)), nanos: 0 });

function iamState(capabilities: string[]): ServiceState {
  return { state: 'available', service: 'iam', descriptor: { service: 'iam', version: '1.0.0', capabilities }, capabilities };
}
const FULL = iamState(['iam.authz.cedar', 'iam.apikeys']);

function accounts(count: number) {
  return Array.from({ length: count }, (_, index) => ({
    prn: serviceAccountPrn(`0190a1e5-0000-7000-8000-${String(index).padStart(12, '0')}`),
    ownerPrn: OWNER,
    name: `bot-${String(index)}`,
    status: 'active',
    audit: { createdAt: at(NOW) },
  }));
}

/** An account SA owned by `ownerPrn`, keys, and an IsAuthorized that answers `allowed` about SA. */
function selectedWorld(opts: { ownerPrn?: string; status?: string; allowed?: boolean } = {}): FakeIamHandlers {
  return {
    'serviceAccounts.listServiceAccounts': () => ({ serviceAccounts: accounts(1) }),
    'serviceAccounts.getServiceAccount': () => ({ serviceAccount: { prn: SA, ownerPrn: opts.ownerPrn ?? OWNER, name: 'ci-bot', status: opts.status ?? 'active', audit: { createdAt: at(NOW) } } }),
    'authz.isAuthorized': () => ({ allowed: opts.allowed ?? false, determiningPolicies: [], reason: '' }),
    'serviceAccounts.listApiKeys': () => ({
      apiKeys: [
        { id: 'k-active', serviceAccountPrn: SA, scopePrn: OWNER, prefix: 'pgs_a', status: ApiKeyStatus.ACTIVE, expiresAt: at(NOW + DAY), audit: { createdAt: at(NOW - DAY) } },
        { id: 'k-revoked', serviceAccountPrn: SA, scopePrn: OWNER, prefix: 'pgs_r', status: ApiKeyStatus.REVOKED },
        { id: 'k-expired', serviceAccountPrn: SA, scopePrn: OWNER, prefix: 'pgs_e', status: ApiKeyStatus.ACTIVE, expiresAt: at(NOW - DAY) },
        { id: 'k-other', serviceAccountPrn: SA, scopePrn: PROJECT, prefix: 'pgs_o', status: ApiKeyStatus.ACTIVE, lastUsedAt: at(NOW) },
      ],
    }),
  };
}

let iam: FakeIam;

beforeAll(async () => {
  iam = await startFakeIam();
});

afterAll(async () => {
  disposeTransports();
  await iam.close();
});

function deps(overrides: Partial<SectionDeps> = {}): SectionDeps {
  const clients = clientsFor(iam);
  return { serviceAccounts: clients.serviceAccounts, authz: clients.authz, mayI: scriptedMayI(ALL), iam: FULL, now: () => NOW, ...overrides };
}

function params(overrides: Partial<SectionParams> = {}): SectionParams {
  return { ownerPrn: OWNER, lifecycle: ACTIVE, saOffset: 0, keyOffset: 0, sa: null, ...overrides };
}

async function loadOk(d: SectionDeps, p: SectionParams): Promise<SectionOk> {
  const view = await loadServiceAccountSection(d, p);
  if (view.kind !== 'ok') throw new Error(`expected ok, got ${view.kind}`);
  return view;
}

function okSelected(selected: SelectedView): Extract<SelectedView, { kind: 'ok' }> {
  if (selected.kind !== 'ok') throw new Error(`expected a selected account, got ${selected.kind}`);
  return selected;
}

describe('loadServiceAccountSection: the list', () => {
  it('asks for 51 accounts at the owner, asks the five affordances there, and lists the rows', async () => {
    iam.setHandlers({ 'serviceAccounts.listServiceAccounts': () => ({ serviceAccounts: accounts(2) }) });
    const calls = callsSince(iam);
    const mayI = scriptedMayI(ALL);

    const view = await loadOk(deps({ mayI }), params({ saOffset: 50 }));

    expect(calls('serviceAccounts.listServiceAccounts')[0]?.request).toMatchObject({ ownerPrn: OWNER, limit: 51, offset: 50n });
    expect([...mayI.asked].map(([action, prn]) => `${action}@${prn}`).sort()).toEqual(
      ['ArchiveServiceAccount', 'CreateServiceAccount', 'GrantRole', 'IssueApiKey', 'RevokeApiKey'].map((action) => `${action}@${OWNER}`),
    );
    expect(view.rows.map((row) => row.name)).toEqual(['bot-0', 'bot-1']);
    expect(view.rows[0]).toMatchObject({ id: '0190a1e5-0000-7000-8000-000000000000', created: '2026-09-18', active: true });
    expect(view.canCreate).toBe(true);
    expect(view.selected).toEqual({ kind: 'none' });
    expect(calls('serviceAccounts.getServiceAccount')).toHaveLength(0);
    // D14: with no `?sa=`, the loader never fans out per row. `mayI` here is `scriptedMayI`,
    // which asks nothing of the fake, so every IsAuthorized call in this window would be a
    // per-row model-call-state check — the total must be zero.
    expect(calls('serviceAccounts.listApiKeys')).toHaveLength(0);
    expect(calls('authz.isAuthorized')).toHaveLength(0);
  });

  it('shows exactly 50 rows with no next page for 50, and a next page for 51 (limit+1)', async () => {
    iam.setHandlers({ 'serviceAccounts.listServiceAccounts': () => ({ serviceAccounts: accounts(50) }) });
    const fifty = await loadOk(deps(), params());
    expect(fifty.rows).toHaveLength(50);
    expect(fifty.page).toEqual({ offset: 0, nextOffset: null });

    iam.setHandlers({ 'serviceAccounts.listServiceAccounts': () => ({ serviceAccounts: accounts(51) }) });
    const more = await loadOk(deps(), params());
    expect(more.rows).toHaveLength(50);
    expect(more.page).toEqual({ offset: 0, nextOffset: 50 });
  });

  it('answers denied for a forbidden list, so only this section shows the denial', async () => {
    iam.setHandlers({
      'serviceAccounts.listServiceAccounts': () => {
        throw denial();
      },
    });
    expect(await loadServiceAccountSection(deps(), params())).toEqual({ kind: 'denied' });
  });

  it('answers the section error, with a correlation id, for any other failure', async () => {
    iam.setHandlers({
      'serviceAccounts.listServiceAccounts': () => {
        throw new ConnectError('down', Code.Unavailable);
      },
    });
    const view = await loadServiceAccountSection(deps(), params());
    expect(view.kind).toBe('error');
    if (view.kind !== 'error') throw new Error('expected error');
    expect(view.error.presentation).toBe('degraded');
    expect(view.error.correlationId).not.toBeNull();
  });

  it.each([
    [{ own: 'archived', effective: 'archived' }, 'archived'],
    [{ own: 'active', effective: 'archived' }, 'archived-parent'],
    [{ own: 'unknown', effective: 'active' }, 'unknown'],
  ] as const)('is read-only for owner lifecycle %j (%s): no create, and no panel control', async (lifecycle, readOnly) => {
    iam.setHandlers(selectedWorld());
    const view = await loadOk(deps(), params({ lifecycle, sa: IDS.saA }));
    expect(view.readOnly).toBe(readOnly);
    expect(view.canCreate).toBe(false);
    expect(okSelected(view.selected).controls).toEqual({ allow: false, issue: false, revoke: false, archive: false });
  });

  it('hides create when mayI(CreateServiceAccount) is false', async () => {
    iam.setHandlers({ 'serviceAccounts.listServiceAccounts': () => ({ serviceAccounts: [] }) });
    const view = await loadOk(deps({ mayI: scriptedMayI({ ...ALL, CreateServiceAccount: false }) }), params());
    expect(view.canCreate).toBe(false);
  });
});

describe('loadServiceAccountSection: the selected account (D14)', () => {
  it('asks IsAuthorized about the account, lists 51 keys, and maps every key status (§ 5.7)', async () => {
    iam.setHandlers(selectedWorld({ allowed: false }));
    const calls = callsSince(iam);

    const view = await loadOk(deps(), params({ sa: IDS.saA, keyOffset: 50 }));
    const selected = okSelected(view.selected);

    expect(calls('authz.isAuthorized').map((call) => call.request)).toEqual([expect.objectContaining({ principalPrn: SA, action: 'InvokeModel', resourcePrn: OWNER })]);
    expect(calls('serviceAccounts.listApiKeys')[0]?.request).toMatchObject({ serviceAccountPrn: SA, limit: 51, offset: 50n });
    expect(selected.modelCalls).toBe('no');
    expect(selected.keys).toEqual({
      kind: 'ok',
      page: { offset: 50, nextOffset: null },
      rows: [
        { id: 'k-active', prefix: 'pgs_a', status: 'active', created: '2026-09-17', expires: '2026-09-19', lastUsed: null, otherScope: null },
        { id: 'k-revoked', prefix: 'pgs_r', status: 'revoked', created: null, expires: null, lastUsed: null, otherScope: null },
        { id: 'k-expired', prefix: 'pgs_e', status: 'expired', created: null, expires: '2026-09-17', lastUsed: null, otherScope: null },
        { id: 'k-other', prefix: 'pgs_o', status: 'active', created: null, expires: null, lastUsed: '2026-09-18', otherScope: PROJECT },
      ],
    });
    expect(selected.controls).toEqual({ allow: true, issue: true, revoke: true, archive: true });
  });

  it('reads an archived account as archived, asks IAM nothing about it, and shows every key inactive', async () => {
    iam.setHandlers(selectedWorld({ status: 'disabled', allowed: true }));
    const calls = callsSince(iam);

    const selected = okSelected((await loadOk(deps(), params({ sa: IDS.saA }))).selected);

    expect(selected.modelCalls).toBe('archived');
    expect(calls('authz.isAuthorized')).toHaveLength(0);
    expect(selected.keys.kind === 'ok' ? selected.keys.rows.map((row) => row.status) : []).toEqual(['inactive', 'inactive', 'inactive', 'inactive']);
    expect(selected.controls).toEqual({ allow: false, issue: false, revoke: false, archive: false });
  });

  it('offers no "Allow model calls" when the account can already call models', async () => {
    iam.setHandlers(selectedWorld({ allowed: true }));
    const selected = okSelected((await loadOk(deps(), params({ sa: IDS.saA }))).selected);
    expect(selected.modelCalls).toBe('yes');
    expect(selected.controls.allow).toBe(false);
  });

  it('makes no ListApiKeys call and shows no key control when iam.apikeys is absent (D13)', async () => {
    iam.setHandlers(selectedWorld());
    const calls = callsSince(iam);

    const selected = okSelected((await loadOk(deps({ iam: iamState(['iam.authz.cedar']) }), params({ sa: IDS.saA }))).selected);

    expect(calls('serviceAccounts.listApiKeys')).toHaveLength(0);
    expect(selected.keys).toEqual({ kind: 'hidden' });
    expect(selected.controls).toMatchObject({ issue: false, revoke: false, archive: true });
  });

  it('offers no "Allow model calls" when iam.authz.cedar is absent (D13)', async () => {
    iam.setHandlers(selectedWorld({ allowed: false }));
    const selected = okSelected((await loadOk(deps({ iam: iamState(['iam.apikeys']) }), params({ sa: IDS.saA }))).selected);
    expect(selected.modelCalls).toBe('no');
    expect(selected.controls.allow).toBe(false);
  });

  it('counts a degraded IAM as offering neither capability, whatever its last descriptor said (D13)', async () => {
    iam.setHandlers(selectedWorld({ allowed: false }));
    const calls = callsSince(iam);
    const degraded: ServiceState = { state: 'degraded', service: 'iam', reason: 'timeout', descriptor: null, capabilities: ['iam.authz.cedar', 'iam.apikeys'] };

    const selected = okSelected((await loadOk(deps({ iam: degraded }), params({ sa: IDS.saA }))).selected);

    expect(calls('serviceAccounts.listApiKeys')).toHaveLength(0);
    expect(selected.controls).toMatchObject({ allow: false, issue: false, revoke: false });
  });

  it('answers other-scope for an account of another owner, and asks nothing more about it', async () => {
    iam.setHandlers(selectedWorld({ ownerPrn: OTHER_OWNER }));
    const calls = callsSince(iam);

    const view = await loadOk(deps(), params({ sa: IDS.saA }));

    expect(view.selected).toEqual({ kind: 'other-scope' });
    expect(calls('authz.isAuthorized')).toHaveLength(0);
    expect(calls('serviceAccounts.listApiKeys')).toHaveLength(0);
  });

  it('answers the panel error when GetServiceAccount fails, and the keys error when ListApiKeys fails', async () => {
    iam.setHandlers({
      ...selectedWorld(),
      'serviceAccounts.getServiceAccount': () => {
        throw denial();
      },
    });
    expect((await loadOk(deps(), params({ sa: IDS.saA }))).selected.kind).toBe('error');

    iam.setHandlers({
      ...selectedWorld(),
      'serviceAccounts.listApiKeys': () => {
        throw new ConnectError('down', Code.Unavailable);
      },
    });
    const selected = okSelected((await loadOk(deps(), params({ sa: IDS.saA }))).selected);
    expect(selected.keys.kind).toBe('error');
  });
});
