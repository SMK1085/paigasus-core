// SPDX-License-Identifier: Apache-2.0
//
// MSW handlers for IAM's HTTP surface in the vitest tier (spec § 9.1, AC 5). MSW intercepts
// `fetch`, which is how @paigasus/discovery probes `GET /v1/service-info`. It cannot intercept the
// SDK's gRPC calls, which run over node:http2 — the fake IAM serves those.
//
// The handler requires a bearer token, as IAM's route does (adapters/http/service_info.rs:10), and
// answers the BARE descriptor as JSON (contracts/proto/paigasus/common/v1/service_info.proto:23-33).
//
// A package-local copy of ts/apps/iam-console/tests/support/msw.ts (SMA-512 PR 2, task 4): that
// file is self-contained (its only dependency is `msw` itself) and stays in the app, where its own
// self-test (tests/integration/doubles/msw.test.ts) and other app-only integration tests keep
// using it. Duplicating this one small helper keeps this package's tests from reaching into the
// app for something that is not "the fake" — fake-iam.ts, which task 6 has since moved to this
// package's own testing/ directory.
import { http, HttpResponse, type HttpHandler } from 'msw';

export function serviceInfoHandlers(httpUrl: string, body: { service: string; version: string; capabilities: string[] } | { status: number }): HttpHandler[] {
  return [
    http.get(`${httpUrl}/v1/service-info`, ({ request }) => {
      if (!/^Bearer .+/.test(request.headers.get('authorization') ?? '')) {
        return HttpResponse.json({ error: { code: 'missing-authorization', message: 'a bearer token is required' } }, { status: 401 });
      }
      if ('status' in body) {
        return HttpResponse.json({ error: { code: 'internal', message: 'IAM is configured to fail' } }, { status: body.status });
      }
      return HttpResponse.json(body);
    }),
  ];
}
