# SPDX-License-Identifier: Apache-2.0
from typing import TYPE_CHECKING, Protocol, runtime_checkable

if TYPE_CHECKING:
    from .generated.paigasus.common.v1 import AuditMetadata


@runtime_checkable
class Auditable(Protocol):
    audit: "AuditMetadata | None"
