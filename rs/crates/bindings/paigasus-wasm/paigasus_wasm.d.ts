/* tslint:disable */
/* eslint-disable */

/**
 * Browser-callable wrapper over [`paigasus_kernel::sum`]. Uses `i32` at the FFI boundary so the
 * JS surface is a plain `number` (matching the napi binding); the kernel fn is `i64`, cast at the
 * boundary. A future kernel fn needing the full `i64` range gets explicit handling then (shared
 * across all bindings — SMA-427 L5).
 */
export function sum(a: number, b: number): number;
