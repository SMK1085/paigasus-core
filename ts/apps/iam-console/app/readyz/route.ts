// SPDX-License-Identifier: Apache-2.0
//
// `GET /iam/readyz`: the chart's readiness probe (SMA-705). Public (proxy.ts lists it). It answers
// 200 {"status":"ready"} after the auth runtime is built and one OIDC discovery succeeded, and 503
// {"status":"unready"} before that. It never waits for discovery. @paigasus/auth holds the logic
// (readinessResponse, SMA-705 D5). `/healthz` stays the check that touches no dependency.
import { connection } from 'next/server';
import { readinessResponse } from '@paigasus/auth/server';
import { logger } from '@paigasus/console-core';
import { authRuntime } from '../../lib/auth';

export async function GET(): Promise<Response> {
  await connection();
  return readinessResponse(authRuntime, logger);
}
