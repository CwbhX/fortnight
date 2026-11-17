//! The encrypted API key file, `fortnight-data/llm-key`. Owner: agent C.
//!
//! Argon2id (per-file random salt) stretches the password into a 256-bit
//! key; XChaCha20-Poly1305 seals the API key under a random nonce. The file
//! holds a format version, the Argon2 parameters, the salt, the nonce and
//! the ciphertext; the header is authenticated (AEAD associated data), so
//! tampering fails the unlock. Writes are atomic (temp file + rename).
//! See PLAN.md section 3.3.

use secrecy::SecretString;
use std::path::{Path, PathBuf};

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

/// `<data_dir>/llm-key`.
pub fn key_path(data_dir: &Path) -> PathBuf {
    data_dir.join("llm-key")
}

/// Encrypts `api_key` under `password` and writes it atomically to `path`,
/// replacing any existing file.
pub fn save(
    _path: &Path,
    _api_key: &SecretString,
    _password: &SecretString,
) -> Result<(), VaultError> {
    todo!("agent C")
}

/// Reads and decrypts the key file.
pub fn unlock(_path: &Path, _password: &SecretString) -> Result<SecretString, VaultError> {
    todo!("agent C")
}

/// Decrypts with `old` and re-encrypts with `new`.
pub fn change_password(
    _path: &Path,
    _old: &SecretString,
    _new: &SecretString,
) -> Result<(), VaultError> {
    todo!("agent C")
}

/// Deletes the key file. Not an error if it does not exist.
pub fn remove(_path: &Path) -> Result<(), VaultError> {
    todo!("agent C")
}
