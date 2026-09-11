// SPDX-License-Identifier: Apache-2.0
//
// Pure functions, and NO 'use client' directive (spec § 5.2): a server layout must be able to
// call them.
//
// The map is read ONLY with Object.hasOwn and Object.entries (spec § 6.1). The map that reaches
// the client is a plain object, so `zones['constructor']` is an inherited function there.
import { ZoneConfigError, ZoneLinkError } from './errors';

/** Zone id -> canonical base path (`''` for a root-mounted zone, else `/segment[/segment]`). */
export type ZoneMap = Readonly<Record<string, string>>;

/** A resolved href: the matched zone, its base path, and the href with that base path removed. */
export type ZoneTarget = {
  readonly zone: string;
  readonly basePath: string;
  /** The pathname without `basePath` (an empty remainder becomes `/`), plus the query and fragment. */
  readonly rest: string;
};

/** Only for the dot-segment check. Any origin works; `.invalid` can never resolve. */
const PARSE_BASE = 'http://zone.invalid';

function reject(reason: string): never {
  throw new ZoneLinkError(`ZoneLink href rejected: ${reason}. Write the full path as the ingress sees it, for example "/iam/users". (The href is withheld: it can carry user data.)`);
}

function hasControlOrWhitespace(href: string): boolean {
  // A loop, not a regex: ESLint's no-control-regex refuses control characters in a pattern.
  for (let index = 0; index < href.length; index += 1) {
    const code = href.charCodeAt(index);
    if (code <= 0x20 || code === 0x7f) return true;
  }
  return /\s/u.test(href);
}

/** The part of `pathWithSuffix` before the first `?` or `#`. */
export function pathOf(pathWithSuffix: string): string {
  const cut = pathWithSuffix.search(/[?#]/);
  return cut === -1 ? pathWithSuffix : pathWithSuffix.slice(0, cut);
}

/**
 * A segment-aware prefix test: `pathname` matches when it equals `prefix` or continues with
 * `prefix + '/'`. `''` matches every absolute path (a root-mounted zone); `'/'` matches only
 * `'/'` itself, never `'/users'`. Shared by `resolveZone` (§ 6.3) and `PrimaryNav`'s active-entry
 * match (§ 7.4), so the segment-boundary rule has one definition, not two that can drift apart.
 */
export function isAtOrUnder(pathname: string, prefix: string): boolean {
  return pathname === prefix || pathname.startsWith(`${prefix}/`);
}

/**
 * The rules of @paigasus/auth's return-to.ts, plus a dot-segment rule (spec § 6.2). The dot-segment
 * test compares the pathname before and after WHATWG URL normalisation: if they differ, the browser
 * would go somewhere other than the zone this function chose.
 */
function validate(href: string): { readonly pathname: string; readonly suffix: string } {
  if (href === '') reject('it is empty');
  if (hasControlOrWhitespace(href)) reject('it contains a control character or whitespace');
  if (href.includes('\\')) reject('it contains a backslash, which a browser reads as "/"');
  if (!href.startsWith('/')) reject('it does not start with "/" (absolute URLs and relative paths are refused)');
  if (href.startsWith('//')) reject('it is protocol-relative');
  const pathname = pathOf(href);
  if (new URL(pathname, PARSE_BASE).pathname !== pathname) {
    reject('its path changes under URL normalisation (a dot segment, or a character that the browser re-encodes)');
  }
  return { pathname, suffix: href.slice(pathname.length) };
}

/**
 * Segment-aware longest-prefix match (spec § 6.3). A zone with base path `b` matches when the
 * pathname equals `b` or starts with `b + '/'`; a root zone (`b === ''`) matches every pathname
 * because `pathname` always starts with `/` after `validate`. Only the pathname takes part in the
 * match. Returns null when no zone matches.
 *
 * Throws ZoneLinkError for a malformed href. There is no fallback to a plain <a>: a silent
 * fallback would hide a wrong href until a user clicks it.
 */
export function resolveZone(href: string, zones: ZoneMap): ZoneTarget | null {
  const { pathname, suffix } = validate(href);
  let best: { readonly zone: string; readonly basePath: string } | null = null;
  for (const [zone, basePath] of Object.entries(zones)) {
    const matches = isAtOrUnder(pathname, basePath);
    // Strictly longer wins. Base paths are unique (assertZoneMap), so the winner is unique.
    if (matches && (best === null || basePath.length > best.basePath.length)) {
      best = { zone, basePath };
    }
  }
  if (best === null) return null;
  const remainder = pathname.slice(best.basePath.length);
  return { zone: best.zone, basePath: best.basePath, rest: `${remainder === '' ? '/' : remainder}${suffix}` };
}

/**
 * The two map checks of spec § 6.1. Returns the current zone's base path. A literal map (a test,
 * the fixture) does not pass through @paigasus/next-config's zoneMapFromJson, so the checks live
 * here too. Zone ids are public routing labels, so the messages name them.
 */
export function assertZoneMap(zone: string, zones: ZoneMap): string {
  if (!Object.hasOwn(zones, zone)) {
    throw new ZoneConfigError(`ZoneProvider: the current zone "${zone}" is not a key of the zone map.`);
  }
  const owners = new Map<string, string>();
  for (const [id, basePath] of Object.entries(zones)) {
    const first = owners.get(basePath);
    if (first !== undefined) {
      throw new ZoneConfigError(`ZoneProvider: zones "${first}" and "${id}" have the same base path. Every zone needs its own prefix.`);
    }
    owners.set(basePath, id);
  }
  const own = zones[zone];
  if (own === undefined) {
    // Unreachable after the hasOwn check; noUncheckedIndexedAccess needs the narrowing.
    throw new ZoneConfigError(`ZoneProvider: the current zone "${zone}" has no base path.`);
  }
  return own;
}
