//! The LLM interface. FROZEN after step 1: do not edit.
//!
//! The prototype makes one kind of call: system + user in, text out,
//! non-streaming. `openai::OpenAiClient` is the real implementation;
//! `MockClient` serves tests and offline UI work.

use secrecy::SecretString;
use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;

/// A boxed, sendable future, so `LlmClient` can be used as a trait object.
pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatRequest {
    pub system: String,
    pub user: String,
}

impl ChatRequest {
    /// The tiny request the Settings "Test" button sends.
    pub fn test() -> Self {
        ChatRequest {
            system: "You are a connection test. Reply with the single word OK.".into(),
            user: "Reply with OK.".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatResponse {
    pub text: String,
    /// The model name the endpoint reports, or the requested one if absent.
    pub model: String,
}

/// The cases the UI must explain differently.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LlmError {
    #[error("no endpoint is configured")]
    NotConfigured,
    #[error("the API key is locked; unlock it in Settings")]
    Locked,
    #[error("could not connect: {0}")]
    Connection(String),
    #[error("the endpoint rejected the API key")]
    Unauthorized,
    #[error("model not found: {0}")]
    ModelNotFound(String),
    #[error("rate limited by the endpoint")]
    RateLimited,
    #[error("the request timed out")]
    Timeout,
    #[error("the model refused the request")]
    Refused,
    #[error("the response was cut off (max tokens)")]
    Truncated,
    #[error("HTTP {status}: {body}")]
    Http { status: u16, body: String },
    #[error("malformed response: {0}")]
    Malformed(String),
}

/// What a client needs to reach an endpoint. Built by the app from
/// `LlmSettings` plus the unlocked key.
#[derive(Debug, Clone)]
pub struct LlmConfig {
    /// e.g. `https://api.openai.com/v1`; `/chat/completions` is appended.
    pub base_url: String,
    pub model: String,
    /// `None` for endpoints that need no key (Ollama, LM Studio).
    pub api_key: Option<SecretString>,
}

impl LlmConfig {
    /// The host part of `base_url`, for `Report::endpoint_host`.
    /// Never includes credentials, path or query.
    pub fn endpoint_host(&self) -> Option<String> {
        let rest = self
            .base_url
            .split_once("://")
            .map_or(self.base_url.as_str(), |(_, r)| r);
        let authority = rest.split(['/', '?', '#']).next()?;
        let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
        (!host.is_empty()).then(|| host.to_string())
    }
}

pub trait LlmClient: Send + Sync {
    /// The model name requests are sent with.
    fn model(&self) -> &str;

    fn chat(&self, req: ChatRequest) -> BoxFuture<Result<ChatResponse, LlmError>>;
}

/// A client that returns canned replies and records requests. For tests.
#[derive(Debug, Default)]
pub struct MockClient {
    /// Returned in order; when exhausted, every call returns `default_reply`.
    pub replies: Mutex<Vec<Result<String, LlmError>>>,
    pub default_reply: String,
    /// Every request received, in order.
    pub requests: Mutex<Vec<ChatRequest>>,
}

impl MockClient {
    pub fn new(default_reply: impl Into<String>) -> Self {
        MockClient {
            default_reply: default_reply.into(),
            ..Default::default()
        }
    }

    /// Queues a reply to return before falling back to `default_reply`.
    pub fn push_reply(&self, reply: Result<String, LlmError>) {
        self.replies.lock().expect("mock lock").push(reply);
    }
}

impl LlmClient for MockClient {
    fn model(&self) -> &str {
        "mock"
    }

    fn chat(&self, req: ChatRequest) -> BoxFuture<Result<ChatResponse, LlmError>> {
        self.requests.lock().expect("mock lock").push(req);
        let next = {
            let mut replies = self.replies.lock().expect("mock lock");
            if replies.is_empty() {
                None
            } else {
                Some(replies.remove(0))
            }
        };
        let result = next.unwrap_or_else(|| Ok(self.default_reply.clone()));
        Box::pin(async move {
            result.map(|text| ChatResponse {
                text,
                model: "mock".into(),
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_host_strips_scheme_credentials_and_path() {
        let cfg = |url: &str| LlmConfig {
            base_url: url.into(),
            model: "m".into(),
            api_key: None,
        };
        assert_eq!(
            cfg("https://api.openai.com/v1").endpoint_host().as_deref(),
            Some("api.openai.com")
        );
        assert_eq!(
            cfg("http://user:pw@localhost:11434/v1")
                .endpoint_host()
                .as_deref(),
            Some("localhost:11434")
        );
        assert_eq!(cfg("").endpoint_host(), None);
    }
}
