use crate::config::TIMEZONE;
use chrono::{DateTime, Datelike, Days, NaiveDate, Utc};

/// The calendar date in Buenos Aires at instant `now`.
pub fn local_date(now: DateTime<Utc>) -> NaiveDate {
    now.with_timezone(&TIMEZONE).date_naive()
}

/// The Monday of the Buenos Aires week containing `now`.
pub fn week_start(now: DateTime<Utc>) -> NaiveDate {
    monday_of(local_date(now))
}

pub fn monday_of(date: NaiveDate) -> NaiveDate {
    let offset = u64::from(date.weekday().num_days_from_monday());
    date.checked_sub_days(Days::new(offset))
        .expect("a Monday exists within six days before any representable date")
}

/// `2026-07-20`, the storage format for `local_date` and `week_start`.
pub fn iso(date: NaiveDate) -> String {
    date.format("%Y-%m-%d").to_string()
}

/// `20/07`, the display format used in replies.
pub fn day_month(date: NaiveDate) -> String {
    date.format("%d/%m").to_string()
}

/// `2026-07`, the key `/mes` and `/export` filter on.
pub fn month_key(date: NaiveDate) -> String {
    date.format("%Y-%m").to_string()
}

/// Accepts a user-supplied `YYYY-MM` and echoes it back normalized, so a typo
/// becomes an error message instead of an empty report.
pub fn parse_month_key(input: &str) -> Option<String> {
    let trimmed = input.trim();
    let (year, month) = trimmed.split_once('-')?;
    if year.len() != 4 || month.len() != 2 {
        return None;
    }
    let year: i32 = year.parse().ok()?;
    let month: u32 = month.parse().ok()?;
    NaiveDate::from_ymd_opt(year, month, 1)?;
    Some(format!("{year:04}-{month:02}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn utc(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text).unwrap().with_timezone(&Utc)
    }

    #[test]
    fn local_date_uses_buenos_aires_not_utc() {
        // 02:00 UTC is still the previous day at UTC-3.
        assert_eq!(
            local_date(utc("2026-07-21T02:00:00Z")),
            NaiveDate::from_ymd_opt(2026, 7, 20).unwrap()
        );
        assert_eq!(
            local_date(utc("2026-07-21T03:00:00Z")),
            NaiveDate::from_ymd_opt(2026, 7, 21).unwrap()
        );
    }

    #[test]
    fn week_start_is_the_monday_of_the_local_week() {
        let monday = NaiveDate::from_ymd_opt(2026, 7, 20).unwrap();
        // Monday through Sunday all collapse to the same Monday.
        for day in 20..=26 {
            let date = NaiveDate::from_ymd_opt(2026, 7, day).unwrap();
            assert_eq!(monday_of(date), monday, "for 2026-07-{day}");
        }
        assert_eq!(
            monday_of(NaiveDate::from_ymd_opt(2026, 7, 27).unwrap()),
            NaiveDate::from_ymd_opt(2026, 7, 27).unwrap()
        );
    }

    #[test]
    fn monday_boundary_is_evaluated_in_buenos_aires() {
        // Sunday 23:00 local (Monday 02:00 UTC) still belongs to the old week.
        assert_eq!(
            week_start(utc("2026-07-20T02:00:00Z")),
            NaiveDate::from_ymd_opt(2026, 7, 13).unwrap()
        );
        // Monday 00:00 local (03:00 UTC) starts the new one.
        assert_eq!(
            week_start(utc("2026-07-20T03:00:00Z")),
            NaiveDate::from_ymd_opt(2026, 7, 20).unwrap()
        );
    }

    #[test]
    fn monday_of_crosses_month_and_year_boundaries() {
        assert_eq!(
            monday_of(NaiveDate::from_ymd_opt(2026, 1, 1).unwrap()),
            NaiveDate::from_ymd_opt(2025, 12, 29).unwrap()
        );
    }

    #[test]
    fn formatting_helpers() {
        let date = Utc.with_ymd_and_hms(2026, 7, 20, 12, 0, 0).unwrap();
        assert_eq!(iso(local_date(date)), "2026-07-20");
        assert_eq!(day_month(local_date(date)), "20/07");
        assert_eq!(month_key(local_date(date)), "2026-07");
    }

    #[test]
    fn month_key_parsing() {
        assert_eq!(parse_month_key("2026-07"), Some("2026-07".to_string()));
        assert_eq!(parse_month_key(" 2026-01 "), Some("2026-01".to_string()));
        assert_eq!(parse_month_key("2026-13"), None);
        assert_eq!(parse_month_key("2026-7"), None);
        assert_eq!(parse_month_key("julio"), None);
        assert_eq!(parse_month_key("2026"), None);
    }
}
