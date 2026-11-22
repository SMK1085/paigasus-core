// SPDX-License-Identifier: Apache-2.0

//! A five-field cron schedule in an IANA time zone. The syntax check refuses what `croner`
//! accepts as an extension, so the accepted set is small and the same everywhere.

use croner::Cron;
use croner::parser::{CronParser, Seconds};
use jiff::civil::DateTime;
use jiff::tz::{AmbiguousOffset, TimeZone};
use jiff::{SignedDuration, Span, Timestamp};

use super::Slot;
use crate::ports::{TzError, TzResolver};

/// The most steps that one walk to the next time takes.
pub const WALK_BOUND: u32 = 10_000;
/// The number of days that the civil gap check covers.
pub const CIVIL_GAP_WINDOW_DAYS: u32 = 366;
/// The smallest gap in seconds between two slots of a five-field cron pattern.
const FIELD_FLOOR_SECS: u64 = 60;
/// A walk starts at most this many hours before `now`. An older slot is a long outage.
const WALK_CLAMP_HOURS: i64 = 25;

const FIELD_NAMES: [&str; 5] = ["minute", "hour", "day of month", "month", "day of week"];
const MONTHS: [&str; 12] = ["JAN", "FEB", "MAR", "APR", "MAY", "JUN", "JUL", "AUG", "SEP", "OCT", "NOV", "DEC"];
const WEEKDAYS: [&str; 7] = ["SUN", "MON", "TUE", "WED", "THU", "FRI", "SAT"];

/// The reason that the syntax check refuses a cron text.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CronSyntaxError {
    #[error("a cron expression has five fields (minute, hour, day of month, month, day of week); this one has {found}. Seconds and years are not supported.")]
    FieldCount { found: usize },
    #[error("the alias {alias} is not supported; use @hourly, @daily, @weekly, @monthly, @yearly or @annually")]
    UnknownAlias { alias: String },
    #[error("the `{token}` extension in the {field} field is not supported")]
    Extension { field: &'static str, token: char },
    #[error("the name {name} is a {kind} name and does not belong in the {field} field")]
    MisplacedName { name: String, kind: &'static str, field: &'static str },
    #[error("the `+` prefix of the day of week field is not supported")]
    PlusPrefix,
    #[error("the day of month and the day of week are both set; set one of them to *")]
    BothDayFields,
    #[error("the cron expression is not valid: {0}")]
    Croner(String),
}

/// The reason that a schedule does not parse.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CronScheduleError {
    #[error(transparent)]
    Syntax(#[from] CronSyntaxError),
    #[error("the time zone {name} is not known")]
    UnknownZone { name: String },
}

/// The reason that a walk to the next time fails.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WalkError {
    #[error("the walk went past the bound of {bound} steps")]
    BoundExceeded { bound: u32 },
    #[error("the cron library failed: {0}")]
    Cron(String),
    #[error("the time zone failed: {0}")]
    Tz(String),
}

/// The instants of a civil time in a zone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resolution {
    /// The civil time has one instant. A time in a gap has the instant of the end of the gap.
    Single(Timestamp),
    /// The civil time occurs twice, because the clock goes back.
    Fold {
        /// The instant of the first occurrence.
        first: Timestamp,
        /// The instant of the second occurrence.
        second: Timestamp,
        /// The first civil time of the repeated period.
        fold_start: DateTime,
        /// The civil time after the repeated period.
        fold_end: DateTime,
    },
}

/// A parsed cron text with its time zone.
#[derive(Clone, Debug)]
pub struct CronSchedule {
    text: String,
    tz_name: String,
    tz: TimeZone,
    interval: bool,
    cron: Cron,
}

impl PartialEq for CronSchedule {
    fn eq(&self, other: &Self) -> bool {
        self.text == other.text && self.tz_name == other.tz_name
    }
}

impl Eq for CronSchedule {}

