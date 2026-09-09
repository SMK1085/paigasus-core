// SPDX-License-Identifier: Apache-2.0
// @vitest-environment jsdom
//
// AC 5's client half. Everything here runs against `src/client.ts` alone — see that file's own
// header for why it imports nothing but `react` and the `SessionView` type.
import { describe, expect, it } from 'vitest';
import { render, renderHook, screen } from '@testing-library/react';
import { can, SessionProvider, UseSessionOutsideProviderError, useSession } from '../src/client.js';
import type { SessionView } from '../src/client.js';

function view(overrides: Partial<SessionView> = {}): SessionView {
  return {
    principalPrn: 'prn:pgs:iam::org1:user/8f1a2b3c-4d5e-6f70-8192-a3b4c5d6e7f8',
    displayName: 'Ada Lovelace',
    email: 'ada@example.com',
    grants: [],
    grantsAvailable: false,
    ...overrides,
  };
}

describe('SessionProvider / useSession', () => {
  it('returns the provided view to a hook rendered inside the provider', () => {
    const value = view();
    const { result } = renderHook(() => useSession(), {
      wrapper: ({ children }) => <SessionProvider value={value}>{children}</SessionProvider>,
    });
    expect(result.current).toBe(value);
  });

  it('hands the view to a component tree, not just a hook', () => {
    function Greeting() {
      const session = useSession();
      return <p>Hello, {session.displayName}</p>;
    }
    render(
      <SessionProvider value={view({ displayName: 'Grace Hopper' })}>
        <Greeting />
      </SessionProvider>,
    );
    expect(screen.getByText('Hello, Grace Hopper')).toBeTruthy();
  });

  it('throws a named error, not a silent undefined, outside a provider', () => {
    // React logs the render error to the console by default; that noise is expected here and not
    // asserted on — only the thrown error itself is.
    expect(() => renderHook(() => useSession())).toThrow(UseSessionOutsideProviderError);
    expect(() => renderHook(() => useSession())).toThrow('useSession() was called outside a SessionProvider');
  });
});

describe('can', () => {
  it('fails OPEN when grantsAvailable is false, regardless of the grants array', () => {
    const session = view({ grantsAvailable: false, grants: [] });
    expect(can(session, { scopePrn: 'prn:pgs:iam::org1:org/8f1a2b3c-4d5e-6f70-8192-a3b4c5d6e7f8', roleKey: 'admin' })).toBe(true);
  });

  it('matches a grant present in the array when grants are available', () => {
    const session = view({
      grantsAvailable: true,
      grants: [{ scopePrn: 'prn:pgs:iam::org1:org/8f1a2b3c-4d5e-6f70-8192-a3b4c5d6e7f8', roleKey: 'admin' }],
    });
    expect(can(session, { scopePrn: 'prn:pgs:iam::org1:org/8f1a2b3c-4d5e-6f70-8192-a3b4c5d6e7f8', roleKey: 'admin' })).toBe(true);
  });

  it('denies a grant absent from the array when grants are available', () => {
    const session = view({
      grantsAvailable: true,
      grants: [{ scopePrn: 'prn:pgs:iam::org1:org/8f1a2b3c-4d5e-6f70-8192-a3b4c5d6e7f8', roleKey: 'admin' }],
    });
    expect(can(session, { scopePrn: 'prn:pgs:iam::org1:org/8f1a2b3c-4d5e-6f70-8192-a3b4c5d6e7f8', roleKey: 'viewer' })).toBe(false);
  });
});
