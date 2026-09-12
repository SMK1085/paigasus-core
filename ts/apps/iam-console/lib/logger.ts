// SPDX-License-Identifier: Apache-2.0
//
// The app's one logger adapter (spec § 4.8): JSON lines on stdout. It implements @paigasus/auth's
// AuthLogger and @paigasus/discovery's DiscoveryLogger, and adds `appEvent` for the app's own
// events, because AuthEventName is a closed union (ts/packages/paigasus-auth/src/ports/logger.ts:14-25).
//
// REDACTION IS THE CALLER'S CONTRACT (both ports say so). This adapter writes the event name, a
// timestamp and EXACTLY the fields it receives, under their own key, and adds nothing else. It
// never serializes an error object: the field types admit only scalars.
import 'server-only';
import type { AuthEventFields, AuthEventName, AuthLogger } from '@paigasus/auth/server';
import type { DiscoveryEventFields, DiscoveryEventName, DiscoveryLogger } from '@paigasus/discovery/server';

export type AppEventName = 'principal.resolve_failed' | 'principal.resolve_crashed' | 'authorize.query_failed' | 'discovery.redis_connect_failed' | 'iam.call_failed';

export type AppEventFields = Readonly<Record<string, string | number | boolean | null>>;

/**
 * `event` is declared again here, as the union of both ports' event names. Without it TypeScript
 * rejects the interface: AuthLogger.event and DiscoveryLogger.event have different parameter types,
 * and an interface cannot extend two bases whose same-named members are not identical.
 */
export interface ConsoleLogger extends AuthLogger, DiscoveryLogger {
  event(name: AuthEventName | DiscoveryEventName, fields: AuthEventFields | DiscoveryEventFields): void;
  appEvent(name: AppEventName, fields: AppEventFields): void;
}

const stdout = (line: string): void => {
  process.stdout.write(`${line}\n`);
};

export function createJsonLogger(write: (line: string) => void = stdout): ConsoleLogger {
  const emit = (event: string, fields: Readonly<Record<string, unknown>>): void => {
    write(JSON.stringify({ time: new Date().toISOString(), event, fields }));
  };
  return {
    event: (name, fields) => emit(name, fields),
    appEvent: (name, fields) => emit(name, fields),
  };
}

/** The process-wide instance. Stateless, so one per process is correct. */
export const logger: ConsoleLogger = createJsonLogger();
