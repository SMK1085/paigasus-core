// SPDX-License-Identifier: Apache-2.0
import { sum, prnCanonicalize, prnErrorKind, prnBuild, prnService, prnRegion, prnOrg, prnResourceType, prnResourceId, mintUuid7, prnCedarEntityType, prnCedarEntityId } from '@paigasus/node-bindings';
import { randHex10 } from './mint-util';

export { sum, prnCanonicalize, prnErrorKind, prnBuild, prnService, prnRegion, prnOrg, prnResourceType, prnResourceId, mintUuid7, prnCedarEntityType, prnCedarEntityId };

/** Mint a UUIDv7 from the ambient clock + CSPRNG (the injected FFI mint is pure). */
export function mint(): string {
  return mintUuid7(Date.now(), randHex10());
}
