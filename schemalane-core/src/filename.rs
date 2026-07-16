use std::cmp::Ordering;

use crate::SchemalaneError;

pub(crate) fn parse_sql_filename(
    file_name: &str,
) -> Result<(String, ParsedVersion, String), SchemalaneError> {
    parse_versioned_filename(file_name, "SQL", ".sql")
}

pub(crate) fn parse_rust_filename(
    file_name: &str,
) -> Result<(String, ParsedVersion, String), SchemalaneError> {
    parse_versioned_filename(file_name, "Rust", ".rs")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedVersion(Vec<String>);

impl ParsedVersion {
    pub(crate) fn parse(value: &str) -> Result<Self, SchemalaneError> {
        let normalized = normalize_version(value);
        let mut parts = Vec::new();
        for part in normalized.split('.') {
            if part.is_empty() || !part.chars().all(|ch| ch.is_ascii_digit()) {
                return Err(SchemalaneError::Validation(format!(
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
        for idx in 0..max_len {
            let left = self.0.get(idx).map_or("0", String::as_str);
            let right = other.0.get(idx).map_or("0", String::as_str);
            match compare_normalized_number(left, right) {
                Ordering::Equal => {}
                ordering => return ordering,
            }
        }
        Ordering::Equal
    }
}

fn parse_versioned_filename(
    file_name: &str,
    kind: &str,
    suffix: &str,
) -> Result<(String, ParsedVersion, String), SchemalaneError> {
    let Some(stem) = strip_suffix_ignore_ascii_case(file_name, suffix) else {
        return Err(SchemalaneError::Validation(format!(
            "invalid {kind} migration filename '{file_name}': expected {suffix} extension"
        )));
    };

    let Some(rest) = stem.strip_prefix('V') else {
        return Err(SchemalaneError::Validation(format!(
            "invalid {kind} migration filename '{file_name}': expected V<version>__<description>{suffix}"
        )));
    };

    let (version_raw, description) = rest.split_once("__").unwrap_or((rest, ""));
    if version_raw.is_empty() {
        return Err(SchemalaneError::Validation(format!(
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

    fn parsed_version(parts: &[&str]) -> ParsedVersion {
        ParsedVersion(parts.iter().map(|part| (*part).to_owned()).collect())
    }

    #[test]
    fn parses_sql_filename() {
        let (version, parsed, description) =
            parse_sql_filename("V2026.02.24.1__price_histories.sql").expect("valid filename");
        assert_eq!(version, "2026.02.24.1");
        assert_eq!(description, "price_histories");
        assert_eq!(
            parsed,
            parsed_version(&["2026", "2", "24", "1"]),
            "version segments should parse numerically"
        );
    }

    #[test]
    fn parses_sql_filename_with_dotted_description() {
        let (version, parsed, description) =
            parse_sql_filename("V10__bitcoin_transaction.import_status.default.sql")
                .expect("valid Flyway filename");
        assert_eq!(version, "10");
        assert_eq!(description, "bitcoin_transaction.import_status.default");
        assert_eq!(parsed, parsed_version(&["10"]));
    }

    #[test]
    fn parses_sql_filename_like_flyway() {
        let (version, parsed, description) =
            parse_sql_filename("V001_002__My-description.data load.SQL")
                .expect("valid Flyway filename");
        assert_eq!(version, "001.002");
        assert_eq!(description, "My-description.data load");
        assert_eq!(parsed, parsed_version(&["1", "2"]));
    }

    #[test]
    fn parses_sql_filename_description_after_first_separator() {
        let (version, parsed, description) =
            parse_sql_filename("V10__table__eth_asset.sql").expect("valid Flyway filename");
        assert_eq!(version, "10");
        assert_eq!(description, "table__eth_asset");
        assert_eq!(parsed, parsed_version(&["10"]));
    }

    #[test]
    fn parses_sql_filename_without_description_like_flyway() {
        let (version, parsed, description) = parse_sql_filename("V1.sql").expect("valid filename");
        assert_eq!(version, "1");
        assert_eq!(description, "");
        assert_eq!(parsed, parsed_version(&["1"]));
    }

    #[test]
    fn rejects_sql_filename_missing_version() {
        let err = parse_sql_filename("V__x.sql").expect_err("missing version");
        assert!(err.to_string().contains("missing version"));
    }

    #[test]
    fn rejects_sql_filename_with_non_numeric_version() {
        let err = parse_sql_filename("Vabc__x.sql").expect_err("invalid version");
        assert!(err.to_string().contains("invalid version"));
    }

    #[test]
    fn rejects_sql_filename_with_empty_version_part() {
        let err = parse_sql_filename("V1..2__x.sql").expect_err("invalid version");
        assert!(err.to_string().contains("invalid version"));
    }

    #[test]
    fn trailing_zero_version_parts_compare_equal() {
        let (_, left, _) = parse_sql_filename("V1.2.3.0__x.sql").expect("left");
        let (_, right, _) = parse_sql_filename("V1.2.3__y.sql").expect("right");
        assert_eq!(left, right);
    }

    #[test]
    fn rejects_invalid_sql_filename() {
        let err = parse_sql_filename("2026_02_24_price_histories.sql")
            .expect_err("invalid filename should fail");
        assert!(
            err.to_string().contains("invalid SQL migration filename"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn parses_rust_filename() {
        let (version, parsed, description) =
            parse_rust_filename("V2026.02.24.2__seed_reference_data.rs").expect("valid filename");
        assert_eq!(version, "2026.02.24.2");
        assert_eq!(description, "seed_reference_data");
        assert_eq!(
            parsed,
            parsed_version(&["2026", "2", "24", "2"]),
            "version segments should parse numerically"
        );
    }

    #[test]
    fn parses_rust_filename_with_dotted_description() {
        let (version, parsed, description) =
            parse_rust_filename("V10__seed.reference_data.rs").expect("valid filename");
        assert_eq!(version, "10");
        assert_eq!(description, "seed.reference_data");
        assert_eq!(parsed, parsed_version(&["10"]));
    }

    #[test]
    fn parses_rust_filename_like_flyway() {
        let (version, parsed, description) =
            parse_rust_filename("V001_002__My-description.data_load.RS").expect("valid filename");
        assert_eq!(version, "001.002");
        assert_eq!(description, "My-description.data_load");
        assert_eq!(parsed, parsed_version(&["1", "2"]));
    }

    #[test]
    fn rejects_invalid_rust_filename() {
        let err = parse_rust_filename("seed_reference_data.rs")
            .expect_err("invalid filename should fail");
        assert!(
            err.to_string().contains("invalid Rust migration filename"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn compares_versions_numerically() {
        let v1 = ParsedVersion::parse("2.10").expect("parse");
        let v2 = ParsedVersion::parse("2.2").expect("parse");
        assert!(v1 > v2);
    }

    #[test]
    fn compares_versions_like_flyway() {
        assert_eq!(
            ParsedVersion::parse("1.0").expect("parse"),
            ParsedVersion::parse("1").expect("parse")
        );
        assert_eq!(
            ParsedVersion::parse("001_002").expect("parse"),
            ParsedVersion::parse("1.2").expect("parse")
        );
        assert!(
            ParsedVersion::parse("99999999999999999999999999999999999999").expect("parse")
                > ParsedVersion::parse("2").expect("parse")
        );
    }
}
