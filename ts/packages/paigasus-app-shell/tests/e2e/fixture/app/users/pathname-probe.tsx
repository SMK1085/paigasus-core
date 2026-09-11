// SPDX-License-Identifier: Apache-2.0
'use client';

import { usePathname } from 'next/navigation';
import type { ReactElement } from 'react';

/** E7: what usePathname() returns on /iam/users. The spec says "/users" (F19). */
export function PathnameProbe(): ReactElement {
  return <p data-testid="pathname">{usePathname()}</p>;
}
