//! Filename-based date extractor.
//!
//! Tries each pattern in `[metadata].filename_date_patterns` against the
//! start of the file's stem, in declared order. The first successful
//! `strptime` parse wins, and any trailing stem text is ignored. Put a
//! longer pattern before a shorter prefix of itself: the first hit wins,
//! so an earlier short pattern drops the rest of the stem, including a
//! time a later pattern would have parsed. Patterns use chrono's strftime
//! vocabulary.

use std::path::Path;

use chrono::{NaiveDate, NaiveDateTime, NaiveTime};

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
        for pat in &self.patterns {
            if let Ok((dt, _)) = NaiveDateTime::parse_and_remainder(stem, pat) {
                return Ok(Some(dt));
            }
            // Date-only patterns won't parse as a full datetime;
            // try as `NaiveDate` and synthesize midnight.
            if let Ok((d, _)) = NaiveDate::parse_and_remainder(stem, pat) {
                return Ok(Some(NaiveDateTime::new(d, NaiveTime::MIN)));
            }
        }
        Ok(None)
    }
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
}
