//! Kind classification.
//!
//! A `kind` is a profile-defined bucket (`photo`, `raw`, `video`, ...).
//! [`Profile::kind_rules`] are tried first, most specific match wins.
//! Anything left falls through to the canonical-extension map in
//! [`Profile::kinds`], then `"other"`. Validation guarantees each
//! canonical extension appears in at most one kind, so that fallback
//! does not depend on map order.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};

use crate::config::{KindRule, Profile};
use crate::error::{Error, Result};
use crate::extension::canonical_extension;
use crate::scan::ScannedFile;

pub const OTHER: &str = "other";

/// Kind rules compiled once per run, ordered most-specific first.
#[derive(Debug)]
pub struct CompiledKindRules {
    rules: Vec<CompiledRule>,
}

#[derive(Debug)]
struct CompiledRule {
    kind: String,
    extensions: Option<BTreeSet<String>>,
    filename: Option<GlobSet>,
    parent: Option<GlobSet>,
}

/// Compile `profile.kind_rules` for repeated matching.
///
/// Rules with neither `filename` nor `parent` are dropped here. Validation
/// already rejects them; dropping them keeps an unvalidated profile from
/// letting an extensions-only rule override [`Profile::kinds`].
pub fn compile_kind_rules(profile: &Profile) -> Result<CompiledKindRules> {
    let mut ranked: Vec<(u8, usize, CompiledRule)> = Vec::new();
    for (order, rule) in profile.kind_rules.iter().enumerate() {
        let filename = compile_globs(&rule.filename)?;
        let parent = compile_globs(&rule.parent)?;
        if filename.is_none() && parent.is_none() {
            continue;
        }
        let specificity = specificity(rule);
        ranked.push((
            specificity,
            order,
            CompiledRule {
                kind: rule.kind.clone(),
                extensions: normalize_extensions(&rule.extensions, &profile.extensions.canonical),
                filename,
                parent,
            },
        ));
    }
    ranked.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    Ok(CompiledKindRules {
        rules: ranked.into_iter().map(|(_, _, rule)| rule).collect(),
    })
}

fn specificity(rule: &KindRule) -> u8 {
    let mut n = 0;
    if !rule.filename.is_empty() {
        n += 1;
    }
    if !rule.parent.is_empty() {
        n += 1;
    }
    if !rule.extensions.is_empty() {
        n += 1;
    }
    n
}

fn normalize_extensions(
    extensions: &[String],
    canonical_map: &BTreeMap<String, String>,
) -> Option<BTreeSet<String>> {
    if extensions.is_empty() {
        return None;
    }
    Some(
        extensions
            .iter()
            .map(|ext| {
                let lower = ext.to_ascii_lowercase();
                canonical_map.get(&lower).cloned().unwrap_or(lower)
            })
            .collect(),
    )
}

fn compile_globs(patterns: &[String]) -> Result<Option<GlobSet>> {
    if patterns.is_empty() {
        return Ok(None);
    }
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        let glob = GlobBuilder::new(pattern)
            .case_insensitive(true)
            .build()
            .map_err(|source| Error::BadGlob {
                pattern: pattern.clone(),
                source,
            })?;
        builder.add(glob);
    }
    let set = builder.build().map_err(|source| Error::BadGlob {
        pattern: patterns.join(", "),
        source,
    })?;
    Ok(Some(set))
}

/// Resolve a canonical extension to its kind, or `"other"` if no kind claims
/// it (or the file had no extension).
#[must_use]
pub fn classify(canonical_ext: Option<&str>, kinds: &BTreeMap<String, Vec<String>>) -> String {
    let Some(ext) = canonical_ext else {
        return OTHER.to_string();
    };
    for (name, exts) in kinds {
        if exts.iter().any(|e| e == ext) {
            return name.clone();
        }
    }
    OTHER.to_string()
}

/// Canonical extension + resolved kind for a scanned file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Classified {
    pub canonical_ext: Option<String>,
    pub kind: String,
}

/// Run extension normalization and kind classification for one scanned file.
///
/// `rules` comes from [`compile_kind_rules`]. The first matching rule wins
/// (already ordered by specificity, then declaration order). No match uses
/// [`classify`].
#[must_use]
pub fn classify_scanned(
    profile: &Profile,
    rules: &CompiledKindRules,
    file: &ScannedFile,
) -> Classified {
    let canonical_ext = canonical_extension(&file.absolute_path, &profile.extensions.canonical);
    let kind = match matching_rule(rules, file, canonical_ext.as_deref()) {
        Some(rule) => rule.kind.clone(),
        None => classify(canonical_ext.as_deref(), &profile.kinds),
    };
    Classified {
        canonical_ext,
        kind,
    }
}

