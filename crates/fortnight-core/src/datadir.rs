//! Resolving the data folder. Owner: agent F.
//!
//! Order: `--data-dir` flag → `FORTNIGHT_DATA_DIR` → `fortnight-data/`
//! beside the executable. On macOS, if the executable is inside an `.app`
//! bundle (`Foo.app/Contents/MacOS/foo`), the folder goes beside the `.app`,
//! never inside it. Never falls back to a per-user directory.
//! See PLAN.md section 3.2.

use std::ffi::OsString;
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
pub fn resolve(flag: Option<PathBuf>) -> Result<PathBuf, DataDirError> {
    let env = std::env::var_os("FORTNIGHT_DATA_DIR");
    // The executable is only needed when neither override is given.
    if flag.is_some() || env.as_ref().is_some_and(|v| !v.is_empty()) {
        return Ok(resolve_from(flag, env, Path::new("")));
    }
    let exe = std::env::current_exe().map_err(|e| DataDirError::NoExecutable(e.to_string()))?;
    let exe = exe.canonicalize().unwrap_or(exe);
    if exe.parent().is_none() {
        return Err(DataDirError::NoExecutable(format!(
            "{} has no parent folder",
            exe.display()
        )));
    }
    Ok(resolve_from(None, None, &exe))
}

/// Pure part of [`resolve`]: flag, then non-empty env, then beside `exe`.
pub(crate) fn resolve_from(flag: Option<PathBuf>, env: Option<OsString>, exe: &Path) -> PathBuf {
    if let Some(flag) = flag {
        return flag;
    }
    if let Some(env) = env.filter(|v| !v.is_empty()) {
        return PathBuf::from(env);
    }
    exe_folder(exe).join(DIR_NAME)
}

/// The folder the executable "lives in": its parent, or for a macOS bundle
/// (`X.app/Contents/MacOS/<bin>`) the folder holding `X.app`.
fn exe_folder(exe: &Path) -> PathBuf {
    let dir = exe.parent().unwrap_or_else(|| Path::new("."));
    let bundle = dir
        .file_name()
        .filter(|n| *n == "MacOS")
        .and_then(|_| dir.parent())
        .filter(|p| p.file_name().is_some_and(|n| n == "Contents"))
        .and_then(|p| p.parent())
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("app")));
    match bundle {
        Some(app) => app.parent().unwrap_or_else(|| Path::new(".")).to_path_buf(),
        None => dir.to_path_buf(),
    }
}

/// Creates the folder if missing and checks it is writable (by creating and
/// removing a probe file).
pub fn ensure(dir: &Path) -> Result<(), DataDirError> {
    let not_writable = |e: std::io::Error| DataDirError::NotWritable {
        path: dir.to_path_buf(),
        reason: e.to_string(),
    };
    std::fs::create_dir_all(dir).map_err(not_writable)?;
    let probe = dir.join(format!(".fortnight-probe-{}", std::process::id()));
    std::fs::write(&probe, b"probe").map_err(not_writable)?;
    std::fs::remove_file(&probe).map_err(not_writable)?;
    Ok(())
}

/// `<dir>/fortnight.db`.
pub fn db_path(dir: &Path) -> PathBuf {
    dir.join("fortnight.db")
}

// ---- lock file (round 2, agent G) ----
//
// PLAN.md section 3.5: one writer at a time. The GUI takes the lock at
// launch and warns (but carries on) if a lock already existed, since the
// folder may be open on another machine through a sync service. The CLI
// does not take the lock (it may run beside the GUI on the same machine);
// it only reads it to warn when another host holds it.

/// Who holds the lock: stored as JSON in the lock file.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LockInfo {
    /// The machine's name, or "unknown".
    pub host: String,
    pub pid: u32,
    pub since: jiff::Timestamp,
}

/// The held lock. Dropping it removes the lock file (if it is still ours).
#[derive(Debug)]
pub struct DataLock {
    path: PathBuf,
}

impl Drop for DataLock {
    fn drop(&mut self) {
        // STUB: agent G removes the file only if it still holds our info.
        let _ = &self.path;
    }
}

/// `<dir>/fortnight.lock`.
pub fn lock_path(dir: &Path) -> PathBuf {
    dir.join("fortnight.lock")
}

/// The name of this machine, or "unknown".
pub fn host_name() -> String {
    "unknown".into() // STUB
}

/// Takes the lock, replacing any existing lock file. Returns the lock and,
/// if a lock file already existed (another instance, another machine, or a
/// crash), what it said, so the caller can warn. An unreadable existing
/// file is reported as a `LockInfo` with host "unknown".
pub fn acquire_lock(dir: &Path) -> Result<(DataLock, Option<LockInfo>), DataDirError> {
    // STUB: agent G implements.
    Ok((
        DataLock {
            path: lock_path(dir),
        },
        None,
    ))
}

/// What the lock file says, if there is one.
pub fn read_lock(dir: &Path) -> Option<LockInfo> {
    let _ = dir;
    None // STUB
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exe() -> PathBuf {
        PathBuf::from("/opt/tools/fortnight")
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "fortnight-datadir-{tag}-{}-{nanos}",
            std::process::id()
        ))
    }

    #[test]
    fn flag_wins_over_env_and_exe() {
        let got = resolve_from(Some("/flag".into()), Some("/env".into()), &exe());
        assert_eq!(got, PathBuf::from("/flag"));
    }

    #[test]
    fn env_wins_over_exe() {
        let got = resolve_from(None, Some("/env".into()), &exe());
        assert_eq!(got, PathBuf::from("/env"));
    }

    #[test]
    fn empty_env_is_ignored() {
        let got = resolve_from(None, Some(OsString::new()), &exe());
        assert_eq!(got, PathBuf::from("/opt/tools/fortnight-data"));
    }

    #[test]
    fn plain_exe_uses_sibling_folder() {
        let got = resolve_from(None, None, &exe());
        assert_eq!(got, PathBuf::from("/opt/tools").join(DIR_NAME));
    }

    #[test]
    fn app_bundle_exe_uses_folder_beside_bundle() {
        let exe = Path::new("/Users/me/Apps/Fortnight.app/Contents/MacOS/fortnight");
        let got = resolve_from(None, None, exe);
        assert_eq!(got, PathBuf::from("/Users/me/Apps").join(DIR_NAME));
    }

    #[test]
    fn macos_folder_outside_a_bundle_is_not_special() {
        let exe = Path::new("/Users/me/MacOS/fortnight");
        let got = resolve_from(None, None, exe);
        assert_eq!(got, PathBuf::from("/Users/me/MacOS").join(DIR_NAME));
    }

    #[test]
    fn ensure_creates_missing_nested_dir() {
        let root = temp_dir("create");
        let nested = root.join("a").join("b");
        ensure(&nested).unwrap();
        assert!(nested.is_dir());
        // The probe file is cleaned up.
        assert_eq!(std::fs::read_dir(&nested).unwrap().count(), 0);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn ensure_read_only_dir_is_not_writable() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp_dir("ro");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();
        // Root ignores permission bits; skip the assertion then.
        let writable_anyway = std::fs::write(dir.join("x"), b"").is_ok();
        let result = ensure(&dir);
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        if !writable_anyway {
            assert!(matches!(result, Err(DataDirError::NotWritable { .. })));
        }
    }
}
