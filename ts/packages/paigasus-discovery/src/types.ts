// SPDX-License-Identifier: Apache-2.0
//
// The client-safe entry. NO `server-only` guard, deliberately — a client component holds a
// resolved ServiceState as data, exactly as @paigasus/sdk's ./errors/types is exempted.
//
// Everything here must survive the RSC server-to-client boundary as plain data. That is why
// `capabilities` is a readonly array and not a ReadonlySet (a Set does not serialize), and why
// ServiceDescriptor is a plain structural type rather than protobuf-es's ServiceInfo message
// (which carries $typeName, and which apps are banned from naming — the
// `paigasus/boundaries/apps` eslint block bans @paigasus/proto including type imports).

/** A service's self-description, converted from `paigasus.common.v1.ServiceInfo`. */
export type ServiceDescriptor = {
  readonly service: string;
  readonly version: string;
  readonly capabilities: readonly string[];
};

/**
 * Why a configured service is not usable right now.
 *
 * A closed union, never free text: the UI branches on the code and never on a message.
 * This vocabulary is owned HERE and not imported from @paigasus/sdk — that package's
 * `Presentation` union cannot express these cases (spec F8).
 */
export type DegradedReason =
  | 'timeout'
  | 'network'
  | 'unauthorized'
  | 'not-implemented'
  | 'bad-response'
  | 'server-error'
  | 'cache-unavailable';

export type ServiceState =
  | { readonly state: 'absent'; readonly service: string }
  | {
      readonly state: 'available';
      readonly service: string;
      readonly descriptor: ServiceDescriptor;
      readonly capabilities: readonly string[];
    }
  | {
      readonly state: 'degraded';
      readonly service: string;
      readonly reason: DegradedReason;
      readonly descriptor: ServiceDescriptor | null;
      readonly capabilities: readonly string[];
    };

export const SERVICE_STATES = ['absent', 'available', 'degraded'] as const;

/**
 * A capability key, as a CLOSED union.
 *
 * Hand-declared, and deliberately not `` `${string}.${string}` ``: that template accepts the typo
 * `iam.audits`, which at runtime returns false forever and silently hides a nav item — the
 * "invisible" failure the design calls worse than a wrongly-shown disabled one. Hand-declaring
 * re-opens a drift risk against the proto registry, and tests/vocabulary.test.ts closes it by
 * comparing these members against the registry-derived runtime list.
 *
 * It lives HERE and not in core/state.ts on purpose. This file is the client-safe entry, and
 * core/state.ts imports `@paigasus/proto` as a VALUE — re-exporting from it would pull
 * protobuf-es into any client bundle that imports `@paigasus/discovery/types`, and would route
 * around the `paigasus/boundaries/apps` eslint ban on apps importing `@paigasus/proto`, which
 * cannot see through a re-export.
 */
export type CapabilityKey =
  | 'iam.authz.cedar'
  | 'iam.apikeys'
  | 'iam.audit'
  | 'gateway.chat.stream';
