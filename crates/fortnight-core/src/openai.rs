//! OpenAI-compatible Chat Completions client. Owner: agent C.
//!
//! `POST {base_url}/chat/completions` with `Authorization: Bearer <key>`
//! (omitted when no key). Sends only `model` and `messages` (plus
//! `stream: false`): no temperature or other sampling parameters.
//! Maps HTTP and body errors onto `LlmError`. Retries up to three attempts
//! with backoff on connection errors, 429 and 5xx; never on other 4xx.

use crate::llm::{BoxFuture, ChatRequest, ChatResponse, LlmClient, LlmConfig, LlmError};

pub struct OpenAiClient {
    _config: LlmConfig,
}

impl OpenAiClient {
    /// Errors with `NotConfigured` if base URL or model is empty.
    pub fn new(_config: LlmConfig) -> Result<Self, LlmError> {
        todo!("agent C")
    }
}

impl LlmClient for OpenAiClient {
    fn model(&self) -> &str {
        todo!("agent C")
    }

    fn chat(&self, _req: ChatRequest) -> BoxFuture<Result<ChatResponse, LlmError>> {
        todo!("agent C")
    }
}
