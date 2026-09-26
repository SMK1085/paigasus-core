// SPDX-License-Identifier: Apache-2.0
//
// The role that holds only InvokeModel (SMA-636 spec § 1.2, SMA-676). ONE definition: the
// service-account commands and the "Model access for people" commands both grant it. A plain
// module with no runtime import, so a client component and a test may import it.
export const GATEWAY_ROLE = 'gateway_user';
