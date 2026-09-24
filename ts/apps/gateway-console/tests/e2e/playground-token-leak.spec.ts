// SPDX-License-Identifier: Apache-2.0
//
// ADR-0017 for the playground route (SMA-635 spec § 7.1): neither the SSE body of a streamed turn
// nor the JSON body of a denied turn carries the access or the refresh token. It lives in the
// playground project because the single-zone project's fake gateway serves no chat route
// (fake-gateway.ts:3-5).
import type { Page } from '@playwright/test';
import { signIn } from './support/login';
import { DONE, delta } from './support/mock-openai';
import { expect, test } from './support/playground-harness';
import { ORG_ID } from './support/world';

const PLAYGROUND = `/gateway/orgs/${ORG_ID}/playground`;

async function turn(page: Page, text: string): Promise<{ body: string; headers: string }> {
  const answered = page.waitForResponse((response) => new URL(response.url()).pathname === '/gateway/api/chat');
  await page.getByLabel('Message').fill(text);
  await page.getByRole('button', { name: 'Send' }).click();
  const response = await answered;
  await response.finished();
  return { body: await response.text(), headers: JSON.stringify(await response.allHeaders()) };
}

test('R28: no /gateway/api/chat SSE or JSON body or header carries a token (ADR-0017, SMA-635)', async ({ page, harness }) => {
  harness.mock.setMode({ kind: 'complete', body: `${delta('fine')}${DONE}` });
  const { accessToken, refreshToken } = await signIn(page, harness, PLAYGROUND);
  await page.getByLabel('Model').fill('gpt-e2e');
  const streamed = await turn(page, 'first');
  await expect(page.getByTestId('assistant-turn').last()).toHaveAttribute('data-status', 'done');

  harness.useWorld({ overrides: { 'authz.isAuthorized': (req) => ({ allowed: req.action !== 'InvokeModel', determiningPolicies: [], reason: '' }) } });
  const denied = await turn(page, 'second');

  expect(streamed.body).toContain('data:');
  expect(denied.body).toContain('insufficient-permissions');
  for (const token of [accessToken, refreshToken]) {
    expect(token.length).toBeGreaterThanOrEqual(16);
    for (const seen of [streamed.body, streamed.headers, denied.body, denied.headers]) expect(seen).not.toContain(token);
  }
});
