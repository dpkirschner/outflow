//! Per-item Plaid access-token store: a 0600 JSON file mapping
//! `item_id → access_token`. Access tokens are secrets — they never touch the
//! DB or argv (invariant #6); a mode-checked file is the headless-server
//! equivalent of the keychain. Non-secret item metadata (institution, cursor,
//! status) lives in the DB's `plaid_items` table instead.

use std::collections::HashMap;

/// Load the token map. A missing file is an empty map (fresh install); an
/// existing file must be 0600 and valid JSON.
pub fn load_tokens(path: &str) -> Result<HashMap<String, String>, String> {
    if !std::path::Path::new(path).exists() {
        return Ok(HashMap::new());
    }
    let raw = read_checked(path)?;
    if raw.trim().is_empty() {
        return Ok(HashMap::new());
    }
    serde_json::from_str(&raw).map_err(|e| format!("{path}: bad token file JSON: {e}"))
}

pub fn token_for(path: &str, item_id: &str) -> Result<String, String> {
    load_tokens(path)?
        .remove(item_id)
        .ok_or_else(|| format!("no access token stored for item {item_id}"))
}

pub fn save_token(path: &str, item_id: &str, access_token: &str) -> Result<(), String> {
    let mut tokens = load_tokens(path)?;
    tokens.insert(item_id.to_string(), access_token.to_string());
    write_map(path, &tokens)
}

pub fn remove_token(path: &str, item_id: &str) -> Result<(), String> {
    let mut tokens = load_tokens(path)?;
    tokens.remove(item_id);
    write_map(path, &tokens)
}

/// Read the file, refusing it if group/other can access it — same posture as
/// `secrets::read_secret_file` but tolerant of an empty map.
fn read_checked(path: &str) -> Result<String, String> {
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
    std::fs::read_to_string(path).map_err(|e| format!("read {path}: {e}"))
}

fn write_map(path: &str, tokens: &HashMap<String, String>) -> Result<(), String> {
    if let Some(dir) = std::path::Path::new(path).parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("mkdir {}: {e}", dir.display()))?;
    }
    let body = serde_json::to_string_pretty(tokens).map_err(|e| format!("serialize tokens: {e}"))?;
    std::fs::write(path, body).map_err(|e| format!("write {path}: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| format!("chmod {path}: {e}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> String {
        let mut p = std::env::temp_dir();
        p.push(format!("outflow-tokens-{}-{}.json", tag, std::process::id()));
        p.to_string_lossy().into_owned()
    }

    #[test]
    fn round_trips_and_removes() {
        let path = tmp("roundtrip");
        let _ = std::fs::remove_file(&path);

        assert!(load_tokens(&path).unwrap().is_empty());
        save_token(&path, "item1", "access-sandbox-aaa").unwrap();
        save_token(&path, "item2", "access-sandbox-bbb").unwrap();
        assert_eq!(token_for(&path, "item1").unwrap(), "access-sandbox-aaa");

        remove_token(&path, "item1").unwrap();
        assert!(token_for(&path, "item1").is_err());
        assert_eq!(token_for(&path, "item2").unwrap(), "access-sandbox-bbb");

        let _ = std::fs::remove_file(&path);
    }

    #[cfg(unix)]
    #[test]
    fn refuses_group_readable_file() {
        use std::os::unix::fs::PermissionsExt;
        let path = tmp("perms");
        save_token(&path, "item1", "tok").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(load_tokens(&path).is_err());
        let _ = std::fs::remove_file(&path);
    }
}