impl CronSchedule {
    /// Parse `cron` in the zone `tz_name`.
    ///
    /// # Errors
    /// `Syntax` for a text that is not a plain five-field cron text. `UnknownZone` for a zone
    /// that `tz` does not know.
    pub fn parse(cron: &str, tz_name: &str, tz: &dyn TzResolver) -> Result<Self, CronScheduleError> {
        let text = cron.trim();
        let normalized = normalize(text)?;
        let parsed = CronParser::builder()
            .seconds(Seconds::Disallowed)
            .build()
            .parse(&normalized)
            .map_err(|e| CronSyntaxError::Croner(e.to_string()))?;
        let zone = tz.resolve(tz_name).map_err(|TzError::Unknown { name }| CronScheduleError::UnknownZone { name })?;
        let hour = normalized.split_whitespace().nth(1).unwrap_or_default();
        Ok(Self {
            text: text.to_string(),
            tz_name: zone.iana_name().unwrap_or(tz_name).to_string(),
            tz: zone,
            interval: hour == "*" || hour.starts_with("*/"),
            cron: parsed,
        })
    }

    /// The cron text after `trim`.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The canonical zone name.
    #[must_use]
    pub fn tz_name(&self) -> &str {
        &self.tz_name
    }

    #[must_use]
    pub fn time_zone(&self) -> &TimeZone {
        &self.tz
    }

    /// True when the hour field is `*` or `*/n`: the schedule fires at an interval of the clock.
    #[must_use]
    pub fn is_interval(&self) -> bool {
        self.interval
    }

    /// The first civil time strictly after `c` that matches the schedule.
    ///
    /// # Errors
    /// `Cron` when the cron library finds no time.
    pub fn next_civil_after(&self, c: DateTime) -> Result<DateTime, WalkError> {
        self.cron.find_next_occurrence(&c, false).map_err(|e| WalkError::Cron(e.to_string()))
    }

    /// The first civil time at or after `c` that matches the schedule.
    fn next_civil_at_or_after(&self, c: DateTime) -> Result<DateTime, WalkError> {
        self.cron.find_next_occurrence(&c, true).map_err(|e| WalkError::Cron(e.to_string()))
    }

    /// The instants of the civil time `c` in the zone of the schedule.
    ///
    /// A time in a gap resolves to the first instant after the gap. A time in a fold resolves to
    /// both occurrences.
    ///
    /// # Errors
    /// `Tz` when the zone has no transition where one must be, or a conversion fails.
    pub fn resolve(&self, c: DateTime) -> Result<Resolution, WalkError> {
        let tz_err = |e: jiff::Error| WalkError::Tz(e.to_string());
        let transition_after = |t: Timestamp| {
            self.tz
                .following(t)
                .next()
                .map(|tr| tr.timestamp())
                .ok_or_else(|| WalkError::Tz("the zone has no transition after the time".to_string()))
        };
        match self.tz.to_ambiguous_timestamp(c).offset() {
            AmbiguousOffset::Unambiguous { offset } => Ok(Resolution::Single(offset.to_timestamp(c).map_err(tz_err)?)),
            AmbiguousOffset::Gap { after, .. } => Ok(Resolution::Single(transition_after(after.to_timestamp(c).map_err(tz_err)?)?)),
            AmbiguousOffset::Fold { before, after } => {
                let first = before.to_timestamp(c).map_err(tz_err)?;
                let second = after.to_timestamp(c).map_err(tz_err)?;
                let t = transition_after(first)?;
                Ok(Resolution::Fold {
                    first,
                    second,
                    fold_start: after.to_datetime(t),
                    fold_end: before.to_datetime(t),
                })
            }
        }
    }

    /// The instant of a slot. In a fold, `second` picks the occurrence.
    ///
    /// # Errors
    /// The errors of `resolve`.
    pub fn instant(&self, slot: Slot) -> Result<Timestamp, WalkError> {
        Ok(match self.resolve(slot.civil)? {
            Resolution::Single(t) => t,
            Resolution::Fold { first, second, .. } => {
                if slot.second {
                    second
                } else {
                    first
                }
            }
        })
    }

