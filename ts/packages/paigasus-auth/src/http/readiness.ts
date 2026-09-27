// SPDX-License-Identifier: Apache-2.0
//
// The readiness probe of a console zone (SMA-705): `GET <basePath>/readyz`. Each app mounts it with
// a route file of three lines (D5). The chart's readinessProbe calls it.
//
// THE STATES (D7). 200 {"status":"ready"} when the auth runtime exists and its OIDC client has
// discovered the IdP. 503 {"status":"unready"} in every other case. Both carry
// `cache-control: no-store`. No other status or body occurs. The body never says why: the route is
// public, so an anonymous caller gets no reason. The operator reads the log.
//
// READY IS STICKY (D2). The adapter keeps its first successful discovery for the life of the
// process and never discovers again, so this route stays ready after that. An IdP outage after
// the first success affects every pod. Removing every pod would also remove the pages of signed-in
// users, and it would not make a sign-in work.
//
// THE ROUTE NEVER WAITS FOR DISCOVERY (D3). It reads a synchronous status. When discovery is idle,
// it starts one through `ensureDiscovered()` (SMA-704) without `await` and answers at once. The
// next probe sees the result. One wait stays: the first call of a process builds the runtime, and
// that includes the Redis connect.
//
// THE LOG (D8, D10). A discovery attempt that this route started and that fails logs one
// `oidc.discovery_failed { zone, stage: 'readiness', reason }` through logDiscoveryFailed. A runtime
// build that fails logs `readiness.runtime_failed { error }`, where `error` is the error's `name`
// only. It never logs the error object, its message or its `input`: node-redis parses the Redis URL
// with `new URL()`, and that TypeError holds the URL, password included (measured, spec D10).
//
// NO THROW OUT OF A DETACHED PROMISE (D11). The `.catch` of the started attempt wraps its log call
// in try/catch. A logger that throws would otherwise make a new unhandled rejection.
import type { AuthLogger } from '../ports/logger';
import type { AuthRuntime } from '../runtime';
import { logDiscoveryFailed } from './discovery-log';

/** The two probe bodies (D7). */
type ReadinessStatus = 'ready' | 'unready';

function probeResponse(code: 200 | 503, status: ReadinessStatus): Response {
  return Response.json({ status }, { status: code, headers: { 'cache-control': 'no-store' } });
}

/**
 * The error's `name`, or 'unknown_error'. This is the rule of adapters/oidc.ts's `libraryErrorName`.
 * That helper is private to the adapter and names library errors, so this file has its own copy.
 */
function errorName(err: unknown): string {
  return err instanceof Error ? err.name : 'unknown_error';
}

/**
 * The readiness answer (SMA-705). `getRuntime` is the app's runtime getter (for example
 * `authRuntime`). It can throw synchronously or reject. `logger` is for the runtime-failure path
 * only, where no runtime exists: pass the logger that the app gives getAuthRuntime.
 */
export async function readinessResponse(getRuntime: () => Promise<AuthRuntime>, logger: AuthLogger): Promise<Response> {
  let runtime: AuthRuntime;
  try {
    // The call is inside the try, so a synchronous throw from the getter lands here too.
    runtime = await getRuntime();
  } catch (err) {
    logger.event('readiness.runtime_failed', { error: errorName(err) });
    return probeResponse(503, 'unready');
  }
  const status = runtime.oidc.discoveryStatus();
  if (status === 'discovered') return probeResponse(200, 'ready');
  if (status === 'idle') {
    // Not awaited (D3). The `.catch` logs every rejection and never rethrows. A rejection that is
    // not an OidcDiscoveryFailed gets the reason 'other' from oidcDiscoveryReason (D8).
    runtime.oidc.ensureDiscovered().catch((err: unknown) => {
      try {
        logDiscoveryFailed(runtime, 'readiness', err);
      } catch {
        // D11. A logger that throws must not make an unhandled rejection. Nothing can log it.
      }
    });
  }
  return probeResponse(503, 'unready');
}
