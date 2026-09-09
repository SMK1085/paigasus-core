// SPDX-License-Identifier: Apache-2.0

/*
 * The single public surface of @paigasus/ui.
 *
 * This file must NEVER carry a 'use client' directive: it would force every consumer of any
 * export into the client bundle. Each component file carries its own.
 */
export { Table, TableHeader, TableBody, TableRow, TableHead, TableCell } from './components/table';
export { Dialog, DialogTrigger, DialogClose, DialogContent, DialogHeader, DialogFooter, DialogTitle, DialogDescription } from './components/dialog';
export { DropdownMenu, DropdownMenuTrigger, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuLabel } from './components/dropdown-menu';
export { Combobox, type ComboboxItem, type ComboboxProps } from './components/combobox';
export { Label, Input, Field, type FieldProps } from './components/form';
export { EmptyState, type EmptyStateProps } from './components/empty-state';
export { ErrorState, type ErrorStateProps } from './components/error-state';
export { cn } from './lib/cn';
export { Link, LinkProvider, useLinkComponent, type LinkComponent, type LinkProps } from './nav/link';
