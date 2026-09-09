// SPDX-License-Identifier: Apache-2.0
'use client';

import NextLink from 'next/link';
import type { ReactElement, ReactNode } from 'react';
import { LinkProvider } from '@paigasus/ui';

/*
 * The injection point for ADR-0021 decision 3: @paigasus/ui never imports next/*.
 *
 * This is a client boundary because LinkProvider uses React context, and next/link cannot be
 * passed as a prop from the server layout to a client component (a function is not
 * serializable across that boundary). Isolating it here, rather than putting 'use client' on
 * the root layout, keeps the rest of the app's tree eligible for server rendering.
 */
export function Providers({ children }: { children: ReactNode }): ReactElement {
  return <LinkProvider link={NextLink}>{children}</LinkProvider>;
}
