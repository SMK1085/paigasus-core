// SPDX-License-Identifier: Apache-2.0
import NextLink from 'next/link';
import type { ReactElement } from 'react';
import { ZoneLink } from '@paigasus/app-shell';
import { Link } from '@paigasus/ui';

/*
 * DOM ORDER IS LOAD-BEARING (spec § 10.3). E2 waits for the RSC prefetches of the three CONTROLS.
 * They come AFTER the links under test, so their requests show that Next processed the prefetch
 * queue past the links under test.
 *   1. links under test: a cross-zone ZoneLink, and a cross-zone @paigasus/ui Link (injected)
 *   2. positive controls: a same-zone ZoneLink, and a same-zone @paigasus/ui Link (injected)
 *   3. negative control: a RAW next/link to another zone — the ADR-0017 bug, on purpose (E3)
 * The page's RSC allowlist is exactly { /iam/users, /iam/settings, /iam/gateway/raw }.
 */
export default function MainPage(): ReactElement {
  return (
    <main>
      <h1>app-shell fixture</h1>
      <ul>
        <li>
          <ZoneLink href="/gateway/usage">Gateway usage</ZoneLink>
        </li>
        <li>
          <Link href="/gateway/ui">Gateway via ui Link</Link>
        </li>
        <li>
          <ZoneLink href="/iam/users">Users</ZoneLink>
        </li>
        <li>
          <Link href="/iam/settings">Settings via ui Link</Link>
        </li>
        <li>
          <NextLink href="/gateway/raw">Raw next/link to gateway</NextLink>
        </li>
      </ul>
    </main>
  );
}
