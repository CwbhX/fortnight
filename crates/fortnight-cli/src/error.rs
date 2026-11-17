//! The CLI's error type: a message for stderr and an exit code.

use fortnight_core::store::StoreError;
use std::fmt;

/// Something went wrong at run time (a failed report, a missing task, I/O).
pub const EXIT_FAILURE: i32 = 1;
/// Bad arguments, or the LLM is not configured.
pub const EXIT_USAGE: i32 = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliError {
    pub code: i32,
    pub message: String,
}

impl CliError {
    pub fn failure(message: impl Into<String>) -> Self {
        CliError {
            code: EXIT_FAILURE,
            message: message.into(),
        }
    }

    pub fn usage(message: impl Into<String>) -> Self {
        CliError {
            code: EXIT_USAGE,
            message: message.into(),
        }
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for CliError {}

impl From<StoreError> for CliError {
    fn from(e: StoreError) -> Self {
        CliError::failure(e.to_string())
    }
}

impl From<std::io::Error> for CliError {
    fn from(e: std::io::Error) -> Self {
        CliError::failure(e.to_string())
    }
}

pub type CliResult<T> = Result<T, CliError>;
