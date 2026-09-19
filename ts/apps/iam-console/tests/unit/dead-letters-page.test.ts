// SPDX-License-Identifier: Apache-2.0
//
// /iam/dead-letters, branch by branch (SMA-629 spec § 6.2–§ 6.4). The page's own accessors are
// mocked at the lib/console boundary, and the test reads the element tree the page returns. It
// proves: the 404 gate, the degraded view inside the frame with no IAM call, a refused query that
// never becomes an IAM call, the 403/404 list errors as PageError, every other list error as a
// SectionError inside the frame, the frame key, the GET form with no `action`, and the paging hrefs.
import { isValidElement, type ReactElement, type ReactNode } from 'react';
import { Code, ConnectError } from '@connectrpc/connect';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { ServiceState } from '@paigasus/discovery/types';
import type { PaigasusError } from '@paigasus/sdk/errors/types';

const mocks = vi.hoisted(() => ({
  state: { current: { state: 'absent', service: 'iam' } },
  listDeadLetters: vi.fn(),
  iamClients: vi.fn(),
}));

vi.mock('../../lib/console', () => ({
  sessionToken: () => Promise.resolve('tok-page'),
  discovery: () => ({ getServiceState: () => Promise.resolve(mocks.state.current) }),
  iamClients: mocks.iamClients,
  iamClientsForAction: vi.fn(),
}));

const { default: DeadLettersPage } = await import('../../app/(console)/dead-letters/page');
const { PageError } = await import('../../app/_components/page-error');
const { SectionError } = await import('../../app/_components/section-error');
const { DeadLettersFrame } = await import('../../app/(console)/dead-letters/dead-letters-frame');
const { DeadLetterTable } = await import('../../app/(console)/dead-letters/dead-letter-table');
const { PAGE_SIZE } = await import('../../lib/paging');

const ID = '0190a1f0-0000-7000-8000-0000000000d1';
const descriptor = (capabilities: string[]) => ({ service: 'iam', version: '1.0.0', capabilities });
const WITH_KEY: ServiceState = { state: 'available', service: 'iam', descriptor: descriptor(['iam.deadletters']), capabilities: ['iam.deadletters'] };
const WITHOUT_KEY: ServiceState = { state: 'available', service: 'iam', descriptor: descriptor(['iam.audit']), capabilities: ['iam.audit'] };
const DEGRADED: ServiceState = { state: 'degraded', service: 'iam', reason: 'timeout', descriptor: descriptor([]), capabilities: [] };

const entry = {
  id: ID,
  occurredAt: { seconds: 1_788_000_000n, nanos: 0 },
  eventType: 'orders',
  schemaVersion: 1,
  aggregatePrn: 'prn:pgs:iam:::organization/0190a100-0000-7000-8000-00000000000a',
  actorPrn: '',
  payload: '{}',
  correlationId: '',
  attempts: 3,
  parkedAt: { seconds: 1_788_000_060n, nanos: 0 },
  lastError: '',
};

/** Every element in the returned tree, depth first. Components are NOT rendered, only walked. */
function* walk(node: ReactNode): Generator<ReactElement> {
  if (Array.isArray(node)) {
    for (const child of node as ReactNode[]) yield* walk(child);
    return;
  }
  if (!isValidElement(node)) return;
  yield node;
  yield* walk((node.props as { children?: ReactNode }).children);
}

const all = (tree: ReactNode, type: unknown): ReactElement[] => [...walk(tree)].filter((element) => element.type === type);
const byTestId = (tree: ReactNode, id: string): ReactElement[] => [...walk(tree)].filter((element) => (element.props as Record<string, unknown>)['data-testid'] === id);

function visit(query: Record<string, string | string[] | undefined>): Promise<ReactElement> {
  return DeadLettersPage({ searchParams: Promise.resolve(query) });
}

beforeEach(() => {
  mocks.listDeadLetters.mockReset();
  mocks.iamClients.mockReset();
  mocks.iamClients.mockImplementation(() => Promise.resolve({ outbox: { listDeadLetters: mocks.listDeadLetters } }));
});

