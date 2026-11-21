// SPDX-License-Identifier: Apache-2.0

//! A five-field cron schedule in an IANA time zone. The syntax check refuses what `croner`
//! accepts as an extension, so the accepted set is small and the same everywhere.

use croner::Cron;
use croner::parser::{CronParser, Seconds};
use jiff::civil::DateTime;
use jiff::tz::TimeZone;

use crate::ports::{TzError, TzResolver};

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
    out[3] = replace_names(&out[3], &MONTHS, 1);
    out[4] = replace_names(&out[4], &WEEKDAYS, 0);
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

/// Replace each run of letters that is a name in `names` with its number (`first` is the number
/// of the first name). A run that is not a name stays, and the next check refuses it.
fn replace_names(field: &str, names: &[&str], first: usize) -> String {
    let mut out = String::new();
    let mut run = String::new();
    let flush = |run: &mut String, out: &mut String| {
        if run.is_empty() {
            return;
        }
        let upper = run.to_ascii_uppercase();
        match names.iter().position(|n| *n == upper) {
            Some(i) => out.push_str(&(i + first).to_string()),
            None => out.push_str(run),
        }
        run.clear();
    };
    for c in field.chars() {
        if c.is_ascii_alphabetic() {
            run.push(c);
        } else {
            flush(&mut run, &mut out);
            out.push(c);
        }
    }
    flush(&mut run, &mut out);
    out
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
    }
}
