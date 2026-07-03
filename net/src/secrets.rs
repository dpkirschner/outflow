//! Access-URL resolution and persistence. Precedence on read: `OUTFLOW_SFIN_URL`
//! env → `OUTFLOW_SFIN_URL_FILE` (a 0600 file, the headless path) → OS keychain.
//! The secret is never placed in the DB or argv.

#[cfg(feature = "keychain")]
const SERVICE: &str = "outflow";
#[cfg(feature = "keychain")]
const ACCOUNT: &str = "simplefin-access-url";

/// Where a persisted access URL landed, so callers can report it.
pub enum Persisted {
    File(String),
    Keychain,
    /// Nowhere durable (no file path, no keychain feature); caller must tell the
    /// user to stash it themselves.
    Ephemeral,
}

/// Resolve the SimpleFIN access URL in precedence order.
pub fn access_url() -> Result<String, String> {
    // 1. Explicit value in the environment (never logged, never in argv).
    if let Ok(u) = std::env::var("OUTFLOW_SFIN_URL") {
        if !u.is_empty() {
            return Ok(u);
        }
    }
    // 2. A 0600 secret file. The headless-cron path: unlike the login keychain,
    //    a file needs no unlocked GUI session.
    if let Ok(p) = std::env::var("OUTFLOW_SFIN_URL_FILE") {
        if !p.is_empty() {
            return read_secret_file(&p);
        }
    }
    // 3. OS keychain — the default for an interactive machine.
    #[cfg(feature = "keychain")]
    {
        keychain_get()
    }
    #[cfg(not(feature = "keychain"))]
    {
        Err("no access URL: set OUTFLOW_SFIN_URL, or OUTFLOW_SFIN_URL_FILE to a 0600 file".into())
    }
}

/// Persist an access URL, mirroring `access_url`'s precedence: a file path (the
/// headless path) wins, else the keychain, else nowhere. Never prints — the
/// caller decides how to report the outcome.
pub fn persist_access_url(access_url: &str) -> Result<Persisted, String> {
    if let Ok(p) = std::env::var("OUTFLOW_SFIN_URL_FILE") {
        if !p.is_empty() {
            write_secret_file(&p, access_url)?;
            return Ok(Persisted::File(p));
        }
    }
    #[cfg(feature = "keychain")]
    {
        keychain_set(access_url)?;
        Ok(Persisted::Keychain)
    }
    #[cfg(not(feature = "keychain"))]
    {
        Ok(Persisted::Ephemeral)
    }
}

/// Read an access URL from a file, refusing it if group/other can read it.
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
    let url = contents.trim();
    if url.is_empty() {
        return Err(format!("{path} is empty"));
    }
    Ok(url.to_string())
}

/// Write the access URL to a file with 0600 permissions (owner-only).
pub fn write_secret_file(path: &str, access_url: &str) -> Result<(), String> {
    std::fs::write(path, format!("{access_url}\n")).map_err(|e| format!("write {path}: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| format!("chmod {path}: {e}"))?;
    }
    Ok(())
}

#[cfg(feature = "keychain")]
pub fn keychain_get() -> Result<String, String> {
    let entry = keyring::Entry::new(SERVICE, ACCOUNT).map_err(|e| format!("keychain: {e}"))?;
    entry.get_password().map_err(|e| {
        format!("keychain: {e} (headless? set OUTFLOW_SFIN_URL_FILE to a 0600 file instead)")
    })
}

#[cfg(feature = "keychain")]
pub fn keychain_set(access_url: &str) -> Result<(), String> {
    let entry = keyring::Entry::new(SERVICE, ACCOUNT).map_err(|e| format!("keychain: {e}"))?;
    entry
        .set_password(access_url)
        .map_err(|e| format!("keychain store: {e}"))
}
