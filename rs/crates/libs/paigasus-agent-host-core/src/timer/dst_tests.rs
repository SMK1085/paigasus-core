// SPDX-License-Identifier: Apache-2.0

//! The pure proof that a schedule fires once per local day across a DST change, and that an outage
//! or a zone rule change does not make an extra firing.

use jiff::civil::{Date, DateTime};
use jiff::tz::TimeZone;
use jiff::{SignedDuration, Timestamp};

use super::fake::MapTzResolver;
use super::{BundledTzResolver, CronSchedule, Next, Note, Slot, Timer, TimerKind, TimerOrigin, TimerParts, TimerStatus, WALK_BOUND, plan_fire};
use crate::ids::{InboxId, TimerId};

fn ts(s: &str) -> Timestamp {
    s.parse().unwrap()
}

fn civil(s: &str) -> DateTime {
    s.parse().unwrap()
}

fn date(s: &str) -> Date {
    s.parse().unwrap()
}

fn slot(c: &str, second: bool) -> Slot {
    Slot { civil: civil(c), second }
}

fn schedule(cron: &str, zone: &str) -> CronSchedule {
    CronSchedule::parse(cron, zone, &BundledTzResolver::new()).unwrap()
}

fn timer_of(kind: TimerKind, next_fire_at: Timestamp) -> Timer {
    Timer::new(TimerParts {
        id: TimerId::from_uuid(uuid::Uuid::from_u128(0x1000)),
        inbox: InboxId::new(uuid::Uuid::from_u128(1), uuid::Uuid::from_u128(2)),
        kind,
        thread: None,
        note: Note::parse("").unwrap(),
        origin: TimerOrigin::Agent,
        status: TimerStatus::Active,
        next_fire_at,
    })
}

fn schedule_timer(schedule: CronSchedule, next_slot: Slot, at: Timestamp) -> Timer {
    timer_of(TimerKind::Schedule { schedule, next_slot }, at)
}

/// How the simulated poll loop moves the clock.
#[derive(Clone, Copy)]
enum Pace {
    /// The clock is `step` after each fire time. The poll never lags.
    AfterEach(SignedDuration),
    /// The clock moves by `step` and every timer that is due then fires.
    Poll(SignedDuration),
}

/// Run a schedule from `from` and return the `fire_at` of every firing before `to`.
fn simulate(cron: &str, zone: &str, from: Timestamp, to: Timestamp, pace: Pace) -> Vec<Timestamp> {
    let sched = schedule(cron, zone);
    let (slot, at) = sched.first_slot(from, WALK_BOUND).unwrap();
    let mut timer = schedule_timer(sched, slot, at);
    let mut fires = Vec::new();
    let mut now = from;
    loop {
        match pace {
            Pace::AfterEach(step) => now = timer.next_fire_at() + step,
            Pace::Poll(step) => {
                if timer.next_fire_at() > now {
                    now += step;
                    if now > to {
                        return fires;
                    }
                    continue;
                }
            }
        }
        if timer.next_fire_at() >= to {
            return fires;
        }
        let plan = plan_fire(&timer, now, WALK_BOUND).unwrap();
        assert_eq!(plan.fire_at, timer.next_fire_at());
        fires.push(plan.fire_at);
        let Next::Advance { slot, at } = plan.next else {
            panic!("a schedule always advances");
        };
        assert!(at > now, "the next fire time {at} is not after the clock {now}");
        let TimerKind::Schedule { schedule, .. } = timer.kind().clone() else { unreachable!() };
        timer = schedule_timer(schedule, slot, at);
    }
}

fn tz_of(zone: &str) -> TimeZone {
    schedule("* * * * *", zone).time_zone().clone()
}

fn local(tz: &TimeZone, t: Timestamp) -> DateTime {
    tz.to_datetime(t)
}

fn fires_on(fires: &[Timestamp], tz: &TimeZone, day: &str) -> Vec<Timestamp> {
    let d = date(day);
    fires.iter().copied().filter(|t| local(tz, *t).date() == d).collect()
}

#[test]
fn follow_up_plans_done() {
    let t = timer_of(TimerKind::FollowUp, ts("2026-03-29T01:00:00Z"));
    let plan = plan_fire(&t, ts("2026-03-29T01:00:05Z"), WALK_BOUND).unwrap();
    assert_eq!(plan.fire_at, ts("2026-03-29T01:00:00Z"));
    assert_eq!(plan.next, Next::Done);
}

#[test]
fn schedule_plans_from_the_stored_slot() {
    let t = schedule_timer(schedule("0 9 * * *", "Europe/Berlin"), slot("2026-01-10T09:00:00", false), ts("2026-01-10T08:00:00Z"));
    let plan = plan_fire(&t, ts("2026-01-13T10:00:00Z"), WALK_BOUND).unwrap();
    assert_eq!(plan.fire_at, ts("2026-01-10T08:00:00Z"));
    assert_eq!(
        plan.next,
        Next::Advance {
            slot: slot("2026-01-14T09:00:00", false),
            at: ts("2026-01-14T08:00:00Z")
        }
    );
}

