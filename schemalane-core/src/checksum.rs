use crate::SchemalaneError;
use crc32fast::Hasher;

/// Computes Flyway's CRC-32 over UTF-8 lines without line terminators.
pub(crate) fn calculate_checksum(script: &str, bytes: &[u8]) -> Result<i32, SchemalaneError> {
    let text = std::str::from_utf8(bytes).map_err(|error| {
        SchemalaneError::Validation(format!(
            "migration {script}: content is not valid UTF-8 (invalid byte at offset {}): {error}",
            error.valid_up_to()
        ))
    })?;
    let mut hasher = Hasher::new();
    for line in text.lines() {
        hasher.update(line.as_bytes());
    }
    Ok(i32::from_be_bytes(hasher.finalize().to_be_bytes()))
}
