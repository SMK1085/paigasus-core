// SPDX-License-Identifier: Apache-2.0
/** 10 CSPRNG bytes as 20 lowercase hex chars — the FFI `randHex` wire format. */
export function randHex10(): string {
  const bytes = new Uint8Array(10);
  crypto.getRandomValues(bytes);
  return [...bytes].map((b) => b.toString(16).padStart(2, '0')).join('');
}
