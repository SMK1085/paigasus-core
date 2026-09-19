// SPDX-License-Identifier: Apache-2.0
//
// The five Server Actions of the gateway settings (SMA-636 spec § 5.1-5.6, § 7.1): what each one
// sends to IAM, which inputs it accepts, what it revalidates, and that an issued token reaches no
// log line (§ 5.4 rule 1). The session read and the discovery read are mocked. Every IAM call still
// goes through the real callIam, so a ConnectError maps to the real presentation.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { Code, ConnectError } from '@connectrpc/connect';
import type { ServiceState } from '@paigasus/discovery/types';
import { denial } from '@paigasus/console-core/testing';
import { resetNextCache, revalidatedPaths } from '../support/next-cache';

const OWNER = 'prn:pgs:iam:::organization/0190a100-0000-7000-8000-00000000000a';
const PROJECT = 'prn:pgs:iam::0190a100-0000-7000-8000-00000000000a:project/0190a1c3-0000-7000-8000-0000000000a1';
const SA = 'prn:pgs:iam:::principal/0190a1e5-0000-7000-8000-0000000000a1';
const TOKEN = 'pgs_secret_0123456789abcdef0123456789abcdef';
const NOW = Date.UTC(2026, 8, 18, 12, 0, 0);
const DAY = 86_400_000;
const LAYOUT = [{ path: '/orgs', type: 'layout' }];

type Handler = (request: Record<string, unknown>) => unknown;

type Fake = {
  session: boolean;
  iam: unknown;
  calls: { method: string; request: Record<string, unknown> }[];
  handlers: Record<string, Handler>;
};

const fake = vi.hoisted<Fake>(() => ({
  session: true,
  iam: null,
  calls: [],
  handlers: {},
}));

vi.mock('../../lib/console', () => {
  const call =
    (method: string) =>
    (request: Record<string, unknown>): Promise<unknown> => {
      fake.calls.push({ method, request });
      const handler = fake.handlers[method];
      if (handler === undefined) throw new Error(`no handler for ${method}`);
      return Promise.resolve(handler(request));
    };
  const serviceAccounts = {
    createServiceAccount: call('serviceAccounts.createServiceAccount'),
    getServiceAccount: call('serviceAccounts.getServiceAccount'),
    issueApiKey: call('serviceAccounts.issueApiKey'),
    revokeApiKey: call('serviceAccounts.revokeApiKey'),
    archiveServiceAccount: call('serviceAccounts.archiveServiceAccount'),
  };
  const authz = { grantRole: call('authz.grantRole') };
  const relogin = {
    presentation: 'relogin',
    domain: null,
    reason: null,
    rawReason: null,
    rawDomain: null,
    message: 'The session has ended.',
    correlationId: null,
    requestId: null,
    retryable: false,
    metadata: {},
    transport: { kind: 'http', status: 401 },
  };
  return {
    iamClientsForAction: () => Promise.resolve(fake.session ? { ok: true, value: { serviceAccounts, authz } } : { ok: false, error: relogin }),
    optionalSession: () => Promise.resolve(fake.session ? { accessToken: 'tok-action' } : null),
    discovery: () => ({ getServiceState: () => Promise.resolve(fake.iam) }),
  };
});

const { allowModelCallsAction, archiveServiceAccountAction, createServiceAccountAction, issueApiKeyAction, revokeApiKeyAction } = await import('../../app/(console)/service-accounts/actions');

function capabilities(list: string[]): ServiceState {
  return { state: 'available', service: 'iam', descriptor: { service: 'iam', version: '1.0.0', capabilities: list }, capabilities: list };
}

function form(fields: Record<string, string>): FormData {
  const data = new FormData();
  for (const [name, value] of Object.entries(fields)) data.set(name, value);
  return data;
}

function requestsTo(method: string): Record<string, unknown>[] {
  return fake.calls.filter((call) => call.method === method).map((call) => call.request);
}

