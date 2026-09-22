// SPDX-License-Identifier: Apache-2.0
//
// SMA-629 AC 1 and AC 2 (spec § 7.4), and SMA-661 AC 3 and AC 4 (spec § 7.5, row R20).
// PAIGASUS_DISCOVERY_*_MS are 1/2/3 ms in the harness, so each page load probes the fake's GET
// /v1/service-info again, and a test changes the IAM state between two loads. Discard and bulk
// replay need JavaScript, so every page.goto before a click waits for hydration.
import { DEAD_LETTER_A_ID, DEAD_LETTER_B_ID, DEAD_LETTER_C, DEAD_LETTER_C_ID, DEAD_LETTERS_DESCRIPTOR, deadLetterHandlers, seededDeadLetters } from './support/world';
import { signIn, waitForHydration } from './support/login';
import { expect, test } from './support/harness';

/** R20's window start, typed in a D6 spelling: a space, no seconds, a lower-case z (SMA-661 spec § 4.1). */
const WINDOW_FROM_TYPED = '2026-08-29 10:40z';
const WINDOW_FROM_CANONICAL = '2026-08-29T10:40:00.000Z';
/** 2026-08-29T10:40:00Z. A (10:45) and B (10:55) are inside the window; C (2026-08-17) is not. */
const WINDOW_FROM_SECONDS = 1_788_000_000n;

test('R17: iam.deadletters reported and the user is Root, so the page lists, replays and discards (SMA-629 AC 1)', async ({ page, harness }) => {
  harness.useWorld({ descriptor: DEAD_LETTERS_DESCRIPTOR });
  await signIn(page, harness);
  const nav = page.getByRole('navigation', { name: 'Primary' });
  await expect(nav.getByRole('link', { name: 'Dead letters', exact: true })).toBeVisible();

  await page.goto(harness.url('/iam/dead-letters'));
  await waitForHydration(page);
  const rows = page.getByTestId('dead-letter-row');
  const region = page.getByTestId('dead-letters-result');
  await expect(rows).toHaveCount(2);

  const replaysBefore = harness.iam.callsTo('outbox.replayDeadLetter').length;
  await page.getByTestId(`dead-letter-controls-${DEAD_LETTER_A_ID}`).getByRole('button', { name: 'Replay' }).click();
  await expect(region).toContainText(`Replayed event ${DEAD_LETTER_A_ID}.`);
  await expect(rows).toHaveCount(1);
  const replays = harness.iam.callsTo('outbox.replayDeadLetter').slice(replaysBefore);
  expect(replays).toHaveLength(1);
  expect(replays[0]?.request).toMatchObject({ id: DEAD_LETTER_A_ID });

  const discardsBefore = harness.iam.callsTo('outbox.discardDeadLetter').length;
  const controlsB = page.getByTestId(`dead-letter-controls-${DEAD_LETTER_B_ID}`);
  await controlsB.getByRole('button', { name: 'Discard' }).click();
  await expect(controlsB.getByText(`Discard event ${DEAD_LETTER_B_ID}?`, { exact: false })).toBeVisible();
  expect(harness.iam.callsTo('outbox.discardDeadLetter').length).toBe(discardsBefore);
  await controlsB.getByRole('button', { name: 'Confirm discard' }).click();

  await expect(region).toContainText(`Discarded event ${DEAD_LETTER_B_ID}.`);
  await expect(rows).toHaveCount(0);
  await expect(page.getByText('No dead letters')).toBeVisible();
  // The frame outlived the swap from the table to the empty state, so the result is still there.
  await expect(region).toContainText(`Discarded event ${DEAD_LETTER_B_ID}.`);
  const discards = harness.iam.callsTo('outbox.discardDeadLetter').slice(discardsBefore);
  expect(discards).toHaveLength(1);
  expect(discards[0]?.request).toMatchObject({ id: DEAD_LETTER_B_ID });
});

test('R18: iam.deadletters not reported, so the entry is absent and /iam/dead-letters is a 404 (SMA-629 AC 2)', async ({ page, harness }) => {
  // The default world: DEFAULT_DESCRIPTOR has no iam.deadletters, an older IAM.
  await signIn(page, harness);
  const nav = page.getByRole('navigation', { name: 'Primary' });

  await expect(nav.getByRole('link', { name: 'Organizations', exact: true })).toBeVisible();
  await expect(nav.getByRole('link', { name: 'Dead letters', exact: true })).toHaveCount(0);
  const before = harness.iam.callsTo('outbox.listDeadLetters').length;
  const response = await page.goto(harness.url('/iam/dead-letters'));
  expect(response?.status()).toBe(404);
  expect(harness.iam.callsTo('outbox.listDeadLetters').length).toBe(before);
});

