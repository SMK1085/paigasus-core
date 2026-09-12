// SPDX-License-Identifier: Apache-2.0
//
// `GET /iam/healthz` — public (proxy.ts lists it), and the target of the standalone-runtime test
// (spec § 9.5). It answers the client-safe slice only: the zone and the zone map. It still runs the
// FULL configuration parse, so a server that answers 200 here has a valid environment.
import { connection } from 'next/server';
import { getPublicConfig } from '../../lib/config';

export async function GET(): Promise<Response> {
  await connection();
  const { zone, zones } = getPublicConfig();
  return Response.json({ zone, zones }, { headers: { 'cache-control': 'no-store' } });
}
