// SPDX-License-Identifier: Apache-2.0
//
// The URL-consistency check of the node pages (spec § 5.2). A team or project PRN holds the org, so
// for /orgs/<wrong org>/teams/<team> IAM itself answers prn-mismatch (InvalidArgument,
// adapters/grpc/tenancy.rs's get_team and get_project handlers), and the loaders map that
// invalid-input answer to notFound(). For [org] this check is a second guard. The project page's
// [team] check is the one IAM cannot make: a project PRN holds no team, so only GetProject's
// team_prn shows a wrong [team]. On a mismatch the page renders notFound(). This is not an access
// check: IAM is that.
import 'server-only';
import { parseTenancyPrn, type TenancyKind } from '@paigasus/console-core';

export function sameNode(prn: string, kind: TenancyKind, id: string): boolean {
  const ref = parseTenancyPrn(prn);
  return ref !== null && ref.kind === kind && ref.id.toLowerCase() === id.toLowerCase();
}