function happy(): Record<string, Handler> {
  return {
    // IAM answers with its canonical, lower-case owner PRN.
    'serviceAccounts.createServiceAccount': (req) => ({ serviceAccount: { prn: SA, ownerPrn: (req['ownerPrn'] as string).toLowerCase(), name: 'ci-bot', status: 'active' } }),
    'serviceAccounts.getServiceAccount': () => ({ serviceAccount: { prn: SA, ownerPrn: PROJECT, name: 'ci-bot', status: 'active' } }),
    'serviceAccounts.issueApiKey': () => ({ apiKey: { id: 'key-1', prefix: 'pgs_secret_01' }, token: TOKEN }),
    'serviceAccounts.revokeApiKey': () => ({}),
    'serviceAccounts.archiveServiceAccount': () => ({}),
    'authz.grantRole': (req) => ({ grant: { id: 'g-1', ...req } }),
  };
}

function failing(method: string, error: Error): Record<string, Handler> {
  return {
    ...happy(),
    [method]: () => {
      throw error;
    },
  };
}

/** Every line written to stdout or stderr from now on. The logger is module-level. */
function captureOutput(): () => string[] {
  const out = vi.spyOn(process.stdout, 'write').mockImplementation(() => true);
  const err = vi.spyOn(process.stderr, 'write').mockImplementation(() => true);
  return () => [...out.mock.calls, ...err.mock.calls].map(([chunk]) => String(chunk));
}

beforeEach(() => {
  fake.session = true;
  fake.iam = capabilities(['iam.authz.cedar', 'iam.apikeys']);
  fake.calls.length = 0;
  fake.handlers = happy();
});

afterEach(() => {
  vi.restoreAllMocks();
  vi.useRealTimers();
});

describe('createServiceAccountAction (§ 5.2)', () => {
  it('creates, grants gateway_user at the owner of IAM’s response, and revalidates', async () => {
    const result = await createServiceAccountAction(null, form({ ownerPrn: OWNER, name: 'ci-bot' }));

    expect(result).toEqual({ kind: 'created', saPrn: SA, granted: true });
    expect(requestsTo('authz.grantRole')).toEqual([{ principalPrn: SA, roleKey: 'gateway_user', scopePrn: OWNER }]);
    expect(revalidatedPaths).toEqual(LAYOUT);
  });

  it('accepts only ownerPrn and name: extra fields change no request (§ 5.1, D6)', async () => {
    await createServiceAccountAction(
      null,
      form({ ownerPrn: OWNER, name: 'ci-bot', scopePrn: PROJECT, roleKey: 'org_admin', principalPrn: 'prn:pgs:iam:::principal/0190a1e5-0000-7000-8000-0000000000ff' }),
    );

    expect(requestsTo('serviceAccounts.createServiceAccount')).toEqual([{ ownerPrn: OWNER, name: 'ci-bot' }]);
    expect(requestsTo('authz.grantRole')).toEqual([{ principalPrn: SA, roleKey: 'gateway_user', scopePrn: OWNER }]);
  });

  it('makes no grant when discovery says IAM offers no role administration (D13)', async () => {
    fake.iam = capabilities(['iam.apikeys']);

    expect(await createServiceAccountAction(null, form({ ownerPrn: OWNER, name: 'ci-bot' }))).toEqual({ kind: 'created', saPrn: SA, granted: false });
    expect(requestsTo('authz.grantRole')).toHaveLength(0);
  });

  it('answers partial, logs gateway.sa.grant_failed, and revalidates when the grant fails', async () => {
    fake.handlers = failing('authz.grantRole', new ConnectError('boom', Code.Internal));
    const lines = captureOutput();

    const result = await createServiceAccountAction(null, form({ ownerPrn: OWNER, name: 'ci-bot' }));

    expect(result).toMatchObject({ kind: 'partial', saPrn: SA, error: { presentation: 'generic' } });
    expect(lines().some((line) => line.includes('"event":"gateway.sa.grant_failed"'))).toBe(true);
    expect(revalidatedPaths).toEqual(LAYOUT);
  });

  it('answers partial and revalidates NOTHING when the grant answers relogin (plan SPEC DEVIATION 6)', async () => {
    fake.handlers = failing('authz.grantRole', new ConnectError('expired', Code.Unauthenticated));
    captureOutput();

    const result = await createServiceAccountAction(null, form({ ownerPrn: OWNER, name: 'ci-bot' }));

    expect(result).toMatchObject({ kind: 'partial', saPrn: SA, error: { presentation: 'relogin' } });
    expect(revalidatedPaths).toEqual([]);
  });

  it.each([
    ['conflict', denial({ code: Code.AlreadyExists, reason: 'service-account-name-conflict' }), true],
    ['forbidden', denial(), true],
    ['degraded', new ConnectError('down', Code.Unavailable), true],
    ['generic', new ConnectError('boom', Code.Internal), true],
    ['invalid-input', denial({ code: Code.InvalidArgument, reason: 'invalid-name' }), false],
    ['relogin', new ConnectError('expired', Code.Unauthenticated), false],
  ] as const)('a failed create with presentation %s revalidates: %s', async (presentation, error, revalidates) => {
    fake.handlers = failing('serviceAccounts.createServiceAccount', error);

    expect(await createServiceAccountAction(null, form({ ownerPrn: OWNER, name: 'ci-bot' }))).toMatchObject({ kind: 'failed', error: { presentation } });
    expect(revalidatedPaths).toEqual(revalidates ? LAYOUT : []);
  });

  it('refuses a name that is only whitespace without calling IAM or revalidating', async () => {
    expect(await createServiceAccountAction(null, form({ ownerPrn: OWNER, name: '   ' }))).toMatchObject({ kind: 'failed', error: { presentation: 'invalid-input' } });
    expect(fake.calls).toHaveLength(0);
    expect(revalidatedPaths).toEqual([]);
  });

  it('answers relogin, calls nothing and revalidates nothing when the session ended', async () => {
    fake.session = false;

    expect(await createServiceAccountAction(null, form({ ownerPrn: OWNER, name: 'ci-bot' }))).toMatchObject({ kind: 'failed', error: { presentation: 'relogin' } });
    expect(fake.calls).toHaveLength(0);
    expect(revalidatedPaths).toEqual([]);
  });
});

