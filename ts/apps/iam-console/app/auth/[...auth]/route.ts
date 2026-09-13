// SPDX-License-Identifier: Apache-2.0
//
// The four auth routes (/iam/auth/login, /callback, /logout, /logout/callback), mounted as one
// catch-all route handler (spec § 4.2). The handler is an ASYNC function that awaits the runtime on
// each request; nothing builds the runtime at module scope, where `next build` would evaluate it.
//
// Task 5 makes createAuthRouteHandler rebuild the request URL from PAIGASUS_PUBLIC_ORIGIN and the
// basePath, because Next hands a route handler a URL with the bind address and no basePath
// (spec § 7.1, § 13 #1).
import { createAuthRouteHandler, type AuthRuntime } from '@paigasus/auth/server';
import { authRuntime } from '../../../lib/auth';

// One handler per runtime. The runtime is a process singleton, so this holds one entry.
const handlers = new WeakMap<AuthRuntime, (req: Request) => Promise<Response>>();

async function handle(req: Request): Promise<Response> {
  const runtime = await authRuntime();
  let handler = handlers.get(runtime);
  if (handler === undefined) {
    handler = createAuthRouteHandler(runtime);
    handlers.set(runtime, handler);
  }
  return handler(req);
}

export async function GET(req: Request): Promise<Response> {
  return handle(req);
}

export async function POST(req: Request): Promise<Response> {
  return handle(req);
}
