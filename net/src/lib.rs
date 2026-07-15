//! Networked adapters for outflow: SimpleFIN fetch/claim, secret resolution,
//! and the Anthropic `Prompter`. Kept out of `core` so the domain stays
//! network-free (invariant #3), and shared by the CLI and the Tauri GUI so both
//! reach SimpleFIN and the LLM through identical code.

pub mod anthropic;
pub mod plaid;
pub mod plaid_tokens;
pub mod secrets;
pub mod simplefin;

pub use anthropic::AnthropicPrompter;
pub use plaid::PlaidConfig;
pub use secrets::{access_url, persist_access_url, Persisted};
pub use simplefin::{claim_access_url, fetch};
