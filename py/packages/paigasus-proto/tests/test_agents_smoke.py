# SPDX-License-Identifier: Apache-2.0
from paigasus_proto.generated.paigasus.agents.v1 import OutboxMessage


def test_outbox_message_carries_run_prn_and_text() -> None:
    msg = OutboxMessage(run_prn="p", text="hi")
    assert msg.run_prn == "p"
    assert msg.text == "hi"
