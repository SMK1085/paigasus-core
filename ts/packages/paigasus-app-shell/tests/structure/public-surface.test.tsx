// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it, vi } from 'vitest';
import * as surface from '../../src/index';

vi.mock('next/link', () => import('../support/next-link-double'));
vi.mock('next/navigation', () => import('../support/next-navigation-double'));

describe('the public surface (spec § 2)', () => {
  it('exports exactly these runtime names from the one entry', () => {
    // A strict list: a new export, or a lost one, is a deliberate change to this test.
    // assertZoneMap, pathOf, SkipLink and MAIN_ID stay internal.
    expect(Object.keys(surface).sort()).toEqual([
      'AppShell',
      'Breadcrumbs',
      'PrimaryNav',
      'PublicShell',
      'Switcher',
      'UserMenu',
      'ZoneConfigError',
      'ZoneLink',
      'ZoneLinkError',
      'ZoneProvider',
      'ZoneProviderMissingError',
      'navStateOf',
      'resolveZone',
      'useZone',
    ]);
  });
});
