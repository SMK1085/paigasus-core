// SPDX-License-Identifier: Apache-2.0
import { Capability, CapabilitySchema } from './generated/paigasus/common/v1/service_info_pb.js';

const PREFIX = 'CAPABILITY_';

/**
 * The wire string a capability is advertised as, or `undefined` for the zero
 * sentinel.
 *
 * Derived from the generated `CapabilitySchema` descriptor's value names
 * rather than tabulated, so there is no second copy of the registry to drift
 * against the proto. Reading the descriptor's full proto name — rather than
 * the TypeScript enum's reverse map — means protobuf-es's prefix-stripping
 * heuristic (`findEnumSharedPrefix`, which degrades for the whole enum if any
 * value's short name is empty or starts with a digit) cannot affect this
 * transform. That keeps exact parity with the Rust `as_wire_key`.
 *
 * There is deliberately no reverse parser. A client compares advertised strings
 * against keys it knows; it never needs to resolve an arbitrary string, because
 * an unknown key is ignored by contract.
 */
export function capabilityWireKey(capability: Capability): string | undefined {
  const name = CapabilitySchema.value[capability]?.name;
  if (name === undefined || name === `${PREFIX}UNSPECIFIED`) {
    return undefined;
  }
  return name.slice(PREFIX.length).toLowerCase().replace(/_/g, '.');
}
