// SPDX-License-Identifier: Apache-2.0
//
// TEMPORARY (SMA-512 PR 2, task 3). The pure factory moved to @paigasus/console-core; this wrapper
// supplies the app's own baseUrl until task 5's createConsoleRuntime takes over. Delete it there.
import 'server-only';
import { createIamClients, type IamClients } from '@paigasus/console-core';
import { getRuntimeConfig } from './config';

/** The clients for a token the caller already holds. No session lookup — used at login. */
export function iamClientsForToken(token: string, correlationId: string | null = null): IamClients {
  return createIamClients({ baseUrl: getRuntimeConfig().PAIGASUS_IAM_GRPC_URL, token, correlationId });
}
