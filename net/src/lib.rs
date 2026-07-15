//! Networked adapters for outflow: Plaid transport + token store, secret
//! resolution, and the Anthropic `Prompter`. Kept out of `core` so the domain
//! stays network-free (invariant #3), and shared by the CLI and the server so
//! both reach Plaid and the LLM through identical code.

pub mod anthropic;
pub mod plaid;
pub mod plaid_tokens;
pub mod secrets;

pub use anthropic::AnthropicPrompter;
pub use plaid::PlaidConfig;
