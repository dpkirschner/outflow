//! Card-payment match review: list detected pairs (enriched with both
//! transactions for display), accept (flag both legs `CardPayment`), reject
//! (and, when undoing an accept, restore both legs to `Spending`).

use axum::extract::{Path, Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use outflow_core::{MatchStatus, Transaction, TxnFlag, TxnMatch};
use serde::{Deserialize, Serialize};

use crate::state::{with_store, ApiError, ApiResult, AppState};

#[derive(Serialize)]
pub struct MatchView {
    #[serde(flatten)]
    pub m: TxnMatch,
    pub bank: Option<Transaction>,
    pub card: Option<Transaction>,
}

#[derive(Deserialize)]
struct StatusParam {
    status: Option<String>,
}

async fn list(
    State(state): State<AppState>,
    Query(p): Query<StatusParam>,
) -> ApiResult<Vec<MatchView>> {
    let status = match p.status.as_deref() {
        None => None,
        Some(s) => Some(
            MatchStatus::from_str(&s.to_lowercase())
                .ok_or_else(|| ApiError::bad_request(format!("bad status {s:?}")))?,
        ),
    };
    with_store(&state, move |store| {
        let ms = store.matches(status).map_err(|e| e.to_string())?;
        ms.into_iter()
            .map(|m| {
                let bank = store.transaction(&m.bank_txn_id).map_err(|e| e.to_string())?;
                let card = store.transaction(&m.card_txn_id).map_err(|e| e.to_string())?;
                Ok(MatchView { m, bank, card })
            })
            .collect::<Result<Vec<_>, String>>()
    })
    .await
    .map(Json)
}

async fn accept(State(state): State<AppState>, Path(id): Path<i64>) -> ApiResult<()> {
    with_store(&state, move |store| {
        let m = store
            .match_by_id(id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("no match {id}"))?;
        store
            .set_flag(&m.bank_txn_id, TxnFlag::CardPayment, false)
            .map_err(|e| e.to_string())?;
        store
            .set_flag(&m.card_txn_id, TxnFlag::CardPayment, false)
            .map_err(|e| e.to_string())?;
        store
            .set_match_status(id, MatchStatus::Accepted)
            .map_err(|e| e.to_string())?;
        store
            .reject_conflicting_proposals(id, &m.bank_txn_id, &m.card_txn_id)
            .map_err(|e| e.to_string())?;
        Ok(())
    })
    .await
    .map(Json)
}

async fn reject(State(state): State<AppState>, Path(id): Path<i64>) -> ApiResult<()> {
    with_store(&state, move |store| {
        let m = store
            .match_by_id(id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("no match {id}"))?;
        // Undoing an accept restores both legs to Spending.
        if m.status == MatchStatus::Accepted {
            store
                .set_flag(&m.bank_txn_id, TxnFlag::Spending, false)
                .map_err(|e| e.to_string())?;
            store
                .set_flag(&m.card_txn_id, TxnFlag::Spending, false)
                .map_err(|e| e.to_string())?;
        }
        store
            .set_match_status(id, MatchStatus::Rejected)
            .map_err(|e| e.to_string())
    })
    .await
    .map(Json)
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/matches", get(list))
        .route("/matches/{id}/accept", post(accept))
        .route("/matches/{id}/reject", post(reject))
}
