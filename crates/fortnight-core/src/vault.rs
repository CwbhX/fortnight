//! The encrypted API key file, `fortnight-data/llm-key`. Owner: agent C.
//!
//! Argon2id (per-file random salt) stretches the password into a 256-bit
//! key; XChaCha20-Poly1305 seals the API key under a random nonce. The file
//! holds a format version, the Argon2 parameters, the salt, the nonce and
//! the ciphertext; the header is authenticated (AEAD associated data), so
//! tampering fails the unlock. Writes are atomic (temp file + rename).
//! See PLAN.md section 3.3.
//!
//! # File layout (format version 1)
//!
//! | Offset | Size | Field                                         |
//! |-------:|-----:|-----------------------------------------------|
//! |      0 |    5 | magic `FNKEY`                                 |
//! |      5 |    1 | format version, `1`                           |
//! |      6 |    4 | Argon2id `m_cost` in KiB, u32 little-endian   |
//! |     10 |    4 | Argon2id `t_cost` (iterations), u32 LE        |
//! |     14 |    4 | Argon2id `p_cost` (lanes), u32 LE             |
//! |     18 |   16 | random salt                                   |
//! |     34 |   24 | random XChaCha20 nonce                        |
//! |     58 |  n+16| ciphertext of the UTF-8 API key, with the tag |
//!
//! Bytes `0..58` (the whole header) are the AEAD associated data. The
//! Argon2 version is 0x13. Unlock uses the parameters stored in the file,
//! bounded by `MAX_M_COST_KIB`, `MAX_T_COST` and `MAX_P_COST` so that a
//! hostile file cannot exhaust memory or CPU.
//!
//! Skipped for the prototype: the `XChaCha20Poly1305` cipher state is not
//! zeroized on drop (that needs the crate's `zeroize` feature, which the
//! workspace does not enable); the derived key and plaintext buffers are.

use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::aead::{Aead, Generate, Payload};
use chacha20poly1305::{KeyInit, XChaCha20Poly1305, XNonce};
use secrecy::{ExposeSecret, SecretString};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use zeroize::{Zeroize, Zeroizing};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum VaultError {
    #[error("wrong password")]
    WrongPassword,
    #[error("the key file is damaged or not a Fortnight key file: {0}")]
    Corrupt(String),
    #[error("password must be at least {0} characters")]
    PasswordTooShort(usize),
    #[error("file error: {0}")]
    Io(String),
}

/// Minimum password length enforced by `save`.
pub const MIN_PASSWORD_LEN: usize = 8;

const MAGIC: &[u8; 5] = b"FNKEY";
const FORMAT_VERSION: u8 = 1;
const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 24;
const TAG_LEN: usize = 16;
const KEY_LEN: usize = 32;
const HEADER_LEN: usize = MAGIC.len() + 1 + 3 * 4 + SALT_LEN + NONCE_LEN;

/// Argon2id parameters written by `save` (OWASP's minimum recommendation).
const DEFAULT_M_COST_KIB: u32 = 19_456;
const DEFAULT_T_COST: u32 = 2;
const DEFAULT_P_COST: u32 = 1;

/// Upper bounds accepted by `unlock`: 1 GiB, 64 passes, 16 lanes.
const MAX_M_COST_KIB: u32 = 256 * 1024;
const MAX_T_COST: u32 = 10;
const MAX_P_COST: u32 = 4;

/// `<data_dir>/llm-key`.
pub fn key_path(data_dir: &Path) -> PathBuf {
    data_dir.join("llm-key")
}

/// `<data_dir>/llm-key-decision`: the decision model's own API key
/// (`Settings::decision_llm`), same format. The app and CLI keep it under
/// the same password as `llm-key`, so one unlock opens both.
pub fn decision_key_path(data_dir: &Path) -> PathBuf {
    data_dir.join("llm-key-decision")
}

/// Encrypts `api_key` under `password` and writes it atomically to `path`,
/// replacing any existing file.
pub fn save(
    path: &Path,
    api_key: &SecretString,
    password: &SecretString,
) -> Result<(), VaultError> {
    if password.expose_secret().chars().count() < MIN_PASSWORD_LEN {
        return Err(VaultError::PasswordTooShort(MIN_PASSWORD_LEN));
    }
    let bytes = seal(api_key, password)?;
    write_atomic(path, &bytes)
}

/// Reads and decrypts the key file.
pub fn unlock(path: &Path, password: &SecretString) -> Result<SecretString, VaultError> {
    let bytes = fs::read(path).map_err(|e| io_error("read", path, &e))?;
    open(&bytes, password)
}

