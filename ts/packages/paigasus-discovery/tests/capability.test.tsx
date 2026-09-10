// SPDX-License-Identifier: Apache-2.0
// @vitest-environment jsdom
import type { ReactElement } from 'react';
import { describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { Capability } from '../src/react.js';
import { createMemoryDescriptorCache } from '../src/adapters/memory-cache.js';
import { createDiscovery } from '../src/server.js';
import { RECORD_VERSION } from '../src/core/record.js';
import type { ProbeOutcome } from '../src/probe.js';

const descriptor = { service: 'iam', version: '1.0.0', capabilities: ['iam.audit'] };

function discoveryWith(
  probe: () => Promise<ProbeOutcome>,
  services: Readonly<Record<string, string>> = { iam: 'http://iam:8080' },
) {
  return createDiscovery({ services, cache: createMemoryDescriptorCache(), probe });
}

/**
 * <Capability> is an ASYNC SERVER COMPONENT. React's client renderer cannot render one, so the
 * element it returns is awaited and THAT is rendered. Calling render() on the component itself
 * is the mistake this helper exists to prevent.
 */
async function renderCapability(element: Promise<ReactElement | null>) {
  const resolved = await element;
  return render(resolved);
}

describe('AC1: three states', () => {
  it('absent: renders nothing at all', async () => {
    const discovery = discoveryWith(() => Promise.resolve({ ok: true, descriptor }), {});
    const { container } = await renderCapability(
      Capability({ discovery, need: 'iam.audit', token: 'tok', children: <a href="/audit">Audit</a> }),
    );
    expect(container).toBeEmptyDOMElement();
  });

  it('available with the key: renders children untouched', async () => {
    const discovery = discoveryWith(() => Promise.resolve({ ok: true, descriptor }));
    await renderCapability(
      Capability({ discovery, need: 'iam.audit', token: 'tok', children: <a href="/audit">Audit</a> }),
    );
    const link = screen.getByRole('link', { name: 'Audit' });
    expect(link).toBeInTheDocument();
    expect(link.closest('[aria-disabled="true"]')).toBeNull();
  });

  it('available without the key: renders nothing', async () => {
    // The service ANSWERED and said it lacks the feature. There is no outage to report, so
    // hiding is correct here and disabling is not.
    const discovery = discoveryWith(() => Promise.resolve({ ok: true, descriptor }));
    const { container } = await renderCapability(
      Capability({ discovery, need: 'iam.apikeys', token: 'tok', children: <a href="/keys">Keys</a> }),
    );
    expect(container).toBeEmptyDOMElement();
  });
});

describe('AC1: degraded is rendered disabled, never hidden', () => {
  // A cached descriptor is seeded directly into the store as a FAIL record that already carries
  // it (`writeRawForTest`, the test seam `DescriptorCache` declares for exactly this). Warming
  // through a real successful probe first does not work here: a fresh 'ok' record answers
  // `getServiceState` from cache without re-probing at all (by design — that IS the freshness
  // cache), so a later discovery handle with a failing probe would never be asked to probe and
  // would report `available`, not `degraded`. Seeding the FAIL record directly is what actually
  // exercises "degraded, descriptor has key" from the state table in src/react.tsx, matched
  // against "degraded, no descriptor at all" for the uncached case below.
  async function renderDegraded(cached: boolean) {
    const cache = createMemoryDescriptorCache();
    if (cached) {
      await cache.writeRawForTest?.(
        'iam',
        JSON.stringify({
          version: RECORD_VERSION,
          rev: 1,
          descriptor,
          descriptorAt: 0,
          outcome: 'fail',
          outcomeAt: 0,
          reason: 'network',
        }),
      );
    }
    const discovery = createDiscovery({
      services: { iam: 'http://iam:8080' },
      cache,
      probe: () => Promise.resolve({ ok: false, reason: 'network' }),
    });
    return renderCapability(
      Capability({ discovery, need: 'iam.audit', token: 'tok', children: <a href="/audit">Audit</a> }),
    );
  }

  it.each([true, false])('renders the item, disabled with a reason (cached descriptor: %s)', async (cached) => {
    await renderDegraded(cached);
    const link = screen.getByRole('link', { name: 'Audit' });

    // NEVER HIDDEN: still present, and still in the accessibility tree.
    expect(link).toBeInTheDocument();

    const wrapper = link.closest('[data-capability-state]');
    expect(wrapper).not.toBeNull();
    expect(wrapper).toHaveAttribute('data-capability-state', 'degraded');
    expect(wrapper).toHaveAttribute('aria-disabled', 'true');

    // NOT `inert`: inert removes the node from the accessibility tree, which hides the item from
    // a screen reader and breaks "never hidden" for exactly the users least able to recover.
    expect(wrapper).not.toHaveAttribute('inert');

    // The reason is reachable, not merely present as a class.
    const describedBy = wrapper?.getAttribute('aria-describedby');
    expect(describedBy).toBeTruthy();
    expect(document.getElementById(describedBy as string)?.textContent).toMatch(/not answering|unreachable/i);
  });

  it('blocks activation — the assertion aria-disabled alone would let through', async () => {
    // THE REGRESSION THIS EXISTS FOR. `tabindex` is NOT inherited: putting tabindex="-1" on the
    // wrapper leaves a nested <a> fully keyboard-activatable while aria-disabled makes the
    // attribute assertions above pass. A green test over a broken control.
    const onNavigate = vi.fn();
    const cache = createMemoryDescriptorCache();
    const discovery = createDiscovery({
      services: { iam: 'http://iam:8080' },
      cache,
      probe: () => Promise.resolve({ ok: false, reason: 'network' }),
    });
    await renderCapability(
      Capability({
        discovery,
        need: 'iam.audit',
        token: 'tok',
        children: (
          <a href="/audit" onClick={onNavigate}>
            Audit
          </a>
        ),
      }),
    );
    const link = screen.getByRole('link', { name: 'Audit' });
    // pointerEventsCheck: 0 — the wrapper's `pointer-events: none` is itself part of what this
    // test proves is not the ONLY guard (see disabled.tsx's comment); without disabling this
    // check, user-event refuses to simulate the click at all rather than exercising the capture
    // handler underneath.
    const user = userEvent.setup({ pointerEventsCheck: 0 });
    await user.click(link);
    expect(onNavigate).not.toHaveBeenCalled();

    link.focus();
    await user.keyboard('{Enter}');
    expect(onNavigate).not.toHaveBeenCalled();
  });

  it('gives each instance a unique description id', async () => {
    // A hardcoded id emits duplicates when two <Capability> elements appear on one page.
    const discovery = discoveryWith(() => Promise.resolve({ ok: false, reason: 'network' }));
    const a = await Capability({ discovery, need: 'iam.audit', token: 'tok', children: <span>A</span> });
    const b = await Capability({ discovery, need: 'iam.audit', token: 'tok', children: <span>B</span> });
    const { container } = render(
      <>
        {a}
        {b}
      </>,
    );
    const ids = [...container.querySelectorAll('[aria-describedby]')].map((e) =>
      e.getAttribute('aria-describedby'),
    );
    expect(ids).toHaveLength(2);
    expect(new Set(ids).size).toBe(2);
  });

  it('uses the degraded render prop when given one', async () => {
    const discovery = discoveryWith(() => Promise.resolve({ ok: false, reason: 'timeout' }));
    await renderCapability(
      Capability({
        discovery,
        need: 'iam.audit',
        token: 'tok',
        children: <a href="/audit">Audit</a>,
        degraded: (reason) => <span data-testid="custom">{reason}</span>,
      }),
    );
    expect(screen.getByTestId('custom')).toHaveTextContent('timeout');
    expect(screen.queryByRole('link')).toBeNull();
  });

  it('ships no Tailwind utility classes', async () => {
    // A package shipping utility classes needs an `@source` line in EVERY consumer; forgetting it
    // drops them silently and ONLY in a production build (the SMA-503 failure class). The
    // functional bits are inline styles; everything cosmetic is a data attribute.
    const discovery = discoveryWith(() => Promise.resolve({ ok: false, reason: 'network' }));
    const { container } = await renderCapability(
      Capability({ discovery, need: 'iam.audit', token: 'tok', children: <a href="/audit">Audit</a> }),
    );
    for (const el of container.querySelectorAll('*')) {
      expect(el.className, el.outerHTML).toBe('');
    }
  });
});
