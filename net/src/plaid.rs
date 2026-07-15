//! Plaid HTTP transport (sync ureq, matching `simplefin`). Only transport
//! lives here — response JSON crosses into `outflow_core::plaid` for parsing,
//! keeping the domain mapping pure and fixture-testable.
//!
//! Error strings from non-2xx responses carry the Plaid error body verbatim so
//! callers can substring-match codes like `ITEM_LOGIN_REQUIRED` and
//! `TRANSACTIONS_SYNC_MUTATION_DURING_PAGINATION`.

use crate::secrets::read_secret_file;

pub struct PlaidConfig {
    pub client_id: String,
    pub secret: String,
    pub base_url: String,
}

impl PlaidConfig {
    /// Resolve from the environment: `OUTFLOW_PLAID_CLIENT_ID` (not secret),
    /// `OUTFLOW_PLAID_SECRET` or `OUTFLOW_PLAID_SECRET_FILE` (a 0600 file, the
    /// headless path), and `OUTFLOW_PLAID_ENV` = `sandbox` (default) or
    /// `production`. The secret never touches argv or the DB.
    pub fn from_env() -> Result<PlaidConfig, String> {
        let client_id = std::env::var("OUTFLOW_PLAID_CLIENT_ID")
            .ok()
            .filter(|s| !s.is_empty())
            .ok_or("OUTFLOW_PLAID_CLIENT_ID not set")?;
        let secret = match std::env::var("OUTFLOW_PLAID_SECRET") {
            Ok(s) if !s.is_empty() => s,
            _ => {
                let path = std::env::var("OUTFLOW_PLAID_SECRET_FILE")
                    .ok()
                    .filter(|s| !s.is_empty())
                    .ok_or("set OUTFLOW_PLAID_SECRET or OUTFLOW_PLAID_SECRET_FILE (0600 file)")?;
                read_secret_file(&path)?
            }
        };
        let base_url = match std::env::var("OUTFLOW_PLAID_ENV").as_deref() {
            Ok("production") => "https://production.plaid.com".to_string(),
            Ok("sandbox") | Err(_) => "https://sandbox.plaid.com".to_string(),
            Ok(other) => return Err(format!("OUTFLOW_PLAID_ENV must be sandbox|production, got {other:?}")),
        };
        Ok(PlaidConfig { client_id, secret, base_url })
    }
}

/// POST a JSON body with client credentials injected; return the raw response
/// body. Plaid returns errors as 4xx with a JSON body — surfaced verbatim in
/// the Err string for code matching.
fn post(cfg: &PlaidConfig, path: &str, mut body: serde_json::Value) -> Result<String, String> {
    body["client_id"] = serde_json::Value::String(cfg.client_id.clone());
    body["secret"] = serde_json::Value::String(cfg.secret.clone());
    let url = format!("{}{}", cfg.base_url, path);
    match ureq::post(&url)
        .set("content-type", "application/json")
        .send_string(&body.to_string())
    {
        Ok(resp) => resp
            .into_string()
            .map_err(|e| format!("plaid {path}: read body: {e}")),
        Err(ureq::Error::Status(code, resp)) => {
            let detail = resp.into_string().unwrap_or_default();
            Err(format!("plaid {path}: HTTP {code}: {detail}"))
        }
        Err(e) => Err(format!("plaid {path}: {e}")),
    }
}

/// Create a Link token. `redirect_uri` is required for OAuth institutions
/// (Chase/Amex/Capital One) and must exactly match an allowed redirect URI in
/// the Plaid dashboard. Pass an `access_token` for update (re-auth) mode — in
/// that mode `products` must be omitted. Returns the raw JSON (the frontend
/// consumes `link_token` directly).
pub fn link_token_create(
    cfg: &PlaidConfig,
    redirect_uri: Option<&str>,
    access_token: Option<&str>,
) -> Result<String, String> {
    let mut body = serde_json::json!({
        "client_name": "outflow",
        "user": { "client_user_id": "outflow-local" },
        "country_codes": ["US"],
        "language": "en",
    });
    match access_token {
        Some(token) => body["access_token"] = token.into(),
        None => body["products"] = serde_json::json!(["transactions"]),
    }
    if let Some(uri) = redirect_uri {
        body["redirect_uri"] = uri.into();
    }
    post(cfg, "/link/token/create", body)
}

/// Exchange a Link `public_token` for the durable `(access_token, item_id)`.
/// The access token is a secret — persist via `plaid_tokens`, never the DB.
pub fn exchange_public_token(
    cfg: &PlaidConfig,
    public_token: &str,
) -> Result<(String, String), String> {
    let raw = post(
        cfg,
        "/item/public_token/exchange",
        serde_json::json!({ "public_token": public_token }),
    )?;
    let v: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| format!("plaid exchange: bad JSON: {e}"))?;
    let access_token = v
        .get("access_token")
        .and_then(|t| t.as_str())
        .ok_or("plaid exchange: no access_token in response")?
        .to_string();
    let item_id = v
        .get("item_id")
        .and_then(|t| t.as_str())
        .ok_or("plaid exchange: no item_id in response")?
        .to_string();
    Ok((access_token, item_id))
}

/// Raw `/accounts/get` JSON for one item; parse with `core::plaid::parse_accounts_get`.
pub fn accounts_get(cfg: &PlaidConfig, access_token: &str) -> Result<String, String> {
    post(cfg, "/accounts/get", serde_json::json!({ "access_token": access_token }))
}

/// One `/transactions/sync` page (raw JSON); parse with
/// `core::plaid::parse_sync_page` and loop while `has_more`. On
/// `TRANSACTIONS_SYNC_MUTATION_DURING_PAGINATION` the caller restarts from the
/// item's stored cursor (which only advances on atomic batch commit).
pub fn transactions_sync_page(
    cfg: &PlaidConfig,
    access_token: &str,
    cursor: Option<&str>,
) -> Result<String, String> {
    let mut body = serde_json::json!({ "access_token": access_token, "count": 500 });
    if let Some(c) = cursor {
        body["cursor"] = c.into();
    }
    post(cfg, "/transactions/sync", body)
}

/// Unlink an item at Plaid (stops billing for it). Local history is kept.
pub fn item_remove(cfg: &PlaidConfig, access_token: &str) -> Result<(), String> {
    post(cfg, "/item/remove", serde_json::json!({ "access_token": access_token })).map(|_| ())
}
