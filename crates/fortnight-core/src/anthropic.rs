//! Anthropic Messages client. Owner: agent D (round 2).
//!
//! `POST {base_url}/v1/messages` with `x-api-key` and `anthropic-version`
//! headers (PLAN.md sections 2.3 and 7.2). STUB: every call fails until
//! agent D implements it.

use crate::llm::{BoxFuture, ChatRequest, ChatResponse, LlmClient, LlmConfig, LlmError};
use std::time::Duration;

pub struct AnthropicClient {
    config: LlmConfig,
    timeout: Duration,
    backoff: Duration,
}

impl AnthropicClient {
    /// Errors with `NotConfigured` if base URL or model is empty.
    pub fn new(config: LlmConfig) -> Result<Self, LlmError> {
        if config.base_url.trim().is_empty() || config.model.trim().is_empty() {
            return Err(LlmError::NotConfigured);
        }
        Ok(AnthropicClient {
            config,
            timeout: Duration::from_secs(120),
            backoff: Duration::from_millis(500),
        })
    }

    /// Total time allowed for each attempt (default 120 s).
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Delay before the first retry; doubled before each later one
    /// (default 500 ms).
    pub fn with_backoff(mut self, backoff: Duration) -> Self {
        self.backoff = backoff;
        self
    }
}

impl LlmClient for AnthropicClient {
    fn model(&self) -> &str {
        &self.config.model
    }

    fn chat(&self, _req: ChatRequest) -> BoxFuture<Result<ChatResponse, LlmError>> {
        let _ = (self.timeout, self.backoff);
        Box::pin(async {
            Err(LlmError::Malformed(
                "the Anthropic client is not implemented yet".into(),
            ))
        })
    }
}
