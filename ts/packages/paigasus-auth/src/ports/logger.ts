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
//
// `session.refresh_failed` may carry `oauthError` (SMA-692 D10). Its value is a code of the RFC
// 6749 § 5.2 list or 'other', never the IdP's raw string: core/errors.ts's toTokenErrorCode maps
// it. This type admits any string key, so that function is the control, not this port.

export type AuthEventName =
  | 'login.started'
  | 'login.callback_rejected'
  | 'session.created'
  | 'session.refreshed'
  | 'session.refresh_failed'
  | 'session.refresh_timeout'
  | 'session.resolve_failed'
  | 'session.refresh.persist_failed'
  | 'session.refresh.id_token_mismatch'
  | 'session.deleted'
  | 'logout.completed'
  | 'store.unavailable'
  | 'store.operation_timeout';

export type AuthEventFields = Readonly<Record<string, string | number | boolean>>;

/**
 * The closed set of `stage` values for `store.unavailable` (SMA-653 § 5). Every emitter uses this
 * type, so an operator can rely on the list. Each value names ONE store call.
 */
export type StoreUnavailableStage =
  'get_session' | 'release_lock' | 'login_put_transaction' | 'login_delete' | 'callback_take_transaction' | 'callback_delete' | 'callback_set' | 'logout_get' | 'logout_delete';

export interface AuthLogger {
  event(name: AuthEventName, fields: AuthEventFields): void;
}

/** Truncate a session id for logging. See the redaction contract above. */
export function sidTag(sid: string): string {
  return sid.slice(0, 8);
}
