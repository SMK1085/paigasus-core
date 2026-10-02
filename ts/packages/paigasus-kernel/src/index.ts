// SPDX-License-Identifier: Apache-2.0
import {
  sum,
  prnCanonicalize,
  prnErrorKind,
  prnBuild,
  prnService,
  prnRegion,
  prnOrg,
  prnResourceType,
  prnResourceId,
  prnParseFields,
  mintUuid7,
  prnCedarEntityType,
  prnCedarEntityId,
} from '@paigasus/node-bindings';
import { randHex10 } from './mint-util';
import { toPrnParseResult, type PrnParseResult } from './prn-parse';

// `prnParseFields` is imported, NOT re-exported: `prnParse` below is its only public form (SMA-673 D2).
export { sum, prnCanonicalize, prnErrorKind, prnBuild, prnService, prnRegion, prnOrg, prnResourceType, prnResourceId, mintUuid7, prnCedarEntityType, prnCedarEntityId };
export type { PrnParseResult };

/** Parse a PRN with ONE kernel call. Never throws for any input; a `TypeError` means a glue defect. */
export function prnParse(prn: string): PrnParseResult {
  return toPrnParseResult(prnParseFields(prn));
}

/** Mint a UUIDv7 from the ambient clock + CSPRNG (the injected FFI mint is pure). */
export function mint(): string {
  return mintUuid7(Date.now(), randHex10());
}
