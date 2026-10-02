// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from 'vitest';
import { prnParseFields } from '@paigasus/node-bindings';
import { prnParse } from '@paigasus/kernel/napi';
import type { PrnParseResult } from '../src/prn-parse';
import { prnParseCases, type PrnParseCase } from './corpus';

function expected(c: PrnParseCase): PrnParseResult {
  return c.error_kind === '' ? { ok: true, service: c.service, region: c.region, org: c.org, resourceType: c.resource_type, resourceId: c.resource_id } : { ok: false, errorKind: c.error_kind };
}

describe('kernel one-call PRN parse parity (napi)', () => {
  it('corpus is present and holds valid, invalid and region-ful rows', () => {
    expect(prnParseCases.length).toBeGreaterThan(0);
    expect(prnParseCases.some((c) => c.error_kind === '')).toBe(true);
    expect(prnParseCases.some((c) => c.error_kind !== '')).toBe(true);
    expect(prnParseCases.some((c) => c.error_kind === '' && c.region !== '' && c.org !== '')).toBe(true);
  });

  it.each(prnParseCases)('prn-parse($input)', (c) => {
    expect(prnParseFields(c.input)).toEqual([c.error_kind, c.service, c.region, c.org, c.resource_type, c.resource_id]);
    expect(prnParse(c.input)).toEqual(expected(c));
  });
});
