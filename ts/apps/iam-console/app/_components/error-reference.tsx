// SPDX-License-Identifier: Apache-2.0
//
// The small pieces every error view shares: the correlation id a user gives to support, and the
// "Sign in again" link. Server-safe and client-safe: it imports only @paigasus/app-shell's ZoneLink.
import type { ReactElement } from 'react';
import { ZoneLink } from '@paigasus/app-shell';

export function CorrelationReference({ id }: { id: string | null }): ReactElement | null {
  if (id === null) return null;
  return (
    <p className="text-muted-foreground mt-2 text-xs">
      Reference: <code data-testid="correlation-id">{id}</code>
    </p>
  );
}

/**
 * A LINK, never an automatic redirect (spec § 6.4): IAM answering "unauthenticated" after
 * getSession() refreshed the token means a wrong configuration or a revoked token, and a redirect
 * would loop. The login route rejects a returnTo under the zone's own /auth/ paths (spec § 7.1).
 */
export function SignInAgain({ returnTo }: { returnTo: string | null }): ReactElement {
  const href = returnTo === null ? '/iam/auth/login' : `/iam/auth/login?returnTo=${encodeURIComponent(returnTo)}`;
  return (
    <ZoneLink href={href} className="mt-2 text-sm underline">
      Sign in again
    </ZoneLink>
  );
}
