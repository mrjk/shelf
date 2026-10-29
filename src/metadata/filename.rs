//! Filename-based date extractor.
//!
//! Tries each pattern in `[metadata].filename_date_patterns` against the
//! start of the file's stem, in declared order. The first successful
//! `strptime` parse wins, and any trailing stem text is ignored. Put a
//! longer pattern before a shorter prefix of itself: the first hit wins,
//! so an earlier short pattern drops the rest of the stem, including a
//! time a later pattern would have parsed. A pattern with no day uses the
//! 1st, and that guess yields to a later pattern that parsed a day.
//! Patterns use chrono's strftime vocabulary.

use std::path::Path;

use chrono::format::{Parsed, StrftimeItems, parse_and_remainder};
use chrono::{NaiveDateTime, NaiveTime};

use super::Extractor;
use crate::error::Result;

pub struct FilenameExtractor {
    patterns: Vec<String>,
}

impl FilenameExtractor {
    #[must_use]
    pub fn new(patterns: Vec<String>) -> Self {
        Self { patterns }
    }
}

impl Extractor for FilenameExtractor {
    fn id(&self) -> &'static str {
        "filename"
    }

    fn try_date(&self, path: &Path, _kind: &str) -> Result<Option<NaiveDateTime>> {
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            return Ok(None);
        };
        let mut fallback = None;
        for pat in &self.patterns {
            match parse_stem(stem, pat) {
                Some(StemHit::Complete(dt)) => return Ok(Some(dt)),
                Some(StemHit::Defaulted(dt)) if fallback.is_none() => fallback = Some(dt),
                _ => {}
            }
        }
        Ok(fallback)
    }
}

/// A parse that already had a day, or one where the day was filled in.
enum StemHit {
    /// Year, month, and day (or an ordinal or week date) came from the stem.
    Complete(NaiveDateTime),
    /// Year and month came from the stem. The day is the 1st.
    Defaulted(NaiveDateTime),
}

/// Parse `pat` against the start of `stem`.
///
/// Chrono will not build a date from year and month alone. When those two
/// are present and the pattern has no day, ordinal, or week date, the day
/// becomes 1. A missing time becomes midnight. A half-parsed time is a miss,
/// same as a pattern that does not match.
fn parse_stem(stem: &str, pat: &str) -> Option<StemHit> {
    let mut parsed = Parsed::new();
    parse_and_remainder(&mut parsed, stem, StrftimeItems::new(pat)).ok()?;

    let defaulted_day = !has_calendar_day(&parsed);
    if defaulted_day {
        // Year alone is not enough: inventing a month is a different guess.
        parsed.month?;
        parsed.set_day(1).ok()?;
    }

    let dt = if let Ok(dt) = parsed.to_naive_datetime_with_offset(0) {
        dt
    } else if time_absent(&parsed) {
        NaiveDateTime::new(parsed.to_naive_date().ok()?, NaiveTime::MIN)
    } else {
        return None;
    };

    Some(if defaulted_day {
        StemHit::Defaulted(dt)
    } else {
        StemHit::Complete(dt)
    })
}

/// True when the pattern already resolved a calendar day.
fn has_calendar_day(parsed: &Parsed) -> bool {
    parsed.day.is_some()
        || parsed.ordinal.is_some()
        || (parsed.isoweek.is_some() && parsed.weekday.is_some())
        || (parsed.week_from_mon.is_some() && parsed.weekday.is_some())
        || (parsed.week_from_sun.is_some() && parsed.weekday.is_some())
}

