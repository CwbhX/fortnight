//! The CLI's error type: a message for stderr and an exit code.

use fortnight_core::store::StoreError;
use std::fmt;

/// Something went wrong at run time (a failed report, a missing task, I/O).
pub const EXIT_FAILURE: i32 = 1;
/// Bad arguments, or the LLM is not configured.
pub const EXIT_USAGE: i32 = 2;

/// What kind of error, where a caller treats one specially.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ErrorKind {
    #[default]
    Other,
    /// No LLM endpoint is configured (exit 2). `run-due` treats this one
    /// as "nothing to do", not as a failure.
    NotConfigured,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliError {
    pub code: i32,
    pub message: String,
    pub kind: ErrorKind,
}

impl CliError {
    pub fn failure(message: impl Into<String>) -> Self {
        CliError {
            code: EXIT_FAILURE,
            message: message.into(),
            kind: ErrorKind::Other,
        }
    }

    pub fn usage(message: impl Into<String>) -> Self {
        CliError {
            code: EXIT_USAGE,
            message: message.into(),
            kind: ErrorKind::Other,
        }
    }

    /// No LLM endpoint is configured: a usage error (exit 2).
    pub fn not_configured(message: impl Into<String>) -> Self {
        CliError {
            kind: ErrorKind::NotConfigured,
            ..CliError::usage(message)
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
