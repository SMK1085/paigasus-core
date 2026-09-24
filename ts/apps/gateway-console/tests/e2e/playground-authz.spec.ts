// SPDX-License-Identifier: Apache-2.0
//
// SMA-635 spec § 7.2 rows 3, 4 and 5. The fake IAM is not Cedar: these rows prove the wiring.
import type { Page } from '@playwright/test';
import { signIn } from './support/login';
import { expect, test } from './support/playground-harness';
import { ORG_ID, ORG_PRN, PRINCIPAL_PRN } from './support/world';

const PLAYGROUND = `/gateway/orgs/${ORG_ID}/playground`;
// app/_components/playground.tsx MISSING_ROLE_TEXT. Copied, not imported: that module is a
// React client component, and this file runs under plain Playwright.
const MISSING_ROLE_TEXT = 'You need the gateway_user role on this organization. Ask an organization admin to grant it.';

async function send(page: Page): Promise<void> {
  await page.getByLabel('Model').fill('gpt-e2e');
  await page.getByLabel('Message').fill('hello');
  await page.getByRole('button', { name: 'Send' }).click();
}

test('R24: IAM denies InvokeModel, and the playground names the missing role (SMA-635 § 7.2 row 3)', async ({ page, harness }) => {
  // Denies ONLY InvokeModel: the page's own reads stay allowed, so the deny can only come from the chat call.
  harness.useWorld({ overrides: { 'authz.isAuthorized': (req) => ({ allowed: req.action !== 'InvokeModel', determiningPolicies: [], reason: '' }) } });
  const before = harness.mock.requests.length;
  await signIn(page, harness, PLAYGROUND);
  await send(page);
  await expect(page.getByTestId('playground-error')).toContainText(MISSING_ROLE_TEXT);
  expect(harness.mock.requests.length).toBe(before);
});

test('R25: the gateway self-query names the org of the URL, InvokeModel, the user and the session bearer (SMA-635 § 7.2 row 4)', async ({ page, harness }) => {
  const { accessToken } = await signIn(page, harness, PLAYGROUND);
  const before = harness.iam.callsTo('authz.isAuthorized').length;
  await send(page);
  await expect(page.getByTestId('assistant-turn').last()).toHaveAttribute('data-status', 'done');
  const invoke = harness.iam
    .callsTo('authz.isAuthorized')
    .slice(before)
    .filter((call) => (call.request as { action: string }).action === 'InvokeModel');
  expect(invoke).toHaveLength(1);
  const [call] = invoke;
  const request = call?.request as { principalPrn: string; action: string; resourcePrn: string };
  expect(request.resourcePrn).toBe(ORG_PRN);
  // The e2e world's Introspect answers PRINCIPAL_PRN for the signed-in user (support/world.ts).
  expect(request.principalPrn).toBe(PRINCIPAL_PRN);
  expect(call?.token).toBe(accessToken);
});

test('R26: /gateway/api/chat with no session answers 401 JSON, not a redirect (SMA-635 § 7.2 row 5)', async ({ harness, playwright }) => {
  const api = await playwright.request.newContext({ ignoreHTTPSErrors: true });
  const before = harness.mock.requests.length;
  try {
    const response = await api.post(harness.url('/gateway/api/chat'), {
      headers: { origin: harness.origin, 'content-type': 'application/json' },
      data: { org: ORG_ID, model: 'gpt-e2e', messages: [{ role: 'user', content: 'hi' }] },
      maxRedirects: 0,
    });
    expect(response.status()).toBe(401);
    expect(response.headers()['content-type']).toContain('application/json');
    const body = (await response.json()) as { error: { presentation: string } };
    expect(body.error.presentation).toBe('relogin');
  } finally {
    await api.dispose();
  }
  expect(harness.mock.requests.length).toBe(before);
});