/// True when the pattern carried no time fields at all.
fn time_absent(parsed: &Parsed) -> bool {
    parsed.hour_div_12.is_none()
        && parsed.hour_mod_12.is_none()
        && parsed.minute.is_none()
        && parsed.second.is_none()
        && parsed.nanosecond.is_none()
        && parsed.timestamp.is_none()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn matches_known_pattern() {
        let ex = FilenameExtractor::new(vec!["IMG_%Y%m%d_%H%M%S".into()]);
        let p = PathBuf::from("/x/IMG_20240315_142210.jpg");
        let dt = ex.try_date(&p, "photo").unwrap().unwrap();
        assert_eq!(dt.to_string(), "2024-03-15 14:22:10");
    }

    #[test]
    fn falls_through_to_later_pattern() {
        let ex =
            FilenameExtractor::new(vec!["IMG_%Y%m%d_%H%M%S".into(), "VID_%Y%m%d_%H%M%S".into()]);
        let p = PathBuf::from("/x/VID_20240315_142210.mp4");
        let dt = ex.try_date(&p, "video").unwrap().unwrap();
        assert_eq!(dt.to_string(), "2024-03-15 14:22:10");
    }

    #[test]
    fn date_only_pattern_synthesizes_midnight() {
        let ex = FilenameExtractor::new(vec!["%Y-%m-%d".into()]);
        let p = PathBuf::from("/x/2024-03-15.pdf");
        let dt = ex.try_date(&p, "document").unwrap().unwrap();
        assert_eq!(dt.to_string(), "2024-03-15 00:00:00");
    }

    #[test]
    fn unmatched_returns_none() {
        let ex = FilenameExtractor::new(vec!["IMG_%Y%m%d_%H%M%S".into()]);
        let p = PathBuf::from("/x/random_name.jpg");
        assert!(ex.try_date(&p, "photo").unwrap().is_none());
    }

    #[test]
    fn empty_patterns_returns_none() {
        let ex = FilenameExtractor::new(vec![]);
        let p = PathBuf::from("/x/IMG_20240315_142210.jpg");
        assert!(ex.try_date(&p, "photo").unwrap().is_none());
    }

    #[test]
    fn prefix_match_ignores_trailing_hash() {
        let ex = FilenameExtractor::new(vec!["RELCPTE_ACCTSTMT_015910002520_%m%d%y-".into()]);
        let p = PathBuf::from("/x/RELCPTE_ACCTSTMT_015910002520_093022-bf5bcac604.pdf");
        let dt = ex.try_date(&p, "document").unwrap().unwrap();
        assert_eq!(dt.to_string(), "2022-09-30 00:00:00");
    }

    #[test]
    fn prefix_match_iso_date_with_suffix() {
        let ex =
            FilenameExtractor::new(vec!["CARTCRED_CREDCARD_5258818501521549_%Y-%m-%d-".into()]);
        let p = PathBuf::from("/x/CARTCRED_CREDCARD_5258818501521549_2024-03-15-abc.pdf");
        let dt = ex.try_date(&p, "document").unwrap().unwrap();
        assert_eq!(dt.to_string(), "2024-03-15 00:00:00");
    }

    #[test]
    fn datetime_prefix_keeps_time() {
        let ex = FilenameExtractor::new(vec!["IMG_%Y%m%d_%H%M%S".into()]);
        let p = PathBuf::from("/x/IMG_20240315_142210-extra.jpg");
        let dt = ex.try_date(&p, "photo").unwrap().unwrap();
        assert_eq!(dt.to_string(), "2024-03-15 14:22:10");
    }

    #[test]
    fn does_not_match_date_in_the_middle() {
        let ex = FilenameExtractor::new(vec!["%Y-%m-%d".into()]);
        let p = PathBuf::from("/x/notes_2024-03-15.pdf");
        assert!(ex.try_date(&p, "document").unwrap().is_none());
    }

    #[test]
    fn shorter_prefix_wins_over_later_pattern() {
        let ex = FilenameExtractor::new(vec!["IMG_%Y%m%d".into(), "IMG_%Y%m%d_%H%M%S".into()]);
        let p = PathBuf::from("/x/IMG_20240315_142210.jpg");
        let dt = ex.try_date(&p, "photo").unwrap().unwrap();
        assert_eq!(dt.to_string(), "2024-03-15 00:00:00");
    }

    #[test]
    fn month_only_pattern_uses_first_of_month() {
        let ex = FilenameExtractor::new(vec!["%Y-%m".into()]);
        let p = PathBuf::from("/x/2022-06.pdf");
        let dt = ex.try_date(&p, "document").unwrap().unwrap();
        assert_eq!(dt.to_string(), "2022-06-01 00:00:00");
    }

    #[test]
    fn month_only_before_full_date_keeps_parsed_day() {
        let ex = FilenameExtractor::new(vec!["%Y-%m".into(), "%Y-%m-%d".into()]);
        let p = PathBuf::from("/x/2022-04-14.pdf");
        let dt = ex.try_date(&p, "document").unwrap().unwrap();
        assert_eq!(dt.to_string(), "2022-04-14 00:00:00");
    }

    #[test]
    fn full_date_before_month_only_still_defaults_day() {
        let ex = FilenameExtractor::new(vec!["%Y-%m-%d".into(), "%Y-%m".into()]);
        let p = PathBuf::from("/x/2022-06.pdf");
        let dt = ex.try_date(&p, "document").unwrap().unwrap();
        assert_eq!(dt.to_string(), "2022-06-01 00:00:00");
    }

    #[test]
    fn invalid_month_returns_none() {
        let ex = FilenameExtractor::new(vec!["%Y-%m".into(), "%Y-%m-%d".into()]);
        let p = PathBuf::from("/x/2022-13.pdf");
        assert!(ex.try_date(&p, "document").unwrap().is_none());
    }

    #[test]
    fn year_only_pattern_returns_none() {
        let ex = FilenameExtractor::new(vec!["%Y".into()]);
        let p = PathBuf::from("/x/2022.pdf");
        assert!(ex.try_date(&p, "document").unwrap().is_none());
    }
}
