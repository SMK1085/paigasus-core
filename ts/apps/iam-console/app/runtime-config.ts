// SPDX-License-Identifier: Apache-2.0
//
// This app's ONE call to defineRuntimeConfig. When @paigasus/auth (SMA-506) and @paigasus/sdk
// (SMA-508) land, each exports a zod shape for the variables it owns and they are composed here —
// one schema, one parse, one place.
import { defineRuntimeConfig } from '@paigasus/next-config/runtime';

export const { getRuntimeConfig, getPublicConfig } = defineRuntimeConfig();
