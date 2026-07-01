/* tslint:disable */
/* eslint-disable */

/**
 * Mint a UUIDv7 from an injected millisecond timestamp and a 20-char lowercase hex string of
 * entropy (throws `"bad-rand-hex"` if `rand_hex` is malformed).
 */
export function mintUuid7(unix_ms: number, rand_hex: string): string;

/**
 * Build a PRN from typed fields and return its canonical form, or throw `kind()`.
 */
export function prnBuild(service: string, region: string, org: string, resource_type: string, resource_id: string): string;

/**
 * Parse `s` and return its canonical form, or throw `kind()` on an invalid PRN.
 */
export function prnCanonicalize(s: string): string;

/**
 * Parse `s` and return its Cedar entity id, or throw `kind()`.
 */
export function prnCedarEntityId(s: string): string;

/**
 * Parse `s` and return its Cedar entity type (e.g. `Pgs::Iam::Project`), or throw `kind()`.
 */
export function prnCedarEntityType(s: string): string;

/**
 * Return the stable `PrnError::kind()` token for an invalid PRN, or `""` if `s` parses.
 */
export function prnErrorKind(s: string): string;

/**
 * Parse `s` and return its org field (hyphenated UUID, or `""` if absent), or throw `kind()`.
 */
export function prnOrg(s: string): string;

/**
 * Parse `s` and return its region field, or throw `kind()`.
 */
export function prnRegion(s: string): string;

/**
 * Parse `s` and return its resource-id field (hyphenated UUID), or throw `kind()`.
 */
export function prnResourceId(s: string): string;

/**
 * Parse `s` and return its resource-type field, or throw `kind()`.
 */
export function prnResourceType(s: string): string;

/**
 * Parse `s` and return its service field, or throw `kind()`.
 */
export function prnService(s: string): string;

/**
 * Browser-callable wrapper over [`paigasus_kernel::sum`]. Uses `i32` at the FFI boundary so the
 * JS surface is a plain `number` (matching the napi binding); the kernel fn is `i64`, cast at the
 * boundary. A future kernel fn needing the full `i64` range gets explicit handling then (shared
 * across all bindings — SMA-427 L5).
 */
export function sum(a: number, b: number): number;
