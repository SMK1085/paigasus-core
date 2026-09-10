// SPDX-License-Identifier: Apache-2.0
//
// Observability is a PORT, not a detail: adding a logger after the adapters exist changes every
// constructor in adapters/ and http/. It lands with the scaffold for that reason.
//
// REDACTION IS THE CALLER'S CONTRACT, and it is absolute. No event may carry an access token, a
// refresh token, an authorization code, an id_token, the client secret, the Redis DSN, or a
// transaction secret. `sid` is logged TRUNCATED TO 8 CHARACTERS — enough to correlate two lines,
// not enough to replay a session.
//
// Never pass a caught library error object into `fields`: node-redis embeds the DSN in its own
// connection errors and openid-client may include a URL. Extract `name` and a fixed message.

export type AuthEventName =
  | 'login.started'
  | 'login.callback_rejected'
  | 'session.created'
  | 'session.refreshed'
  | 'session.refresh_failed'
  | 'session.refresh_timeout'
  | 'session.resolve_failed'
  | 'session.refresh.persist_failed'
  | 'session.deleted'
  | 'logout.completed'
  | 'store.unavailable';

export type AuthEventFields = Readonly<Record<string, string | number | boolean>>;

export interface AuthLogger {
  event(name: AuthEventName, fields: AuthEventFields): void;
}

/** Truncate a session id for logging. See the redaction contract above. */
export function sidTag(sid: string): string {
  return sid.slice(0, 8);
}
