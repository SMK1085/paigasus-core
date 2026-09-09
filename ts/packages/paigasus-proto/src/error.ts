// SPDX-License-Identifier: Apache-2.0
import { ErrorDomain, ErrorDomainSchema, ErrorReason, ErrorReasonSchema } from './generated/paigasus/common/v1/error_pb.js';

const REASON_PREFIX = 'ERROR_REASON_';
const DOMAIN_PREFIX = 'ERROR_DOMAIN_';
const DOMAIN_SUFFIX = '.paigasus.io';
const UNSPECIFIED = 'UNSPECIFIED';

/**
 * The grammar a wire token must already satisfy, as an ASCII ALLOW-list:
 * lowercase letter first, then lowercase letters, digits and single hyphens,
 * no trailing hyphen.
 *
 * This is checked BEFORE any case transform, and that order is the whole point.
 * MEASURED on Node 24.16.0: `"ınternal".toUpperCase()` is `"INTERNAL"` and
 * `"ſlug-conflict".toUpperCase()` is `"SLUG-CONFLICT"` — JavaScript folds
 * U+0131 (dotless i) and U+017F (long s) exactly as Rust's `str::to_uppercase`
 * does. So a deny-list applied to the uppercased string would resolve both of
 * those to real registry values. Mirrors `is_wire_token` in
 * rs/crates/libs/paigasus-proto/src/error.rs:51-64.
 */
const WIRE_TOKEN = /^[a-z][a-z0-9]*(-[a-z0-9]+)*$/;

/**
 * Reverse lookups, built once from the generated descriptors rather than
 * tabulated. Same reasoning as `capabilityWireKey`: reading the descriptor's
 * full proto `name` — not the TypeScript enum's reverse map — means
 * protobuf-es's prefix-stripping heuristic (`findEnumSharedPrefix`) cannot
 * affect the transform, which keeps exact parity with the Rust side.
 */
const REASON_BY_PROTO_NAME = new Map<string, ErrorReason>(ErrorReasonSchema.values.map((v) => [v.name, v.number]));
const DOMAIN_BY_PROTO_NAME = new Map<string, ErrorDomain>(ErrorDomainSchema.values.map((v) => [v.name, v.number]));

/** The kebab wire spelling of a reason, or `undefined` for the zero sentinel. */
export function asWireReason(reason: ErrorReason): string | undefined {
  const name = ErrorReasonSchema.value[reason]?.name;
  if (name === undefined || !name.startsWith(REASON_PREFIX)) {
    return undefined;
  }
  const short = name.slice(REASON_PREFIX.length);
  if (short === UNSPECIFIED) {
    return undefined;
  }
  return short.toLowerCase().replace(/_/g, '-');
}

/**
 * The reason a kebab wire string names, or `undefined` when it is malformed or
 * absent from this build's registry.
 *
 * An unknown-but-well-formed code is not an error: a newer service may emit a
 * code this build predates, and ADR-0019 decision 9 requires the consumer to
 * fall back to a generic presentation rather than throw.
 */
export function fromWireReason(reason: string): ErrorReason | undefined {
  if (!WIRE_TOKEN.test(reason)) {
    return undefined;
  }
  // Safe only because the allow-list above already restricted the input to
  // [a-z0-9-]; on that alphabet toUpperCase is pure ASCII.
  const name = `${REASON_PREFIX}${reason.toUpperCase().replace(/-/g, '_')}`;
  const value = REASON_BY_PROTO_NAME.get(name);
  return value === undefined || value === ErrorReason.UNSPECIFIED ? undefined : value;
}

/** The wire spelling of a domain, or `undefined` for the zero sentinel. */
export function asWireDomain(domain: ErrorDomain): string | undefined {
  const name = ErrorDomainSchema.value[domain]?.name;
  if (name === undefined || !name.startsWith(DOMAIN_PREFIX)) {
    return undefined;
  }
  const short = name.slice(DOMAIN_PREFIX.length);
  if (short === UNSPECIFIED) {
    return undefined;
  }
  return `${short.toLowerCase().replace(/_/g, '-')}${DOMAIN_SUFFIX}`;
}

/** The domain a wire string names, or `undefined` when it is malformed or unknown. */
export function fromWireDomain(domain: string): ErrorDomain | undefined {
  if (!domain.endsWith(DOMAIN_SUFFIX)) {
    return undefined;
  }
  const label = domain.slice(0, domain.length - DOMAIN_SUFFIX.length);
  if (!WIRE_TOKEN.test(label)) {
    return undefined;
  }
  const name = `${DOMAIN_PREFIX}${label.toUpperCase().replace(/-/g, '_')}`;
  const value = DOMAIN_BY_PROTO_NAME.get(name);
  return value === undefined || value === ErrorDomain.UNSPECIFIED ? undefined : value;
}