describe('the gate', () => {
  it.each<[string, ServiceState]>([
    ['absent', { state: 'absent', service: 'iam' }],
    ['available without iam.deadletters (AC 2)', WITHOUT_KEY],
  ])('answers 404 when IAM is %s, and never builds a client', async (_label, state) => {
    mocks.state.current = state;

    await expect(visit({})).rejects.toMatchObject({ digest: 'NEXT_HTTP_ERROR_FALLBACK;404' });
    expect(mocks.iamClients).not.toHaveBeenCalled();
  });

  it('renders the degraded view INSIDE the frame, with no IAM call, whatever the query', async () => {
    mocks.state.current = DEGRADED;

    const tree = await visit({ eventType: 'e'.repeat(201) });

    const [frame] = all(tree, DeadLettersFrame);
    expect(frame).toBeDefined();
    expect(byTestId(frame, 'dead-letters-degraded')).toHaveLength(1);
    expect(mocks.iamClients).not.toHaveBeenCalled();
  });
});

describe('the query', () => {
  it.each([
    ['an event type of 201 characters', { eventType: 'e'.repeat(201) }],
    ['a cursor past the bound', { cursor: 'c'.repeat(1025) }],
  ])('refuses %s as a PageError before any client is built', async (_label, query) => {
    mocks.state.current = WITH_KEY;

    const tree = await visit(query);

    expect(tree.type).toBe(PageError);
    expect((tree.props as { error: PaigasusError }).error.presentation).toBe('invalid-input');
    expect(mocks.iamClients).not.toHaveBeenCalled();
  });
});

describe('the list', () => {
  beforeEach(() => {
    mocks.state.current = WITH_KEY;
  });

  it('sends the filter and the cursor, keys the frame by both, and renders the table, the GET form and the paging links', async () => {
    mocks.listDeadLetters.mockResolvedValue({ entries: [entry], nextCursor: 'c2' });

    const tree = await visit({ eventType: ' orders ', cursor: 'c1' });

    expect(mocks.listDeadLetters).toHaveBeenCalledWith({ eventType: 'orders', cursor: 'c1', limit: PAGE_SIZE });
    const [frame] = all(tree, DeadLettersFrame);
    expect(frame?.key).toBe('orders|c1');
    const [table] = all(tree, DeadLetterTable);
    expect((table?.props as { rows: { id: string }[] }).rows.map((row) => row.id)).toEqual([ID]);

    const [form] = all(tree, 'form');
    expect(form?.props).toMatchObject({ method: 'get' });
    // § 6.3: no `action`, so the form submits to the current URL and stays in the /iam zone.
    expect('action' in (form?.props as object)).toBe(false);

    const hrefs = [...walk(tree)].map((element) => (element.props as { href?: unknown }).href).filter((href) => typeof href === 'string');
    expect(hrefs).toEqual(['/iam/dead-letters?eventType=orders', '/iam/dead-letters?eventType=orders&cursor=c2']);
  });

  it('shows the empty state and no paging links on an empty first page, and keys the frame "|"', async () => {
    mocks.listDeadLetters.mockResolvedValue({ entries: [], nextCursor: '' });

    const tree = await visit({});

    expect(all(tree, DeadLettersFrame)[0]?.key).toBe('|');
    expect(all(tree, DeadLetterTable)).toHaveLength(0);
    expect([...walk(tree)].some((element) => (element.props as { title?: unknown }).title === 'No dead letters')).toBe(true);
    expect([...walk(tree)].filter((element) => typeof (element.props as { href?: unknown }).href === 'string')).toHaveLength(0);
  });

  it.each([
    ['forbidden', Code.PermissionDenied],
    ['not-found', Code.NotFound],
  ])('returns a %s list error as a PageError, so a fresh GET keeps its real status', async (presentation, code) => {
    mocks.listDeadLetters.mockRejectedValue(new ConnectError('nope', code));

    const tree = await visit({});

    expect(tree.type).toBe(PageError);
    expect((tree.props as { error: PaigasusError }).error.presentation).toBe(presentation);
  });

  it('renders every other list error as a SectionError INSIDE the frame', async () => {
    mocks.listDeadLetters.mockRejectedValue(new ConnectError('nope', Code.Unavailable));

    const tree = await visit({});

    const [frame] = all(tree, DeadLettersFrame);
    const [section] = all(frame, SectionError);
    expect((section?.props as { error: PaigasusError }).error.presentation).toBe('degraded');
  });
});
