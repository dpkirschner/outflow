//! Anthropic Messages API adapter for the core `Prompter` trait. Lives here (not
//! in `core`) so the domain keeps zero network deps. Endpoint and model are
//! env-configurable; auth is `ANTHROPIC_API_KEY`.

use outflow_core::{LlmError, Prompter};

pub struct AnthropicPrompter {
    url: String,
    model: String,
    api_key: String,
}

impl AnthropicPrompter {
    pub fn new(url: String, model: String, api_key: String) -> Self {
        AnthropicPrompter { url, model, api_key }
    }

    /// Build from the environment: `ANTHROPIC_API_KEY` (required),
    /// `OUTFLOW_LLM_URL` (default Anthropic Messages API), `OUTFLOW_LLM_MODEL`
    /// (default `claude-opus-4-8`).
    pub fn from_env() -> Result<Self, String> {
        let api_key =
            std::env::var("ANTHROPIC_API_KEY").map_err(|_| "ANTHROPIC_API_KEY not set".to_string())?;
        if api_key.is_empty() {
            return Err("ANTHROPIC_API_KEY is empty".into());
        }
        let url = std::env::var("OUTFLOW_LLM_URL")
            .unwrap_or_else(|_| "https://api.anthropic.com/v1/messages".into());
        let model = std::env::var("OUTFLOW_LLM_MODEL").unwrap_or_else(|_| "claude-opus-4-8".into());
        Ok(AnthropicPrompter::new(url, model, api_key))
    }
}

impl Prompter for AnthropicPrompter {
    fn complete(&self, system: &str, user: &str) -> Result<String, LlmError> {
        let body = serde_json::json!({
            "model": self.model,
            "max_tokens": 4096,
            "system": system,
            "messages": [{ "role": "user", "content": user }],
        })
        .to_string();
        let resp = ureq::post(&self.url)
            .set("x-api-key", &self.api_key)
            .set("anthropic-version", "2023-06-01")
            .set("content-type", "application/json")
            .send_string(&body)
            .map_err(|e| LlmError::Transport(format!("{e}")))?;
        let raw = resp
            .into_string()
            .map_err(|e| LlmError::Transport(format!("read body: {e}")))?;
        let value: serde_json::Value = serde_json::from_str(&raw)
            .map_err(|e| LlmError::Parse(format!("response not JSON: {e}")))?;
        // Concatenate all text blocks in content[].
        let text: String = value
            .get("content")
            .and_then(|c| c.as_array())
            .map(|blocks| {
                blocks
                    .iter()
                    .filter(|b| b.get("type").and_then(|t| t.as_str()) == Some("text"))
                    .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                    .collect::<Vec<_>>()
                    .join("")
            })
            .unwrap_or_default();
        if text.is_empty() {
            return Err(LlmError::Parse(format!("no text in response: {}", value)));
        }
        Ok(text)
    }
}