describe('allow, revoke and archive (§ 5.3, § 5.5, § 5.6)', () => {
  it('allow: grants at the owner that GetServiceAccount names, never at a form field', async () => {
    expect(await allowModelCallsAction(null, form({ saPrn: SA, scopePrn: OWNER }))).toEqual({ ok: true });

    expect(requestsTo('serviceAccounts.getServiceAccount')).toEqual([{ prn: SA }]);
    expect(requestsTo('authz.grantRole')).toEqual([{ principalPrn: SA, roleKey: 'gateway_user', scopePrn: PROJECT }]);
    expect(revalidatedPaths).toEqual(LAYOUT);
  });

  it.each([
    ['a success', null, true],
    ['a generic error (a duplicate grant)', new ConnectError('duplicate', Code.Internal), true],
    ['a refusal', denial(), true],
    ['relogin', new ConnectError('expired', Code.Unauthenticated), false],
  ] as const)('allow revalidates after %s: %s', async (_label, error, revalidates) => {
    if (error !== null) fake.handlers = failing('authz.grantRole', error);

    await allowModelCallsAction(null, form({ saPrn: SA }));

    expect(revalidatedPaths).toEqual(revalidates ? LAYOUT : []);
  });

  it('revoke: sends the key id, and revalidates after a success and after a refusal', async () => {
    expect(await revokeApiKeyAction(null, form({ saPrn: SA, keyId: 'key-1' }))).toEqual({ ok: true });
    expect(requestsTo('serviceAccounts.revokeApiKey')).toEqual([{ id: 'key-1' }]);
    expect(revalidatedPaths).toEqual(LAYOUT);

    resetNextCache();
    fake.handlers = failing('serviceAccounts.revokeApiKey', denial());
    expect(await revokeApiKeyAction(null, form({ saPrn: SA, keyId: 'key-1' }))).toMatchObject({ ok: false, error: { presentation: 'forbidden' } });
    expect(revalidatedPaths).toEqual(LAYOUT);
  });

  it('archive: sends the account PRN, and revalidates after a success and after a refusal', async () => {
    expect(await archiveServiceAccountAction(null, form({ saPrn: SA }))).toEqual({ ok: true });
    expect(requestsTo('serviceAccounts.archiveServiceAccount')).toEqual([{ prn: SA }]);
    expect(revalidatedPaths).toEqual(LAYOUT);

    resetNextCache();
    fake.handlers = failing('serviceAccounts.archiveServiceAccount', denial());
    expect(await archiveServiceAccountAction(null, form({ saPrn: SA }))).toMatchObject({ ok: false, error: { presentation: 'forbidden' } });
    expect(revalidatedPaths).toEqual(LAYOUT);
  });

  it('refuses a missing key id without calling IAM', async () => {
    expect(await revokeApiKeyAction(null, form({ saPrn: SA }))).toMatchObject({ ok: false, error: { presentation: 'invalid-input' } });
    expect(fake.calls).toHaveLength(0);
  });
});

