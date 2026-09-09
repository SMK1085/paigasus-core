// SPDX-License-Identifier: Apache-2.0
import '@testing-library/jest-dom/vitest';

/*
 * Browser APIs jsdom does not implement but Radix needs. This list is MEASURED (SMA-503 M2):
 * add only what a failing test actually demands, and delete anything no test needs. A copied
 * list is a list nobody can justify later.
 *
 * Measured 2026-09-08 against vitest@5.0.0 / jsdom@30.0.1: ResizeObserver,
 * hasPointerCapture/setPointerCapture/releasePointerCapture, and scrollIntoView were all tried
 * and none were needed — this task's suite (no-next-imports.test.ts, table.test.tsx) renders
 * no Radix primitive, so nothing here exercises them. All three were deleted as unjustified.
 * Add one back, with a comment naming the failing test that demanded it, when a later task's
 * Radix-backed component actually needs it.
 *
 * Task 10 (tests/combobox.test.tsx) re-added ResizeObserver: cmdk's CommandList measures its
 * own height via `new ResizeObserver(...)` in a mount effect, and jsdom throws
 * `ReferenceError: ResizeObserver is not defined` the instant a Combobox test renders — all
 * three combobox tests failed on it, not only the ones that open the popover. Popover and
 * DropdownMenu (Popper-based, no ResizeObserver) still need no polyfill.
 */
class ResizeObserverStub {
  observe(): void {}
  unobserve(): void {}
  disconnect(): void {}
}

globalThis.ResizeObserver ??= ResizeObserverStub as unknown as typeof ResizeObserver;