    /// The step from one slot to the next one. An interval pattern walks a repeated period in
    /// instant order, so that every slot in it fires twice (the fold rule). A fixed-hour pattern
    /// never sets `second`.
    fn step(&self, cur: Slot) -> Result<Slot, WalkError> {
        let n = self.next_civil_after(cur.civil)?;
        if self.interval
            && let Resolution::Fold { fold_start, fold_end, .. } = self.resolve(cur.civil)?
        {
            if cur.second {
                return Ok(Slot { civil: n, second: n < fold_end });
            }
            if n < fold_end {
                return Ok(Slot { civil: n, second: false });
            }
            let s = self.next_civil_at_or_after(fold_start)?;
            return Ok(if s < fold_end { Slot { civil: s, second: true } } else { Slot { civil: n, second: false } });
        }
        Ok(Slot { civil: n, second: false })
    }

    /// The first slot after `fired` whose instant is after `now`, and that instant.
    ///
    /// The walk starts at most 25 hours before `now`, so a long outage costs few steps.
    ///
    /// # Errors
    /// `BoundExceeded` when `bound` steps find no slot after `now`. The errors of `resolve`.
    pub fn next_after(&self, fired: Slot, now: Timestamp, bound: u32) -> Result<(Slot, Timestamp), WalkError> {
        let floor = self
            .tz
            .to_datetime(now)
            .checked_sub(SignedDuration::from_hours(WALK_CLAMP_HOURS))
            .map_err(|e| WalkError::Tz(e.to_string()))?;
        let mut cur = if fired.civil < floor { Slot { civil: floor, second: false } } else { fired };
        for _ in 0..bound {
            let cand = self.step(cur)?;
            let t = self.instant(cand)?;
            if t > now {
                return Ok((cand, t));
            }
            cur = cand;
        }
        Err(WalkError::BoundExceeded { bound })
    }

    /// The first slot after the time `created`, and its instant.
    ///
    /// # Errors
    /// The errors of `next_after`.
    pub fn first_slot(&self, created: Timestamp, bound: u32) -> Result<(Slot, Timestamp), WalkError> {
        self.next_after(
            Slot {
                civil: self.tz.to_datetime(created),
                second: false,
            },
            created,
            bound,
        )
    }

    /// The first gap in seconds between two consecutive civil slots that is below `min_secs`, or
    /// `None`. The earlier slot of the gap is in `[from, from + days]`, the end included. The later
    /// slot can be after the end. The check uses civil times only and no zone.
    ///
    /// # Errors
    /// `Cron` when the cron library finds no time. `Tz` when the end of the window is not valid.
    pub fn min_civil_gap(&self, from: DateTime, days: u32, min_secs: u64) -> Result<Option<u64>, WalkError> {
        if min_secs <= FIELD_FLOOR_SECS {
            // A five-field pattern has no gap below 60 s, so no gap can be below the minimum. The
            // walk below runs only for a minimum above 60 s, and it stops at the first gap below
            // the minimum.
            return Ok(None);
        }
        let end = from.checked_add(Span::new().days(i64::from(days))).map_err(|e| WalkError::Tz(e.to_string()))?;
        let mut prev: Option<DateTime> = None;
        let mut cur = from;
        loop {
            // The first search includes `from`, so a slot exactly at `from` counts.
            let n = if prev.is_none() { self.next_civil_at_or_after(cur)? } else { self.next_civil_after(cur)? };
            // `prev` is at or before `end`, so the gap that starts at `prev` is in the window,
            // even when `n` is after `end`.
            if let Some(p) = prev {
                let gap = u64::try_from(p.duration_until(n).as_secs()).unwrap_or(0);
                if gap < min_secs {
                    return Ok(Some(gap));
                }
            }
            if n > end {
                return Ok(None);
            }
            prev = Some(n);
            cur = n;
        }
    }
}

