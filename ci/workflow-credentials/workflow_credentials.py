# SPDX-License-Identifier: Apache-2.0
"""Assert no pull-request-triggered workflow can obtain a repository credential.

Exit codes are DELIBERATELY not the repo's usual 0/1/2. This process exits 3 for an
assertion failure so that `uv`'s own rc 1 — measured on a failed resolution, online and
under UV_OFFLINE=1 — cannot be mistaken for "a workflow declares a credential". run.sh
maps 3 -> 1 and everything else -> 2. (SMA-593 spec §6.)
"""

from __future__ import annotations

import sys

RC_OK = 0
RC_INFRA = 2
RC_ASSERT = 3


class InfraError(Exception):
    """The check could not run. Maps to RC_INFRA."""


def main(argv: list[str]) -> int:
    if argv[1:2] == ["--exit-code-probe"]:
        # Used only by run.sh's negative control to prove the 3 -> 1 mapping is wired.
        return int(argv[2])
    raise InfraError("not implemented yet")


if __name__ == "__main__":
    try:
        raise SystemExit(main(sys.argv))
    except InfraError as exc:
        print(f"workflow-credentials: {exc}", file=sys.stderr)
        raise SystemExit(RC_INFRA) from exc
