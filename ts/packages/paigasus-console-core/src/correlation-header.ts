// SPDX-License-Identifier: Apache-2.0
//
// The two request-header names proxy.ts writes, in a module that imports nothing but the
// `server-only` guard. proxy.ts imports THIS file and not ./correlation, so the proxy bundle never
// reaches `next/headers`. spec § 13 measured that `server-only` itself loads in proxy.ts.
import 'server-only';

/** IAM adopts an incoming value that parses as a UUID (paigasus-observability correlation.rs:103-119). */
export const CORRELATION_HEADER = 'paigasus-correlation-id';

/** The public path of the console request, with the basePath and without the query. */
export const REQUEST_PATH_HEADER = 'x-paigasus-request-path';
