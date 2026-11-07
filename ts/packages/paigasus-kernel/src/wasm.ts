// SPDX-License-Identifier: Apache-2.0
import {
  sum,
  prnCanonicalize,
  prnErrorKind as rawPrnErrorKind,
  prnBuild,
  prnService as rawPrnService,
  prnRegion as rawPrnRegion,
  prnOrg as rawPrnOrg,
  prnResourceType as rawPrnResourceType,
  prnResourceId as rawPrnResourceId,
  prnParseFields,
  mintUuid7,
  prnCedarEntityType,
  prnCedarEntityId,
} from '@paigasus/wasm';
import { randHex10 } from './mint-util';
import { toPrnParseResult, type PrnParseResult } from './prn-parse';

// `prnParseFields` is imported, NOT re-exported: `prnParse` below is its only public form (SMA-673 D2).
export { sum, prnCanonicalize, prnBuild, mintUuid7, prnCedarEntityType, prnCedarEntityId };
export type { PrnParseResult };

// SMA-725: the six single-field accessors are deprecated. Each one parses the PRN again. Each is
// the raw binding function under a JSDoc-tagged `const`, so the behaviour does not change. The six
// blocks are the same in src/index.ts; tests/deprecated-accessors.test.ts holds the two in step.
// `paigasus/no-single-field-prn-accessor` (@paigasus/next-config/eslint) stops new callers.

/**
 * Return the stable `PrnError::kind()` token for an invalid PRN, or `""` if `s` parses.
 *
 * @deprecated Use `prnParse(prn)`: one kernel call returns every field or the error kind. For an
 * invalid PRN it returns `{ ok: false, errorKind }` (SMA-725). This accessor parses the PRN again.
 */
export const prnErrorKind = rawPrnErrorKind;

/**
 * Parse `s` and return its service field, or throw `kind()`.
 *
 * @deprecated Use `prnParse(prn)`: one kernel call returns every field or the error kind
 * (SMA-725). This accessor parses the PRN again on each call.
 */
export const prnService = rawPrnService;

/**
 * Parse `s` and return its region field, or throw `kind()`.
 *
 * @deprecated Use `prnParse(prn)`: one kernel call returns every field or the error kind
 * (SMA-725). This accessor parses the PRN again on each call.
 */
export const prnRegion = rawPrnRegion;

/**
 * Parse `s` and return its org field (hyphenated UUID, or `""` if absent), or throw `kind()`.
 *
 * @deprecated Use `prnParse(prn)`: one kernel call returns every field or the error kind
 * (SMA-725). This accessor parses the PRN again on each call.
 */
export const prnOrg = rawPrnOrg;

/**
 * Parse `s` and return its resource-type field, or throw `kind()`.
 *
 * @deprecated Use `prnParse(prn)`: one kernel call returns every field or the error kind
 * (SMA-725). This accessor parses the PRN again on each call.
 */
export const prnResourceType = rawPrnResourceType;

/**
 * Parse `s` and return its resource-id field (hyphenated UUID), or throw `kind()`.
 *
 * @deprecated Use `prnParse(prn)`: one kernel call returns every field or the error kind
 * (SMA-725). This accessor parses the PRN again on each call.
 */
export const prnResourceId = rawPrnResourceId;

/** Parse a PRN with ONE kernel call. Never throws for any input; a `TypeError` means a glue defect. */
export function prnParse(prn: string): PrnParseResult {
  return toPrnParseResult(prnParseFields(prn));
}

/** Mint a UUIDv7 from the ambient clock + CSPRNG (the injected FFI mint is pure). */
export function mint(): string {
  return mintUuid7(Date.now(), randHex10());
}
