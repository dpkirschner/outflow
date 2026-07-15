//! Secret-file and keychain helpers. Secrets never touch the DB or argv
//! (invariant): they resolve from env, a 0600 file, or the OS keychain only.
//! The 0600-file path is the headless-server posture (launchd has no unlocked
//! GUI keychain).

#[cfg(feature = "keychain")]
const SERVICE: &str = "outflow";
#[cfg(feature = "keychain")]
const DB_KEY_ACCOUNT: &str = "db-key";

/// Read a secret from a file, refusing it if group/other can read it.
pub fn read_secret_file(path: &str) -> Result<String, String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(path)
            .map_err(|e| format!("stat {path}: {e}"))?
            .permissions()
            .mode();
        if mode & 0o077 != 0 {
            return Err(format!(
                "{path} is group/other-accessible (mode {:o}); run `chmod 600 {path}`",
                mode & 0o777
            ));
        }
    }
    let contents = std::fs::read_to_string(path).map_err(|e| format!("read {path}: {e}"))?;
    let secret = contents.trim();
    if secret.is_empty() {
        return Err(format!("{path} is empty"));
    }
    Ok(secret.to_string())
}

/// Write a secret to a file with 0600 permissions (owner-only).
pub fn write_secret_file(path: &str, secret: &str) -> Result<(), String> {
    std::fs::write(path, format!("{secret}\n")).map_err(|e| format!("write {path}: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| format!("chmod {path}: {e}"))?;
    }
    Ok(())
}

/// Fetch the SQLCipher DB key from the keychain, generating and storing a fresh
/// random one on first use. This is what makes transparent, passphrase-free
/// encryption work for interactive (non-headless) use.
///
/// Critical: only generate a new key when the entry genuinely does NOT exist
/// (`NoEntry`). On any other error — keychain locked, access denied — we must
/// return the error, never regenerate: a new key would orphan an existing
/// encrypted DB, making it permanently unreadable.
#[cfg(feature = "keychain")]
pub fn db_key_get_or_create() -> Result<String, String> {
    let entry =
        keyring::Entry::new(SERVICE, DB_KEY_ACCOUNT).map_err(|e| format!("keychain: {e}"))?;
    match entry.get_password() {
        Ok(k) if !k.is_empty() => Ok(k),
        Ok(_) | Err(keyring::Error::NoEntry) => {
            let key = random_hex_32()?;
            entry
                .set_password(&key)
                .map_err(|e| format!("keychain store db-key: {e}"))?;
            Ok(key)
        }
        Err(e) => Err(format!("keychain read db-key: {e}")),
    }
}

/// 32 random bytes as a 64-char hex string, for use as a SQLCipher passphrase.
#[cfg(feature = "keychain")]
fn random_hex_32() -> Result<String, String> {
    let mut buf = [0u8; 32];
    getrandom::getrandom(&mut buf).map_err(|e| format!("rng: {e}"))?;
    Ok(buf.iter().map(|b| format!("{b:02x}")).collect())
}
