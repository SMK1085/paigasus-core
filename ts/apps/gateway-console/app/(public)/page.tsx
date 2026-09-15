// SPDX-License-Identifier: Apache-2.0
//
// `/gateway/` — the landing page (spec § 4, plan D14). It checks only that the session cookie
// EXISTS and never resolves the session, so it needs no identity provider. A stale cookie
// therefore reaches requireSession() in the (console) layout, which sends the browser to login.
//
// redirect('/overview') is basePath-relative: Next adds /gateway exactly once (spec § 13 #1).
import type { ReactElement } from 'react';
import { cookies } from 'next/headers';
import { redirect } from 'next/navigation';
import { connection } from 'next/server';
import { PublicShell } from '@paigasus/app-shell';
import { SESSION_COOKIE } from '@paigasus/auth/server';

export default async function PublicHome(): Promise<ReactElement> {
  await connection();
  if ((await cookies()).has(SESSION_COOKIE)) redirect('/overview');
  return (
    <PublicShell brand={{ label: 'Paigasus AI Gateway', href: '/gateway' }}>
      <section className="p-8" data-testid="public-home">
        <h1 className="mb-2 text-2xl font-semibold">Paigasus AI Gateway</h1>
        <p className="text-muted-foreground text-sm">Sign in to view the gateway's status and usage.</p>
      </section>
    </PublicShell>
  );
}
