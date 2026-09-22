// SPDX-License-Identifier: Apache-2.0
//
// The max_rows grammar of the bulk-replay form (SMA-661 spec § 6.4, § 6.5). NO directive and NO
// server-only import, on purpose: the client form gates its confirmation on this pattern, and
// commands.ts's zod schema parses the posted value with it. One value, so the number the
// confirmation names and the number IAM receives cannot disagree.

/** One to five ASCII digits. Not `z.coerce.number()`: that reads '0x10' as 16 and '7.' as 7 (§ 6.5). */
export const MAX_ROWS_PATTERN = /^\d{1,5}$/;
