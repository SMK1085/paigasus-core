// SPDX-License-Identifier: Apache-2.0
//
// The shape of a Server Action that a client form posts to. A TYPE-only module: the import below is
// erased (verbatimModuleSyntax), so no server-only module reaches a client bundle.
import type { ActionState } from '@paigasus/console-core';

export type FormAction = (previous: ActionState, form: FormData) => Promise<ActionState>;
