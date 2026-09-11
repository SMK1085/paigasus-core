// SPDX-License-Identifier: Apache-2.0
'use client';

import { useRef, type ReactElement } from 'react';
import { useSession } from '@paigasus/auth/client';
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuLabel, DropdownMenuSeparator, DropdownMenuTrigger } from '@paigasus/ui';
import { useZone } from '../zone/context';

/**
 * The account menu. Calls useSession(), so it must be under a SessionProvider (AppShell is).
 *
 * THE LOGOUT FORM IS OUTSIDE THE MENU PORTAL, ON PURPOSE (spec § 8.5, F18). A mouse click closes the
 * menu inside the click handler. With no exit animation, the portaled content unmounts BEFORE a
 * submit button's default action runs, so a submit button in the menu would silently not submit.
 * Production has an exit animation today (@paigasus/ui dropdown-menu.tsx), but a class change must
 * not break sign-out for mouse users. This form stays mounted, and requestSubmit() runs the same
 * native submission as a submit button. It is not fetch(): the response is a redirect to the IdP
 * that only a document navigation can follow (F8).
 */
export function UserMenu(): ReactElement {
  const session = useSession();
  const { basePath } = useZone();
  const formRef = useRef<HTMLFormElement>(null);
  // The visible text is also the accessible name (WCAG 2.5.3).
  const name = session.displayName ?? session.email ?? 'Account';
  const hasIdentity = session.displayName !== null || session.email !== null;

  return (
    <>
      <DropdownMenu>
        <DropdownMenuTrigger className="rounded-pgs border border-border px-2 py-1 text-sm">{name}</DropdownMenuTrigger>
        <DropdownMenuContent align="end">
          {hasIdentity ? (
            <>
              <DropdownMenuLabel>
                {session.displayName !== null ? <span className="block">{session.displayName}</span> : null}
                {session.email !== null ? <span className="block text-xs text-muted-foreground">{session.email}</span> : null}
              </DropdownMenuLabel>
              <DropdownMenuSeparator />
            </>
          ) : null}
          <DropdownMenuItem onSelect={() => formRef.current?.requestSubmit()}>Sign out</DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
      <form ref={formRef} method="post" action={`${basePath}/auth/logout`} hidden />
    </>
  );
}
