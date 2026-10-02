// SPDX-License-Identifier: Apache-2.0
//
// The ONE typed view of the kernel's one-call PRN parse (SMA-673 D1, D2). Both bindings return the
// positional wire form `[errorKind, service, region, org, resourceType, resourceId]`, so that the
// wasm and napi signatures are identical (`string[]`). The position map lives HERE and nowhere
// else: src/index.ts and src/wasm.ts export `prnParse` and do NOT re-export the raw
// `prnParseFields`, so no consumer can index a position.
//
// The two TypeErrors below are glue or binding defects, not bad input. They check the wire
// convention only. PRN grammar (for example a non-empty service on success) is the kernel's, and
// the corpus replays prove it.

export type PrnParseResult = { ok: true; service: string; region: string; org: string; resourceType: string; resourceId: string } | { ok: false; errorKind: string };

type Wire = readonly [string, string, string, string, string, string];

function isWire(wire: readonly unknown[]): wire is Wire {
  return wire.length === 6 && wire.every((element) => typeof element === 'string');
}

/** Map the six-string wire form to a `PrnParseResult`. Throws a `TypeError` on a broken wire form. */
export function toPrnParseResult(wire: readonly unknown[]): PrnParseResult {
  if (!isWire(wire)) {
    // The message names the shape only, never a value: a value can hold part of an attacker's PRN.
    throw new TypeError(`prnParseFields returned ${wire.length} elements or a non-string element; the wire form is six strings`);
  }
  const [errorKind, service, region, org, resourceType, resourceId] = wire;
  if (errorKind !== '') {
    if (service !== '' || region !== '' || org !== '' || resourceType !== '' || resourceId !== '') {
      throw new TypeError('prnParseFields returned an error kind and a non-empty field');
    }
    return { ok: false, errorKind };
  }
  return { ok: true, service, region, org, resourceType, resourceId };
}
