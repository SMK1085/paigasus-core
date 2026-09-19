// SPDX-License-Identifier: Apache-2.0
//
// The shared prelude of the organization and project settings pages (SMA-636 spec § 4.1, § 4.2;
// controller F6): the IAM clients, mayI(), and both services' discovery state — IAM (the settings
// pages' own gRPC calls) and the gateway (the compact state line). Both pages need the same three
// things before their own loader runs. One copy keeps the two pages from drifting apart by accident.
import 'server-only';
import type { ServiceState } from '@paigasus/discovery/types';
import type { IamClients, MayI } from '@paigasus/console-core';
import { currentSession, discovery, iamClients, mayI } from '../../lib/console';

export type SettingsPrelude = {
  readonly clients: IamClients;
  readonly may: MayI;
  readonly iam: ServiceState;
  readonly gateway: ServiceState;
};

export async function loadSettingsPrelude(): Promise<SettingsPrelude> {
  const [session, clients, may] = await Promise.all([currentSession(), iamClients(), mayI()]);
  const probe = discovery();
  const [iam, gateway] = await Promise.all([probe.getServiceState('iam', session.accessToken), probe.getServiceState('gateway', session.accessToken)]);
  return { clients, may, iam, gateway };
}
