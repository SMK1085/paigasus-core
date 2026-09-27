// SPDX-License-Identifier: Apache-2.0
//
// The one emitter of `oidc.discovery_failed` (SMA-656 D7, SMA-705 D8). http/routes.ts calls it for
// the login and the callback stage. http/readiness.ts calls it for the readiness stage.
// `AuthEventFields` is a free record, so the typed `stage` parameter is what checks the literal.
//
// The event holds the zone, the stage and the closed `reason`. It never holds the caught error, its
// message, its name or a URL (ports/logger.ts's redaction contract). `oidcDiscoveryReason` maps any
// value outside the closed list to 'other'.
import { oidcDiscoveryReason } from '../core/errors';
import type { OidcDiscoveryStage } from '../ports/logger';
import type { AuthRuntime } from '../runtime';

export function logDiscoveryFailed(runtime: AuthRuntime, stage: OidcDiscoveryStage, err: unknown): void {
  runtime.logger.event('oidc.discovery_failed', { zone: runtime.zone, stage, reason: oidcDiscoveryReason(err) });
}
