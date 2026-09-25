// SPDX-License-Identifier: Apache-2.0
//
// POST /gateway/api/chat (SMA-635 spec § 6.2). Public in proxy.ts: it answers its OWN 401, never a
// login redirect. The logic is lib/chat-route.ts; this file only connects the real dependencies.
// Nothing here runs at module scope except the factory call, whose dependencies are all lazy.
import { createChatClient } from '@paigasus/sdk/chat';
import { requestCorrelationId } from '@paigasus/console-core';
import { authRuntime } from '../../../lib/auth';
import { GATEWAY_DEFAULT_MAX_REQUEST_BYTES, createChatRoute } from '../../../lib/chat-route';
import { getRuntimeConfig } from '../../../lib/config';
import { optionalSession } from '../../../lib/console';

export const dynamic = 'force-dynamic';

const handle = createChatRoute({
  publicOrigin: async () => (await authRuntime()).publicOrigin,
  session: () => optionalSession(),
  gatewayBaseUrl: () => getRuntimeConfig().PAIGASUS_SERVICES['gateway'] ?? null,
  correlationId: () => requestCorrelationId(),
  chatClient: (options, auth) => createChatClient(options, auth),
  maxBodyBytes: GATEWAY_DEFAULT_MAX_REQUEST_BYTES,
});

export async function POST(request: Request): Promise<Response> {
  return handle(request);
}
