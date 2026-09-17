// SPDX-License-Identifier: Apache-2.0
//
// The nine lifecycle commands (SMA-630 spec § 4.3, § 9.1) against the fake IAM. One table-driven
// suite runs for each node kind. For each command it proves the request that reaches IAM, a success,
// and a 403. For a rename it also proves D6 (only the changed fields go to IAM, compared on trimmed
// values), that an empty rename still calls IAM and returns IAM's nothing-to-rename, and that the
// reasons the form copy knows survive. A command takes no mayI, so nothing here scripts IsAuthorized.
import { Code } from '@connectrpc/connect';
import { ErrorReason, type Presentation } from '@paigasus/sdk/errors/types';
import { disposeTransports } from '@paigasus/sdk/iam';
import { NodeStatus } from '@paigasus/sdk/iam/types';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { organizationPrn, projectPrn, teamPrn } from '@paigasus/console-core';
import { denial, startFakeIam, type FakeIam, type FakeIamHandlers, type FakeIamMethod } from '@paigasus/console-core/testing';
import { archiveOrganization, archiveOrganizationForm, renameOrganization, renameOrganizationForm, restoreOrganization, restoreOrganizationForm } from '../../app/(console)/orgs/[org]/commands';
import { archiveTeam, renameTeam, restoreTeam } from '../../app/(console)/orgs/[org]/teams/[team]/commands';
import { archiveProject, renameProject, restoreProject } from '../../app/(console)/orgs/[org]/teams/[team]/projects/[project]/commands';
import { renameChange, type ActionResult } from '../../lib/form';
import { IDS, callsSince, clientsFor } from './support';

let iam: FakeIam;

beforeAll(async () => {
  iam = await startFakeIam();
});

afterAll(async () => {
  disposeTransports();
  await iam.close();
});

const ORG_A = organizationPrn(IDS.orgA);
const TEAM_A1 = teamPrn(IDS.orgA, IDS.teamA1);
const PROJECT_A1 = projectPrn(IDS.orgA, IDS.projectA1);
const SLUG = 'current-slug';
const NAME = 'Current Name';

type Tenancy = ReturnType<typeof clientsFor>['tenancy'];
type Deps = { readonly tenancy: Tenancy };
type RenameInput = { readonly prn: string; readonly slug: string; readonly name: string; readonly currentSlug: string; readonly currentName: string };
type RenameRequest = { readonly prn: string; readonly newSlug?: string | undefined; readonly newName?: string | undefined };

/** How the scripted handlers answer: with the node, or by throwing `fail`. */
type Script = { readonly fail?: Error };

type LifecycleSuite = {
  readonly node: 'organization' | 'team' | 'project';
  readonly prn: string;
  readonly methods: { readonly rename: FakeIamMethod; readonly archive: FakeIamMethod; readonly restore: FakeIamMethod };
  readonly handlers: (script: Script) => FakeIamHandlers;
  readonly rename: (deps: Deps, input: RenameInput) => Promise<ActionResult>;
  readonly archive: (deps: Deps, input: { readonly prn: string }) => Promise<ActionResult>;
  readonly restore: (deps: Deps, input: { readonly prn: string }) => Promise<ActionResult>;
};

/** The node fields every answer carries. A rename answer shows the new values, as IAM does. */
function nodeFields(status: NodeStatus, change: { readonly newSlug?: string | undefined; readonly newName?: string | undefined } = {}) {
  return { slug: change.newSlug ?? SLUG, name: change.newName ?? NAME, status, effectiveStatus: status };
}

function answer(script: Script): void {
  if (script.fail !== undefined) throw script.fail;
}

/** IAM refuses a rename that carries neither field (pg_teams.rs:198-214 and the equivalents). */
function answerRename(script: Script, request: { readonly newSlug?: string | undefined; readonly newName?: string | undefined }): void {
  answer(script);
  if (request.newSlug === undefined && request.newName === undefined) throw denial({ code: Code.InvalidArgument, reason: 'nothing-to-rename' });
}

type RenameCase = { readonly label: string; readonly change: { readonly slug?: string; readonly name?: string }; readonly sent: { readonly newSlug?: string; readonly newName?: string } };

const RENAME_CASES: readonly RenameCase[] = [
  { label: 'only the slug changed: sends newSlug and no newName', change: { slug: 'new-slug' }, sent: { newSlug: 'new-slug' } },
  { label: 'only the name changed: sends newName and no newSlug', change: { name: 'New Name' }, sent: { newName: 'New Name' } },
  { label: 'both changed: sends both', change: { slug: 'new-slug', name: 'New Name' }, sent: { newSlug: 'new-slug', newName: 'New Name' } },
];

