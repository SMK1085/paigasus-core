// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from 'vitest';
import { prnBuild, prnOrg, prnRegion, prnResourceId, prnResourceType, prnService } from '@paigasus/kernel';
import { prnFieldsCases } from './corpus';

describe('kernel PRN fields + build parity (wasm)', () => {
  it('corpus is present and non-empty', () => {
    expect(prnFieldsCases.length).toBeGreaterThan(0);
  });

  it.each(prnFieldsCases)('prn-fields($prn)', ({ prn, service, region, org, resource_type, resource_id }) => {
    expect(prnService(prn)).toBe(service);
    expect(prnRegion(prn)).toBe(region);
    expect(prnOrg(prn)).toBe(org);
    expect(prnResourceType(prn)).toBe(resource_type);
    expect(prnResourceId(prn)).toBe(resource_id);
    expect(prnBuild(service, region, org, resource_type, resource_id)).toBe(prn);
  });
});
