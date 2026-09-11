// SPDX-License-Identifier: Apache-2.0
//
// The package's named errors. This file has NO 'use client' directive, so server code can import
// them as real classes. (An export of a 'use client' module is a client reference on the server,
// and `instanceof` against it is always false.)

/**
 * An href that is not a single-slash path (spec § 6.2), or a nav entry whose href and declared
 * zone disagree (§ 7.2 rule 2). The message never echoes the href: an href can carry user data.
 */
export class ZoneLinkError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'ZoneLinkError';
  }
}

/** A zone map that cannot work: the current zone is not in it, or two zones share a base path. */
export class ZoneConfigError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'ZoneConfigError';
  }
}

/**
 * useZone() with no ZoneProvider above it. There is deliberately no default map: a default would
 * make every cross-zone link look same-zone, which is the AC 1 bug (spec § 6.1).
 */
export class ZoneProviderMissingError extends Error {
  constructor() {
    super('useZone() was called outside a ZoneProvider. There is no default zone map: a default would make every cross-zone link look same-zone.');
    this.name = 'ZoneProviderMissingError';
  }
}
