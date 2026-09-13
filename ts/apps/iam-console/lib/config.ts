// SPDX-License-Identifier: Apache-2.0
//
// This app's ONE call to defineRuntimeConfig (spec § 4.1). It composes three parts into one
// schema, one parse and one place: @paigasus/auth's shape, @paigasus/discovery's shape, and the one
// key this app owns.
//
// No accessor runs at module scope. getRuntimeConfig() throws during `next build`
// (phase-production-build), so a module-scope read fails the build loudly instead of baking the
// builder's environment into the image.
import 'server-only';
import { z } from 'zod';
import { authEnvShape } from '@paigasus/auth/server';
import { discoveryEnvShape } from '@paigasus/discovery/server';
import { defineRuntimeConfig } from '@paigasus/next-config/runtime';

/** Why `value` is not an acceptable IAM gRPC address, or null when it is. */
function grpcUrlProblem(value: string): string | null {
  if (!URL.canParse(value)) return 'must be an absolute URL';
  const url = new URL(value);
  if (url.protocol !== 'http:' && url.protocol !== 'https:') return 'must use http: or https:';
  if (url.username !== '' || url.password !== '') return 'must not carry credentials';
  if (url.search !== '' || url.hash !== '') return 'must not carry a query or a fragment';
  return null;
}

/**
 * IAM's gRPC address. A separate key from PAIGASUS_SERVICES.iam because IAM listens on two
 * addresses: HTTP on 8080, which discovery probes, and gRPC on 9090, which the SDK calls
 * (rs/crates/services/paigasus-iam/src/config.rs:811-812).
 *
 * One transform, not a chain of refines: a refine that calls `new URL` after a failed
 * `URL.canParse` refine would THROW, because zod 4 runs every refine. The value is canonicalized
 * to `<origin><path>` with no trailing slash, the form @paigasus/discovery gives its addresses.
 * The message never contains the value (next-config renders only the issue code for this key).
 */
const iamGrpcUrl = z.string().transform((value, ctx) => {
  const problem = grpcUrlProblem(value);
  if (problem !== null) {
    ctx.addIssue({ code: 'custom', message: problem });
    return z.NEVER;
  }
  const url = new URL(value);
  return `${url.origin}${url.pathname.replace(/\/+$/, '')}`;
});

/** discovery's service map, plus one rule: the console cannot run without an `iam` entry. */
const servicesWithIam = discoveryEnvShape.PAIGASUS_SERVICES.refine((services) => Object.hasOwn(services, 'iam'), { error: 'must contain an "iam" entry' });

export const { getRuntimeConfig, getPublicConfig } = defineRuntimeConfig({
  ...authEnvShape,
  ...discoveryEnvShape,
  PAIGASUS_SERVICES: servicesWithIam,
  PAIGASUS_IAM_GRPC_URL: iamGrpcUrl,
});

export type ConsoleConfig = ReturnType<typeof getRuntimeConfig>;
