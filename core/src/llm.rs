//! Model-backed categorizer for the tail of merchants the rule pass can't match.
//!
//! This module is pure and network-free: it builds the prompt and parses /
//! validates the model's reply against a fixed category vocabulary. The actual
//! HTTP call lives behind the `Prompter` trait, implemented by the CLI (so
//! `core` keeps zero network deps — invariant #3). Merchants are matched on the
//! same `subscriptions::normalize_payee` key used everywhere else (invariant #4).

use crate::money::Money;
use std::collections::HashMap;

#[derive(Debug, PartialEq)]
pub enum LlmError {
    /// The transport (network, auth, HTTP) failed.
    Transport(String),
    /// The model's reply couldn't be parsed into the expected shape.
    Parse(String),
}

/// The transport boundary. Given a system prompt and a user prompt, return the
/// model's text output. Implemented by the CLI's Anthropic adapter; a fake impl
/// drives the unit tests here with no network.
pub trait Prompter {
    fn complete(&self, system: &str, user: &str) -> Result<String, LlmError>;
}

/// One merchant to classify, with a representative outflow magnitude (cents,
/// positive) for context. The `merchant` is the normalized payee key.
#[derive(Debug, Clone, PartialEq)]
pub struct MerchantSample {
    pub merchant: String,
    pub amount_cents: i64,
}

/// A validated category assignment for a merchant. `category` is guaranteed to
/// be one of the configured vocabulary entries (canonical casing).
#[derive(Debug, Clone, PartialEq)]
pub struct Suggestion {
    pub merchant: String,
    pub category: String,
}

pub struct LlmCategorizer<P: Prompter> {
    prompter: P,
    categories: Vec<String>,
    max_batch: usize,
}

impl<P: Prompter> LlmCategorizer<P> {
    pub fn new(prompter: P, categories: Vec<String>) -> Self {
        LlmCategorizer {
            prompter,
            categories,
            max_batch: 40,
        }
    }

    pub fn with_batch_size(mut self, n: usize) -> Self {
        self.max_batch = n.max(1);
        self
    }

    pub fn system_prompt(&self) -> String {
        let list = self
            .categories
            .iter()
            .map(|c| format!("- {c}"))
            .collect::<Vec<_>>()
            .join("\n");
        format!(
            "You categorize bank-transaction merchants for a personal spending \
analyzer. Assign each merchant exactly one category from this fixed list:\n\
{list}\n\n\
Respond with ONLY a JSON object mapping each merchant string (copied verbatim, \
lowercase, as given) to one category from the list. Use the category name \
exactly as written above. If you are not confident, use \"Uncategorized\". Do \
not add commentary, code fences, or any text outside the JSON object."
        )
    }

    fn user_prompt(&self, batch: &[MerchantSample]) -> String {
        let mut s = String::from("Categorize these merchants:\n");
        for m in batch {
            let amt = Money::from_cents(m.amount_cents.abs()).to_display();
            s.push_str(&format!("- \"{}\" (typical ${amt})\n", m.merchant));
        }
        s
    }

    /// Classify a batch of merchants. Chunks internally, calls the prompter per
    /// chunk, and returns only assignments whose category is in the vocabulary.
    /// Merchants the model skips or maps outside the vocabulary are dropped
    /// (they simply stay uncategorized) — never guessed at.
    pub fn suggest(&self, merchants: &[MerchantSample]) -> Result<Vec<Suggestion>, LlmError> {
        let mut out = Vec::new();
        for chunk in merchants.chunks(self.max_batch) {
            let text = self
                .prompter
                .complete(&self.system_prompt(), &self.user_prompt(chunk))?;
            let map = parse_map(&text)?;
            for sample in chunk {
                if let Some(raw) = map.get(&sample.merchant) {
                    if let Some(canon) = self.canonical(raw) {
                        out.push(Suggestion {
                            merchant: sample.merchant.clone(),
                            category: canon,
                        });
                    }
                }
            }
        }
        Ok(out)
    }

    /// Map a model-returned category string to its canonical vocabulary entry,
    /// case-insensitively. Returns `None` for anything not in the vocabulary
    /// (including the sentinel "Uncategorized").
    fn canonical(&self, cat: &str) -> Option<String> {
        let c = cat.trim();
        self.categories
            .iter()
            .find(|v| v.eq_ignore_ascii_case(c))
            .cloned()
    }
}

