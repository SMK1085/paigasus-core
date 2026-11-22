# SPDX-License-Identifier: Apache-2.0
import datetime

from paigasus_proto.generated.paigasus.agents.v1 import OutboxMessage, TimerFired, TimerKind, TimerOrigin


def test_outbox_message_carries_run_prn_and_text() -> None:
    msg = OutboxMessage(run_prn="p", text="hi")
    assert msg.run_prn == "p"
    assert msg.text == "hi"


def test_timer_fired_carries_its_fields() -> None:
    fired = TimerFired(
        timer_id="00000000-0000-0000-0000-000000000001",
        kind=TimerKind.SCHEDULE,
        origin=TimerOrigin.SYSTEM,
        fire_at=datetime.datetime(2026, 3, 29, 1, 0, tzinfo=datetime.UTC),
        note="check the reply",
        cron="0 9 * * *",
        tz="Europe/Berlin",
    )
    assert fired.kind == TimerKind.SCHEDULE
    assert fired.origin == TimerOrigin.SYSTEM
    assert fired.cron == "0 9 * * *"
    assert fired.tz == "Europe/Berlin"
    assert fired.fire_at is not None and fired.fire_at.year == 2026
    assert TimerFired().cron is None
