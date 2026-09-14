// SPDX-License-Identifier: Apache-2.0
//
// The request-scoped IAM clients, with NO session lookup (spec § 4.3). `runtime.ts`'s
// `createConsoleRuntime()` adds the session and the app's IAM gRPC address through its
// `iamClientsForToken` field; the login callback (the app's `lib/auth.ts`) reads that same field,
// where no session exists yet.
//
// No client and no token lives past the request (ts/packages/paigasus-sdk/src/iam.ts:23-28). Only
// the SDK's transport is process-scoped.
import 'server-only';
import type { DescService } from '@bufbuild/protobuf';
import type { CallOptions, Client } from '@connectrpc/connect';
import { AuditService, AuthnService, AuthorizationService, createIamClient, ServiceInfoService, TenancyService } from '@paigasus/sdk/iam';
import { CORRELATION_HEADER } from './correlation-header';

export type IamClients = {
  tenancy: Client<typeof TenancyService>;
  authn: Client<typeof AuthnService>;
  authz: Client<typeof AuthorizationService>;
  audit: Client<typeof AuditService>;
  serviceInfo: Client<typeof ServiceInfoService>;
};

/**
 * Adds `paigasus-correlation-id` to every call a client makes. A Proxy over the SDK's own Proxy:
 * it touches only `headers`, never `contextValues`, which the SDK refuses from a caller. A header
 * the caller set itself wins.
 */
function withCorrelation<S extends DescService>(client: Client<S>, correlationId: string | null): Client<S> {
  if (correlationId === null) return client;
  return new Proxy(client, {
    get(target, property, receiver) {
      const value: unknown = Reflect.get(target, property, receiver);
      if (typeof value !== 'function') return value;
      return (request: unknown, options?: CallOptions) => {
        const headers = new Headers(options?.headers);
        if (!headers.has(CORRELATION_HEADER)) headers.set(CORRELATION_HEADER, correlationId);
        return (value as (r: unknown, o: CallOptions) => unknown)(request, { ...options, headers });
      };
    },
  });
}

/** The five clients for one bearer token, over one base URL. Pure: tests call it with a fake IAM. */
export function createIamClients(opts: { baseUrl: string; token: string; correlationId?: string | null }): IamClients {
  const transport = { baseUrl: opts.baseUrl };
  const auth = { bearer: opts.token };
  const id = opts.correlationId ?? null;
  return {
    tenancy: withCorrelation(createIamClient(TenancyService, transport, auth), id),
    authn: withCorrelation(createIamClient(AuthnService, transport, auth), id),
    authz: withCorrelation(createIamClient(AuthorizationService, transport, auth), id),
    audit: withCorrelation(createIamClient(AuditService, transport, auth), id),
    serviceInfo: withCorrelation(createIamClient(ServiceInfoService, transport, auth), id),
  };
}
