//! Flyway-compatible migration version and filename parsing.

use std::cmp::Ordering;
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionError(pub String);

impl fmt::Display for VersionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for VersionError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedVersion(Vec<String>);

impl ParsedVersion {
    pub fn parse(value: &str) -> Result<Self, VersionError> {
        let normalized = normalize_version(value);
        let mut parts = Vec::new();
        for part in normalized.split('.') {
            if part.is_empty() || !part.chars().all(|ch| ch.is_ascii_digit()) {
                return Err(VersionError(format!(
                    "invalid version '{value}': expected Flyway numeric dotted notation"
                )));
            }
            parts.push(normalize_version_part(part));
        }
        while parts.len() > 1 && parts.last().is_some_and(|part| part == "0") {
            parts.pop();
        }
        Ok(Self(parts))
    }
}

impl PartialOrd for ParsedVersion {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ParsedVersion {
    fn cmp(&self, other: &Self) -> Ordering {
        let max_len = self.0.len().max(other.0.len());
        for index in 0..max_len {
            let left = self.0.get(index).map_or("0", String::as_str);
            let right = other.0.get(index).map_or("0", String::as_str);
            match compare_normalized_number(left, right) {
                Ordering::Equal => {}
                ordering => return ordering,
            }
        }
        Ordering::Equal
    }
}

pub fn parse_sql_filename(
    file_name: &str,
) -> Result<(String, ParsedVersion, String), VersionError> {
    parse_versioned_filename(file_name, "SQL", ".sql")
}

pub fn parse_rust_filename(
    file_name: &str,
) -> Result<(String, ParsedVersion, String), VersionError> {
    parse_versioned_filename(file_name, "Rust", ".rs")
}

pub fn parse_versioned_filename(
    file_name: &str,
    kind: &str,
    suffix: &str,
) -> Result<(String, ParsedVersion, String), VersionError> {
    let Some(stem) = strip_suffix_ignore_ascii_case(file_name, suffix) else {
        return Err(VersionError(format!(
            "invalid {kind} migration filename '{file_name}': expected {suffix} extension"
        )));
    };
    let Some(rest) = stem.strip_prefix('V') else {
        return Err(VersionError(format!(
            "invalid {kind} migration filename '{file_name}': expected V<version>__<description>{suffix}"
        )));
    };
    let (version_raw, description) = rest.split_once("__").unwrap_or((rest, ""));
    if version_raw.is_empty() {
        return Err(VersionError(format!(
            "invalid {kind} migration filename '{file_name}': missing version"
        )));
    }
    let parsed = ParsedVersion::parse(version_raw)?;
    Ok((
        normalize_version(version_raw),
        parsed,
        description.to_owned(),
    ))
}

fn strip_suffix_ignore_ascii_case<'a>(value: &'a str, suffix: &str) -> Option<&'a str> {
    value
        .get(value.len().checked_sub(suffix.len())?..)
        .is_some_and(|tail| tail.eq_ignore_ascii_case(suffix))
        .then(|| &value[..value.len() - suffix.len()])
}

fn normalize_version(version: &str) -> String {
    version.replace('_', ".")
}
fn normalize_version_part(part: &str) -> String {
    let trimmed = part.trim_start_matches('0');
    if trimmed.is_empty() {
        "0".to_owned()
    } else {
        trimmed.to_owned()
    }
}
fn compare_normalized_number(left: &str, right: &str) -> Ordering {
    left.len().cmp(&right.len()).then_with(|| left.cmp(right))
}

#[cfg(test)]
mod tests {
    use super::{ParsedVersion, parse_rust_filename, parse_sql_filename};

    #[test]
    fn parses_flyway_sql_names() {
        let (text, version, description) =
            parse_sql_filename("V001_002__My-description.data load.SQL").expect("parse");
        assert_eq!(text, "001.002");
        assert_eq!(description, "My-description.data load");
        assert_eq!(version, ParsedVersion::parse("1.2").expect("version"));
    }
    #[test]
    fn parses_description_after_first_separator() {
        assert_eq!(
            parse_sql_filename("V10__table__asset.sql")
                .expect("parse")
                .2,
            "table__asset"
        );
    }
    #[test]
    fn parses_dotted_sql_description() {
        assert_eq!(
            parse_sql_filename("V10__bitcoin_transaction.import_status.default.sql")
                .expect("parse")
                .2,
            "bitcoin_transaction.import_status.default"
        );
    }
    #[test]
    fn parses_descriptionless_filename() {
        assert_eq!(parse_sql_filename("V1.sql").expect("parse").2, "");
    }
    #[test]
    fn parses_uppercase_rust_extension() {
        assert!(parse_rust_filename("V3__seed.RS").is_ok());
    }
    #[test]
    fn parses_rust_filename_and_description() {
        let (text, version, description) =
            parse_rust_filename("V2026_02_24_2__seed_price_histories.rs").expect("parse");
        assert_eq!(text, "2026.02.24.2");
        assert_eq!(description, "seed_price_histories");
        assert_eq!(
            version,
            ParsedVersion::parse("2026.2.24.2").expect("version")
        );
    }
    #[test]
    fn parses_dotted_rust_description() {
        assert_eq!(
            parse_rust_filename("V10__seed.reference_data.rs")
                .expect("parse")
                .2,
            "seed.reference_data"
        );
    }
    #[test]
    fn parses_flyway_rust_name() {
        let (_, version, description) =
            parse_rust_filename("V001_002__My-description.data load.RS").expect("parse");
        assert_eq!(version, ParsedVersion::parse("1.2").expect("version"));
        assert_eq!(description, "My-description.data load");
    }
    #[test]
    fn rejects_invalid_prefix_and_extension() {
        assert!(parse_sql_filename("1__x.sql").is_err());
        assert!(parse_rust_filename("V1__x.txt").is_err());
    }
    #[test]
    fn rejects_missing_version() {
        assert!(
            parse_sql_filename("V__x.sql")
                .expect_err("error")
                .to_string()
                .contains("missing version")
        );
    }
    #[test]
    fn rejects_invalid_version_parts() {
        for name in ["Vabc__x.sql", "V1..2__x.sql"] {
            assert!(parse_sql_filename(name).is_err());
        }
    }
    #[test]
    fn compares_versions_like_flyway() {
        assert!(ParsedVersion::parse("2").expect("2") < ParsedVersion::parse("10").expect("10"));
        assert_eq!(
            ParsedVersion::parse("1.2.3.0").expect("left"),
            ParsedVersion::parse("1.2.3").expect("right")
        );
        assert_eq!(
            ParsedVersion::parse("01.002").expect("left"),
            ParsedVersion::parse("1.2").expect("right")
        );
    }
    #[test]
    fn compares_each_numeric_segment() {
        assert!(
            ParsedVersion::parse("1.10").expect("left")
                > ParsedVersion::parse("1.2").expect("right")
        );
        assert!(
            ParsedVersion::parse("1.0.1").expect("left")
                > ParsedVersion::parse("1").expect("right")
        );
    }
    #[test]
    fn supports_arbitrarily_large_parts() {
        assert!(ParsedVersion::parse("99999999999999999999999999999999999999").is_ok());
    }
}
