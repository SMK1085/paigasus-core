// SPDX-License-Identifier: Apache-2.0
//
// `/iam/` — the landing page (spec § 3.3). It checks only that the session cookie EXISTS and never
// resolves the session, so it needs no identity provider. A stale cookie therefore reaches
// requireSession() in the (console) layout, which sends the browser to login. @paigasus/auth sends
// `idp_error` and the default post-logout redirect here (server.ts:112-113, runtime.ts:129).
//
// redirect('/orgs') is basePath-relative: Next adds /iam exactly once (spec § 13 #1).
import type { ReactElement } from 'react';
import { cookies } from 'next/headers';
import { redirect } from 'next/navigation';
import { connection } from 'next/server';
import { PublicShell } from '@paigasus/app-shell';
import { SESSION_COOKIE_NAME } from '../../lib/auth';

export default async function PublicHome(): Promise<ReactElement> {
  await connection();
  if ((await cookies()).has(SESSION_COOKIE_NAME)) redirect('/orgs');
  return (
    <PublicShell brand={{ label: 'Paigasus IAM', href: '/iam' }}>
      <section className="p-8" data-testid="public-home">
        <h1 className="mb-2 text-2xl font-semibold">Paigasus IAM</h1>
        <p className="text-muted-foreground text-sm">Sign in to manage your organizations, teams and projects.</p>
      </section>
    </PublicShell>
  );
}
