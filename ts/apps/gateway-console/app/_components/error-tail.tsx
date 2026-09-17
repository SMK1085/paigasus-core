// SPDX-License-Identifier: Apache-2.0
//
// The relogin/correlation tail shared by PageError (spec § 6.1): a "Sign in again" link for
// `relogin`, else the correlation id — never IAM's message.
//
// A plain async FUNCTION, awaited inline by its caller, not a JSX component: PageError must still
// return a fully resolved element tree with no async component left inside it, because the unit
// tests render it with react-dom/server's synchronous renderToStaticMarkup, which cannot suspend
// on an unawaited async component.
import type { ReactElement } from 'react';
import type { PaigasusError } from '@paigasus/sdk/errors';
import { requestPath } from '@paigasus/console-core';
import { CorrelationReference, SignInAgain } from './error-reference';

export async function errorTail(error: PaigasusError): Promise<ReactElement> {
  return error.presentation === 'relogin' ? <SignInAgain returnTo={await requestPath()} /> : <CorrelationReference id={error.correlationId} />;
}
