// SPDX-License-Identifier: Apache-2.0
//
// The hop-by-hop rule that every in-process proxy in ts/ shares (SMA-640): the TLS terminator in
// this directory, gateway-console's counting forwarder, and ts/tooling/dev-stack.ts's default-zone
// proxy. It exists ONCE so that a later change cannot reach only one of them. SMA-640 replaced
// three copies of a fixed name list; the copies did not remove the fields that a `Connection`
// header names.
import type { IncomingHttpHeaders } from 'node:http';

/**
 * RFC 9110 § 7.6.1 hop-by-hop fields, plus proxy-authenticate / proxy-authorization. RFC 9110
 * § 11.7 does not call the last two hop-by-hop (a proxy MAY relay credentials); RFC 2616
 * § 13.5.1 did. These proxies never relay proxy credentials, so they are removed by policy. Do not
 * remove them from this set on the argument that RFC 9110 does not list them.
 */
const HOP_BY_HOP: ReadonlySet<string> = new Set(['connection', 'keep-alive', 'proxy-authenticate', 'proxy-authorization', 'proxy-connection', 'te', 'trailer', 'transfer-encoding', 'upgrade']);

/**
 * Names that a Connection nomination never removes (SMA-640 decision D6). A deliberate deviation
 * from RFC 9110 § 7.6.1: in Node the outgoing headers object IS the message framing, so removing
 * a nominated content-length from a GET makes Node write the body with no framing, and the
 * upstream reads it as a second, pipelined request (measured, SMA-640 spec § 6.5).
 */
const NEVER_NOMINATED: ReadonlySet<string> = new Set(['content-length']);

/**
 * The headers a proxy may forward: every entry except the fixed hop-by-hop fields, the fields that
 * the `Connection` header names (RFC 9110 § 7.6.1), and `undefined` values. Use it for the request
 * AND the response. Pure and total: it never throws (both proxy files call it from event callbacks,
 * where a throw kills the process), it never logs, and it returns a new object.
 */
export function forwardableHeaders(headers: IncomingHttpHeaders): IncomingHttpHeaders {
  // Node joins repeated Connection lines into one string with ", ". The array branch is defence in
  // depth for a caller that builds headers by hand; @types/node does not type one.
  const connection = headers.connection as string | readonly string[] | undefined;
  const values: readonly string[] = typeof connection === 'string' ? [connection] : (connection ?? []);
  const nominated = new Set<string>();
  for (const value of values) {
    for (const token of value.split(',')) {
      // Node keeps the sender's case and spaces, so both steps are needed. An empty token needs no
      // special case: no header name is empty, so it removes nothing.
      const name = token.trim().toLowerCase();
      if (!NEVER_NOMINATED.has(name)) nominated.add(name);
    }
  }
  const out: IncomingHttpHeaders = {};
  for (const [name, value] of Object.entries(headers)) {
    // Node lower-cases header names in req.headers and res.headers, so the comparison is exact.
    if (value === undefined || HOP_BY_HOP.has(name) || nominated.has(name)) continue;
    out[name] = value;
  }
  return out;
}
