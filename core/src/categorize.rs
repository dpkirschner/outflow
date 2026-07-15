use crate::model::Transaction;
use crate::subscriptions::normalize_payee;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchType {
    Exact,
    Contains,
}

impl MatchType {
    pub fn as_str(self) -> &'static str {
        match self {
            MatchType::Exact => "exact",
            MatchType::Contains => "contains",
        }
    }

    pub fn from_str(s: &str) -> Option<MatchType> {
        match s {
            "exact" => Some(MatchType::Exact),
            "contains" => Some(MatchType::Contains),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct CategoryRule {
    pub id: i64,
    pub match_type: MatchType,
    pub pattern: String,
    pub category: String,
}

pub trait Categorizer {
    fn categorize(&self, txn: &Transaction) -> Option<String>;
}

pub struct RuleSet {
    rules: Vec<CategoryRule>,
}

impl RuleSet {
    pub fn new(mut rules: Vec<CategoryRule>) -> Self {
        for r in &mut rules {
            r.pattern = r.pattern.to_lowercase();
        }
        RuleSet { rules }
    }

    pub fn match_rule(&self, merchant: &str) -> Option<&CategoryRule> {
        let m = normalize_payee(merchant);
        if m.is_empty() {
            return None;
        }
        for r in &self.rules {
            if r.match_type == MatchType::Exact && r.pattern == m {
                return Some(r);
            }
        }
        let mut best: Option<&CategoryRule> = None;
        for r in &self.rules {
            if r.match_type == MatchType::Contains
                && !r.pattern.is_empty()
                && m.contains(&r.pattern)
            {
                match best {
                    Some(b) if b.pattern.len() >= r.pattern.len() => {}
                    _ => best = Some(r),
                }
            }
        }
        best
    }
}

impl Categorizer for RuleSet {
    fn categorize(&self, txn: &Transaction) -> Option<String> {
        self.match_rule(txn.merchant()).map(|r| r.category.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::money::Money;

    fn rule(id: i64, mt: MatchType, pat: &str, cat: &str) -> CategoryRule {
        CategoryRule {
            id,
            match_type: mt,
            pattern: pat.into(),
            category: cat.into(),
        }
    }

    fn txn(merchant: &str) -> Transaction {
        Transaction {
            id: "x".into(),
            account_id: "a".into(),
            posted: 0,
            amount: Money::from_cents(-100),
            description: merchant.into(),
            payee: Some(merchant.into()),
            category: None,
            category_source: None,
            pending: false,
            transacted_at: None,
            flag: crate::model::TxnFlag::Spending,
            raw: "{}".into(),
            source: crate::model::default_source(),
        }
    }

    #[test]
    fn exact_beats_contains() {
        let rs = RuleSet::new(vec![
            rule(1, MatchType::Contains, "coffee", "Coffee"),
            rule(2, MatchType::Exact, "blue bottle coffee", "Treats"),
        ]);
        assert_eq!(rs.categorize(&txn("Blue Bottle Coffee")).as_deref(), Some("Treats"));
    }

    #[test]
    fn longest_contains_wins() {
        let rs = RuleSet::new(vec![
            rule(1, MatchType::Contains, "uber", "Transport"),
            rule(2, MatchType::Contains, "uber eats", "Dining"),
        ]);
        assert_eq!(rs.categorize(&txn("UBER EATS 88")).as_deref(), Some("Dining"));
        assert_eq!(rs.categorize(&txn("UBER TRIP")).as_deref(), Some("Transport"));
    }

    #[test]
    fn matches_through_processor_noise() {
        let rs = RuleSet::new(vec![rule(1, MatchType::Exact, "blue bottle", "Coffee")]);
        assert_eq!(rs.categorize(&txn("SQ *BLUE BOTTLE 4471")).as_deref(), Some("Coffee"));
    }

    #[test]
    fn no_match_returns_none() {
        let rs = RuleSet::new(vec![rule(1, MatchType::Exact, "netflix", "Streaming")]);
        assert!(rs.categorize(&txn("Whole Foods")).is_none());
    }
}
