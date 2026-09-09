// SPDX-License-Identifier: Apache-2.0
import './server-guard.js';

import { createClient, createContextValues } from '@connectrpc/connect';
import type { CallOptions, Client } from '@connectrpc/connect';
import type { DescService } from '@bufbuild/protobuf';
import { AuditService, AuthnService, AuthorizationService, OutboxService, ServiceAccountService, TenancyService, UserService } from '@paigasus/proto/iam';

import { authContextKey, getTransport } from './transport.js';
import type { Auth, TransportOptions } from './transport.js';

export { AuditService, AuthnService, AuthorizationService, OutboxService, ServiceAccountService, TenancyService, UserService };
export { disposeTransports } from './transport.js';
export type { Auth, TransportOptions } from './transport.js';

/**
 * Bind an `Auth` to every call a client makes.
 *
 * The transport is cached and shared; the CLIENT is not. It holds a bearer, so its lifetime is one
 * request. A module-scope `const client = createIamClient(...)` is a cross-request token leak — the
 * same leak the transport cache exists to prevent, moved one level up. Nothing about `Client<S>`'s
 * shape marks it request-scoped, so this is a rule the type system cannot express (spec § 7.5).
 *
 * Exported for the test suite, which proves two clients over ONE transport keep their tokens apart.
 */
export function bindAuth<S extends DescService>(client: Client<S>, auth: Auth): Client<S> {
  const contextValues = createContextValues().set(authContextKey, auth);

  return new Proxy(client, {
    get(target, property, receiver) {
      const value: unknown = Reflect.get(target, property, receiver);
      if (typeof value !== 'function') return value;

      return (request: unknown, options?: CallOptions) => {
        if (options?.contextValues !== undefined) {
          // Refused rather than dropped or merged. ContextValues exposes get/set/delete and no
          // iteration, so this object cannot be copied: merging would mean writing the bearer into
          // an object the CALLER holds, which they could then reuse against another client — the
          // cross-request leak spec § 7.5 exists to prevent. Dropping it silently is worse still.
          //
          // Returned as a rejected promise, not thrown synchronously: every Client<S> method is
          // async, so a caller awaits the call. A synchronous throw here would escape that await
          // entirely and surface as an unhandled exception instead of a rejection.
          return Promise.reject(
            new Error('@paigasus/sdk: a caller-supplied `contextValues` is not supported, because the ' + 'client binds its own to carry the bearer token. Remove it from the CallOptions.'),
          );
        }
        return (value as (r: unknown, o: CallOptions) => unknown)(request, {
          ...options,
          contextValues,
        });
      };
    },
  });
}

/**
 * Build a typed, request-scoped client for one IAM service.
 *
 * `auth` is a REQUIRED parameter, so a forgotten token is a compile error rather than a runtime 401
 * — the most likely mistake in a package whose stated purpose is attaching bearer tokens. It is a
 * parameter of the CLIENT and never of `getTransport`, so it stays absent from the cached transport
 * and out of its cache key (spec § 7.4).
 */
export function createIamClient<S extends DescService>(service: S, options: TransportOptions, auth: Auth): Client<S> {
  return bindAuth(createClient(service, getTransport(options)), auth);
}
