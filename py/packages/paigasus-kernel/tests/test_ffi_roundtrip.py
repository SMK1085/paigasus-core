# SPDX-License-Identifier: Apache-2.0
"""Runtime proof a value crosses kernel -> PyO3 -> wheel -> Python (SMA-419)."""

from paigasus_kernel import sum_as_string


def test_sum_crosses_ffi_boundary() -> None:
    assert sum_as_string(2, 3) == "5"
    assert sum_as_string(-4, 4) == "0"
