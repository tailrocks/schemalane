#![allow(clippy::print_stdout, clippy::print_stderr, clippy::future_not_send)]

mod connect;
mod prompt;
mod runner;

pub use runner::{EmbeddedRunner, Verbosity, run_cli, run_cli_with};
