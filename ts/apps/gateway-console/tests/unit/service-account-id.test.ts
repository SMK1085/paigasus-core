// SPDX-License-Identifier: Apache-2.0
//
// A service account is a principal: IAM names it `prn:pgs:iam:::principal/<uuid>`
// (paigasus-iam-core value.rs). The `sa` search parameter carries the UUID only (SMA-636 D14).
import { describe, expect, it } from 'vitest';
import { parseAccountParam, serviceAccountIdOf, serviceAccountPrn } from '../../app/(console)/service-accounts/service-account-id';

const ID = '0190a1e5-0000-7000-8000-0000000000a1';

describe('service account ids', () => {
  it('builds the principal PRN, lower case', () => {
    expect(serviceAccountPrn(ID.toUpperCase())).toBe(`prn:pgs:iam:::principal/${ID}`);
  });

  it('reads the id back, lower case, and refuses anything else', () => {
    expect(serviceAccountIdOf(`prn:pgs:iam:::principal/${ID.toUpperCase()}`)).toBe(ID);
    expect(serviceAccountIdOf(`prn:pgs:iam:::organization/${ID}`)).toBeNull();
    expect(serviceAccountIdOf('prn:pgs:iam:::principal/not-a-uuid')).toBeNull();
    expect(serviceAccountIdOf('')).toBeNull();
  });

  it('reads ?sa= as a UUID or as no selection, never as an error', () => {
    expect(parseAccountParam(ID)).toBe(ID);
    expect(parseAccountParam([ID.toUpperCase(), 'x'])).toBe(ID);
    for (const raw of [undefined, '', 'abc', `${ID} `]) expect(parseAccountParam(raw)).toBeNull();
  });
});
