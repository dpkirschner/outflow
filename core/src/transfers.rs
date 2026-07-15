//! Checking→credit-card payment detection. Pairs an outflow on a bank
//! (checking/savings) account with an equal-magnitude inflow on a credit
//! account a few days apart, so both legs can be flagged `CardPayment` and
//! excluded from spend analytics (the card's underlying charges are the real
//! spending). Pure — the sync engine runs it post-ingest and the server
//! exposes accept/reject.

use crate::model::{Account, AccountKind, MatchConfidence, Transaction, TxnFlag};
use crate::subscriptions::normalize_payee;
use std::collections::{HashMap, HashSet};

const DAY: i64 = 86_400;

/// Payment-ish payee vocabulary, matched against the normalized merchant of
/// either leg. Deliberately conservative — a hit only raises confidence, never
/// gates detection.
const PAYMENT_LEXICON: &[&str] = &[
    "payment",
    "pymt",
    "pmt",
    "autopay",
    "epay",
    "e-pay",
    "thank you",
    "crcardpmt",
    "directpay",
    "cardmember",
    "bill pay",
];

pub struct MatchOptions {
    /// Max days between the two legs' behavioral dates (inclusive).
    pub max_day_gap: i64,
}

impl Default for MatchOptions {
    fn default() -> Self {
        MatchOptions { max_day_gap: 5 }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProposedMatch {
    pub bank_txn_id: String,
    pub card_txn_id: String,
    pub amount_cents: i64,
    pub day_gap: i64,
    pub confidence: MatchConfidence,
    pub reason: String,
}

fn has_payment_signal(t: &Transaction) -> bool {
    // Plaid classified it as a card payment / transfer at parse time.
    if t.flag == TxnFlag::CardPayment {
        return true;
    }
    if t.raw.contains("LOAN_PAYMENTS_CREDIT_CARD_PAYMENT") {
        return true;
    }
    let m = normalize_payee(t.merchant());
    PAYMENT_LEXICON.iter().any(|w| m.contains(w))
}

/// Detect candidate payment pairs. `already_decided` carries every
/// (bank_txn_id, card_txn_id) pair that has a `txn_matches` row in any status —
/// none of those are re-proposed, so rejections stick.
///
/// Confidence: `High` only when a leg carries a payment signal (lexicon or
/// Plaid category) AND the pairing is unambiguous (exactly one candidate on
/// each side for that amount/window). High-confidence pairs are safe to
/// auto-accept; everything else queues for review.
pub fn detect_card_payments(
    accounts: &[Account],
    txns: &[Transaction],
    already_decided: &[(String, String)],
    opts: &MatchOptions,
) -> Vec<ProposedMatch> {
    let mut bank_accounts: HashSet<&str> = HashSet::new();
    let mut card_accounts: HashSet<&str> = HashSet::new();
    for a in accounts {
        match a.kind {
            AccountKind::Checking | AccountKind::Savings => {
                bank_accounts.insert(a.id.as_str());
            }
            AccountKind::Credit => {
                card_accounts.insert(a.id.as_str());
            }
            AccountKind::Other => {}
        }
    }

    // Candidate legs. Pending rows are excluded: a Plaid pending id can be
    // superseded by a differently-id'd posted row, which would orphan a match.
    let eligible = |t: &Transaction| {
        !t.pending && matches!(t.flag, TxnFlag::Spending | TxnFlag::CardPayment)
    };
    let bank_legs: Vec<&Transaction> = txns
        .iter()
        .filter(|t| {
            bank_accounts.contains(t.account_id.as_str()) && t.amount.cents() < 0 && eligible(t)
        })
        .collect();
    let card_legs: Vec<&Transaction> = txns
        .iter()
        .filter(|t| {
            card_accounts.contains(t.account_id.as_str()) && t.amount.cents() > 0 && eligible(t)
        })
        .collect();

    // Index card legs by magnitude for pairing.
    let mut by_amount: HashMap<i64, Vec<&Transaction>> = HashMap::new();
    for c in &card_legs {
        by_amount.entry(c.amount.cents()).or_default().push(c);
    }

    let decided: HashSet<(&str, &str)> = already_decided
        .iter()
        .map(|(b, c)| (b.as_str(), c.as_str()))
        .collect();
    let window = opts.max_day_gap * DAY;
    let in_window =
        |a: &Transaction, b: &Transaction| (a.effective_date() - b.effective_date()).abs() <= window;

    let mut out = Vec::new();
    for bank in &bank_legs {
        let magnitude = -bank.amount.cents();
        let Some(cards) = by_amount.get(&magnitude) else {
            continue;
        };
        let partners: Vec<&&Transaction> =
            cards.iter().filter(|c| in_window(bank, c)).collect();
        for card in &partners {
            if bank.flag == TxnFlag::CardPayment && card.flag == TxnFlag::CardPayment {
                continue; // both legs already suppressed — nothing to do
            }
            if decided.contains(&(bank.id.as_str(), card.id.as_str())) {
                continue;
            }
            // Ambiguity check from the card side too: how many bank legs of
            // this magnitude sit within the window of this card leg?
            let rival_banks = bank_legs
                .iter()
                .filter(|b| -b.amount.cents() == magnitude && in_window(b, card))
                .count();
            let unambiguous = partners.len() == 1 && rival_banks == 1;
            let signal = has_payment_signal(bank) || has_payment_signal(card);
            let confidence = if unambiguous && signal {
                MatchConfidence::High
            } else {
                MatchConfidence::Medium
            };
            let day_gap = (bank.effective_date() - card.effective_date()).abs() / DAY;
            let mut reason = format!(
                "{} on both legs, {} day(s) apart",
                crate::money::Money::from_cents(magnitude).to_display(),
                day_gap
            );
            if signal {
                reason.push_str(", payment-like payee");
            }
            if !unambiguous {
                reason.push_str(", multiple candidates");
            }
            out.push(ProposedMatch {
                bank_txn_id: bank.id.clone(),
                card_txn_id: card.id.clone(),
                amount_cents: magnitude,
                day_gap,
                confidence,
                reason,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::default_source;
    use crate::money::Money;

    fn acct(id: &str, kind: AccountKind) -> Account {
        Account {
            id: id.into(),
            org: "Bank".into(),
            name: id.into(),
            kind,
            balance: Money::from_cents(0),
            currency: "USD".into(),
            last_synced: 0,
            source: default_source(),
        }
    }

    fn txn(id: &str, acct: &str, day: i64, cents: i64, merchant: &str) -> Transaction {
        Transaction {
            id: id.into(),
            account_id: acct.into(),
            posted: day * DAY,
            transacted_at: None,
            amount: Money::from_cents(cents),
            description: merchant.into(),
            payee: Some(merchant.into()),
            category: None,
            category_source: None,
            pending: false,
            flag: TxnFlag::Spending,
            raw: "{}".into(),
            source: default_source(),
        }
    }

    fn setup() -> Vec<Account> {
        vec![
            acct("checking", AccountKind::Checking),
            acct("card", AccountKind::Credit),
        ]
    }

    #[test]
    fn detects_exact_pair_with_payment_payee_as_high() {
        let accounts = setup();
        let txns = vec![
            txn("b1", "checking", 100, -50_000, "CHASE CREDIT CRD AUTOPAY"),
            txn("c1", "card", 102, 50_000, "Payment Thank You - Web"),
        ];
        let m = detect_card_payments(&accounts, &txns, &[], &MatchOptions::default());
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].bank_txn_id, "b1");
        assert_eq!(m[0].card_txn_id, "c1");
        assert_eq!(m[0].confidence, MatchConfidence::High);
        assert_eq!(m[0].day_gap, 2);
    }

    #[test]
    fn window_edge_five_days_in_six_out() {
        let accounts = setup();
        let inside = vec![
            txn("b1", "checking", 100, -1000, "autopay"),
            txn("c1", "card", 105, 1000, "payment"),
        ];
        assert_eq!(
            detect_card_payments(&accounts, &inside, &[], &MatchOptions::default()).len(),
            1
        );
        let outside = vec![
            txn("b1", "checking", 100, -1000, "autopay"),
            txn("c1", "card", 106, 1000, "payment"),
        ];
        assert!(detect_card_payments(&accounts, &outside, &[], &MatchOptions::default()).is_empty());
    }

    #[test]
    fn ambiguous_same_amount_stays_medium() {
        let accounts = setup();
        // Two card credits of the same amount inside the window → ambiguous.
        let txns = vec![
            txn("b1", "checking", 100, -25_000, "autopay"),
            txn("c1", "card", 101, 25_000, "payment"),
            txn("c2", "card", 103, 25_000, "payment"),
        ];
        let m = detect_card_payments(&accounts, &txns, &[], &MatchOptions::default());
        assert_eq!(m.len(), 2);
        assert!(m.iter().all(|p| p.confidence == MatchConfidence::Medium));
    }

    #[test]
    fn no_signal_stays_medium_even_when_unambiguous() {
        let accounts = setup();
        let txns = vec![
            txn("b1", "checking", 100, -7_700, "Zelle to landlord"),
            txn("c1", "card", 100, 7_700, "misc credit"),
        ];
        let m = detect_card_payments(&accounts, &txns, &[], &MatchOptions::default());
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].confidence, MatchConfidence::Medium);
    }

    #[test]
    fn decided_pairs_are_never_reproposed() {
        let accounts = setup();
        let txns = vec![
            txn("b1", "checking", 100, -1000, "autopay"),
            txn("c1", "card", 100, 1000, "payment"),
        ];
        let decided = vec![("b1".to_string(), "c1".to_string())];
        assert!(detect_card_payments(&accounts, &txns, &decided, &MatchOptions::default()).is_empty());
    }

    #[test]
    fn plaid_seeded_leg_pairs_the_other_and_counts_as_signal() {
        let accounts = setup();
        let mut bank = txn("b1", "checking", 100, -30_000, "ACH WITHDRAWAL");
        bank.flag = TxnFlag::CardPayment; // seeded by Plaid PFC at parse time
        let card = txn("c1", "card", 101, 30_000, "ONLINE CREDIT");
        let m = detect_card_payments(&accounts, &[bank, card], &[], &MatchOptions::default());
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].confidence, MatchConfidence::High);
    }

    #[test]
    fn both_legs_already_flagged_is_skipped() {
        let accounts = setup();
        let mut bank = txn("b1", "checking", 100, -30_000, "autopay");
        bank.flag = TxnFlag::CardPayment;
        let mut card = txn("c1", "card", 101, 30_000, "payment");
        card.flag = TxnFlag::CardPayment;
        assert!(detect_card_payments(&accounts, &[bank, card], &[], &MatchOptions::default())
            .is_empty());
    }

    #[test]
    fn ignores_pending_transfers_and_wrong_directions() {
        let accounts = setup();
        let mut pending = txn("b1", "checking", 100, -1000, "autopay");
        pending.pending = true;
        let mut transfer = txn("b2", "checking", 100, -1000, "autopay");
        transfer.flag = TxnFlag::Transfer;
        let txns = vec![
            pending,
            transfer,
            // Wrong directions: card charge (negative) and bank inflow.
            txn("c1", "card", 100, 1000, "payment"),
            txn("c2", "card", 100, -1000, "purchase"),
            txn("b3", "checking", 100, 1000, "deposit"),
        ];
        assert!(detect_card_payments(&accounts, &txns, &[], &MatchOptions::default()).is_empty());
    }
}
