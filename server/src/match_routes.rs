//! Card-payment match review: list detected pairs (enriched with both
//! transactions for display), accept (flag both legs `CardPayment`), reject
//! (and, when undoing an accept, restore both legs to `Spending`), and
//! unaccept (an accepted pair back to review, legs restored).

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use outflow_core::{MatchStatus, Transaction, TxnFlag, TxnMatch, Unaccept};
use serde::{Deserialize, Serialize};

use crate::state::{with_store, ApiError, ApiResult, AppState};

/// A card-payment pair enriched with both legs for display: the `TxnMatch`
/// fields flattened in, plus each transaction (`null` if it has since gone
/// from the archive).
#[derive(Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct MatchView {
    #[serde(flatten)]
    pub m: TxnMatch,
    /// The outflow leg on checking/savings.
    pub bank: Option<Transaction>,
    /// The offsetting inflow on the credit card.
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
        let hidden: std::collections::HashSet<String> = store
            .hidden_account_ids()
            .map_err(|e| e.to_string())?
            .into_iter()
            .collect();
        ms.into_iter()
            .map(|m| {
                let bank = store
                    .transaction(&m.bank_txn_id)
                    .map_err(|e| e.to_string())?;
                let card = store
                    .transaction(&m.card_txn_id)
                    .map_err(|e| e.to_string())?;
                Ok(MatchView { m, bank, card })
            })
            .collect::<Result<Vec<_>, String>>()
            // A pair with a leg on a hidden account stays recorded (and keeps
            // suppressing) but isn't shown for review.
            .map(|views| {
                views
                    .into_iter()
                    .filter(|v| {
                        ![&v.bank, &v.card]
                            .iter()
                            .any(|t| t.as_ref().is_some_and(|t| hidden.contains(&t.account_id)))
                    })
                    .collect()
            })
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
        // Undoing an accept drops both legs' overrides: each goes back to
        // the flag Plaid's category gives.
        if m.status == MatchStatus::Accepted {
            store
                .clear_flag(&m.bank_txn_id)
                .map_err(|e| e.to_string())?;
            store
                .clear_flag(&m.card_txn_id)
                .map_err(|e| e.to_string())?;
        }
        store
            .set_match_status(id, MatchStatus::Rejected)
            .map_err(|e| e.to_string())
    })
    .await
    .map(Json)
}

/// Undo an accept: the pair goes back to `proposed` (review) and each leg's
/// flag override is dropped, unless another accepted pair still uses it
/// (`Store::unaccept_match`). 404 for an unknown id, 409 when the pair isn't
/// accepted.
async fn unaccept(State(state): State<AppState>, Path(id): Path<i64>) -> ApiResult<()> {
    let done = with_store(&state, move |store| {
        store.unaccept_match(id).map_err(|e| e.to_string())
    })
    .await?;
    match done {
        Unaccept::Done => Ok(Json(())),
        Unaccept::NotFound => Err(ApiError(StatusCode::NOT_FOUND, format!("no match {id}"))),
        Unaccept::NotAccepted(s) => Err(ApiError(
            StatusCode::CONFLICT,
            format!("match {id} is {}, not accepted", s.as_str()),
        )),
    }
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/matches", get(list))
        .route("/matches/{id}/accept", post(accept))
        .route("/matches/{id}/reject", post(reject))
        .route("/matches/{id}/unaccept", post(unaccept))
}
