# SPDX-License-Identifier: Apache-2.0
from paigasus_proto.generated.paigasus.gateway.v1 import CheckResponse


def test_check_response_carries_status() -> None:
    resp = CheckResponse(status="ok")
    assert resp.status == "ok"
