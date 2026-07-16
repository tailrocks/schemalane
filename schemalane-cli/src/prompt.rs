use std::io::{BufRead, Write};

use owo_colors::{OwoColorize, Stream};
use schemalane_core::SchemalaneError;

pub(crate) fn prompt_yes_no(prompt: &str) -> Result<bool, SchemalaneError> {
    let stdin = std::io::stdin();
    let mut reader = stdin.lock();
    loop {
        eprint!("{prompt}");
        std::io::stderr().flush().map_err(SchemalaneError::Io)?;
        let mut answer = String::new();
        let read = reader.read_line(&mut answer).map_err(SchemalaneError::Io)?;
        if read == 0 {
            eprintln!();
            return Ok(false);
        }
        let trimmed = answer.trim();
        if trimmed.eq_ignore_ascii_case("yes") {
            return Ok(true);
        }
        if trimmed.eq_ignore_ascii_case("no") {
            return Ok(false);
        }
        eprintln!(
            "{}",
            "Please answer 'yes' or 'no'."
                .if_supports_color(Stream::Stderr, |text| text.bright_yellow())
        );
    }
}
