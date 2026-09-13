// SPDX-License-Identifier: Apache-2.0
//
// basePath-relative. Route groups such as (console) are not part of the URL path, so '/orgs' with
// 'layout' covers every page under /orgs — organizations, their teams and their projects alike.
// Task 22's e2e test asserts that a created organization appears.
//
// A plain module, not a 'use server' one: a Server Actions file may export only async functions,
// so this constant cannot live in (and be re-exported from) an actions.ts.
export const TENANCY_PATH = '/orgs';