/// Pull the JSON object out of a model reply and parse it to a merchant→category
/// map. Tolerates surrounding prose or code fences by slicing the first `{` to
/// the last `}` (category/merchant strings never contain braces).
fn parse_map(text: &str) -> Result<HashMap<String, String>, LlmError> {
    let start = text
        .find('{')
        .ok_or_else(|| LlmError::Parse("no JSON object in response".into()))?;
    let end = text
        .rfind('}')
        .ok_or_else(|| LlmError::Parse("no JSON object in response".into()))?;
    if end < start {
        return Err(LlmError::Parse("malformed JSON braces".into()));
    }
    serde_json::from_str::<HashMap<String, String>>(&text[start..=end])
        .map_err(|e| LlmError::Parse(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// Fake prompter: records the prompts it received and returns a canned reply.
    struct Fake {
        reply: String,
        seen: RefCell<Vec<(String, String)>>,
    }

    impl Fake {
        fn new(reply: &str) -> Self {
            Fake {
                reply: reply.into(),
                seen: RefCell::new(Vec::new()),
            }
        }
    }

    impl Prompter for &Fake {
        fn complete(&self, system: &str, user: &str) -> Result<String, LlmError> {
            self.seen
                .borrow_mut()
                .push((system.to_string(), user.to_string()));
            Ok(self.reply.clone())
        }
    }

    fn vocab() -> Vec<String> {
        vec!["Groceries".into(), "Dining".into(), "Streaming".into()]
    }

    fn samples(names: &[&str]) -> Vec<MerchantSample> {
        names
            .iter()
            .map(|n| MerchantSample {
                merchant: (*n).into(),
                amount_cents: -1200,
            })
            .collect()
    }

    #[test]
    fn maps_merchants_to_vocabulary() {
        let fake = Fake::new(r#"{"whole foods": "Groceries", "netflix com": "Streaming"}"#);
        let cat = LlmCategorizer::new(&fake, vocab());
        let out = cat.suggest(&samples(&["whole foods", "netflix com"])).unwrap();
        assert_eq!(
            out,
            vec![
                Suggestion { merchant: "whole foods".into(), category: "Groceries".into() },
                Suggestion { merchant: "netflix com".into(), category: "Streaming".into() },
            ]
        );
    }

    #[test]
    fn canonicalizes_case_and_drops_out_of_vocab() {
        // Model returns lowercased vocab, an off-list category, and the sentinel.
        let fake = Fake::new(
            r#"{"whole foods": "groceries", "acme corp": "Payroll", "mystery": "Uncategorized"}"#,
        );
        let cat = LlmCategorizer::new(&fake, vocab());
        let out = cat
            .suggest(&samples(&["whole foods", "acme corp", "mystery"]))
            .unwrap();
        // "groceries" canonicalizes; the other two are not in the vocabulary → dropped.
        assert_eq!(
            out,
            vec![Suggestion { merchant: "whole foods".into(), category: "Groceries".into() }]
        );
    }

    #[test]
    fn tolerates_code_fences_and_prose() {
        let fake = Fake::new("Here you go:\n```json\n{\"blue bottle\": \"Dining\"}\n```\n");
        let cat = LlmCategorizer::new(&fake, vocab());
        let out = cat.suggest(&samples(&["blue bottle"])).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].category, "Dining");
    }

    #[test]
    fn chunks_into_multiple_calls() {
        let fake = Fake::new(r#"{"a": "Dining", "b": "Dining", "c": "Dining"}"#);
        let cat = LlmCategorizer::new(&fake, vocab()).with_batch_size(2);
        let out = cat.suggest(&samples(&["a", "b", "c"])).unwrap();
        // 3 merchants, batch size 2 → 2 prompter calls.
        assert_eq!(fake.seen.borrow().len(), 2);
        assert_eq!(out.len(), 3);
    }

    #[test]
    fn unparseable_reply_is_error() {
        let fake = Fake::new("I cannot help with that.");
        let cat = LlmCategorizer::new(&fake, vocab());
        assert!(matches!(
            cat.suggest(&samples(&["x"])),
            Err(LlmError::Parse(_))
        ));
    }

    #[test]
    fn system_prompt_lists_vocabulary() {
        let fake = Fake::new("{}");
        let cat = LlmCategorizer::new(&fake, vocab());
        let sys = cat.system_prompt();
        assert!(sys.contains("- Groceries"));
        assert!(sys.contains("- Streaming"));
    }
}
