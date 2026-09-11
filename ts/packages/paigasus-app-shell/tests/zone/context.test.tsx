// SPDX-License-Identifier: Apache-2.0
import { render, renderHook } from '@testing-library/react';
import type { ReactElement, ReactNode } from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { ZoneProvider, useZone } from '../../src/zone/context';
import { ZoneConfigError, ZoneProviderMissingError } from '../../src/zone/errors';
import { silenceReactErrorLog } from '../support/console';
import { ZONES } from '../support/providers';

function GatewayZone({ children }: { children: ReactNode }): ReactElement {
  return (
    <ZoneProvider zone="gateway" zones={ZONES}>
      {children}
    </ZoneProvider>
  );
}

afterEach(() => {
  vi.restoreAllMocks();
});

describe('ZoneProvider and useZone', () => {
  it('exposes the zone, its base path and the map', () => {
    const { result } = renderHook(() => useZone(), { wrapper: GatewayZone });
    expect(result.current).toEqual({ zone: 'gateway', basePath: '/gateway', zones: ZONES });
  });

  it('throws ZoneConfigError when the current zone is not a key of the map', () => {
    silenceReactErrorLog();
    expect(() =>
      render(
        <ZoneProvider zone="billing" zones={ZONES}>
          <p>x</p>
        </ZoneProvider>,
      ),
    ).toThrow(ZoneConfigError);
  });

  it("throws for zone 'constructor' when the map has no OWN key of that name", () => {
    silenceReactErrorLog();
    // ZONES is a plain object literal, so it INHERITS `constructor` — like getPublicConfig()'s map (F17).
    expect(() =>
      render(
        <ZoneProvider zone="constructor" zones={ZONES}>
          <p>x</p>
        </ZoneProvider>,
      ),
    ).toThrow(ZoneConfigError);
  });

  it('throws ZoneConfigError when two zones share a base path', () => {
    silenceReactErrorLog();
    expect(() =>
      render(
        <ZoneProvider zone="iam" zones={{ iam: '/iam', legacy: '/iam' }}>
          <p>x</p>
        </ZoneProvider>,
      ),
    ).toThrow(ZoneConfigError);
  });

  it('useZone() outside a provider throws ZoneProviderMissingError: there is no default map', () => {
    silenceReactErrorLog();
    expect(() => renderHook(() => useZone())).toThrow(ZoneProviderMissingError);
  });
});
