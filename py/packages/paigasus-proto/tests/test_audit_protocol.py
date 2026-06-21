# SPDX-License-Identifier: Apache-2.0
from datetime import UTC, datetime

from paigasus_proto.audit import Auditable
from paigasus_proto.generated.paigasus.common.v1 import AuditableExample, AuditMetadata


def test_generated_example_satisfies_auditable() -> None:
    obj = AuditableExample(
        id="x",
        audit=AuditMetadata(
            created_by="svc",
            created_at=datetime(2026, 1, 1, tzinfo=UTC),
        ),
    )
    assert isinstance(obj, Auditable)


def test_example_with_no_audit_still_satisfies() -> None:
    # The `audit` attribute is present (defaults to None) -> still a structural match.
    assert isinstance(AuditableExample(id="y"), Auditable)


def test_object_without_audit_is_not_auditable() -> None:
    class NotAuditable:
        id: str = "z"

    assert not isinstance(NotAuditable(), Auditable)