/// Check the text and return the normalized five-field text.
fn normalize(text: &str) -> Result<String, CronSyntaxError> {
    let expanded;
    let text = if text.starts_with('@') {
        expanded = match text.to_ascii_lowercase().as_str() {
            "@hourly" => "0 * * * *",
            "@daily" => "0 0 * * *",
            "@weekly" => "0 0 * * 0",
            "@monthly" => "0 0 1 * *",
            "@yearly" | "@annually" => "0 0 1 1 *",
            _ => return Err(CronSyntaxError::UnknownAlias { alias: text.to_string() }),
        };
        expanded
    } else {
        text
    };
    let fields: Vec<&str> = text.split_whitespace().collect();
    if fields.len() != 5 {
        return Err(CronSyntaxError::FieldCount { found: fields.len() });
    }
    let mut out: Vec<String> = fields.iter().map(|f| (*f).to_string()).collect();
    misplaced_name(&out[3], &WEEKDAYS, "weekday", FIELD_NAMES[3])?;
    misplaced_name(&out[4], &MONTHS, "month", FIELD_NAMES[4])?;
    out[3] = replace_names(&out[3], &MONTHS, 1, FIELD_NAMES[3])?;
    out[4] = replace_names(&out[4], &WEEKDAYS, 0, FIELD_NAMES[4])?;
    for (i, field) in out.iter().enumerate() {
        if i == 4 && field.starts_with('+') {
            return Err(CronSyntaxError::PlusPrefix);
        }
        for c in field.chars() {
            match c.to_ascii_uppercase() {
                token @ ('L' | 'W' | '#' | '?') => return Err(CronSyntaxError::Extension { field: FIELD_NAMES[i], token }),
                '0'..='9' | ',' | '-' | '*' | '/' => {}
                other => {
                    return Err(CronSyntaxError::Croner(format!("the character `{other}` is not allowed in the {} field", FIELD_NAMES[i])));
                }
            }
        }
    }
    if out[2] != "*" && out[4] != "*" {
        return Err(CronSyntaxError::BothDayFields);
    }
    Ok(out.join(" "))
}

/// Refuse a field that holds a name of the other kind, for example a weekday name in the month
/// field. `names` are the names that do not belong in the field.
fn misplaced_name(field: &str, names: &[&str], kind: &'static str, field_name: &'static str) -> Result<(), CronSyntaxError> {
    for run in field.split(|c: char| !c.is_ascii_alphabetic()).filter(|r| !r.is_empty()) {
        let upper = run.to_ascii_uppercase();
        if names.contains(&upper.as_str()) {
            return Err(CronSyntaxError::MisplacedName { name: upper, kind, field: field_name });
        }
    }
    Ok(())
}

