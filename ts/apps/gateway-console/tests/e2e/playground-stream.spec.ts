// SPDX-License-Identifier: Apache-2.0
//
// SMA-635 spec § 7.2 rows 1, 2 and 6, against the REAL gateway binary.
import type { Page } from '@playwright/test';
import { signIn } from './support/login';
import { DONE, delta } from './support/mock-openai';
import { expect, test } from './support/playground-harness';
import { ORG_ID, ORG_PRN, PRINCIPAL_PRN } from './support/world';
import { GATEWAY_OPENAI_KEY } from './support/gateway-process';

const PLAYGROUND = `/gateway/orgs/${ORG_ID}/playground`;
// lib/chat-route.ts STREAM_FAILED_MESSAGE. Copied, not imported: that module opens with
// `import 'server-only'`, and this file runs under plain Playwright.
const STREAM_FAILED_MESSAGE = 'The answer stream failed.';
const UUID_RE = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;

async function send(page: Page, text = 'hello'): Promise<void> {
  await page.getByLabel('Model').fill('gpt-e2e');
  await page.getByLabel('Message').fill(text);
  await page.getByRole('button', { name: 'Send' }).click();
}

test('R22: a streamed answer reaches the page chunk by chunk through the real gateway, logged as oidc (SMA-635 § 7.2 row 1)', async ({ page, harness }) => {
  harness.mock.setMode({ kind: 'stepped', first: delta('Hello'), rest: `${delta(' world')}${DONE}` });
  const logBefore = harness.gatewayLog().length;
  await signIn(page, harness, PLAYGROUND);
  await send(page);
  const answer = page.getByTestId('assistant-turn').last();
  // The mock holds chunk 2 until release(): a buffering regression hides chunk 1 here and fails.
  await expect(answer.getByTestId('turn-text')).toHaveText('Hello');
  await expect(answer).toHaveAttribute('data-status', 'streaming');
  harness.mock.release();
  await expect(answer.getByTestId('turn-text')).toHaveText('Hello world');
  await expect(answer).toHaveAttribute('data-status', 'done');

  // Only the real binary writes this line: the row fails if anything else served the call.
  const proxied = harness
    .gatewayLog()
    .slice(logBefore)
    .filter((line) => line.fields?.['message'] === 'chat completion proxied');
  expect(proxied).toHaveLength(1);
  expect(proxied[0]?.fields).toMatchObject({ auth: 'oidc', scope: ORG_PRN, principal: PRINCIPAL_PRN });
  expect(harness.mock.requests.at(-1)?.authorization).toBe(`Bearer ${GATEWAY_OPENAI_KEY}`);
});

test('R23: Stop closes the upstream connection and keeps the partial answer (SMA-635 § 7.2 row 2)', async ({ page, harness }) => {
  harness.mock.setMode({ kind: 'endless', chunk: delta('tick '), everyMs: 100 });
  await signIn(page, harness, PLAYGROUND);
  await send(page);
  const answer = page.getByTestId('assistant-turn').last();
  await expect(answer.getByTestId('turn-text')).toContainText('tick');
  await page.getByRole('button', { name: 'Stop' }).click();
  expect(await harness.mock.waitForClose(5_000)).toBe(true);
  await expect(answer).toHaveAttribute('data-status', 'stopped');
  await expect(answer.getByTestId('turn-text')).toContainText('tick');
});

test('R27: an upstream failure inside a record reaches the page as a paigasus-error with a correlation id (SMA-635 § 7.2 row 6)', async ({ page, harness }) => {
  harness.mock.setMode({ kind: 'break-mid-record', partial: `${delta('partial ')}data: {"choices":[{"delta":{"content":"cu` });
  await signIn(page, harness, PLAYGROUND);
  await send(page);
  const error = page.getByTestId('playground-error');
  await expect(error).toBeVisible();
  await expect(error).toContainText(STREAM_FAILED_MESSAGE);
  await expect(error).toHaveAttribute('data-correlation-id', UUID_RE);
  await expect(page.getByTestId('assistant-turn').last().getByTestId('turn-text')).toContainText('partial');
});
