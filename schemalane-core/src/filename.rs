pub(crate) use schemalane_version::ParsedVersion;

use crate::SchemalaneError;

pub(crate) fn parse_sql_filename(
    file_name: &str,
) -> Result<(String, ParsedVersion, String), SchemalaneError> {
    schemalane_version::parse_sql_filename(file_name)
        .map_err(|error| SchemalaneError::Validation(error.to_string()))
}

pub(crate) fn parse_rust_filename(
    file_name: &str,
) -> Result<(String, ParsedVersion, String), SchemalaneError> {
    schemalane_version::parse_rust_filename(file_name)
        .map_err(|error| SchemalaneError::Validation(error.to_string()))
}
