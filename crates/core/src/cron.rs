//! UTC-only five-field Cron parser and logical-slot calculator.

use crate::{ErrorCode, PlatformError};
use croner::{
    Cron,
    parser::{CronParser, Seconds, Year},
};
use regex::Regex;
use std::sync::LazyLock;

const MINUTE_MS: i64 = 60_000;
const SEARCH_YEARS: i64 = 12;

/// Parsed immutable UTC Cron expression.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CronSchedule {
    normalized: String,
    cron: Cron,
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
        let normalized = fields
            .iter()
            .map(|field| field.to_ascii_uppercase())
            .collect::<Vec<_>>()
            .join(" ");
        if !cloudflare_syntax()
            .as_ref()
            .map_err(|_| invalid())?
            .is_match(&normalized)
            || fields
                .iter()
                .flat_map(|field| field.split(','))
                .any(|item| {
                    item.split_once('/')
                        .is_some_and(|(_, step)| step.parse::<u8>().is_err())
                })
        {
            return Err(invalid());
        }
        // Croner's offset weekday mode needs an explicit range for wildcard steps and list items.
        let parser_expression = normalized
            .split(' ')
            .enumerate()
            .map(|(index, field)| {
                let (minimum, maximum) = [(0, 59), (0, 23), (1, 31), (1, 12), (1, 7)][index];
                field
                    .split(',')
                    .map(|item| {
                        if item == "*" && field != "*" {
                            format!("{minimum}-{maximum}")
                        } else if index == 4
                            && let Some(step) = item.strip_prefix("*/")
                        {
                            format!("1-7/{step}")
                        } else {
                            item.to_owned()
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(",")
            })
            .collect::<Vec<_>>()
            .join(" ");
        let cron = CronParser::builder()
            .seconds(Seconds::Disallowed)
            .year(Year::Disallowed)
            .alternative_weekdays(true)
            .dom_and_dow(false)
            // Cloudflare accepts a/b as a-maximum/b; the syntax guard rejects bare /b.
            .sloppy_ranges(true)
            .build()
            .parse(&parser_expression)
            .map_err(|_| invalid())?;
        Ok(Self { normalized, cron })
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
        self.next_from_minute(after_ms.div_euclid(MINUTE_MS) + 1)
    }

    fn next_from_minute(&self, mut candidate: i64) -> Result<i64, PlatformError> {
        let limit = candidate
            .checked_add(SEARCH_YEARS * 366 * 24 * 60)
            .ok_or_else(invalid)?;
        let includes_last_day = self
            .normalized
            .split(' ')
            .nth(2)
            .is_some_and(|field| field.split(',').any(|item| item == "L"));
        while candidate <= limit {
            let timestamp = candidate.checked_mul(MINUTE_MS).ok_or_else(invalid)?;
            // Gregorian dates and weekdays repeat every 400 years; keep all i64 timestamps representable.
            let calendar_ms = timestamp % (146_097 * 24 * 60 * MINUTE_MS);
            let time = jiff::Timestamp::from_millisecond(calendar_ms)
                .map_err(|_| invalid())?
                .to_zoned(jiff::tz::TimeZone::UTC);
            let pattern = &self.cron.pattern;
            // Use field matching to retain our 12-year budget rather than croner's year-5000 search limit.
            let matches = || {
                // Croner combines L and LW flags; preserve their list union using its calendar.
                let last_day_matches = includes_last_day
                    && time.day() as u32
                        == croner::time::days_in_month(i32::from(time.year()), time.month() as u32);
                Ok::<_, croner::errors::CronError>(
                    pattern.minute_match(time.minute() as u32)?
                        && pattern.hour_match(time.hour() as u32)?
                        && pattern.month_match(time.month() as u32)?
                        && (pattern.day_match(
                            i32::from(time.year()),
                            time.month() as u32,
                            time.day() as u32,
                            croner::Weekday::from_days_from_sunday(
                                time.weekday().to_sunday_zero_offset() as u32,
                            ),
                        )? || last_day_matches),
                )
            };
            if matches().map_err(|_| invalid())? {
                return Ok(timestamp);
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
        let mut next = self.next_from_minute(not_before_ms.div_euclid(MINUTE_MS))?;
        let mut latest = None;
        while next <= at_or_before_ms {
            if next >= not_before_ms {
                latest = Some(next);
            }
            next = self.next_after_ms(next)?;
        }
        Ok(latest)
    }
}

fn cloudflare_syntax() -> &'static Result<Regex, regex::Error> {
    static SYNTAX: LazyLock<Result<Regex, regex::Error>> = LazyLock::new(|| {
        let number = "[0-9]+";
        let month = "(?:[0-9]+|JAN|FEB|MAR|APR|MAY|JUN|JUL|AUG|SEP|OCT|NOV|DEC)";
        let weekday = "(?:[0-9]+|SUN|MON|TUE|WED|THU|FRI|SAT)";
        let ordinary = |value| format!("(?:\\*|{value}(?:-{value})?)(?:/[0-9]+)?");
        let list = |item| format!("(?:{item})(?:,(?:{item}))*");
        Regex::new(&format!(
            "\\A{} {} {} {} {}\\z",
            list(ordinary(number)),
            list(ordinary(number)),
            list(format!("{}|L|LW|{number}W", ordinary(number))),
            list(ordinary(month)),
            list(format!(
                "{}|{weekday}L|{weekday}#0*[1-5]",
                ordinary(weekday)
            ))
        ))
    });
    &SYNTAX
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
    fn differential_next_three_utc_slots() {
        // Captured from the pre-croner parser at 2024-02-28 and 2026-01-30 23:59:59.999 UTC.
        const ANCHORS: [i64; 2] = [1_709_164_799_999, 1_769_817_599_999];
        #[rustfmt::skip]
        let cases = [
            ("* * * * *", [[1709164800000, 1709164860000, 1709164920000], [1769817600000, 1769817660000, 1769817720000]]),
            ("0 * * * *", [[1709164800000, 1709168400000, 1709172000000], [1769817600000, 1769821200000, 1769824800000]]),
            ("*/5 * * * *", [[1709164800000, 1709165100000, 1709165400000], [1769817600000, 1769817900000, 1769818200000]]),
            ("0,15,45 * * * *", [[1709164800000, 1709165700000, 1709167500000], [1769817600000, 1769818500000, 1769820300000]]),
            ("10-20/3 * * * *", [[1709165400000, 1709165580000, 1709165760000], [1769818200000, 1769818380000, 1769818560000]]),
            ("0 */6 * * *", [[1709164800000, 1709186400000, 1709208000000], [1769817600000, 1769839200000, 1769860800000]]),
            ("30 2,14 * * *", [[1709173800000, 1709217000000, 1709260200000], [1769826600000, 1769869800000, 1769913000000]]),
            ("0 9-17/2 * * *", [[1709197200000, 1709204400000, 1709211600000], [1769850000000, 1769857200000, 1769864400000]]),
            ("0 0 1 * *", [[1709251200000, 1711929600000, 1714521600000], [1769904000000, 1772323200000, 1775001600000]]),
            ("0 0 28-31 * *", [[1709164800000, 1711584000000, 1711670400000], [1769817600000, 1772236800000, 1774656000000]]),
            ("0 0 */7 * *", [[1709164800000, 1709251200000, 1709856000000], [1769904000000, 1770508800000, 1771113600000]]),
            ("0 0 L * *", [[1709164800000, 1711843200000, 1714435200000], [1769817600000, 1772236800000, 1774915200000]]),
            ("0 0 LW * *", [[1709164800000, 1711670400000, 1714435200000], [1772150400000, 1774915200000, 1777507200000]]),
            ("0 0 1W * *", [[1709251200000, 1711929600000, 1714521600000], [1769990400000, 1772409600000, 1775001600000]]),
            ("0 0 15W * *", [[1710460800000, 1713139200000, 1715731200000], [1771200000000, 1773619200000, 1776211200000]]),
            ("0 0 31W * *", [[1711670400000, 1717113600000, 1722384000000], [1774915200000, 1780012800000, 1785456000000]]),
            ("0 0 1,L * *", [[1709164800000, 1709251200000, 1711843200000], [1769817600000, 1769904000000, 1772236800000]]),
            ("0 0 1W,15W * *", [[1709251200000, 1710460800000, 1711929600000], [1769990400000, 1771200000000, 1772409600000]]),
            ("0 0 * * 1", [[1709424000000, 1710028800000, 1710633600000], [1769904000000, 1770508800000, 1771113600000]]),
            ("0 0 * * 7", [[1709337600000, 1709942400000, 1710547200000], [1769817600000, 1770422400000, 1771027200000]]),
            ("0 0 * * mOn", [[1709510400000, 1710115200000, 1710720000000], [1769990400000, 1770595200000, 1771200000000]]),
            ("0 0 * * sUn-sAt", [[1709164800000, 1709251200000, 1709337600000], [1769817600000, 1769904000000, 1769990400000]]),
            ("0 0 * * 2-6/2", [[1709251200000, 1709510400000, 1709683200000], [1769990400000, 1770163200000, 1770336000000]]),
            ("0 0 * * 1,4,7", [[1709337600000, 1709424000000, 1709683200000], [1769817600000, 1769904000000, 1770163200000]]),
            ("0 0 * * 6L", [[1711670400000, 1714089600000, 1717113600000], [1772150400000, 1774569600000, 1776988800000]]),
            ("0 0 * * fRiL", [[1711670400000, 1714089600000, 1717113600000], [1772150400000, 1774569600000, 1776988800000]]),
            ("0 0 * * 2#1", [[1709510400000, 1711929600000, 1714953600000], [1769990400000, 1772409600000, 1775433600000]]),
            ("0 0 * * MON#2", [[1710115200000, 1712534400000, 1715558400000], [1770595200000, 1773014400000, 1776038400000]]),
            ("0 0 * * 4#3", [[1710892800000, 1713312000000, 1715731200000], [1771372800000, 1773792000000, 1776211200000]]),
            ("0 0 * * 5#4", [[1711584000000, 1714003200000, 1716422400000], [1772064000000, 1774483200000, 1776902400000]]),
            ("0 0 * * 7#5", [[1711756800000, 1719619200000, 1725062400000], [1769817600000, 1780099200000, 1787961600000]]),
            ("0 0 * * 2#1,6L", [[1709510400000, 1711670400000, 1711929600000], [1769990400000, 1772150400000, 1772409600000]]),
            ("0 0 1 * MON", [[1709251200000, 1709510400000, 1710115200000], [1769904000000, 1769990400000, 1770595200000]]),
            ("0 0 13 * FRI", [[1709251200000, 1709856000000, 1710288000000], [1770336000000, 1770940800000, 1771545600000]]),
            ("0 0 29 fEb *", [[1709164800000, 1835395200000, 1961625600000], [1835395200000, 1961625600000, 2087856000000]]),
            ("0 0 31 APR,JUN,DEC *", [[1735603200000, 1767139200000, 1798675200000], [1798675200000, 1830211200000, 1861833600000]]),
            ("0 0 1 jAn-mAr *", [[1709251200000, 1735689600000, 1738368000000], [1769904000000, 1772323200000, 1798761600000]]),
            ("0 0 1 */3 *", [[1711929600000, 1719792000000, 1727740800000], [1775001600000, 1782864000000, 1790812800000]]),
            ("59 23 L * *", [[1709251140000, 1711929540000, 1714521540000], [1769903940000, 1772323140000, 1775001540000]]),
            ("0 0 */1 * MON", [[1709164800000, 1709251200000, 1709337600000], [1769817600000, 1769904000000, 1769990400000]]),
            ("0 0 * * */2", [[1709164800000, 1709337600000, 1709424000000], [1769817600000, 1769904000000, 1770076800000]]),
            ("*,5 0 * * *", [[1709164800000, 1709164860000, 1709164920000], [1769817600000, 1769817660000, 1769817720000]]),
            ("0 0 *,L * *", [[1709164800000, 1709251200000, 1709337600000], [1769817600000, 1769904000000, 1769990400000]]),
            ("0 0 * * *,MON", [[1709164800000, 1709251200000, 1709337600000], [1769817600000, 1769904000000, 1769990400000]]),
            ("0 0 L,LW * *", [[1709164800000, 1711670400000, 1711843200000], [1769817600000, 1772150400000, 1772236800000]]),
            ("0 0 * * MON#02", [[1710115200000, 1712534400000, 1715558400000], [1770595200000, 1773014400000, 1776038400000]]),
            ("0 0 L,LW * MON", [[1709164800000, 1709510400000, 1710115200000], [1769817600000, 1769990400000, 1770595200000]]),
            // Intentional correction: a/b expands through the field maximum, rather than matching only a.
            ("5/5 * * * *", [[1709165100000, 1709165400000, 1709165700000], [1769817900000, 1769818200000, 1769818500000]]),
            ("1/7 * * * *", [[1709164860000, 1709165280000, 1709165700000], [1769817660000, 1769818080000, 1769818500000]]),
        ];
        for (expression, expected) in cases {
            let schedule = CronSchedule::parse(expression).unwrap();
            assert_eq!(schedule.normalized(), expression.to_ascii_uppercase());
            for (anchor, expected) in ANCHORS.into_iter().zip(expected) {
                let mut after = anchor;
                let actual: [i64; 3] = std::array::from_fn(|_| {
                    after = schedule.next_after_ms(after).unwrap();
                    after
                });
                assert_eq!(actual, expected, "{expression} after {anchor}");
            }
        }
    }

    #[test]
    fn cloudflare_syntax_guard_rejects_croner_extensions_and_malformed_items() {
        for expression in [
            "@daily",
            "@hourly",
            "@weekly",
            "@yearly",
            "@reboot",
            "L * * * *",
            "0 L * * *",
            "0 0 * L *",
            "0 0 * * L",
            "0 0 * * +MON",
            "+1 * * * *",
            "H * * * *",
            "R * * * *",
            "/5 * * * *",
            "0 0 * * /2",
            "0 0 L-3 * *",
            "0 0 3L * *",
            "0 0 1W2 * *",
            "0 0 1WW * *",
            "0 0 * * 1#L",
            "0 0 * * 1-3#2",
            "0 0 * * 1-3L",
            "0 0 * * 1#0",
            "0 0 * * 1#2#3",
            "0 0 * * 1#6",
            "0 0 * * 8",
            "0 0 * JANJAN *",
            "0 0 * * MONMON",
            "0 0 * * MONW",
            "1, * * * *",
            ",1 * * * *",
            "1,,2 * * * *",
            "*/0 * * * *",
            "*/256 * * * *",
            "1/2/3 * * * *",
            "2-1 * * * *",
            "0 0 * * 0-7",
            "0 0 * * 1-8",
            " * * * * *",
            "* * * * * ",
            "* * * * *\n",
            "\t* * * * *",
        ] {
            assert_eq!(
                CronSchedule::parse(expression).unwrap_err().code(),
                ErrorCode::CronExpressionInvalid,
                "{expression}"
            );
        }
        for expression in ["0 0 0 * * *", "0 0 0 * * * 2026", "? * * * *", "0 0 ? * *"] {
            assert_eq!(
                CronSchedule::parse(expression).unwrap_err().code(),
                ErrorCode::CronExpressionUnsupported,
                "{expression}"
            );
        }
        let maximum = format!("{} * * * *", "0".repeat(248));
        assert_eq!(maximum.len(), 256);
        assert!(CronSchedule::parse(&maximum).is_ok());
        assert_eq!(
            CronSchedule::parse(&format!("0{maximum}"))
                .unwrap_err()
                .code(),
            ErrorCode::CronExpressionInvalid
        );
        assert_eq!(
            CronSchedule::parse("0\t 0  1 jAn\r\nMON")
                .unwrap()
                .normalized(),
            "0 0 1 JAN MON"
        );
    }

    #[test]
    fn single_value_steps_expand_to_the_field_maximum() {
        let anchor = 1_769_904_000_000; // 2026-02-01 00:00 UTC.
        for (expression, expected) in [
            (
                "1/7 * * * *",
                [1_769_904_060_000, 1_769_904_480_000, 1_769_904_900_000],
            ),
            (
                "0 1/7 * * *",
                [1_769_907_600_000, 1_769_932_800_000, 1_769_958_000_000],
            ),
            (
                "0 0 1/7 * *",
                [1_770_508_800_000, 1_771_113_600_000, 1_771_718_400_000],
            ),
            (
                "0 0 1 FEB/3 *",
                [1_777_593_600_000, 1_785_542_400_000, 1_793_491_200_000],
            ),
            (
                "0 0 * * MON/2",
                [1_769_990_400_000, 1_770_163_200_000, 1_770_336_000_000],
            ),
        ] {
            let schedule = CronSchedule::parse(expression).unwrap();
            let mut after = anchor;
            for expected in expected {
                after = schedule.next_after_ms(after).unwrap();
                assert_eq!(after, expected, "{expression}");
            }
        }
        let schedule = CronSchedule::parse("1/7 * * * *").unwrap();
        assert_eq!(
            schedule.next_after_ms(anchor + 56 * MINUTE_MS).unwrap(),
            anchor + 57 * MINUTE_MS
        );
    }

    #[test]
    fn search_is_strict_recovery_is_inclusive_and_timestamps_are_checked() {
        let every_minute = CronSchedule::parse("* * * * *").unwrap();
        for after in [0, 1, MINUTE_MS - 1] {
            assert_eq!(every_minute.next_after_ms(after).unwrap(), MINUTE_MS);
        }
        assert_eq!(
            every_minute.next_after_ms(MINUTE_MS).unwrap(),
            2 * MINUTE_MS
        );
        for (start, end, expected) in [
            (0, 0, Some(0)),
            (0, MINUTE_MS, Some(MINUTE_MS)),
            (1, MINUTE_MS - 1, None),
            (MINUTE_MS, MINUTE_MS, Some(MINUTE_MS)),
            (MINUTE_MS + 1, 2 * MINUTE_MS - 1, None),
        ] {
            assert_eq!(
                every_minute.latest_at_or_before_ms(start, end).unwrap(),
                expected
            );
        }
        for (start, end) in [(-1, 0), (1, 0), (0, -1)] {
            assert_eq!(
                every_minute
                    .latest_at_or_before_ms(start, end)
                    .unwrap_err()
                    .code(),
                ErrorCode::CronExpressionInvalid
            );
        }
        for after in [-1, i64::MAX] {
            assert_eq!(
                every_minute.next_after_ms(after).unwrap_err().code(),
                ErrorCode::CronExpressionInvalid
            );
        }
        assert_eq!(
            every_minute.next_after_ms(253_402_300_799_999).unwrap(),
            253_402_300_800_000
        );
        assert_eq!(
            every_minute.next_after_ms(i64::MAX - 120_000).unwrap(),
            9_223_372_036_854_660_000
        );
        let leap_day = CronSchedule::parse("0 0 29 FEB *").unwrap();
        let after: jiff::Timestamp = "2096-02-29T00:00:00Z".parse().unwrap();
        let next: jiff::Timestamp = "2104-02-29T00:00:00Z".parse().unwrap();
        assert_eq!(
            leap_day.next_after_ms(after.as_millisecond()).unwrap(),
            next.as_millisecond()
        );
        let impossible = CronSchedule::parse("0 0 30 FEB *").unwrap();
        let error = impossible.next_after_ms(0).unwrap_err();
        assert_eq!(error.code(), ErrorCode::CronExpressionInvalid);
        assert!(error.to_string().contains("no bounded future UTC slot"));
    }

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