fn matching_rule<'a>(
    rules: &'a CompiledKindRules,
    file: &ScannedFile,
    canonical_ext: Option<&str>,
) -> Option<&'a CompiledRule> {
    let name = file_name(&file.relative_path);
    let parent = parent_rel(&file.relative_path);
    rules.rules.iter().find(|rule| {
        if let Some(exts) = &rule.extensions {
            let Some(ext) = canonical_ext else {
                return false;
            };
            if !exts.contains(ext) {
                return false;
            }
        }
        if let Some(set) = &rule.filename
            && !set.is_match(&name)
        {
            return false;
        }
        if let Some(set) = &rule.parent
            && !set.is_match(&parent)
        {
            return false;
        }
        true
    })
}

fn file_name(relative: &Path) -> String {
    relative
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Parent of `relative`, with `/` separators. A file in the input root has
/// parent `.`.
fn parent_rel(relative: &Path) -> String {
    let parent = relative.parent().unwrap_or(Path::new(""));
    if parent.as_os_str().is_empty() {
        return ".".to_string();
    }
    let raw = parent.to_string_lossy();
    if std::path::MAIN_SEPARATOR == '/' {
        raw.into_owned()
    } else {
        raw.replace(std::path::MAIN_SEPARATOR, "/")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn photo_kinds() -> BTreeMap<String, Vec<String>> {
        let mut m = BTreeMap::new();
        m.insert(
            "photo".to_string(),
            vec!["jpg", "png", "heic", "webp", "tiff", "bmp"]
                .into_iter()
                .map(String::from)
                .collect(),
        );
        m.insert(
            "raw".to_string(),
            vec![
                "cr2", "cr3", "nef", "arw", "dng", "raf", "orf", "rw2", "srw",
            ]
            .into_iter()
            .map(String::from)
            .collect(),
        );
        m.insert(
            "video".to_string(),
            vec!["mp4", "mov", "mkv", "avi", "m4v", "mts", "m2ts"]
                .into_iter()
                .map(String::from)
                .collect(),
        );
        m
    }

    #[test]
    fn jpg_resolves_to_photo() {
        assert_eq!(classify(Some("jpg"), &photo_kinds()), "photo");
    }

    #[test]
    fn cr3_resolves_to_raw() {
        assert_eq!(classify(Some("cr3"), &photo_kinds()), "raw");
    }

    #[test]
    fn mp4_resolves_to_video() {
        assert_eq!(classify(Some("mp4"), &photo_kinds()), "video");
    }

    #[test]
    fn unknown_ext_is_other() {
        assert_eq!(classify(Some("xyz"), &photo_kinds()), "other");
    }

    #[test]
    fn none_is_other() {
        assert_eq!(classify(None, &photo_kinds()), "other");
    }

    #[test]
    fn empty_kinds_is_other() {
        assert_eq!(classify(Some("jpg"), &BTreeMap::new()), "other");
    }

    #[test]
    fn classify_scanned_uses_canonical_map_and_kinds() {
        let mut profile = Profile {
            inputs: vec![PathBuf::from("/in")],
            filters: Default::default(),
            extensions: Default::default(),
            kinds: photo_kinds(),
            kind_rules: Vec::new(),
            metadata: Default::default(),
            sequence: Default::default(),
            dedupe: Default::default(),
            health: Default::default(),
            templates: Default::default(),
            state: Default::default(),
            outputs: Vec::new(),
        };
        profile
            .extensions
            .canonical
            .insert("jpeg".to_string(), "jpg".to_string());

        let file = ScannedFile {
            source_root: PathBuf::from("/in"),
            absolute_path: PathBuf::from("/in/holiday.JPEG"),
            relative_path: PathBuf::from("holiday.JPEG"),
        };

        let rules = compile_kind_rules(&profile).unwrap();
        let c = classify_scanned(&profile, &rules, &file);
        assert_eq!(c.canonical_ext.as_deref(), Some("jpg"));
        assert_eq!(c.kind, "photo");

        let oddball = ScannedFile {
            source_root: PathBuf::from("/in"),
            absolute_path: PathBuf::from("/in/notes.xyz"),
            relative_path: PathBuf::from("notes.xyz"),
        };
        let c2 = classify_scanned(&profile, &rules, &oddball);
        assert_eq!(c2.canonical_ext.as_deref(), Some("xyz"));
        assert_eq!(c2.kind, "other");

        let no_ext = ScannedFile {
            source_root: PathBuf::from("/in"),
            absolute_path: PathBuf::from("/in/README"),
            relative_path: PathBuf::from("README"),
        };
        let c3 = classify_scanned(&profile, &rules, &no_ext);
        assert_eq!(c3.canonical_ext, None);
        assert_eq!(c3.kind, "other");
    }

    fn rule_profile(kinds_toml_extra: &str) -> Profile {
        let toml_str = format!(
            r#"
inputs = ["/in"]

[extensions.canonical]
jpeg = "jpg"

[kinds]
photo = ["jpg"]
document = ["pdf"]
{kinds_toml_extra}
[[output]]
name = "lib"
path = "/out"
directory = "{{yyyy}}"
filename = "{{yyyy}}"
"#
        );
        toml::from_str(&toml_str).unwrap()
    }

    fn scanned(relative: &str) -> ScannedFile {
        ScannedFile {
            source_root: PathBuf::from("/in"),
            absolute_path: PathBuf::from("/in").join(relative),
            relative_path: PathBuf::from(relative),
        }
    }

    fn kind_of(profile: &Profile, relative: &str) -> String {
        let rules = compile_kind_rules(profile).unwrap();
        classify_scanned(profile, &rules, &scanned(relative)).kind
    }

    #[test]
    fn parent_rule_overrides_extension_and_falls_back() {
        let profile = rule_profile(
            r#"
[[kind_rule]]
kind = "invoice"
parent = ["**/invoices"]
"#,
        );
        assert_eq!(kind_of(&profile, "invoices/notes.pdf"), "invoice");
        assert_eq!(kind_of(&profile, "2024/invoices/notes.pdf"), "invoice");
        assert_eq!(kind_of(&profile, "scan.pdf"), "document");
        assert_eq!(kind_of(&profile, "holiday.jpg"), "photo");
        assert_eq!(kind_of(&profile, "invoices/2024/notes.pdf"), "document");
    }

    #[test]
    fn more_specific_rule_wins_over_an_earlier_broader_rule() {
        let profile = rule_profile(
            r#"
[[kind_rule]]
kind = "invoice"
parent = ["**/invoices"]

[[kind_rule]]
kind = "receipt"
extensions = ["pdf"]
filename = ["INV_*", "FACTURE_*"]
parent = ["**/invoices"]
"#,
        );
        assert_eq!(kind_of(&profile, "invoices/INV_1.pdf"), "receipt");
        assert_eq!(kind_of(&profile, "invoices/notes.pdf"), "invoice");
        assert_eq!(kind_of(&profile, "scan.pdf"), "document");
        assert_eq!(kind_of(&profile, "photo.jpg"), "photo");
    }

    #[test]
    fn equal_specificity_keeps_declaration_order() {
        let profile = rule_profile(
            r#"
[[kind_rule]]
kind = "invoice"
parent = ["**/invoices"]

[[kind_rule]]
kind = "statement"
parent = ["**/invoices"]
"#,
        );
        assert_eq!(kind_of(&profile, "invoices/notes.pdf"), "invoice");
    }

    #[test]
    fn filename_glob_is_case_insensitive() {
        let profile = rule_profile(
            r#"
[[kind_rule]]
kind = "invoice"
filename = ["INV_*"]
"#,
        );
        assert_eq!(kind_of(&profile, "inv_march.pdf"), "invoice");
        assert_eq!(kind_of(&profile, "notes.pdf"), "document");
    }

    #[test]
    fn rule_extension_follows_canonical_map() {
        let profile = rule_profile(
            r#"
[[kind_rule]]
kind = "scan"
extensions = ["jpeg"]
filename = ["SCAN_*"]
"#,
        );
        assert_eq!(kind_of(&profile, "SCAN_01.JPEG"), "scan");
        assert_eq!(kind_of(&profile, "SCAN_01.jpg"), "scan");
        assert_eq!(kind_of(&profile, "holiday.jpg"), "photo");
    }

    #[test]
    fn root_parent_is_dot() {
        let profile = rule_profile(
            r#"
[[kind_rule]]
kind = "loose"
parent = ["."]
"#,
        );
        assert_eq!(kind_of(&profile, "notes.pdf"), "loose");
        assert_eq!(kind_of(&profile, "invoices/notes.pdf"), "document");
    }

    #[test]
    fn extensions_only_rule_does_not_override_kinds() {
        let mut profile = rule_profile("");
        profile.kind_rules.push(crate::config::KindRule {
            kind: "invoice".to_string(),
            extensions: vec!["pdf".to_string()],
            filename: Vec::new(),
            parent: Vec::new(),
        });
        assert_eq!(kind_of(&profile, "scan.pdf"), "document");
    }
}