describe('issueApiKeyAction (§ 5.4)', () => {
  it('scopes the key to the owner from GetServiceAccount; a scopePrn form field changes nothing', async () => {
    vi.useFakeTimers({ toFake: ['Date'] });
    vi.setSystemTime(NOW);

    const result = await issueApiKeyAction(null, form({ saPrn: SA, expiry: '30', scopePrn: OWNER }));

    expect(result).toEqual({ ok: true, token: TOKEN, prefix: 'pgs_secret_01' });
    expect(requestsTo('serviceAccounts.issueApiKey')).toEqual([
      { serviceAccountPrn: SA, scopePrn: PROJECT, expiresAt: { seconds: BigInt(Math.floor((NOW + 30 * DAY) / 1000)), nanos: 0 }, scopeActions: [], scopeRoles: [] },
    ]);
    expect(revalidatedPaths).toEqual(LAYOUT);
  });

  it('sends no expires_at for the IAM default', async () => {
    await issueApiKeyAction(null, form({ saPrn: SA, expiry: 'default' }));
    expect(requestsTo('serviceAccounts.issueApiKey')[0]).not.toHaveProperty('expiresAt');
  });

  it('refuses an expiry outside the four choices without calling IAM', async () => {
    expect(await issueApiKeyAction(null, form({ saPrn: SA, expiry: '7' }))).toMatchObject({ ok: false, error: { presentation: 'invalid-input' } });
    expect(fake.calls).toHaveLength(0);
  });

  it('revalidates nothing after a refusal', async () => {
    fake.handlers = failing('serviceAccounts.issueApiKey', denial());

    expect(await issueApiKeyAction(null, form({ saPrn: SA, expiry: '30' }))).toMatchObject({ ok: false, error: { presentation: 'forbidden' } });
    expect(revalidatedPaths).toEqual([]);
  });

  it('writes the token to no log line, on stdout or on stderr (§ 5.4 rule 1, § 7.1)', async () => {
    const lines = captureOutput();

    // Vacuity: a failed call DOES write a line, so the spies see what the logger writes.
    fake.handlers = failing('authz.grantRole', new ConnectError('boom', Code.Internal));
    await allowModelCallsAction(null, form({ saPrn: SA }));
    fake.handlers = happy();
    expect(await issueApiKeyAction(null, form({ saPrn: SA, expiry: '90' }))).toMatchObject({ ok: true, token: TOKEN });
    // A failed issue after it: its log line must not carry the earlier token either.
    fake.handlers = failing('serviceAccounts.issueApiKey', new ConnectError('boom', Code.Internal));
    await issueApiKeyAction(null, form({ saPrn: SA, expiry: '90' }));

    expect(lines().length).toBeGreaterThan(0);
    expect(lines().filter((line) => line.includes(TOKEN))).toEqual([]);
  });
});