/// Decrypts with `old` and re-encrypts with `new`.
pub fn change_password(
    path: &Path,
    old: &SecretString,
    new: &SecretString,
) -> Result<(), VaultError> {
    // Check the new password first so a too-short one costs no KDF run.
    if new.expose_secret().chars().count() < MIN_PASSWORD_LEN {
        return Err(VaultError::PasswordTooShort(MIN_PASSWORD_LEN));
    }
    let api_key = unlock(path, old)?;
    save(path, &api_key, new)
}

/// Deletes the key file. Not an error if it does not exist.
pub fn remove(path: &Path) -> Result<(), VaultError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(io_error("remove", path, &e)),
    }
}

struct KdfParams {
    m_cost: u32,
    t_cost: u32,
    p_cost: u32,
}

fn derive_key(
    password: &SecretString,
    salt: &[u8],
    p: &KdfParams,
) -> Result<Zeroizing<[u8; KEY_LEN]>, VaultError> {
    let params = Params::new(p.m_cost, p.t_cost, p.p_cost, Some(KEY_LEN))
        .map_err(|e| VaultError::Corrupt(format!("invalid Argon2 parameters: {e}")))?;
    let mut key = Zeroizing::new([0u8; KEY_LEN]);
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(password.expose_secret().as_bytes(), salt, &mut key[..])
        .map_err(|e| VaultError::Corrupt(format!("key derivation failed: {e}")))?;
    Ok(key)
}

fn cipher(key: &[u8; KEY_LEN]) -> XChaCha20Poly1305 {
    XChaCha20Poly1305::new_from_slice(key).expect("key is 32 bytes")
}