/// Replace each name in `names` with its number (`first` is the number of the first name). A name
/// must be a whole part of a token: it starts the field or follows `,`, `-` or `/`, and it ends the
/// field or comes before `,`, `-` or `/`. Any other token that holds a name or a run of three or
/// more letters is refused, so `JAN0` does not become `10`. A short run that is not a name stays,
/// and the next check refuses it.
fn replace_names(field: &str, names: &[&str], first: usize, field_name: &'static str) -> Result<String, CronSyntaxError> {
    let is_sep = |c: char| matches!(c, ',' | '-' | '/');
    let chars: Vec<char> = field.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        if !chars[i].is_ascii_alphabetic() {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        let start = i;
        while i < chars.len() && chars[i].is_ascii_alphabetic() {
            i += 1;
        }
        let run: String = chars[start..i].iter().collect();
        let upper = run.to_ascii_uppercase();
        let position = names.iter().position(|n| *n == upper);
        if position.is_some() || run.len() >= 3 {
            // A leading `+` stays: the next check names it.
            let before_ok = start == 0 || is_sep(chars[start - 1]) || (start == 1 && chars[0] == '+');
            let after_ok = i == chars.len() || is_sep(chars[i]);
            if position.is_none() || !(before_ok && after_ok) {
                // The token is the text between separators that holds the run.
                let mut from = start;
                while from > 0 && !is_sep(chars[from - 1]) {
                    from -= 1;
                }
                let mut to = i;
                while to < chars.len() && !is_sep(chars[to]) {
                    to += 1;
                }
                let token: String = chars[from..to].iter().collect();
                return Err(CronSyntaxError::Croner(format!(
                    "the token `{token}` in the {field_name} field is not valid: a name must stand alone between `,`, `-` or `/`"
                )));
            }
        }
        match position {
            Some(p) => out.push_str(&(p + first).to_string()),
            None => out.push_str(&run),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timer::BundledTzResolver;

    fn parse(cron: &str) -> Result<CronSchedule, CronScheduleError> {
        CronSchedule::parse(cron, "Europe/Berlin", &BundledTzResolver::new())
    }

    fn syntax(cron: &str) -> CronSyntaxError {
        match parse(cron) {
            Err(CronScheduleError::Syntax(e)) => e,
            other => panic!("{cron}: expected a syntax error, got {other:?}"),
        }
    }

    fn civil(s: &str) -> DateTime {
        s.parse().unwrap()
    }

    fn walk(cron: &str, from: &str, n: usize) -> Vec<String> {
        let s = parse(cron).unwrap();
        let mut at = civil(from);
        (0..n)
            .map(|_| {
                at = s.next_civil_after(at).unwrap();
                at.to_string()
            })
            .collect()
    }

    #[test]
    fn zone_names_are_canonical() {
        let r = BundledTzResolver::new();
        assert_eq!(CronSchedule::parse("0 9 * * *", "europe/berlin", &r).unwrap().tz_name(), "Europe/Berlin");
        assert_eq!(CronSchedule::parse("0 9 * * *", "UTC", &r).unwrap().tz_name(), "UTC");
        assert_eq!(CronSchedule::parse("0 9 * * *", "US/Eastern", &r).unwrap().tz_name(), "US/Eastern");
        assert_eq!(
            CronSchedule::parse("0 9 * * *", "Mars/Olympus", &r),
            Err(CronScheduleError::UnknownZone { name: "Mars/Olympus".into() })
        );
    }

    #[test]
    fn civil_matches() {
        let from = "2026-03-28T00:00:00";
        assert_eq!(walk("0 9 * * *", from, 1), ["2026-03-28T09:00:00"]);
        assert_eq!(walk("30 2 * * *", from, 2), ["2026-03-28T02:30:00", "2026-03-29T02:30:00"]);
        assert_eq!(walk("*/15 * * * *", from, 2), ["2026-03-28T00:15:00", "2026-03-28T00:30:00"]);
        assert_eq!(walk("0 */2 * * *", from, 1), ["2026-03-28T02:00:00"]);
        assert_eq!(walk("@daily", from, 1), ["2026-03-29T00:00:00"]);
        assert_eq!(walk("0 0 * JUL WED", "2026-07-01T00:00:00", 1), ["2026-07-08T00:00:00"]);
    }

    #[test]
    fn interval_patterns() {
        for c in ["0 * * * *", "*/15 * * * *", "@hourly", "0 */2 * * *"] {
            assert!(parse(c).unwrap().is_interval(), "{c}");
        }
        for c in ["0 9 * * *", "30 1 * * *", "0 9-17 * * *", "@daily", "@weekly", "@monthly", "@yearly", "@annually"] {
            assert!(!parse(c).unwrap().is_interval(), "{c}");
        }
    }

    #[test]
    fn refused_syntax() {
        assert_eq!(syntax("0 0 9 * * *"), CronSyntaxError::FieldCount { found: 6 });
        assert_eq!(syntax("0 9 * * * 2026"), CronSyntaxError::FieldCount { found: 6 });
        assert_eq!(syntax("0 9 * *"), CronSyntaxError::FieldCount { found: 4 });
        assert_eq!(syntax("0 9 L * *"), CronSyntaxError::Extension { field: "day of month", token: 'L' });
        assert_eq!(syntax("0 9 15W * *"), CronSyntaxError::Extension { field: "day of month", token: 'W' });
        assert_eq!(syntax("0 9 * * 5#2"), CronSyntaxError::Extension { field: "day of week", token: '#' });
        assert_eq!(syntax("0 9 ? * *"), CronSyntaxError::Extension { field: "day of month", token: '?' });
        assert_eq!(syntax("0 9 * * +MON"), CronSyntaxError::PlusPrefix);
        assert_eq!(
            syntax("0 9 * WED *"),
            CronSyntaxError::MisplacedName {
                name: "WED".into(),
                kind: "weekday",
                field: "month"
            }
        );
        assert_eq!(
            syntax("0 9 * 1,fri *"),
            CronSyntaxError::MisplacedName {
                name: "FRI".into(),
                kind: "weekday",
                field: "month"
            }
        );
        assert_eq!(
            syntax("0 9 * * JAN"),
            CronSyntaxError::MisplacedName {
                name: "JAN".into(),
                kind: "month",
                field: "day of week"
            }
        );
        assert_eq!(
            syntax("0 9 * * mon-DEC"),
            CronSyntaxError::MisplacedName {
                name: "DEC".into(),
                kind: "month",
                field: "day of week"
            }
        );
        assert_eq!(syntax("0 9 1 * MON"), CronSyntaxError::BothDayFields);
        assert_eq!(syntax("0 9 */2 * MON"), CronSyntaxError::BothDayFields);
        assert_eq!(syntax("@reboot"), CronSyntaxError::UnknownAlias { alias: "@reboot".into() });
    }

    #[test]
    fn names_that_hold_extension_letters_are_accepted() {
        for c in ["0 9 * JUL *", "0 9 * * WED", "0 9 * APR-JUL SAT"] {
            assert!(parse(c).is_ok(), "{c}");
        }
    }

    #[test]
    fn a_name_is_a_whole_token_part() {
        for c in ["0 9 * JAN *", "0 9 * JAN-MAR *", "0 9 * JAN-JUN/2 *", "0 9 * * mon-fri", "0 9 * JAN,MAR *"] {
            assert!(parse(c).is_ok(), "{c}: {:?}", parse(c).err());
        }
        for (c, token) in [("0 9 * JAN0 *", "JAN0"), ("0 9 * 0JAN *", "0JAN"), ("0 9 * JANX *", "JANX"), ("0 9 * * MON1", "MON1")] {
            let err = syntax(c).to_string();
            assert!(err.contains(token), "{c}: {err}");
        }
    }

    #[test]
    fn spelling_variants_match_the_plain_form() {
        let from = "2026-03-28T00:00:00";
        assert_eq!(walk("0\t9  * * mon-fri", from, 5), walk("0 9 * * 1-5", from, 5));
        assert_eq!(walk("0 9 * * 7", from, 5), walk("0 9 * * 0", from, 5));
        assert_eq!(walk("@HOURLY", from, 5), walk("0 * * * *", from, 5));
    }

    #[test]
    fn refusal_texts_name_the_part() {
        assert!(CronSyntaxError::PlusPrefix.to_string().contains('+'));
        let both = CronSyntaxError::BothDayFields.to_string();
        assert!(both.contains("day of month") && both.contains("day of week"));
        assert!(CronSyntaxError::Extension { field: "day of month", token: 'L' }.to_string().contains('L'));
        assert_eq!(
            CronSyntaxError::MisplacedName {
                name: "WED".into(),
                kind: "weekday",
                field: "month"
            }
            .to_string(),
            "the name WED is a weekday name and does not belong in the month field"
        );
    }

    fn sched(cron: &str, zone: &str) -> CronSchedule {
        CronSchedule::parse(cron, zone, &BundledTzResolver::new()).unwrap()
    }

    fn ts(s: &str) -> Timestamp {
        s.parse().unwrap()
    }

    fn slot(c: &str, second: bool) -> Slot {
        Slot { civil: civil(c), second }
    }

    #[test]
    fn gap_resolves_to_the_end_of_the_gap() {
        let s = sched("0 * * * *", "Europe/Berlin");
        assert_eq!(s.resolve(civil("2026-03-29T02:30:00")), Ok(Resolution::Single(ts("2026-03-29T01:00:00Z"))));
    }

    #[test]
    fn fold_resolves_both_occurrences() {
        let s = sched("0 * * * *", "Europe/Berlin");
        assert_eq!(
            s.resolve(civil("2026-10-25T02:30:00")),
            Ok(Resolution::Fold {
                first: ts("2026-10-25T00:30:00Z"),
                second: ts("2026-10-25T01:30:00Z"),
                fold_start: civil("2026-10-25T02:00:00"),
                fold_end: civil("2026-10-25T03:00:00"),
            })
        );
        assert_eq!(s.instant(slot("2026-10-25T02:30:00", false)), Ok(ts("2026-10-25T00:30:00Z")));
        assert_eq!(s.instant(slot("2026-10-25T02:30:00", true)), Ok(ts("2026-10-25T01:30:00Z")));
    }

    #[test]
    fn hourly_fires_twice_in_the_berlin_repeated_hour() {
        let s = sched("0 * * * *", "Europe/Berlin");
        let mut cur = slot("2026-10-25T01:00:00", false);
        let mut now = ts("2026-10-24T23:00:01Z");
        let mut got = Vec::new();
        for _ in 0..3 {
            let (next, t) = s.next_after(cur, now, WALK_BOUND).unwrap();
            got.push((next, t));
            cur = next;
            now = t + jiff::SignedDuration::from_secs(1);
        }
        assert_eq!(
            got,
            [
                (slot("2026-10-25T02:00:00", false), ts("2026-10-25T00:00:00Z")),
                (slot("2026-10-25T02:00:00", true), ts("2026-10-25T01:00:00Z")),
                (slot("2026-10-25T03:00:00", false), ts("2026-10-25T02:00:00Z")),
            ]
        );
    }

    #[test]
    fn quarter_hours_fire_eight_times_in_order() {
        let s = sched("*/15 * * * *", "Europe/Berlin");
        let mut cur = slot("2026-10-25T01:45:00", false);
        let mut now = ts("2026-10-24T23:45:00Z");
        let mut got = Vec::new();
        for _ in 0..8 {
            let (next, t) = s.next_after(cur, now, WALK_BOUND).unwrap();
            got.push(t);
            cur = next;
            now = t;
        }
        let want = ["00:00", "00:15", "00:30", "00:45", "01:00", "01:15", "01:30", "01:45"].map(|h| ts(&format!("2026-10-25T{h}:00Z")));
        assert_eq!(got, want);
    }

    #[test]
    fn fixed_hour_fires_once_on_the_fall_day() {
        let s = sched("30 1 * * *", "America/New_York");
        let (a, ta) = s.next_after(slot("2026-10-31T01:30:00", false), ts("2026-10-31T05:31:00Z"), WALK_BOUND).unwrap();
        assert_eq!((a, ta), (slot("2026-11-01T01:30:00", false), ts("2026-11-01T05:30:00Z")));
        let (b, tb) = s.next_after(a, ta, WALK_BOUND).unwrap();
        assert_eq!((b, tb), (slot("2026-11-02T01:30:00", false), ts("2026-11-02T06:30:00Z")));
    }

    #[test]
    fn spring_gap_fires_once() {
        let s = sched("0 * * * *", "Europe/Berlin");
        let from = slot("2026-03-29T01:00:00", false);
        let (a, ta) = s.next_after(from, ts("2026-03-29T00:00:01Z"), WALK_BOUND).unwrap();
        assert_eq!((a, ta), (slot("2026-03-29T02:00:00", false), ts("2026-03-29T01:00:00Z")));
        let (b, tb) = s.next_after(a, ts("2026-03-29T01:00:01Z"), WALK_BOUND).unwrap();
        assert_eq!((b, tb), (slot("2026-03-29T04:00:00", false), ts("2026-03-29T02:00:00Z")));
    }

    #[test]
    fn recovery_between_the_two_occurrences() {
        let s = sched("0 * * * *", "Europe/Berlin");
        let fired = slot("2026-10-25T02:00:00", false);
        assert_eq!(
            s.next_after(fired, ts("2026-10-25T00:30:00Z"), WALK_BOUND),
            Ok((slot("2026-10-25T02:00:00", true), ts("2026-10-25T01:00:00Z")))
        );
        assert_eq!(
            s.next_after(fired, ts("2026-10-25T01:30:00Z"), WALK_BOUND),
            Ok((slot("2026-10-25T03:00:00", false), ts("2026-10-25T02:00:00Z")))
        );
    }

    #[test]
    fn recovery_at_the_second_0115_does_not_repeat_0130() {
        let s = sched("30 1 * * *", "America/New_York");
        let (next, _) = s.next_after(slot("2026-11-01T01:30:00", false), ts("2026-11-01T06:15:00Z"), WALK_BOUND).unwrap();
        assert_eq!(next, slot("2026-11-02T01:30:00", false));
    }

    #[test]
    fn first_slot_inside_the_second_pass() {
        let s = sched("*/15 * * * *", "Europe/Berlin");
        assert_eq!(
            s.first_slot(ts("2026-10-25T01:20:00Z"), WALK_BOUND),
            Ok((slot("2026-10-25T02:30:00", true), ts("2026-10-25T01:30:00Z")))
        );
    }

    #[test]
    fn a_long_outage_walk_is_clamped() {
        let s = sched("*/15 * * * *", "Europe/Berlin");
        let now = ts("2027-03-01T12:00:00Z");
        let started = std::time::Instant::now();
        let (next, t) = s.next_after(slot("2026-03-01T12:00:00", false), now, WALK_BOUND).unwrap();
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
        assert!(t > now);
        assert!(t <= now + jiff::SignedDuration::from_secs(900));
        assert!(next.civil > civil("2027-02-28T00:00:00"));
        assert_eq!(s.next_after(slot("2026-03-01T12:00:00", false), now, 1), Err(WalkError::BoundExceeded { bound: 1 }));
    }

    #[test]
    fn civil_gap_check() {
        let from = civil("2026-01-01T00:00:00");
        let gap = |cron: &str, min: u64| sched(cron, "Europe/Berlin").min_civil_gap(from, CIVIL_GAP_WINDOW_DAYS, min);
        assert_eq!(gap("0 */2 * * *", 7200), Ok(None));
        assert_eq!(gap("0 9 * * *", 86_400), Ok(None));
        assert_eq!(gap("*/15 * * * *", 900), Ok(None));
        assert_eq!(gap("*/15 * * * *", 901), Ok(Some(900)));
        assert_eq!(gap("* * * * *", 900), Ok(Some(60)));
    }

    #[test]
    fn civil_gap_check_counts_a_slot_exactly_at_the_start() {
        let from = civil("2027-03-01T00:00:00");
        assert_eq!(sched("0,1 0 1 3 *", "UTC").min_civil_gap(from, CIVIL_GAP_WINDOW_DAYS, 900), Ok(Some(60)));
    }

    #[test]
    fn civil_gap_check_covers_a_gap_that_starts_at_the_window_end() {
        // The first slot is exactly at the end of the 366-day window. Its successor is 60 s later.
        let from = civil("2027-02-28T00:00:00");
        assert_eq!(sched("* 0 29 2 *", "UTC").min_civil_gap(from, CIVIL_GAP_WINDOW_DAYS, 900), Ok(Some(60)));
    }

    #[test]
    fn civil_gap_check_ignores_a_gap_that_starts_after_the_window_end() {
        // The first slot is one day after the window end, so no earlier slot is in the window.
        let from = civil("2027-02-27T00:00:00");
        assert_eq!(sched("* 0 29 2 *", "UTC").min_civil_gap(from, CIVIL_GAP_WINDOW_DAYS, 900), Ok(None));
    }

    #[test]
    fn civil_gap_check_stops_early_at_the_five_field_floor() {
        let from = civil("2026-01-01T00:00:00");
        let started = std::time::Instant::now();
        // No pattern has a gap below 60 s, so a minimum of 60 s needs no walk.
        let none = sched("* * * * *", "UTC").min_civil_gap(from, CIVIL_GAP_WINDOW_DAYS, 60);
        let low = sched("* * * * *", "UTC").min_civil_gap(from, CIVIL_GAP_WINDOW_DAYS, 1);
        assert_eq!((none, low), (Ok(None), Ok(None)));
        assert!(started.elapsed() < std::time::Duration::from_millis(300), "the walk did not stop early: {:?}", started.elapsed());
    }
}