#[test]
fn daily_nine_fires_once_per_local_day() {
    let from = ts("2026-01-01T00:00:00Z");
    let to = ts("2027-01-01T00:00:00Z");
    for zone in ["Europe/Berlin", "America/New_York", "Australia/Sydney", "Australia/Lord_Howe", "America/Santiago"] {
        let tz = tz_of(zone);
        for pace in [Pace::AfterEach(SignedDuration::from_secs(1)), Pace::Poll(SignedDuration::from_hours(5))] {
            let fires = simulate("0 9 * * *", zone, from, to, pace);
            // The expected local dates come from the interval and the schedule, not from the fires.
            let mut expected: Vec<Date> = Vec::new();
            let mut day = date("2025-12-31");
            while day <= date("2027-01-02") {
                let at = day.at(9, 0, 0, 0).to_zoned(tz.clone()).unwrap().timestamp();
                if at >= from && at < to {
                    expected.push(day);
                }
                day = day.tomorrow().unwrap();
            }
            assert_eq!(expected.len(), 365, "{zone}: the interval holds 365 days");
            let observed: Vec<Date> = fires.iter().map(|f| local(&tz, *f).date()).collect();
            assert_eq!(observed, expected, "{zone}: the fire dates differ from the expected dates");
            for f in &fires {
                let l = local(&tz, *f);
                assert_eq!((l.hour(), l.minute(), l.second()), (9, 0, 0), "{zone}: fire {f} is not at 09:00:00");
            }
        }
    }
}

#[test]
fn berlin_0230() {
    let tz = tz_of("Europe/Berlin");
    for pace in [Pace::AfterEach(SignedDuration::from_secs(1)), Pace::Poll(SignedDuration::from_hours(5))] {
        let spring = simulate("30 2 * * *", "Europe/Berlin", ts("2026-03-27T00:00:00Z"), ts("2026-03-31T00:00:00Z"), pace);
        assert_eq!(fires_on(&spring, &tz, "2026-03-29"), [ts("2026-03-29T01:00:00Z")]);
        let fall = simulate("30 2 * * *", "Europe/Berlin", ts("2026-10-23T00:00:00Z"), ts("2026-10-27T00:00:00Z"), pace);
        assert_eq!(fires_on(&fall, &tz, "2026-10-25"), [ts("2026-10-25T00:30:00Z")]);
    }
}

#[test]
fn new_york_0130_fall_day() {
    let tz = tz_of("America/New_York");
    for pace in [Pace::AfterEach(SignedDuration::from_secs(1)), Pace::Poll(SignedDuration::from_hours(5))] {
        let fires = simulate("30 1 * * *", "America/New_York", ts("2026-10-30T00:00:00Z"), ts("2026-11-03T00:00:00Z"), pace);
        assert_eq!(fires_on(&fires, &tz, "2026-11-01"), [ts("2026-11-01T05:30:00Z")]);
    }
}

#[test]
fn hourly_on_both_berlin_change_days() {
    let tz = tz_of("Europe/Berlin");
    let t = timer_of(TimerKind::FollowUp, ts("2026-10-25T00:00:00Z"));
    // A poll that lags by 5 h skips the missed hourly slots by design, so only the fine mode counts.
    {
        let pace = Pace::AfterEach(SignedDuration::from_secs(1));
        let fall = simulate("0 * * * *", "Europe/Berlin", ts("2026-10-24T00:00:00Z"), ts("2026-10-27T00:00:00Z"), pace);
        let day = fires_on(&fall, &tz, "2026-10-25");
        assert_eq!(day.len(), 25);
        let in_hour_2: Vec<Timestamp> = day.iter().copied().filter(|f| local(&tz, *f).hour() == 2).collect();
        assert_eq!(in_hour_2, [ts("2026-10-25T00:00:00Z"), ts("2026-10-25T01:00:00Z")]);
        assert_ne!(t.fire_event_id(in_hour_2[0]), t.fire_event_id(in_hour_2[1]));

        let spring = simulate("0 * * * *", "Europe/Berlin", ts("2026-03-28T00:00:00Z"), ts("2026-03-31T00:00:00Z"), pace);
        let day = fires_on(&spring, &tz, "2026-03-29");
        assert_eq!(day.len(), 23);
        assert_eq!(day.iter().filter(|f| **f == ts("2026-03-29T01:00:00Z")).count(), 1);
    }
}

#[test]
fn quarter_hours_on_the_berlin_fall_day() {
    let tz = tz_of("Europe/Berlin");
    // A lagging poll skips missed slots by design, so only the fine mode counts.
    {
        let pace = Pace::AfterEach(SignedDuration::from_secs(1));
        let fires = simulate("*/15 * * * *", "Europe/Berlin", ts("2026-10-24T00:00:00Z"), ts("2026-10-27T00:00:00Z"), pace);
        let day = fires_on(&fires, &tz, "2026-10-25");
        assert_eq!(day.len(), 100);
        let hour_2: Vec<Timestamp> = day.iter().copied().filter(|f| local(&tz, *f).hour() == 2).collect();
        assert_eq!(
            hour_2,
            [
                "2026-10-25T00:00:00Z",
                "2026-10-25T00:15:00Z",
                "2026-10-25T00:30:00Z",
                "2026-10-25T00:45:00Z",
                "2026-10-25T01:00:00Z",
                "2026-10-25T01:15:00Z",
                "2026-10-25T01:30:00Z",
                "2026-10-25T01:45:00Z",
            ]
            .map(ts)
        );
    }
}

