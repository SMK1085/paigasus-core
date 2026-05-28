# SPDX-License-Identifier: Apache-2.0
# TODO(SMA-379): remove this shim once at least one package has tests; until then it keeps the
# empty workspace green. The on-disk guard means it does NOT mask a "discovery broke" regression
# in a package that previously had tests.
import pytest


def pytest_sessionfinish(session: pytest.Session, exitstatus: int) -> None:
    if exitstatus == pytest.ExitCode.NO_TESTS_COLLECTED and not any(p.is_dir() for p in session.config.rootpath.glob("packages/*/tests")):
        session.exitstatus = 0
