//! Client for llama.cpp's OpenAI-compatible `/v1/chat/completions` endpoint.

use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::time::Duration;

pub struct LlmClient {
    base_url: String,
    http: reqwest::blocking::Client,
}

pub struct Completion {
    pub text: String,
    pub gen_tokens_per_s: f64,
}

impl LlmClient {
    pub fn new(base_url: &str) -> Result<Self> {
        Ok(Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            http: reqwest::blocking::Client::builder().timeout(Duration::from_secs(1800)).build()?,
        })
    }

    /// One user turn, thinking disabled, Qwen's recommended non-thinking sampling.
    pub fn complete(&self, prompt: &str, max_tokens: u32) -> Result<Completion> {
        let body = json!({
            "messages": [{"role": "user", "content": prompt}],
            "chat_template_kwargs": {"enable_thinking": false},
            "temperature": 0.7, "top_p": 0.8, "top_k": 20,
            "max_tokens": max_tokens,
        });
        let v: Value = self
            .http
            .post(format!("{}/v1/chat/completions", self.base_url))
            .json(&body)
            .send()
            .context("calling llama-server")?
            .error_for_status()?
            .json()?;
        let text = v["choices"][0]["message"]["content"]
            .as_str()
            .context("llama-server returned no content")?
            .trim()
            .to_string();
        Ok(Completion { text, gen_tokens_per_s: v["timings"]["predicted_per_second"].as_f64().unwrap_or(0.0) })
    }

    /// Like `complete`, but the output is constrained to `schema` (llama-server grammar from a
    /// JSON schema) and sampled cooler — for extraction, not prose (spec 005).
    pub fn complete_json(&self, prompt: &str, schema: &Value, max_tokens: u32) -> Result<Completion> {
        let body = json!({
            "messages": [{"role": "user", "content": prompt}],
            "chat_template_kwargs": {"enable_thinking": false},
            "temperature": 0.2, "top_p": 0.8, "top_k": 20,
            "max_tokens": max_tokens,
            "response_format": {"type": "json_schema", "json_schema": {"name": "notes", "schema": schema}},
        });
        let v: Value = self
            .http
            .post(format!("{}/v1/chat/completions", self.base_url))
            .json(&body)
            .send()
            .context("calling llama-server")?
            .error_for_status()?
            .json()?;
        let text = v["choices"][0]["message"]["content"].as_str().context("llama-server returned no content")?.trim().to_string();
        Ok(Completion { text, gen_tokens_per_s: v["timings"]["predicted_per_second"].as_f64().unwrap_or(0.0) })
    }
}
