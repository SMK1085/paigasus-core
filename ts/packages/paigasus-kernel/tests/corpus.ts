// SPDX-License-Identifier: Apache-2.0
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

// From this file: tests -> paigasus-kernel -> packages -> ts -> repo root == four `../`.
function load<T>(name: string): T[] {
  const p = fileURLToPath(new URL(`../../../../rs/crates/libs/paigasus-kernel-parity/vectors/${name}.json`, import.meta.url));
  return JSON.parse(readFileSync(p, 'utf8')) as T[];
}

export interface SumCase {
  a: number;
  b: number;
  expected: number;
}
export interface Uuid7Case {
  unix_ms: number;
  rand_hex: string;
  expected_uuid: string;
}
export interface PrnCanonicalCase {
  input: string;
  error_kind: string;
  canonical: string | null;
}
export interface PrnCedarCase {
  prn: string;
  entity_type: string;
  entity_id: string;
}
export interface PrnFieldsCase {
  prn: string;
  service: string;
  region: string;
  org: string;
  resource_type: string;
  resource_id: string;
}

export const sumCases = load<SumCase>('sum');
export const uuid7Cases = load<Uuid7Case>('uuid7');
export const prnCanonicalCases = load<PrnCanonicalCase>('prn_canonical');
export const prnCedarCases = load<PrnCedarCase>('prn_cedar');
export const prnFieldsCases = load<PrnFieldsCase>('prn_fields');

// Back-compat for the existing sum replays.
export type ParityCase = SumCase;
export const cases = sumCases;
