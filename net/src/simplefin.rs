//! SimpleFIN transport: fetch an account set from an access URL, and exchange a
//! base64 setup token for a durable access URL. Returns raw JSON / URL strings;
//! parsing into domain types is `outflow_core::parse_account_set`.

/// Fetch up to ~90 days of accounts+transactions (the bridge hard-caps there).
/// Basic-auth credentials, if present, come from the access URL's userinfo.
pub fn fetch(access_url: &str) -> Result<String, String> {
    let (creds, base) = split_userinfo(access_url);
    let since = (now_secs() - 90 * 86400).max(0);
    let endpoint = format!("{base}/accounts?pending=1&start-date={since}");
    let mut req = ureq::get(&endpoint);
    if let Some(c) = creds {
        if let Some((user, pass)) = c.split_once(':') {
            let token = base64_basic(user, pass);
            req = req.set("Authorization", &format!("Basic {token}"));
        }
    }
    let resp = req.call().map_err(|e| format!("simplefin request: {e}"))?;
    resp.into_string().map_err(|e| format!("read body: {e}"))
}

/// Exchange a base64 setup token for an access URL: decode to the claim URL,
/// POST to it, and return the access URL the bridge hands back. Persistence is
/// the caller's job (`secrets::persist_access_url`).
pub fn claim_access_url(setup_token: &str) -> Result<String, String> {
    let decoded =
        base64_decode(setup_token.trim()).map_err(|e| format!("decode setup token: {e}"))?;
    let claim_url =
        String::from_utf8(decoded).map_err(|_| "setup token did not decode to a URL".to_string())?;
    let claim_url = claim_url.trim();
    if !claim_url.starts_with("http") {
        return Err(format!("decoded claim URL looks wrong: {claim_url}"));
    }
    let resp = ureq::post(claim_url)
        .call()
        .map_err(|e| format!("claim request: {e}"))?;
    let access_url = resp
        .into_string()
        .map_err(|e| format!("read claim body: {e}"))?
        .trim()
        .to_string();
    if !access_url.starts_with("http") {
        return Err(format!("claim did not return an access URL: {access_url}"));
    }
    Ok(access_url)
}

/// Split `scheme://user:pass@host/...` into `(Some("user:pass"), "scheme://host/...")`.
fn split_userinfo(url: &str) -> (Option<String>, String) {
    match url.split_once("://") {
        Some((scheme, rest)) => match rest.split_once('@') {
            Some((userinfo, host)) => (Some(userinfo.to_string()), format!("{scheme}://{host}")),
            None => (None, url.to_string()),
        },
        None => (None, url.to_string()),
    }
}

fn now_secs() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn base64_basic(user: &str, pass: &str) -> String {
    const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let raw = format!("{user}:{pass}");
    let b = raw.as_bytes();
    let mut out = String::new();
    for chunk in b.chunks(3) {
        let n = chunk.len();
        let b0 = chunk[0] as u32;
        let b1 = if n > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if n > 2 { chunk[2] as u32 } else { 0 };
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(A[((triple >> 18) & 63) as usize] as char);
        out.push(A[((triple >> 12) & 63) as usize] as char);
        out.push(if n > 1 { A[((triple >> 6) & 63) as usize] as char } else { '=' });
        out.push(if n > 2 { A[(triple & 63) as usize] as char } else { '=' });
    }
    out
}

fn base64_decode(s: &str) -> Result<Vec<u8>, String> {
    fn val(c: u8) -> Option<u32> {
        match c {
            b'A'..=b'Z' => Some((c - b'A') as u32),
            b'a'..=b'z' => Some((c - b'a' + 26) as u32),
            b'0'..=b'9' => Some((c - b'0' + 52) as u32),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let clean: Vec<u8> = s
        .bytes()
        .filter(|b| !b.is_ascii_whitespace() && *b != b'=')
        .collect();
    let mut out = Vec::with_capacity(clean.len() * 3 / 4);
    for chunk in clean.chunks(4) {
        let mut acc = 0u32;
        let mut bits = 0;
        for &c in chunk {
            let v = val(c).ok_or_else(|| format!("invalid base64 char {:?}", c as char))?;
            acc = (acc << 6) | v;
            bits += 6;
        }
        for shift in (0..bits / 8).map(|i| bits - 8 * (i + 1)) {
            out.push(((acc >> shift) & 0xff) as u8);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_auth_matches_known_vector() {
        // "Aladdin:open sesame" -> classic RFC 7617 example.
        assert_eq!(base64_basic("Aladdin", "open sesame"), "QWxhZGRpbjpvcGVuIHNlc2FtZQ==");
    }

    #[test]
    fn base64_decode_roundtrips_a_url() {
        let token = "aHR0cHM6Ly9leGFtcGxlLmNvbS9jbGFpbQ==";
        assert_eq!(
            String::from_utf8(base64_decode(token).unwrap()).unwrap(),
            "https://example.com/claim"
        );
    }

    #[test]
    fn split_userinfo_extracts_creds() {
        let (creds, base) = split_userinfo("https://demo:demo@bridge.example/simplefin");
        assert_eq!(creds.as_deref(), Some("demo:demo"));
        assert_eq!(base, "https://bridge.example/simplefin");
    }

    #[test]
    fn split_userinfo_without_creds() {
        let (creds, base) = split_userinfo("https://bridge.example/simplefin");
        assert!(creds.is_none());
        assert_eq!(base, "https://bridge.example/simplefin");
    }
}
