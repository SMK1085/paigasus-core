// SPDX-License-Identifier: Apache-2.0
import root from '../../eslint.config.js';
import nextPlugin from '@next/eslint-plugin-next';

export default [
  ...root,
  {
    files: ['**/*.{ts,tsx}'],
    plugins: { '@next/next': nextPlugin },
    rules: { ...nextPlugin.configs.recommended.rules },
  },
];