/// Builds the complete file contents.
fn seal(api_key: &SecretString, password: &SecretString) -> Result<Vec<u8>, VaultError> {
    let params = KdfParams {
        m_cost: DEFAULT_M_COST_KIB,
        t_cost: DEFAULT_T_COST,
        p_cost: DEFAULT_P_COST,
    };
    let salt = <[u8; SALT_LEN]>::try_generate()
        .map_err(|e| VaultError::Io(format!("system random number generator failed: {e}")))?;
    let nonce = XNonce::try_generate()
        .map_err(|e| VaultError::Io(format!("system random number generator failed: {e}")))?;

    let mut out = Vec::with_capacity(HEADER_LEN + api_key.expose_secret().len() + TAG_LEN);
    out.extend_from_slice(MAGIC);
    out.push(FORMAT_VERSION);
    out.extend_from_slice(&params.m_cost.to_le_bytes());
    out.extend_from_slice(&params.t_cost.to_le_bytes());
    out.extend_from_slice(&params.p_cost.to_le_bytes());
    out.extend_from_slice(&salt);
    out.extend_from_slice(&nonce);
    debug_assert_eq!(out.len(), HEADER_LEN);

    let key = derive_key(password, &salt, &params)?;
    let ciphertext = cipher(&key)
        .encrypt(
            &nonce,
            Payload {
                msg: api_key.expose_secret().as_bytes(),
                aad: &out,
            },
        )
        .map_err(|_| VaultError::Io("encryption failed".into()))?;
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

/// Parses and decrypts complete file contents.
fn open(bytes: &[u8], password: &SecretString) -> Result<SecretString, VaultError> {
    if bytes.len() < MAGIC.len() || &bytes[..MAGIC.len()] != MAGIC {
        return Err(VaultError::Corrupt("not a Fortnight key file".into()));
    }
    if bytes.len() < MAGIC.len() + 1 {
        return Err(VaultError::Corrupt("file is truncated".into()));
    }
    let version = bytes[MAGIC.len()];
    if version != FORMAT_VERSION {
        return Err(VaultError::Corrupt(format!(
            "unsupported format version {version}"
        )));
    }
    if bytes.len() < HEADER_LEN + TAG_LEN {
        return Err(VaultError::Corrupt("file is truncated".into()));
    }
    let (header, ciphertext) = bytes.split_at(HEADER_LEN);
    let u32_at = |i: usize| u32::from_le_bytes(header[i..i + 4].try_into().expect("4 bytes"));
    let params = KdfParams {
        m_cost: u32_at(6),
        t_cost: u32_at(10),
        p_cost: u32_at(14),
    };
    if params.m_cost > MAX_M_COST_KIB || params.t_cost > MAX_T_COST || params.p_cost > MAX_P_COST {
        return Err(VaultError::Corrupt(
            "Argon2 parameters exceed the allowed maximum".into(),
        ));
    }
    let salt = &header[18..18 + SALT_LEN];
    let nonce = XNonce::try_from(&header[18 + SALT_LEN..HEADER_LEN]).expect("24 bytes");

    let key = derive_key(password, salt, &params)?;
    let mut plaintext = cipher(&key)
        .decrypt(
            &nonce,
            Payload {
                msg: ciphertext,
                aad: header,
            },
        )
        .map_err(|_| VaultError::WrongPassword)?;

    // Copy into an exactly-sized allocation, then wipe the decrypt buffer.
    let result = match std::str::from_utf8(&plaintext) {
        Ok(s) => Ok(SecretString::from(s)),
        Err(_) => Err(VaultError::Corrupt("API key is not valid UTF-8".into())),
    };
    plaintext.zeroize();
    result
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), VaultError> {
    let dir = match path.parent() {
        Some(d) if !d.as_os_str().is_empty() => d,
        _ => Path::new("."),
    };
    let file_name = path
        .file_name()
        .ok_or_else(|| VaultError::Io(format!("invalid key file path {}", path.display())))?;
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let tmp = dir.join(format!(
        ".{}.tmp-{}-{nanos}",
        file_name.to_string_lossy(),
        std::process::id()
    ));

    let result = (|| -> io::Result<()> {
        let mut opts = fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut file = opts.open(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&tmp, path)
    })();

    result.map_err(|e| {
        let _ = fs::remove_file(&tmp);
        io_error("write", path, &e)
    })
}

fn io_error(action: &str, path: &Path, e: &io::Error) -> VaultError {
    VaultError::Io(format!("could not {action} {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// A unique temporary directory, removed on drop.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            static COUNTER: AtomicU32 = AtomicU32::new(0);
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let dir = std::env::temp_dir().join(format!(
                "fortnight-vault-test-{}-{n}-{nanos}",
                std::process::id()
            ));
            fs::create_dir_all(&dir).unwrap();
            TempDir(dir)
        }

        fn key(&self) -> PathBuf {
            key_path(&self.0)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn s(v: &str) -> SecretString {
        SecretString::from(v)
    }

    const API_KEY: &str = "sk-test-0123456789abcdef";
    const PASSWORD: &str = "correct horse";

    fn saved() -> (TempDir, PathBuf) {
        let dir = TempDir::new();
        let path = dir.key();
        save(&path, &s(API_KEY), &s(PASSWORD)).unwrap();
        (dir, path)
    }

    fn flip_byte(path: &Path, index: usize) {
        let mut bytes = fs::read(path).unwrap();
        bytes[index] ^= 0x01;
        fs::write(path, bytes).unwrap();
    }

    #[test]
    fn key_path_is_llm_key() {
        assert_eq!(key_path(Path::new("/d")), PathBuf::from("/d/llm-key"));
    }

    #[test]
    fn round_trip() {
        let (_dir, path) = saved();
        let key = unlock(&path, &s(PASSWORD)).unwrap();
        assert_eq!(key.expose_secret(), API_KEY);
    }

    #[test]
    fn save_replaces_existing_file() {
        let (_dir, path) = saved();
        save(&path, &s("sk-other"), &s(PASSWORD)).unwrap();
        assert_eq!(
            unlock(&path, &s(PASSWORD)).unwrap().expose_secret(),
            "sk-other"
        );
    }

    #[test]
    fn wrong_password() {
        let (_dir, path) = saved();
        assert_eq!(
            unlock(&path, &s("incorrect horse")).unwrap_err(),
            VaultError::WrongPassword
        );
    }

    #[test]
    fn tampered_header_byte_fails() {
        // Every header byte past magic and version is only protected by the
        // AEAD; check a salt byte, a nonce byte and the t_cost field.
        for index in [10, 20, 40, HEADER_LEN - 1] {
            let (_dir, path) = saved();
            flip_byte(&path, index);
            assert_eq!(
                unlock(&path, &s(PASSWORD)).unwrap_err(),
                VaultError::WrongPassword,
                "byte {index}"
            );
        }
    }

    #[test]
    fn tampered_ciphertext_fails() {
        let (_dir, path) = saved();
        flip_byte(&path, HEADER_LEN + 2);
        assert_eq!(
            unlock(&path, &s(PASSWORD)).unwrap_err(),
            VaultError::WrongPassword
        );
    }

    #[test]
    fn truncated_file_is_corrupt() {
        let (_dir, path) = saved();
        let bytes = fs::read(&path).unwrap();
        fs::write(&path, &bytes[..HEADER_LEN + 4]).unwrap();
        assert!(matches!(
            unlock(&path, &s(PASSWORD)),
            Err(VaultError::Corrupt(_))
        ));
        fs::write(&path, &bytes[..3]).unwrap();
        assert!(matches!(
            unlock(&path, &s(PASSWORD)),
            Err(VaultError::Corrupt(_))
        ));
    }

    #[test]
    fn bad_magic_is_corrupt() {
        let (_dir, path) = saved();
        flip_byte(&path, 0);
        assert!(matches!(
            unlock(&path, &s(PASSWORD)),
            Err(VaultError::Corrupt(_))
        ));
    }

    #[test]
    fn bad_version_is_corrupt() {
        let (_dir, path) = saved();
        flip_byte(&path, 5);
        assert!(matches!(
            unlock(&path, &s(PASSWORD)),
            Err(VaultError::Corrupt(_))
        ));
    }

    #[test]
    fn excessive_params_are_rejected_before_kdf() {
        let (_dir, path) = saved();
        let mut bytes = fs::read(&path).unwrap();
        bytes[6..10].copy_from_slice(&u32::MAX.to_le_bytes());
        fs::write(&path, bytes).unwrap();
        assert!(matches!(
            unlock(&path, &s(PASSWORD)),
            Err(VaultError::Corrupt(_))
        ));
    }

    #[test]
    fn missing_file_is_io_error() {
        let dir = TempDir::new();
        assert!(matches!(
            unlock(&dir.key(), &s(PASSWORD)),
            Err(VaultError::Io(_))
        ));
    }

    #[test]
    fn password_too_short() {
        let dir = TempDir::new();
        let path = dir.key();
        assert_eq!(
            save(&path, &s(API_KEY), &s("1234567")).unwrap_err(),
            VaultError::PasswordTooShort(MIN_PASSWORD_LEN)
        );
        assert!(!path.exists());
        // Length counts characters, not bytes.
        assert!(save(&path, &s(API_KEY), &s("ééééééé")).is_err());
        save(&path, &s(API_KEY), &s("12345678")).unwrap();
    }

    #[test]
    fn file_does_not_contain_plaintext_key() {
        let (_dir, path) = saved();
        let bytes = fs::read(&path).unwrap();
        assert!(
            !bytes
                .windows(API_KEY.len())
                .any(|w| w == API_KEY.as_bytes())
        );
        assert!(
            !bytes
                .windows(PASSWORD.len())
                .any(|w| w == PASSWORD.as_bytes())
        );
        assert!(bytes.starts_with(b"FNKEY\x01"));
        assert_eq!(bytes.len(), HEADER_LEN + API_KEY.len() + TAG_LEN);
    }

    #[test]
    fn no_temp_file_left_after_save() {
        let (dir, _path) = saved();
        let names: Vec<_> = fs::read_dir(&dir.0)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(names, vec!["llm-key".to_string()]);
    }

    #[test]
    fn change_password_switches_passwords() {
        let (_dir, path) = saved();
        change_password(&path, &s(PASSWORD), &s("new password!")).unwrap();
        assert_eq!(
            unlock(&path, &s(PASSWORD)).unwrap_err(),
            VaultError::WrongPassword
        );
        assert_eq!(
            unlock(&path, &s("new password!")).unwrap().expose_secret(),
            API_KEY
        );
    }

    #[test]
    fn change_password_with_wrong_old_password_keeps_file() {
        let (_dir, path) = saved();
        assert_eq!(
            change_password(&path, &s("not the password"), &s("new password!")).unwrap_err(),
            VaultError::WrongPassword
        );
        assert_eq!(
            unlock(&path, &s(PASSWORD)).unwrap().expose_secret(),
            API_KEY
        );
    }

    #[test]
    fn remove_deletes_and_tolerates_missing() {
        let (_dir, path) = saved();
        remove(&path).unwrap();
        assert!(!path.exists());
        remove(&path).unwrap();
    }

    #[test]
    fn errors_do_not_leak_secrets() {
        let (_dir, path) = saved();
        let err = unlock(&path, &s("a wrong password")).unwrap_err();
        let text = format!("{err} {err:?}");
        assert!(!text.contains("a wrong password"));
        assert!(!text.contains(API_KEY));
    }

    /// Written once by `save(…, "sk-fixed-test-vector", "fortnight-vector")`
    /// with the default parameters. If this stops unlocking, the file format
    /// changed: bump `FORMAT_VERSION` and keep reading version 1.
    const VECTOR_HEX: &str = "464e4b455901004c0000020000000100000063311939e4eb854fa1c47a607f6b3b255a28da3472784e79d08555976ed906b1123c51c9ee5c100c8504988804a316b7b032c0777407d4a9521a3302ae4d0211376868e18ad36ecad63df45b";

    fn from_hex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn fixed_test_vector_unlocks() {
        let dir = TempDir::new();
        let path = dir.key();
        fs::write(&path, from_hex(VECTOR_HEX)).unwrap();
        let key = unlock(&path, &s("fortnight-vector")).unwrap();
        assert_eq!(key.expose_secret(), "sk-fixed-test-vector");
    }

    #[test]
    #[ignore = "prints a new test vector; run with --ignored --nocapture"]
    fn print_test_vector() {
        let dir = TempDir::new();
        let path = dir.key();
        save(&path, &s("sk-fixed-test-vector"), &s("fortnight-vector")).unwrap();
        let hex: String = fs::read(&path)
            .unwrap()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        println!("VECTOR {hex}");
    }
}
