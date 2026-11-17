//! Resolving the data folder. Owner: agent F.
//!
//! Order: `--data-dir` flag → `FORTNIGHT_DATA_DIR` → `fortnight-data/`
//! beside the executable. On macOS, if the executable is inside an `.app`
//! bundle (`Foo.app/Contents/MacOS/foo`), the folder goes beside the `.app`,
//! never inside it. Never falls back to a per-user directory.
//! See PLAN.md section 3.2.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DataDirError {
    #[error("cannot find the executable's location: {0}")]
    NoExecutable(String),
    #[error("the data folder {path} is not writable: {reason}")]
    NotWritable { path: PathBuf, reason: String },
}

/// The folder name used beside the executable.
pub const DIR_NAME: &str = "fortnight-data";

/// Picks the data folder (does not create it). `flag` is the value of
/// `--data-dir`, if given.
pub fn resolve(_flag: Option<PathBuf>) -> Result<PathBuf, DataDirError> {
    todo!("agent F")
}

/// Creates the folder if missing and checks it is writable (by creating and
/// removing a probe file).
pub fn ensure(_dir: &Path) -> Result<(), DataDirError> {
    todo!("agent F")
}

/// `<dir>/fortnight.db`.
pub fn db_path(dir: &Path) -> PathBuf {
    dir.join("fortnight.db")
}
