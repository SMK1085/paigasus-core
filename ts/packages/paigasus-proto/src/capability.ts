// SPDX-License-Identifier: Apache-2.0
import { Capability } from './generated/paigasus/common/v1/service_info_pb.js';

/**
 * The wire string a capability is advertised as, or `undefined` for the zero
 * sentinel.
 *
 * Derived from the generated enum's member names rather than tabulated, so
 * there is no second copy of the registry to drift against the proto. Note the
 * asymmetry with the Rust helper: protobuf-es already strips the `CAPABILITY_`
 * prefix from member names, so only the lowercase-and-dot half of the mapping
 * rule remains here.
 *
 * There is deliberately no reverse parser. A client compares advertised strings
 * against keys it knows; it never needs to resolve an arbitrary string, because
 * an unknown key is ignored by contract.
 */
export function capabilityWireKey(capability: Capability): string | undefined {
  const name: string | undefined = Capability[capability];
  if (name === undefined || name === 'UNSPECIFIED') {
    return undefined;
  }
  return name.toLowerCase().replace(/_/g, '.');
}
