// SPDX-License-Identifier: Apache-2.0
'use client';

import { useRef, type ReactElement } from 'react';
import { useSession } from '@paigasus/auth/client';
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuLabel, DropdownMenuSeparator, DropdownMenuTrigger } from '@paigasus/ui';
import { useZone } from '../zone/context';
import { initialsOf } from './initials';

/* Lucide's `user` (ISC), inline: the avatar when the session has neither a name nor an email. */
function UserGlyph(): ReactElement {
  return (
    <svg
      xmlns="http://www.w3.org/2000/svg"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={2}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      focusable="false"
      className="size-4"
    >
      <path d="M19 21v-2a4 4 0 0 0-4-4H9a4 4 0 0 0-4 4v2" />
      <circle cx="12" cy="7" r="4" />
    </svg>
  );
}

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
  // The trigger shows an avatar, not the name (SMA-783). The avatar's letters are decorative, like
  // an image: the accessible name stays the full name (sr-only text), and `title` gives the same
  // name to a mouse user as a tooltip.
  const name = session.displayName ?? session.email ?? 'Account';
  const initials = initialsOf(session.displayName, session.email);
  const hasIdentity = session.displayName !== null || session.email !== null;

  return (
    <>
      <DropdownMenu>
        <DropdownMenuTrigger
          title={name}
          className="inline-flex size-8 shrink-0 items-center justify-center rounded-full bg-muted text-xs font-medium text-foreground outline-hidden hover:ring-1 hover:ring-border focus-visible:ring-1 focus-visible:ring-ring data-[state=open]:ring-1 data-[state=open]:ring-ring"
        >
          <span aria-hidden="true">{initials ?? <UserGlyph />}</span>
          <span className="sr-only">{name}</span>
        </DropdownMenuTrigger>
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
