// SPDX-License-Identifier: Apache-2.0
//
// The guard-free `./iam/types` entry (SMA-630 spec § 4.5, D8). This file carries NO
// `import '../server-guard'` and must not gain one: an app's client code and its Playwright support
// files import this entry, and `tests/server-guard.test.ts` lists './iam/types' in
// UNGUARDED_ENTRIES and asserts that the guard is absent.
//
// An app cannot import @paigasus/proto (the eslint boundary `paigasus/boundaries/apps` bans it,
// type imports included). Without this line a screen receives `status: 2` and cannot write the
// name. It is the same trade `./errors/types` makes for ErrorReason: one small frozen enum object
// in a client bundle.
export { NodeStatus } from '@paigasus/proto/iam';
