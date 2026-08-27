//! UTC-only five-field Cron parser and logical-slot calculator.

use crate::{ErrorCode, PlatformError};

const MINUTE_MS: i64 = 60_000;
const SEARCH_YEARS: i64 = 12;

/// Parsed immutable UTC Cron expression.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CronSchedule {
    normalized: String,
    minute: u64,
    hour: u64,
    month: u64,
    day_of_month: DayOfMonth,
    day_of_week: DayOfWeek,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct DayOfMonth {
    ordinary: u64,
    any: bool,
    last: bool,
    last_weekday: bool,
    nearest_weekdays: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct DayOfWeek {
    ordinary: u64,
    any: bool,
    last: Vec<u8>,
    nth: Vec<(u8, u8)>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct UtcMinute {
    year: i64,
    month: u8,
    day: u8,
    hour: u8,
    minute: u8,
    weekday: u8,
}

impl CronSchedule {
    /// Parse one UTC-only five-field expression.
    pub fn parse(expression: &str) -> Result<Self, PlatformError> {
        if expression.is_empty() || expression.len() > 256 || expression.trim() != expression {
            return Err(invalid());
        }
        let fields = expression.split_ascii_whitespace().collect::<Vec<_>>();
        if fields.len() != 5 {
            return Err(if fields.len() == 6 || fields.len() == 7 {
                unsupported()
            } else {
                invalid()
            });
        }
        if fields.iter().any(|field| field.contains('?')) {
            return Err(unsupported());
        }
        Ok(Self {
            normalized: fields
                .iter()
                .map(|field| field.to_ascii_uppercase())
                .collect::<Vec<_>>()
                .join(" "),
            minute: parse_ordinary(fields[0], 0, 59, &[])?,
            hour: parse_ordinary(fields[1], 0, 23, &[])?,
            day_of_month: parse_day_of_month(fields[2])?,
            month: parse_ordinary(fields[3], 1, 12, &month_names())?,
            day_of_week: parse_day_of_week(fields[4])?,
        })
    }

    /// Stable parser-normalized spelling used for integrity hashing.
    #[must_use]
    pub fn normalized(&self) -> &str {
        &self.normalized
    }

    /// Find the first matching UTC minute strictly after the supplied Unix millisecond time.
    pub fn next_after_ms(&self, after_ms: i64) -> Result<i64, PlatformError> {
        if after_ms < 0 {
            return Err(invalid());
        }
        let floor = after_ms.div_euclid(MINUTE_MS);
        let mut candidate = floor.checked_add(1).ok_or_else(invalid)?;
        let limit = candidate
            .checked_add(SEARCH_YEARS * 366 * 24 * 60)
            .ok_or_else(invalid)?;
        while candidate <= limit {
            let parts = utc_minute(candidate)?;
            if self.matches(parts) {
                return candidate.checked_mul(MINUTE_MS).ok_or_else(invalid);
            }
            candidate = candidate.checked_add(1).ok_or_else(invalid)?;
        }
        Err(PlatformError::new(
            ErrorCode::CronExpressionInvalid,
            "Cron expression has no bounded future UTC slot",
        ))
    }

    /// Find the newest matching UTC minute in the inclusive bounded interval.
    pub fn latest_at_or_before_ms(
        &self,
        not_before_ms: i64,
        at_or_before_ms: i64,
    ) -> Result<Option<i64>, PlatformError> {
        if not_before_ms < 0 || at_or_before_ms < not_before_ms {
            return Err(invalid());
        }
        let start = not_before_ms.saturating_sub(MINUTE_MS).max(0);
        let mut next = self.next_after_ms(start)?;
        let mut latest = None;
        while next <= at_or_before_ms {
            if next >= not_before_ms {
                latest = Some(next);
            }
            next = self.next_after_ms(next)?;
        }
        Ok(latest)
    }

    fn matches(&self, value: UtcMinute) -> bool {
        bit(self.minute, value.minute)
            && bit(self.hour, value.hour)
            && bit(self.month, value.month)
            && self.matches_day(value)
    }

    fn matches_day(&self, value: UtcMinute) -> bool {
        let month_days = days_in_month(value.year, value.month);
        let dom = bit(self.day_of_month.ordinary, value.day)
            || (self.day_of_month.last && value.day == month_days)
            || (self.day_of_month.last_weekday
                && value.day == nearest_weekday(value.year, value.month, month_days))
            || self
                .day_of_month
                .nearest_weekdays
                .iter()
                .copied()
                .filter(|day| *day <= month_days)
                .any(|day| value.day == nearest_weekday(value.year, value.month, day));
        let dow =
            bit(self.day_of_week.ordinary, value.weekday)
                || self.day_of_week.last.iter().copied().any(|weekday| {
                    value.weekday == weekday && value.day.saturating_add(7) > month_days
                })
                || self.day_of_week.nth.iter().copied().any(|(weekday, nth)| {
                    value.weekday == weekday && ((value.day - 1) / 7 + 1) == nth
                });
        match (self.day_of_month.any, self.day_of_week.any) {
            (true, true) => true,
            (true, false) => dow,
            (false, true) => dom,
            (false, false) => dom || dow,
        }
    }
}

fn parse_day_of_month(field: &str) -> Result<DayOfMonth, PlatformError> {
    let any = field == "*";
    let mut result = DayOfMonth {
        ordinary: 0,
        any,
        last: false,
        last_weekday: false,
        nearest_weekdays: Vec::new(),
    };
    for item in field.split(',') {
        let upper = item.to_ascii_uppercase();
        if upper == "L" {
            result.last = true;
        } else if upper == "LW" {
            result.last_weekday = true;
        } else if let Some(day) = upper.strip_suffix('W') {
            let day = parse_value(day, 1, 31, &[])?;
            result.nearest_weekdays.push(day);
        } else {
            result.ordinary |= parse_ordinary(item, 1, 31, &[])?;
        }
    }
    result.nearest_weekdays.sort_unstable();
    result.nearest_weekdays.dedup();
    if result.ordinary == 0
        && !result.last
        && !result.last_weekday
        && result.nearest_weekdays.is_empty()
    {
        return Err(invalid());
    }
    Ok(result)
}

fn parse_day_of_week(field: &str) -> Result<DayOfWeek, PlatformError> {
    let any = field == "*";
    let names = weekday_names();
    let mut result = DayOfWeek {
        ordinary: 0,
        any,
        last: Vec::new(),
        nth: Vec::new(),
    };
    for item in field.split(',') {
        let upper = item.to_ascii_uppercase();
        if let Some((weekday, nth)) = upper.split_once('#') {
            if nth.contains('#') {
                return Err(invalid());
            }
            let weekday = parse_value(weekday, 1, 7, &names)?;
            let nth = parse_value(nth, 1, 5, &[])?;
            result.nth.push((weekday, nth));
        } else if upper.len() > 1 && upper.ends_with('L') {
            let weekday = parse_value(&upper[..upper.len() - 1], 1, 7, &names)?;
            result.last.push(weekday);
        } else {
            result.ordinary |= parse_ordinary(item, 1, 7, &names)?;
        }
    }
    result.last.sort_unstable();
    result.last.dedup();
    result.nth.sort_unstable();
    result.nth.dedup();
    if result.ordinary == 0 && result.last.is_empty() && result.nth.is_empty() {
        return Err(invalid());
    }
    Ok(result)
}

fn parse_ordinary(
    field: &str,
    minimum: u8,
    maximum: u8,
    names: &[(&str, u8)],
) -> Result<u64, PlatformError> {
    if field.is_empty() {
        return Err(invalid());
    }
    let mut mask = 0_u64;
    for item in field.split(',') {
        if item.is_empty() {
            return Err(invalid());
        }
        let (base, step) = item.split_once('/').map_or((item, 1), |(base, step)| {
            (base, step.parse::<u8>().unwrap_or(0))
        });
        if step == 0 || item.matches('/').count() > 1 {
            return Err(invalid());
        }
        let (start, end) = if base == "*" {
            (minimum, maximum)
        } else if let Some((start, end)) = base.split_once('-') {
            if base.matches('-').count() > 1 {
                return Err(invalid());
            }
            (
                parse_value(start, minimum, maximum, names)?,
                parse_value(end, minimum, maximum, names)?,
            )
        } else {
            let value = parse_value(base, minimum, maximum, names)?;
            (value, value)
        };
        if start > end {
            return Err(invalid());
        }
        let mut value = start;
        loop {
            mask |= 1_u64.checked_shl(u32::from(value)).ok_or_else(invalid)?;
            let Some(next) = value.checked_add(step) else {
                break;
            };
            if next > end {
                break;
            }
            value = next;
        }
    }
    (mask != 0).then_some(mask).ok_or_else(invalid)
}

fn parse_value(
    value: &str,
    minimum: u8,
    maximum: u8,
    names: &[(&str, u8)],
) -> Result<u8, PlatformError> {
    let upper = value.to_ascii_uppercase();
    let parsed = names
        .iter()
        .find_map(|(name, number)| (*name == upper).then_some(*number))
        .or_else(|| upper.parse::<u8>().ok())
        .ok_or_else(invalid)?;
    (minimum..=maximum)
        .contains(&parsed)
        .then_some(parsed)
        .ok_or_else(invalid)
}

fn utc_minute(unix_minutes: i64) -> Result<UtcMinute, PlatformError> {
    if unix_minutes < 0 {
        return Err(invalid());
    }
    let days = unix_minutes.div_euclid(24 * 60);
    let minute_of_day = unix_minutes.rem_euclid(24 * 60);
    let (year, month, day) = civil_from_days(days);
    let weekday = u8::try_from((days + 4).rem_euclid(7) + 1).map_err(|_| invalid())?;
    Ok(UtcMinute {
        year,
        month,
        day,
        hour: u8::try_from(minute_of_day / 60).map_err(|_| invalid())?,
        minute: u8::try_from(minute_of_day % 60).map_err(|_| invalid())?,
        weekday,
    })
}

fn civil_from_days(days_since_epoch: i64) -> (i64, u8, u8) {
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    (
        year,
        u8::try_from(month).unwrap_or(1),
        u8::try_from(day).unwrap_or(1),
    )
}

fn nearest_weekday(year: i64, month: u8, day: u8) -> u8 {
    let weekday = weekday_for_date(year, month, day);
    match weekday {
        7 if day == 1 => 3,
        7 => day - 1,
        1 if day == days_in_month(year, month) => day - 2,
        1 => day + 1,
        _ => day,
    }
}

fn weekday_for_date(year: i64, month: u8, day: u8) -> u8 {
    let mut total = 0_i64;
    if year >= 1970 {
        for current in 1970..year {
            total += if is_leap(current) { 366 } else { 365 };
        }
    } else {
        for current in year..1970 {
            total -= if is_leap(current) { 366 } else { 365 };
        }
    }
    for current in 1..month {
        total += i64::from(days_in_month(year, current));
    }
    total += i64::from(day) - 1;
    u8::try_from((total + 4).rem_euclid(7) + 1).unwrap_or(1)
}

const fn days_in_month(year: i64, month: u8) -> u8 {
    match month {
        2 if is_leap(year) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

const fn is_leap(year: i64) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

const fn bit(mask: u64, value: u8) -> bool {
    mask & (1_u64 << value) != 0
}

const fn month_names() -> [(&'static str, u8); 12] {
    [
        ("JAN", 1),
        ("FEB", 2),
        ("MAR", 3),
        ("APR", 4),
        ("MAY", 5),
        ("JUN", 6),
        ("JUL", 7),
        ("AUG", 8),
        ("SEP", 9),
        ("OCT", 10),
        ("NOV", 11),
        ("DEC", 12),
    ]
}

const fn weekday_names() -> [(&'static str, u8); 7] {
    [
        ("SUN", 1),
        ("MON", 2),
        ("TUE", 3),
        ("WED", 4),
        ("THU", 5),
        ("FRI", 6),
        ("SAT", 7),
    ]
}

fn invalid() -> PlatformError {
    PlatformError::new(
        ErrorCode::CronExpressionInvalid,
        "Cron expression is invalid",
    )
}

fn unsupported() -> PlatformError {
    PlatformError::new(
        ErrorCode::CronExpressionUnsupported,
        "Cron expression uses unsupported syntax",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_fields_names_and_dom_dow_or_match_utc_slots() {
        let every_five = CronSchedule::parse("*/5 * * * *").unwrap();
        assert_eq!(
            every_five.next_after_ms(1_776_254_400_000).unwrap(),
            1_776_254_700_000
        );
        assert_eq!(
            CronSchedule::parse("0 0 1 JAN MON").unwrap().normalized(),
            "0 0 1 JAN MON"
        );
        let monday_or_first = CronSchedule::parse("0 0 1 * MON").unwrap();
        assert_eq!(
            monday_or_first.next_after_ms(1_780_358_400_000).unwrap(),
            1_780_876_800_000
        );
    }

    #[test]
    fn quartz_like_last_weekday_nearest_and_nth_are_bounded() {
        let last = CronSchedule::parse("0 0 L * *").unwrap();
        assert_eq!(
            last.next_after_ms(1_769_904_000_000).unwrap(),
            1_772_236_800_000
        );
        let last_weekday = CronSchedule::parse("0 0 LW * *").unwrap();
        assert!(last_weekday.next_after_ms(1_769_904_000_000).is_ok());
        let nearest = CronSchedule::parse("0 0 1W * *").unwrap();
        assert!(nearest.next_after_ms(1_769_904_000_000).is_ok());
        let second_monday = CronSchedule::parse("0 0 * * MON#2").unwrap();
        assert!(second_monday.next_after_ms(1_769_904_000_000).is_ok());
        let last_friday = CronSchedule::parse("0 0 * * 6L").unwrap();
        assert!(last_friday.next_after_ms(1_769_904_000_000).is_ok());
    }

    #[test]
    fn malformed_negative_and_extra_fields_fail_closed() {
        for value in [
            "",
            "* * * *",
            "60 * * * *",
            "* * 0 * *",
            "* * * * 0",
            "* * * * MON#6",
        ] {
            assert_eq!(
                CronSchedule::parse(value).unwrap_err().code(),
                ErrorCode::CronExpressionInvalid
            );
        }
        assert_eq!(
            CronSchedule::parse("0 * * * * *").unwrap_err().code(),
            ErrorCode::CronExpressionUnsupported
        );
        assert_eq!(
            CronSchedule::parse("0 0 ? * *").unwrap_err().code(),
            ErrorCode::CronExpressionUnsupported
        );
        assert_eq!(
            CronSchedule::parse("* * * * *")
                .unwrap()
                .next_after_ms(-1)
                .unwrap_err()
                .code(),
            ErrorCode::CronExpressionInvalid
        );
    }
}
