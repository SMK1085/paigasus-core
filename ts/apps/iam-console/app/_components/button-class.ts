// SPDX-License-Identifier: Apache-2.0
//
// The two button classes the form and lifecycle components share. Not a client and not a
// server module: a plain constant, so any component can import it without a boundary check.

/** The submit button of every form: create, rename, restore, and the archive confirmation. */
export const PRIMARY_BUTTON_CLASS = 'bg-primary text-primary-foreground rounded-pgs self-start px-3 py-1.5 text-sm font-medium disabled:opacity-50';

/** The plain, non-primary button: "Archive" before confirmation, and "Cancel" during it. */
export const SECONDARY_BUTTON_CLASS = 'border-input rounded-pgs self-start border px-3 py-1.5 text-sm font-medium disabled:opacity-50';
