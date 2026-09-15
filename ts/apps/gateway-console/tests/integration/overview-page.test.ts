// SPDX-License-Identifier: Apache-2.0
//
// /gateway/overview against the REAL discovery() and a scripted fake gateway (spec § 10.3). The
// page is awaited (its own body is async), then the SYNCHRONOUS element it returns — a
// <ZoneOverview> — is rendered with renderToStaticMarkup, exactly as tests/unit/zone-overview.test.tsx
// does for the same component with hand-built ServiceState values.
//
// RECORDED LIMIT (spec § 10.3, task 4 brief): the `absent` state of the gateway view is UNREACHABLE
// at this tier, by construction — this app's lib/config.ts refuses to parse a PAIGASUS_SERVICES map
// with no `gateway` entry (tests/unit/config.test.ts, "rejects a PAIGASUS_SERVICES map with no
// gateway entry"). That branch of ZoneOverview is covered only by tests/unit/zone-overview.test.tsx
// (hand-built ServiceState) and is not, and cannot be, exercised here. Do not fake it by reaching
// into @paigasus/discovery's internals.
import { renderToStaticMarkup } from 'react-dom/server';
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it } from 'vitest';
import { resetDiscoveryForTest } from '@paigasus/console-core';
import OverviewPage from '../../app/(console)/overview/page';
import { installSession, startIntegrationEnv, stopIntegrationEnv, type IntegrationEnv } from './support';

let env: IntegrationEnv;

beforeAll(async () => {
  env = await startIntegrationEnv();
});

afterAll(async () => {
  await stopIntegrationEnv(env);
});

beforeEach(async () => {
  await installSession();
  // Every case starts from a healthy, capable gateway so a case that forgets to script anything
  // fails loudly rather than inheriting the PREVIOUS case's degraded/unreachable state.
  env.gateway.setReachable(true);
  env.gateway.setServiceInfo({ service: 'gateway', version: '0.0.0-fake', capabilities: ['gateway.chat.stream'] });
});

afterEach(() => {
  resetDiscoveryForTest();
});

describe('OverviewPage', () => {
  it('reports available with streaming and the version when the descriptor lists gateway.chat.stream', async () => {
    env.gateway.setServiceInfo({ service: 'gateway', version: '1.2.3', capabilities: ['gateway.chat.stream'] });

    const html = renderToStaticMarkup(await OverviewPage());

    expect(html).toContain('data-state="available"');
    expect(html).toContain('data-streaming="true"');
    expect(html).toContain('1.2.3');
  });

  it('reports available without streaming when the descriptor omits the capability', async () => {
    env.gateway.setServiceInfo({ service: 'gateway', version: '1.2.3', capabilities: [] });

    const html = renderToStaticMarkup(await OverviewPage());

    expect(html).toContain('data-state="available"');
    expect(html).toContain('data-streaming="false"');
  });

  it('reports degraded with a rendered reason when the gateway answers a bad status', async () => {
    env.gateway.setServiceInfo({ status: 503 });

    const html = renderToStaticMarkup(await OverviewPage());

    expect(html).toContain('data-state="degraded"');
    expect(html).toContain('data-testid="gateway-reason"');
  });

  it('reports degraded with a rendered reason when the gateway is unreachable', async () => {
    env.gateway.setReachable(false);

    const html = renderToStaticMarkup(await OverviewPage());

    expect(html).toContain('data-state="degraded"');
    expect(html).toContain('data-testid="gateway-reason"');
  });
});
