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
    /// What we wrote, to recognise our own file on drop.
    info: LockInfo,
}

impl Drop for DataLock {
    fn drop(&mut self) {
        // Another instance may have taken over: only remove what is ours.
        if read_file(&self.path).as_ref() == Some(&self.info) {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// `<dir>/fortnight.lock`.
pub fn lock_path(dir: &Path) -> PathBuf {
    dir.join("fortnight.lock")
}

/// The name of this machine, or "unknown".
pub fn host_name() -> String {
    let clean = |s: &str| {
        let s = s.trim();
        (!s.is_empty()).then(|| s.to_string())
    };
    for var in ["HOSTNAME", "COMPUTERNAME"] {
        if let Some(name) = std::env::var(var).ok().and_then(|v| clean(&v)) {
            return name;
        }
    }
    std::process::Command::new("hostname")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| clean(&String::from_utf8_lossy(&o.stdout)))
        .unwrap_or_else(|| "unknown".into())
}

/// Parses a lock file; `None` if missing or not valid.
fn read_file(path: &Path) -> Option<LockInfo> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// The same machine, allowing for macOS adding or dropping `.local` from its
/// host name depending on the network, and for case. An "unknown" host (an
/// unreadable lock file) is never the same machine.
pub fn same_host(a: &str, b: &str) -> bool {
    let base = |s: &str| {
        let s = s.trim().to_ascii_lowercase();
        s.strip_suffix(".local").unwrap_or(&s).to_string()
    };
    let a = base(a);
    let b = base(b);
    a != "unknown" && a == b
}

/// Whether `lock` was written on a machine other than `host` (this one, from
/// [`host_name`]): what the CLI warns about. A lock from an unknown host
/// counts as elsewhere.
pub fn held_elsewhere(lock: &LockInfo, host: &str) -> bool {
    !same_host(&lock.host, host)
}

/// Whether a process with this id is running on this machine. When it
/// can't tell, it says yes (so the warning is kept).
fn process_running(pid: u32) -> bool {
    #[cfg(unix)]
    {
        std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map_or(true, |s| s.success())
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH"])
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .map_or(true, |o| {
                !o.status.success() || String::from_utf8_lossy(&o.stdout).contains(&pid.to_string())
            })
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = pid;
        true
    }
}

/// Takes the lock, replacing any existing lock file. Returns the lock and,
/// if a lock file already existed (another instance, another machine, or a
/// crash), what it said, so the caller can warn. An unreadable existing
/// file is reported as a `LockInfo` with host "unknown".
///
/// A lock left by this machine whose process has gone is not reported: the
/// app was closed without running its clean-up (macOS's Cmd+Q ends the
/// process without dropping the app state) or crashed.
///
/// With no lock file, the lock is created with `create_new`, so of two
/// instances starting together only one gets it that way and the other sees
/// its lock and warns. Otherwise the existing file is read and replaced.
pub fn acquire_lock(dir: &Path) -> Result<(DataLock, Option<LockInfo>), DataDirError> {
    let path = lock_path(dir);
    let now = jiff::Timestamp::now();
    let host = host_name();
    let pid = std::process::id();
    let not_writable = |e: std::io::Error| DataDirError::NotWritable {
        path: dir.to_path_buf(),
        reason: e.to_string(),
    };
    let to_json = |info: &LockInfo| {
        serde_json::to_string_pretty(info).map_err(|e| DataDirError::NotWritable {
            path: dir.to_path_buf(),
            reason: e.to_string(),
        })
    };

    let fresh = LockInfo {
        host: host.clone(),
        pid,
        since: now,
    };
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let temp_lock = || {
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        dir.join(format!(".fortnight.lock.{pid}.{n}.tmp"))
    };
    // Write the full JSON to a temp file and atomically link it as `path`
    // (`hard_link` fails with `AlreadyExists` if `path` exists), so another
    // instance racing us never observes a 0-byte lock file.
    let fresh_json = to_json(&fresh)?;
    let tmp = temp_lock();
    if std::fs::write(&tmp, &fresh_json).is_ok() {
        let linked = std::fs::hard_link(&tmp, &path);
        let _ = std::fs::remove_file(&tmp);
        if linked.is_ok() {
            return Ok((DataLock { path, info: fresh }, None));
        }
    }

    let previous = match path.try_exists() {
        Ok(false) => None,
        // Exists (or cannot tell): report what it says, or "unknown".
        _ => Some(read_file(&path).unwrap_or_else(|| LockInfo {
            host: "unknown".into(),
            pid: 0,
            since: now,
        })),
    };
    let previous = previous
        .filter(|p| !(same_host(&p.host, &host) && p.pid != pid && !process_running(p.pid)));
    let mut since = now;
    // Keep our info distinguishable from a previous lock of this same
    // process (a coarse clock could otherwise repeat the timestamp).
    if let Some(prev) = previous.as_ref().filter(|p| p.host == host && p.pid == pid)
        && since <= prev.since
    {
        since = prev.since + jiff::SignedDuration::from_nanos(1);
    }
    let info = LockInfo { host, pid, since };
    let json = to_json(&info)?;
    // Write beside the target, then rename, so readers never see half a file.
    // Unique per call, so two acquires in one process cannot collide.
    let tmp = temp_lock();
    std::fs::write(&tmp, json).map_err(not_writable)?;
    if let Err(e) = std::fs::rename(&tmp, &path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(not_writable(e));
    }
    Ok((DataLock { path, info }, previous))
}

/// What the lock file says, if there is one.
pub fn read_lock(dir: &Path) -> Option<LockInfo> {
    read_file(&lock_path(dir))
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

    #[test]
    fn acquire_on_empty_dir_has_no_previous() {
        let dir = temp_dir("lock-empty");
        std::fs::create_dir_all(&dir).unwrap();
        let (lock, previous) = acquire_lock(&dir).unwrap();
        assert_eq!(previous, None);
        assert!(lock_path(&dir).is_file());
        let info = read_lock(&dir).unwrap();
        assert_eq!(info.pid, std::process::id());
        assert_eq!(info.host, host_name());
        drop(lock);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn second_acquire_reports_the_first() {
        let dir = temp_dir("lock-second");
        std::fs::create_dir_all(&dir).unwrap();
        let (first, _) = acquire_lock(&dir).unwrap();
        let first_info = read_lock(&dir).unwrap();
        let (second, previous) = acquire_lock(&dir).unwrap();
        assert_eq!(previous, Some(first_info));
        drop(second);
        drop(first);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Instances starting together: only one finds no lock.
    #[test]
    fn simultaneous_acquires_see_each_other() {
        let dir = temp_dir("lock-race");
        std::fs::create_dir_all(&dir).unwrap();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let (dir, barrier) = (dir.clone(), barrier.clone());
                std::thread::spawn(move || {
                    barrier.wait();
                    acquire_lock(&dir).unwrap()
                })
            })
            .collect();
        let results: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
        let unwarned = results.iter().filter(|(_, prev)| prev.is_none()).count();
        drop(results);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(unwarned, 1);
    }

    #[test]
    fn drop_removes_our_lock_file() {
        let dir = temp_dir("lock-drop");
        std::fs::create_dir_all(&dir).unwrap();
        let (lock, _) = acquire_lock(&dir).unwrap();
        drop(lock);
        assert!(!lock_path(&dir).exists());
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn drop_leaves_a_lock_another_acquire_replaced() {
        let dir = temp_dir("lock-replaced");
        std::fs::create_dir_all(&dir).unwrap();
        let (first, _) = acquire_lock(&dir).unwrap();
        let (second, _) = acquire_lock(&dir).unwrap();
        let second_info = read_lock(&dir).unwrap();
        drop(first);
        assert_eq!(read_lock(&dir), Some(second_info));
        drop(second);
        assert!(!lock_path(&dir).exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn hosts_match_with_or_without_local() {
        assert!(same_host(
            "Clements-MacBook-Pro.local",
            "clements-macbook-pro"
        ));
        assert!(same_host(
            "Clements-MacBook-Pro.LOCAL",
            "clements-macbook-pro"
        ));
        assert!(!same_host("a.local", "b.local"));
        assert!(!same_host("unknown", "unknown"));
        assert!(!same_host("Unknown", "UNKNOWN"));
    }

    #[cfg(unix)]
    #[test]
    fn a_lock_left_by_a_finished_process_here_is_ignored() {
        let dir = temp_dir("lock-stale");
        ensure(&dir).unwrap();
        let mut child = std::process::Command::new("true").spawn().unwrap();
        let dead = child.id();
        child.wait().unwrap();
        let write = |pid: u32| {
            let info = LockInfo {
                host: host_name(),
                pid,
                since: jiff::Timestamp::now(),
            };
            std::fs::write(lock_path(&dir), serde_json::to_string(&info).unwrap()).unwrap();
        };
        write(dead);
        let (lock, previous) = acquire_lock(&dir).unwrap();
        assert_eq!(previous, None);
        drop(lock);

        // A running process on this machine is still reported.
        let mut live = std::process::Command::new("sleep")
            .arg("5")
            .spawn()
            .unwrap();
        write(live.id());
        let (lock, previous) = acquire_lock(&dir).unwrap();
        assert_eq!(previous.map(|p| p.pid), Some(live.id()));
        let _ = live.kill();
        let _ = live.wait();
        drop(lock);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn garbage_lock_file_reports_unknown() {
        let dir = temp_dir("lock-garbage");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(lock_path(&dir), b"not json {{").unwrap();
        assert_eq!(read_lock(&dir), None);
        let (lock, previous) = acquire_lock(&dir).unwrap();
        let previous = previous.unwrap();
        assert_eq!(previous.host, "unknown");
        assert_eq!(previous.pid, 0);
        drop(lock);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn read_lock_missing_is_none() {
        let dir = temp_dir("lock-missing");
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(read_lock(&dir), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn held_elsewhere_ignores_local_suffix_and_case() {
        let lock = |host: &str| LockInfo {
            host: host.into(),
            pid: 1,
            since: jiff::Timestamp::UNIX_EPOCH,
        };
        assert!(!held_elsewhere(&lock("Foo.local"), "Foo"));
        assert!(!held_elsewhere(&lock("foo"), "FOO.local"));
        assert!(held_elsewhere(&lock("Bar"), "Foo"));
        assert!(held_elsewhere(&lock("unknown"), "Foo"));
    }

    #[test]
    fn host_name_is_trimmed_and_not_empty() {
        let name = host_name();
        assert!(!name.is_empty());
        assert_eq!(name.trim(), name);
    }
}
