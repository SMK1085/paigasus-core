// SPDX-License-Identifier: Apache-2.0
//! Anthropic Messages (spec § 4.2–§ 4.4). The gateway `/v1/messages` route uses it (SMA-750, D26).

pub(crate) mod body;
pub(crate) mod stream;