#[test]
fn hourly_is_interval_and_daily_is_fixed() {
    let tz = tz_of("Europe/Berlin");
    let step = Pace::AfterEach(SignedDuration::from_secs(1));
    let hourly = simulate("@hourly", "Europe/Berlin", ts("2026-10-24T00:00:00Z"), ts("2026-10-27T00:00:00Z"), step);
    assert_eq!(fires_on(&hourly, &tz, "2026-10-25").len(), 25);
    let daily = simulate("@daily", "Europe/Berlin", ts("2026-10-23T00:00:00Z"), ts("2026-10-28T00:00:00Z"), step);
    for d in ["2026-10-24", "2026-10-25", "2026-10-26", "2026-10-27"] {
        assert_eq!(fires_on(&daily, &tz, d).len(), 1, "@daily on {d}");
    }
}

#[test]
fn outage_across_the_spring_change() {
    let t = schedule_timer(schedule("0 9 * * *", "Europe/Berlin"), slot("2026-03-26T09:00:00", false), ts("2026-03-26T08:00:00Z"));
    let now = ts("2026-03-29T02:00:00Z");
    let plan = plan_fire(&t, now, WALK_BOUND).unwrap();
    assert_eq!(plan.fire_at, ts("2026-03-26T08:00:00Z"));
    assert_eq!(t.fire_event_id(plan.fire_at), "timer:00000000-0000-0000-0000-000000001000:2026-03-26T08:00:00Z");
    let Next::Advance { slot: s, at } = plan.next else { panic!("a schedule advances") };
    assert_eq!((s, at), (slot("2026-03-29T09:00:00", false), ts("2026-03-29T07:00:00Z")));
    assert!(at > now);
}

#[test]
fn a_tzdb_rule_change_gives_no_extra_fire() {
    // Rules B put the start of DST one week early: 2026-03-22. Rules A are the real Berlin rules.
    let b = MapTzResolver::new(&[("Test/Zone", TimeZone::posix("CET-1CEST,M3.4.0,M10.5.0/3").unwrap())]);
    let a = MapTzResolver::new(&[("Test/Zone", tz_of("Europe/Berlin"))]);
    let sched_b = CronSchedule::parse("0 9 * * *", "Test/Zone", &b).unwrap();
    let sched_a = CronSchedule::parse("0 9 * * *", "Test/Zone", &a).unwrap();
    let stored = slot("2026-03-23T09:00:00", false);
    // Under rules B the stored slot is 07:00Z.
    assert_eq!(sched_b.instant(stored), Ok(ts("2026-03-23T07:00:00Z")));
    // Rebuild with rules A: 07:00Z is 08:00 local, so a walk from the instant fires 09:00 again.
    let from_instant = sched_a.first_slot(ts("2026-03-23T07:00:00Z"), WALK_BOUND).unwrap();
    assert_eq!(from_instant.0, stored, "this is the extra fire that the slot walk avoids");
    let t = schedule_timer(sched_a, stored, ts("2026-03-23T07:00:00Z"));
    let plan = plan_fire(&t, ts("2026-03-23T07:00:01Z"), WALK_BOUND).unwrap();
    assert_eq!(plan.fire_at, ts("2026-03-23T07:00:00Z"));
    assert_eq!(
        plan.next,
        Next::Advance {
            slot: slot("2026-03-24T09:00:00", false),
            at: ts("2026-03-24T08:00:00Z")
        }
    );
}

#[test]
fn hourly_over_a_year_fires_every_hour_exactly() {
    // The offset of each zone is a whole number of hours all year, so an hourly schedule fires
    // at every full UTC hour. A fold or gap error makes a gap or a repeat in this line.
    let from = ts("2026-01-01T00:00:00Z");
    let to = ts("2027-01-01T00:00:00Z");
    for zone in ["Europe/Berlin", "America/New_York", "Australia/Sydney", "America/Santiago"] {
        let fires = simulate("0 * * * *", zone, from, to, Pace::AfterEach(SignedDuration::from_secs(1)));
        assert_eq!(fires.first(), Some(&ts("2026-01-01T01:00:00Z")), "{zone}");
        assert_eq!(fires.last(), Some(&ts("2026-12-31T23:00:00Z")), "{zone}");
        assert_eq!(fires.len(), 8759, "{zone}");
        for w in fires.windows(2) {
            assert_eq!(w[1].duration_since(w[0]), SignedDuration::from_hours(1), "{zone}: the fires {} and {}", w[0], w[1]);
        }
    }
}
