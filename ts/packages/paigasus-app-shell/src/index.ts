// SPDX-License-Identifier: Apache-2.0
//
// The ONE public entry of @paigasus/app-shell (spec § 5.1).
//
// NO 'use client' directive here, and that is load-bearing (spec § 5.2). Each component file
// carries its own directive. The pure helpers (resolveZone, navStateOf) live in files WITHOUT one,
// so a server layout can CALL them: a function exported from a 'use client' module is only a
// client reference on the server.
//
// EXTENSIONLESS relative specifiers in all of src/: Turbopack (Next 16.3.4) does not map './x.js'
// to './x.ts' (measured, SMA-510), and a consuming Next app compiles every file here.
// tests/structure/source-shape.test.ts pins both rules.
export { ZoneConfigError, ZoneLinkError, ZoneProviderMissingError } from './zone/errors';
export { resolveZone, type ZoneMap, type ZoneTarget } from './zone/resolve';
export { ZoneProvider, useZone, type ZoneContextValue, type ZoneProviderProps } from './zone/context';
export { ZoneLink, type ZoneLinkProps } from './zone/zone-link';
export { navStateOf, type NavEntryState } from './nav/state';
export { PrimaryNav, type NavEntry, type PrimaryNavProps } from './nav/primary-nav';
export { Switcher, type SwitcherItem, type SwitcherProps } from './shell/switcher';
export { UserMenu } from './shell/user-menu';
export { Breadcrumbs, type BreadcrumbsProps, type Crumb } from './shell/breadcrumbs';
