// SPDX-License-Identifier: Apache-2.0
//
// The create-organization command (spec § 5.3) against the fake IAM. A command takes NO mayI, so
// it cannot pre-judge: the "direction 1" case sends what a hidden button would send, and the call
// reaches IAM. The other half of the rule, that no actions.ts names mayI, lives in
// tests/unit/actions-structure.test.ts.
import { Code } from '@connectrpc/connect';
import { ErrorReason } from '@paigasus/sdk/errors/types';
import { disposeTransports } from '@paigasus/sdk/iam';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { createOrganization, createOrganizationForm } from '../../app/(console)/orgs/commands';
import { denial, startFakeIam, type FakeIam } from '../support/fake-iam';
import { callsSince, clientsFor } from './support';

let iam: FakeIam;

beforeAll(async () => {
  iam = await startFakeIam();
});

afterAll(async () => {
  disposeTransports();
  await iam.close();
});

const created = (req: { slug: string; name: string }) => ({ organization: { prn: 'prn:pgs:iam:::organization/0190a100-0000-7000-8000-0000000000f1', slug: req.slug, name: req.name } });

describe('createOrganizationForm', () => {
  it('trims the fields and refuses an empty, missing or non-text one', () => {
    expect(createOrganizationForm.safeParse({ slug: ' acme ', name: 'Acme' }).data).toEqual({ slug: 'acme', name: 'Acme' });
    expect(createOrganizationForm.safeParse({ slug: '', name: 'Acme' }).success).toBe(false);
    expect(createOrganizationForm.safeParse({ slug: 'acme', name: null }).success).toBe(false);
    expect(createOrganizationForm.safeParse({ slug: new File(['x'], 'x.txt'), name: 'Acme' }).success).toBe(false);
  });
});

describe('createOrganization', () => {
  it('sends the slug and the name and returns ok', async () => {
    iam.setHandlers({ 'tenancy.createOrganization': created });
    const calls = callsSince(iam);

    const result = await createOrganization({ tenancy: clientsFor(iam).tenancy }, { slug: 'acme', name: 'Acme' });

    expect(result).toEqual({ ok: true });
    expect(calls('tenancy.createOrganization').map((call) => call.request)).toEqual([expect.objectContaining({ slug: 'acme', name: 'Acme' })]);
  });

  it('is not pre-judged: the command takes no mayI, so a hidden button does not stop the call (AC 2, direction 1)', async () => {
    iam.setHandlers({ 'tenancy.createOrganization': created });
    const calls = callsSince(iam);

    const result = await createOrganization({ tenancy: clientsFor(iam).tenancy }, { slug: 'hidden', name: 'Hidden' });

    expect(result).toEqual({ ok: true });
    expect(calls('tenancy.createOrganization')).toHaveLength(1);
  });

  it("returns IAM's 403 as a form error with the correlation id", async () => {
    iam.setHandlers({
      'tenancy.createOrganization': () => {
        throw denial({ correlationId: 'corr-create-org' });
      },
    });

    const result = await createOrganization({ tenancy: clientsFor(iam).tenancy }, { slug: 'acme', name: 'Acme' });

    if (result.ok) throw new Error('expected a denial');
    expect(result.error.presentation).toBe('forbidden');
    expect(result.error.correlationId).toBe('corr-create-org');
  });

  it('returns a slug conflict with its reason, so the form can show the conflict copy', async () => {
    iam.setHandlers({
      'tenancy.createOrganization': () => {
        throw denial({ code: Code.AlreadyExists, reason: 'slug-conflict', correlationId: 'corr-slug' });
      },
    });

    const result = await createOrganization({ tenancy: clientsFor(iam).tenancy }, { slug: 'taken', name: 'Taken' });

    if (result.ok) throw new Error('expected a conflict');
    expect(result.error.presentation).toBe('conflict');
    expect(result.error.reason).toBe(ErrorReason.SLUG_CONFLICT);
  });
});
