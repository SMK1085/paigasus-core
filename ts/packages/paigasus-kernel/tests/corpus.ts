// SPDX-License-Identifier: Apache-2.0
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

export interface ParityCase {
  a: number;
  b: number;
  expected: number;
}

// Single resolved path constant: the committed corpus lives in the Rust parity crate. From this
// file: tests -> paigasus-kernel -> packages -> ts -> repo root == four `../`.
const corpusPath = fileURLToPath(
  new URL('../../../../rs/crates/libs/paigasus-kernel-parity/vectors/sum.json', import.meta.url),
);

export const cases: ParityCase[] = JSON.parse(readFileSync(corpusPath, 'utf8')) as ParityCase[];
