//! Plaid Link + item management routes. Linking is browser-driven: the SPA
//! requests a link token, runs Plaid Link, and posts the resulting
//! public_token back here for exchange. Access tokens land in the 0600 token
//! file (never the DB); non-secret item metadata lands in `plaid_items`.

use axum::extract::{Path, State};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use outflow_net::{plaid, plaid_tokens, PlaidConfig};
use serde::{Deserialize, Serialize};

use crate::state::{now_secs, with_store, ApiError, ApiResult, AppState};

#[derive(Deserialize)]
struct LinkTokenBody {
    /// Present = update (re-auth) mode for an existing item.
    item_id: Option<String>,
    /// Update mode only: also ask the user to grant Investments (holdings)
    /// access on an item linked before outflow requested it.
    #[serde(default)]
    consent_investments: bool,
}

/// Create a Link token. Passes the configured OAuth redirect URI when set —
/// required for Chase/Amex/Capital One, and it must exactly match a dashboard
/// allowed redirect URI. Returns Plaid's response verbatim (`link_token`, ...).
async fn link_token(
    State(state): State<AppState>,
    Json(body): Json<LinkTokenBody>,
) -> ApiResult<serde_json::Value> {
    let cfg = state.cfg.clone();
    let raw = tokio::task::spawn_blocking(move || {
        let pcfg = PlaidConfig::from_env()?;
        let access_token = match &body.item_id {
            Some(item_id) => Some(plaid_tokens::token_for(&cfg.plaid_tokens_file, item_id)?),
            None => None,
        };
        plaid::link_token_create(
            &pcfg,
            cfg.oauth_redirect.as_deref(),
            access_token.as_deref(),
            body.consent_investments,
        )
    })
    .await
    .map_err(|e| ApiError::internal(format!("blocking task: {e}")))?
    .map_err(ApiError::internal)?;
    serde_json::from_str(&raw)
        .map_err(|e| ApiError::internal(format!("plaid link_token: bad JSON: {e}")))
        .map(Json)
}

#[derive(Deserialize)]
struct ExchangeBody {
    public_token: String,
    /// Institution display name from Link's onSuccess metadata.
    institution: Option<String>,
}

#[derive(Serialize)]
struct ExchangeResult {
    item_id: String,
    institution: String,
    added: usize,
    updated: usize,
    /// Set when the link succeeded but its first sync didn't; the background
    /// sync retries it.
    sync_error: Option<String>,
}

fn exchange_result(
    item_id: String,
    institution: String,
    leg: crate::sync::LegReport,
) -> ExchangeResult {
    if let Some(e) = &leg.error {
        tracing::warn!(%item_id, %institution, error = %e, "first sync after link failed");
    }
    ExchangeResult {
        item_id,
        institution,
        added: leg.added,
        updated: leg.updated,
        sync_error: leg.error,
    }
}

/// Exchange the Link public_token, persist the access token (0600 file),
/// register the item, and run its first sync immediately so accounts and
/// history appear before the user leaves the Connections screen.
async fn exchange(
    State(state): State<AppState>,
    Json(body): Json<ExchangeBody>,
) -> ApiResult<ExchangeResult> {
    let cfg = state.cfg.clone();
    let store = state.store.clone();
    let institution = body
        .institution
        .clone()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Bank".to_string());

    let result = tokio::task::spawn_blocking(move || {
        let pcfg = PlaidConfig::from_env()?;
        let (access_token, item_id) = plaid::exchange_public_token(&pcfg, &body.public_token)?;
        plaid_tokens::save_token(&cfg.plaid_tokens_file, &item_id, &access_token)?;
        {
            let guard = store
                .lock()
                .map_err(|e| format!("store lock poisoned: {e}"))?;
            guard
                .upsert_plaid_item(&item_id, &institution, now_secs())
                .map_err(|e| e.to_string())?;
        }
        // First sync, immediately — reuse the engine's single-item runner.
        let item = outflow_core::PlaidItem {
            item_id: item_id.clone(),
            institution: institution.clone(),
            cursor: None,
            status: "ok".into(),
            created: now_secs(),
            last_synced: None,
            investments_consent: false,
        };
        // The link itself has succeeded at this point (token saved, item
        // registered), so a failed first sync must not turn into a 500 — the
        // background sync retries it. Report it alongside the success instead.
        let leg = crate::sync::sync_plaid_item(&store, &cfg, &pcfg, &item);
        Ok::<ExchangeResult, String>(exchange_result(item_id, institution, leg))
    })
    .await
    .map_err(|e| ApiError::internal(format!("blocking task: {e}")))?
    .map_err(ApiError::internal)?;
    Ok(Json(result))
}

async fn items(State(state): State<AppState>) -> ApiResult<Vec<outflow_core::PlaidItem>> {
    with_store(&state, |s| s.plaid_items().map_err(|e| e.to_string()))
        .await
        .map(Json)
}

/// Unlink an item: best-effort remove at Plaid (stops billing), drop the
/// stored token, delete the metadata row. Pulled history is kept — the local
/// archive outliving the connection is the point of this app.
async fn remove_item(State(state): State<AppState>, Path(item_id): Path<String>) -> ApiResult<()> {
    let cfg = state.cfg.clone();
    let store = state.store.clone();
    tokio::task::spawn_blocking(move || {
        if let (Ok(pcfg), Ok(token)) = (
            PlaidConfig::from_env(),
            plaid_tokens::token_for(&cfg.plaid_tokens_file, &item_id),
        ) {
            // Plaid-side removal is best-effort; a dead token shouldn't strand
            // the local unlink.
            let _ = plaid::item_remove(&pcfg, &token);
        }
        plaid_tokens::remove_token(&cfg.plaid_tokens_file, &item_id)?;
        let guard = store
            .lock()
            .map_err(|e| format!("store lock poisoned: {e}"))?;
        guard.delete_plaid_item(&item_id).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| ApiError::internal(format!("blocking task: {e}")))?
    .map_err(ApiError::internal)?;
    Ok(Json(()))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/plaid/link_token", post(link_token))
        .route("/plaid/exchange", post(exchange))
        .route("/plaid/items", get(items))
        .route("/plaid/items/{item_id}", delete(remove_item))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::LegReport;

    fn leg(error: Option<&str>) -> LegReport {
        LegReport {
            source: "plaid:Vanguard".into(),
            added: 3,
            updated: 1,
            error: error.map(Into::into),
        }
    }

    /// A first sync that fails after a successful link is reported in the
    /// body, not as an error status — the item is already registered and the
    /// background sync retries it.
    #[test]
    fn failed_first_sync_is_reported_not_raised() {
        let r = exchange_result("item1".into(), "Vanguard".into(), leg(Some("boom")));
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(v["item_id"], "item1");
        assert_eq!(v["sync_error"], "boom");
    }

    #[test]
    fn successful_first_sync_has_null_sync_error() {
        let r = exchange_result("item1".into(), "Vanguard".into(), leg(None));
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(v["added"], 3);
        assert_eq!(v["updated"], 1);
        // Present as null (the frontend type is `string | null`).
        assert!(v.as_object().unwrap().contains_key("sync_error"));
        assert!(v["sync_error"].is_null());
    }
}