type RefusalCase = { readonly code: Code; readonly reason: string; readonly presentation: Presentation; readonly expected: ErrorReason };

const REFUSALS: readonly RefusalCase[] = [
  { code: Code.PermissionDenied, reason: 'forbidden', presentation: 'forbidden', expected: ErrorReason.FORBIDDEN },
  { code: Code.AlreadyExists, reason: 'slug-conflict', presentation: 'conflict', expected: ErrorReason.SLUG_CONFLICT },
  { code: Code.InvalidArgument, reason: 'invalid-slug', presentation: 'invalid-input', expected: ErrorReason.INVALID_SLUG },
  { code: Code.FailedPrecondition, reason: 'node-archived', presentation: 'conflict', expected: ErrorReason.NODE_ARCHIVED },
];

function lifecycleSuite(suite: LifecycleSuite): void {
  const deps = (): Deps => ({ tenancy: clientsFor(iam).tenancy });
  const input = (change: { readonly slug?: string; readonly name?: string } = {}): RenameInput => ({
    prn: suite.prn,
    slug: change.slug ?? SLUG,
    name: change.name ?? NAME,
    currentSlug: SLUG,
    currentName: NAME,
  });

  describe(`rename ${suite.node}`, () => {
    it.each(RENAME_CASES)('$label', async ({ change, sent }) => {
      iam.setHandlers(suite.handlers({}));
      const calls = callsSince(iam);

      expect(await suite.rename(deps(), input(change))).toEqual({ ok: true });

      const requests = calls(suite.methods.rename).map((call) => call.request as RenameRequest);
      expect(requests).toHaveLength(1);
      expect(requests[0]?.prn).toBe(suite.prn);
      expect(requests[0]?.newSlug).toBe(sent.newSlug);
      expect(requests[0]?.newName).toBe(sent.newName);
    });

    it("still calls IAM when nothing changed, with neither field, and returns IAM's nothing-to-rename", async () => {
      iam.setHandlers(suite.handlers({}));
      const calls = callsSince(iam);

      const result = await suite.rename(deps(), input());

      if (result.ok) throw new Error('expected nothing-to-rename');
      expect(result.error.presentation).toBe('invalid-input');
      expect(result.error.reason).toBe(ErrorReason.NOTHING_TO_RENAME);
      const requests = calls(suite.methods.rename).map((call) => call.request as RenameRequest);
      expect(requests).toHaveLength(1);
      expect(requests[0]?.newSlug).toBeUndefined();
      expect(requests[0]?.newName).toBeUndefined();
    });

    it('compares trimmed values: padded current values are no change', async () => {
      iam.setHandlers(suite.handlers({}));
      const calls = callsSince(iam);

      const result = await suite.rename(deps(), input({ slug: ` ${SLUG} `, name: ` ${NAME} ` }));

      expect(result).toMatchObject({ ok: false, error: { reason: ErrorReason.NOTHING_TO_RENAME } });
      const requests = calls(suite.methods.rename).map((call) => call.request as RenameRequest);
      expect(requests[0]?.newSlug).toBeUndefined();
      expect(requests[0]?.newName).toBeUndefined();
    });

    it.each(REFUSALS)('keeps the reason $reason ($presentation)', async ({ code, reason, presentation, expected }) => {
      iam.setHandlers(suite.handlers({ fail: denial({ code, reason, correlationId: `corr-${suite.node}-${reason}` }) }));

      const result = await suite.rename(deps(), input({ slug: 'new-slug' }));

      if (result.ok) throw new Error(`expected ${reason}`);
      expect(result.error.presentation).toBe(presentation);
      expect(result.error.reason).toBe(expected);
      expect(result.error.correlationId).toBe(`corr-${suite.node}-${reason}`);
    });
  });

  describe.each([
    ['archive', suite.archive, suite.methods.archive],
    ['restore', suite.restore, suite.methods.restore],
  ] as const)(`%s ${suite.node}`, (_verb, command, method) => {
    it('sends only the node PRN and returns ok', async () => {
      iam.setHandlers(suite.handlers({}));
      const calls = callsSince(iam);

      expect(await command(deps(), { prn: suite.prn })).toEqual({ ok: true });

      expect(calls(method).map((call) => call.request)).toEqual([expect.objectContaining({ prn: suite.prn })]);
    });

    it("returns IAM's 403 with the correlation id", async () => {
      iam.setHandlers(suite.handlers({ fail: denial({ correlationId: `corr-${suite.node}-lifecycle` }) }));

      const result = await command(deps(), { prn: suite.prn });

      if (result.ok) throw new Error('expected a denial');
      expect(result.error.presentation).toBe('forbidden');
      expect(result.error.reason).toBe(ErrorReason.FORBIDDEN);
      expect(result.error.correlationId).toBe(`corr-${suite.node}-lifecycle`);
    });
  });
}

