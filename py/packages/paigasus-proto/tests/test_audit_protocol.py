# SPDX-License-Identifier: Apache-2.0
from datetime import UTC, datetime

from paigasus_proto.audit import Auditable
from paigasus_proto.generated.paigasus.common.v1 import Actor, AuditableExample, AuditMetadata


def test_generated_example_satisfies_auditable() -> None:
    prn = "prn:pgs:iam:::principal/0192f1c0-0000-7000-8000-000000000001"
    obj = AuditableExample(
        id="x",
        audit=AuditMetadata(
            creator=Actor(prn=prn),
            created_at=datetime(2026, 1, 1, tzinfo=UTC),
        ),
    )
    assert isinstance(obj, Auditable)
    # `isinstance` against a runtime_checkable Protocol declaring only `audit` checks
    # attribute PRESENCE and would pass with AuditMetadata empty. Assert a field so this
    # test actually proves something about the Actor rename (SMA-439).
    assert obj.audit is not None
    assert obj.audit.creator is not None
    assert obj.audit.creator.prn == prn
    # SMA-439: `modified_by` used to be `""`; it is now an absent `modifier`. Mirrors the
    # Rust `audit().is_some()` + `modifier().is_none()` pairing in audit.rs's tests.
    assert obj.audit.modifier is None


def test_example_with_no_audit_still_satisfies() -> None:
    # The `audit` attribute is present (defaults to None) -> still a structural match.
    assert isinstance(AuditableExample(id="y"), Auditable)


def test_object_without_audit_is_not_auditable() -> None:
    class NotAuditable:
        id: str = "z"

    assert not isinstance(NotAuditable(), Auditable)
