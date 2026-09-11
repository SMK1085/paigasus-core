// SPDX-License-Identifier: Apache-2.0
import type { ReactElement } from 'react';
import { AppShell, navStateOf, type NavEntry } from '@paigasus/app-shell';
import type { ServiceState } from '@paigasus/discovery/types';

// A SERVER component that CALLS navStateOf, on purpose (spec § 5.2). If src/nav/state.ts ever
// gained 'use client', this call would get a client reference instead of a function, the render
// would throw, and E5 and E6 would fail on page load.
const IAM_UP: ServiceState = { state: 'available', service: 'iam', descriptor: { service: 'iam', version: '0.0.0', capabilities: [] }, capabilities: [] };
const GATEWAY_DOWN: ServiceState = { state: 'degraded', service: 'gateway', reason: 'timeout', descriptor: null, capabilities: [] };

export default function ShellPage(): ReactElement {
  const nav: NavEntry[] = [
    { zone: 'iam', href: '/iam/users', label: 'Users', state: navStateOf(IAM_UP) },
    { zone: 'gateway', href: '/gateway/usage', label: 'Gateway', state: navStateOf(GATEWAY_DOWN) },
  ];
  return (
    <AppShell brand={{ label: 'Paigasus', href: '/iam' }} nav={nav}>
      <h1>Shell</h1>
    </AppShell>
  );
}
