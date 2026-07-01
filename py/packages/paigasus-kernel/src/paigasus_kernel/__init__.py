# SPDX-License-Identifier: Apache-2.0
import os
import time

from paigasus_py_bindings import (
    mint_uuid7,
    prn_build,
    prn_canonicalize,
    prn_cedar_entity_id,
    prn_cedar_entity_type,
    prn_error_kind,
    prn_org,
    prn_region,
    prn_resource_id,
    prn_resource_type,
    prn_service,
    sum_as_string,
)


def mint() -> str:
    """Mint a UUIDv7 from the ambient clock + CSPRNG (the injected FFI mint is pure)."""
    return mint_uuid7(time.time_ns() / 1_000_000, os.urandom(10).hex())


__all__ = [
    "mint",
    "mint_uuid7",
    "prn_build",
    "prn_canonicalize",
    "prn_cedar_entity_id",
    "prn_cedar_entity_type",
    "prn_error_kind",
    "prn_org",
    "prn_region",
    "prn_resource_id",
    "prn_resource_type",
    "prn_service",
    "sum_as_string",
]