lifecycleSuite({
  node: 'organization',
  prn: ORG_A,
  methods: { rename: 'tenancy.renameOrganization', archive: 'tenancy.archiveOrganization', restore: 'tenancy.restoreOrganization' },
  handlers: (script) => ({
    'tenancy.renameOrganization': (req) => {
      answerRename(script, req);
      return { organization: { prn: req.prn, ...nodeFields(NodeStatus.ACTIVE, req) } };
    },
    'tenancy.archiveOrganization': (req) => {
      answer(script);
      return { organization: { prn: req.prn, ...nodeFields(NodeStatus.ARCHIVED) } };
    },
    'tenancy.restoreOrganization': (req) => {
      answer(script);
      return { organization: { prn: req.prn, ...nodeFields(NodeStatus.ACTIVE) } };
    },
  }),
  rename: renameOrganization,
  archive: archiveOrganization,
  restore: restoreOrganization,
});

lifecycleSuite({
  node: 'team',
  prn: TEAM_A1,
  methods: { rename: 'tenancy.renameTeam', archive: 'tenancy.archiveTeam', restore: 'tenancy.restoreTeam' },
  handlers: (script) => ({
    'tenancy.renameTeam': (req) => {
      answerRename(script, req);
      return { team: { prn: req.prn, orgPrn: ORG_A, ...nodeFields(NodeStatus.ACTIVE, req) } };
    },
    'tenancy.archiveTeam': (req) => {
      answer(script);
      return { team: { prn: req.prn, orgPrn: ORG_A, ...nodeFields(NodeStatus.ARCHIVED) } };
    },
    'tenancy.restoreTeam': (req) => {
      answer(script);
      return { team: { prn: req.prn, orgPrn: ORG_A, ...nodeFields(NodeStatus.ACTIVE) } };
    },
  }),
  rename: renameTeam,
  archive: archiveTeam,
  restore: restoreTeam,
});

lifecycleSuite({
  node: 'project',
  prn: PROJECT_A1,
  methods: { rename: 'tenancy.renameProject', archive: 'tenancy.archiveProject', restore: 'tenancy.restoreProject' },
  handlers: (script) => ({
    'tenancy.renameProject': (req) => {
      answerRename(script, req);
      return { project: { prn: req.prn, teamPrn: TEAM_A1, orgPrn: ORG_A, ...nodeFields(NodeStatus.ACTIVE, req) } };
    },
    'tenancy.archiveProject': (req) => {
      answer(script);
      return { project: { prn: req.prn, teamPrn: TEAM_A1, orgPrn: ORG_A, ...nodeFields(NodeStatus.ARCHIVED) } };
    },
    'tenancy.restoreProject': (req) => {
      answer(script);
      return { project: { prn: req.prn, teamPrn: TEAM_A1, orgPrn: ORG_A, ...nodeFields(NodeStatus.ACTIVE) } };
    },
  }),
  rename: renameProject,
  archive: archiveProject,
  restore: restoreProject,
});

describe('the organization lifecycle forms', () => {
  it('trim every field, allow an empty current value, and refuse an empty PRN', () => {
    expect(renameOrganizationForm.safeParse({ prn: ` ${ORG_A} `, slug: ' acme ', name: ' Acme ', currentSlug: ' acme ', currentName: '' }).data).toEqual({
      prn: ORG_A,
      slug: 'acme',
      name: 'Acme',
      currentSlug: 'acme',
      currentName: '',
    });
    expect(renameOrganizationForm.safeParse({ prn: ORG_A, slug: 'acme', name: 'Acme', currentSlug: 'acme', currentName: null }).success).toBe(false);
    expect(archiveOrganizationForm.safeParse({ prn: ' ' }).success).toBe(false);
    expect(restoreOrganizationForm.safeParse({ prn: ORG_A }).data).toEqual({ prn: ORG_A });
  });

  // SMA-630 CR round 1, spec § 4.2: the name bound applies only when the trimmed name changed, so a
  // slug-only rename of a node with an overlong stored name (IAM does not validate a rename name,
  // spec F11) still works, and sends newSlug only.
  it('accepts a slug-only rename of a node whose stored name exceeds the name bound', () => {
    const longName = 'a'.repeat(300);
    const parsed = renameOrganizationForm.safeParse({ prn: ORG_A, slug: 'new-slug', name: longName, currentSlug: SLUG, currentName: longName });
    expect(parsed.success).toBe(true);
    if (!parsed.success) return;
    expect(renameChange(parsed.data)).toEqual({ newSlug: 'new-slug' });
  });
});