test('R19: IAM degraded, so the entry is disabled with a reason and /iam/dead-letters shows the degraded view (SMA-629 § 8)', async ({ page, harness }) => {
  await signIn(page, harness);
  harness.useWorld({ descriptor: { status: 503 } });

  await page.goto(harness.url('/iam/orgs'));

  const nav = page.getByRole('navigation', { name: 'Primary' });
  const entry = nav.getByRole('link', { name: 'Dead letters', exact: true });
  await expect(entry).toHaveAttribute('aria-disabled', 'true');
  const reasonId = await entry.getAttribute('aria-describedby');
  expect(reasonId).toBeTruthy();
  await expect(page.locator(`[id="${String(reasonId)}"]`)).toHaveText(/\S/);

  const before = harness.iam.callsTo('outbox.listDeadLetters').length;
  const response = await page.goto(harness.url('/iam/dead-letters'));
  expect(response?.status()).toBe(200);
  await expect(page.getByTestId('dead-letters-degraded')).toBeVisible();
  // A degraded IAM never reaches ListDeadLetters.
  expect(harness.iam.callsTo('outbox.listDeadLetters').length).toBe(before);
});

test('R20: a parked-time filter reaches IAM and survives paging, and one bulk replay runs against the fake IAM (SMA-661 AC 3, AC 4)', async ({ page, harness }) => {
  // Three entries and one row per page, through `overrides`, so R17's two-entry world stays as it is.
  harness.useWorld({ descriptor: DEAD_LETTERS_DESCRIPTOR, overrides: deadLetterHandlers(seededDeadLetters([DEAD_LETTER_C]), 1) });
  await signIn(page, harness);

  await page.goto(harness.url('/iam/dead-letters'));
  await waitForHydration(page);
  const rows = page.getByTestId('dead-letter-row');
  const region = page.getByTestId('dead-letters-result');
  await expect(rows).toHaveCount(1);
  await expect(rows.first()).toHaveAttribute('data-id', DEAD_LETTER_C_ID);

  // AC 3, part 1: the typed spelling reaches IAM as the canonical instant, and C leaves the list.
  await page.getByLabel('Parked from').fill(WINDOW_FROM_TYPED);
  await page.getByRole('button', { name: 'Filter' }).click();
  await expect.poll(() => new URL(page.url()).searchParams.get('parkedFrom')).toBe(WINDOW_FROM_TYPED);
  await waitForHydration(page);
  await expect(rows.first()).toHaveAttribute('data-id', DEAD_LETTER_B_ID);
  await expect(rows).toHaveCount(1);
  const firstPage = harness.iam.callsTo('outbox.listDeadLetters').at(-1)?.request;
  expect(firstPage).toMatchObject({ cursor: '', parkedFrom: { seconds: WINDOW_FROM_SECONDS, nanos: 0 } });
  expect((firstPage as { parkedTo?: unknown }).parkedTo).toBeUndefined();

  // AC 3, part 2: the Next link carries the CANONICAL bound, and the input converges on it.
  await page.getByRole('navigation', { name: 'Dead-letter pages' }).getByRole('link', { name: 'Next' }).click();
  await expect.poll(() => new URL(page.url()).searchParams.get('cursor')).toBe(DEAD_LETTER_B_ID);
  // A client navigation keeps the attribute, so this returns at once; a full load waits here.
  await waitForHydration(page);
  expect(new URL(page.url()).searchParams.get('parkedFrom')).toBe(WINDOW_FROM_CANONICAL);
  await expect(rows.first()).toHaveAttribute('data-id', DEAD_LETTER_A_ID);
  await expect(page.getByLabel('Parked from')).toHaveValue(WINDOW_FROM_CANONICAL);
  expect(harness.iam.callsTo('outbox.listDeadLetters').at(-1)?.request).toMatchObject({ cursor: DEAD_LETTER_B_ID, parkedFrom: { seconds: WINDOW_FROM_SECONDS, nanos: 0 } });

  // AC 4: one bulk replay of the filtered scope. The confirmation names the budget before any call.
  const bulkBefore = harness.iam.callsTo('outbox.bulkReplayDeadLetters').length;
  const bulk = page.getByRole('form', { name: 'Bulk replay' });
  await bulk.getByLabel('Max rows').fill('5');
  await bulk.getByRole('button', { name: 'Bulk replay…' }).click();
  await expect(bulk.getByText('Replay up to 5 parked events.', { exact: false })).toBeVisible();
  expect(harness.iam.callsTo('outbox.bulkReplayDeadLetters').length).toBe(bulkBefore);
  await bulk.getByRole('button', { name: 'Confirm bulk replay' }).click();

  // A and B are in the window and C is not, so a budget of 5 replays 2.
  await expect(region).toContainText('Replayed 2 events.');
  const bulkCalls = harness.iam.callsTo('outbox.bulkReplayDeadLetters').slice(bulkBefore);
  expect(bulkCalls).toHaveLength(1);
  expect(bulkCalls[0]?.request).toMatchObject({ eventType: '', parkedFrom: { seconds: WINDOW_FROM_SECONDS, nanos: 0 }, maxRows: 5n });
  expect((bulkCalls[0]?.request as { parkedTo?: unknown }).parkedTo).toBeUndefined();

  // C was outside the window, so it is still parked.
  await page.goto(harness.url('/iam/dead-letters'));
  await waitForHydration(page);
  await expect(rows).toHaveCount(1);
  await expect(rows.first()).toHaveAttribute('data-id', DEAD_LETTER_C_ID);
});
