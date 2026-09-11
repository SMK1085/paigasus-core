// SPDX-License-Identifier: Apache-2.0
import '@testing-library/jest-dom/vitest';

/*
 * No jsdom polyfills. @paigasus/ui measured (SMA-503) that Radix DropdownMenu needs none, and this
 * package renders no cmdk component. Add a polyfill only when a failing test demands it, with a
 * comment that names that test — the @paigasus/ui rule. A copied list cannot be justified later.
 */
