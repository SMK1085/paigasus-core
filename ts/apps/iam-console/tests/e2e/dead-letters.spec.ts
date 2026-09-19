// SPDX-License-Identifier: Apache-2.0
//
// SMA-629 AC 1 and AC 2 (spec § 7.4). PAIGASUS_DISCOVERY_*_MS are 1/2/3 ms in the harness, so each
// page load probes the fake's GET /v1/service-info again, and a test changes the IAM state between
// two loads. Discard needs JavaScript, so every page.goto before a click waits for hydration.
import { DEAD_LETTER_A_ID, DEAD_LETTER_B_ID, DEAD_LETTERS_DESCRIPTOR } from './support/world';
import { signIn, waitForHydration } from './support/login';
import { expect, test } from './support/harness';

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
