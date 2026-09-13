// SPDX-License-Identifier: Apache-2.0
//
// The relogin/correlation tail shared by PageError and SectionError (spec § 6.1, § 6.2): a
// "Sign in again" link for `relogin`, else the correlation id — never IAM's message.
//
// A plain async FUNCTION, awaited inline by its caller, not a JSX component: PageError and
// SectionError must still return a fully resolved element tree with no async component left
// inside it, because the unit tests render them with react-dom/server's synchronous
// renderToStaticMarkup, which cannot suspend on an unawaited async component.
import type { ReactElement } from 'react';
import type { PaigasusError } from '@paigasus/sdk/errors';
import { requestPath } from '../../lib/correlation';
import { CorrelationReference, SignInAgain } from './error-reference';

export async function errorTail(error: PaigasusError): Promise<ReactElement> {
  return error.presentation === 'relogin' ? <SignInAgain returnTo={await requestPath()} /> : <CorrelationReference id={error.correlationId} />;
}
