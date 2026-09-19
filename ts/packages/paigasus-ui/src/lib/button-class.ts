// SPDX-License-Identifier: Apache-2.0
//
// The two button classes the console forms share (SMA-636 D11, spec § 4.5). They moved here from
// iam-console's app/_components/button-class.ts, so the gateway zone uses the same two. Plain
// constants, not a component and not a client module: a server or a client component may import
// them. They live in src/, so every consumer's Tailwind `@source` line for this package scans them.

/** The submit button of every form: create, rename, add member, restore, and a confirmation. */
export const PRIMARY_BUTTON_CLASS = 'bg-primary text-primary-foreground rounded-pgs self-start px-3 py-1.5 text-sm font-medium disabled:opacity-50';

/** The plain, non-primary button: the first step of a two-step confirm, and "Cancel" during it. */
export const SECONDARY_BUTTON_CLASS = 'border-input rounded-pgs self-start border px-3 py-1.5 text-sm font-medium disabled:opacity-50';
