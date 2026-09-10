// SPDX-License-Identifier: Apache-2.0
//
// Observability is a PORT, not a detail. An operator seeing a degraded console needs to know
// WHICH service, WHY, and how often — that is the primary use case in the spec's § 1, and every
// decision point below is otherwise silent (a floated promise swallows its error, a lock timeout
// reaches only the UI).
//
// REDACTION IS THE CALLER'S CONTRACT. No event may carry the bearer token or a service base URL:
// the URL is cluster-internal topology and the token is a credential. Never pass a caught library
// error object into `fields` — node-redis embeds the DSN in its own connection errors. Extract a
// fixed message.

export type DiscoveryEventName =
  /** A probe failed. Fields: service, reason. */
  | 'discovery.probe_failed'
  /** A cold loser exhausted lockWaitMs. Fields: service. */
  | 'discovery.lock_timeout'
  /** A cache read or write threw. Fields: service, stage. */
  | 'discovery.cache_unavailable'
  /** The descriptor's own `service` disagreed with the configured key. Fields: configured, reported. */
  | 'discovery.service_mismatch'
  /** A fenced write lost its compare-and-set and was discarded. Fields: service. */
  | 'discovery.write_fenced'
  /** A stored record was unparseable or from another schema version. Fields: service. */
  | 'discovery.record_discarded';

export type DiscoveryEventFields = Readonly<Record<string, string | number | boolean>>;

export interface DiscoveryLogger {
  event(name: DiscoveryEventName, fields: DiscoveryEventFields): void;
}
