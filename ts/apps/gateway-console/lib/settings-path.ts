// SPDX-License-Identifier: Apache-2.0
//
// The one path the gateway settings actions revalidate (SMA-636 § 5). basePath-relative. Route
// groups such as (console) are not part of the URL, so '/orgs' with 'layout' covers the
// organization page and every project page under it. The pattern of iam-console's
// lib/tenancy-path.ts. A plain module: a Server Actions file may export only async functions.
export const SETTINGS_PATH = '/orgs';
