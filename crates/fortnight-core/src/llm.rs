//! The LLM interface. FROZEN (round 2): do not edit.
//!
//! One kind of call: system + user in, text out, either whole (`chat`) or
//! streamed (`chat_stream`). `openai::OpenAiClient` and
//! `anthropic::AnthropicClient` are the real implementations; `connect`
//! picks one by protocol. `MockClient` serves tests and offline UI work.

use crate::domain::{LlmSettings, Protocol};
use futures_util::{Stream, StreamExt};
use secrecy::SecretString;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

/// A boxed, sendable future, so `LlmClient` can be used as a trait object.
pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

/// A boxed, sendable stream, for `LlmClient::chat_stream`.
pub type BoxStream<T> = Pin<Box<dyn Stream<Item = T> + Send + 'static>>;

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

/// One item of a streamed reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatEvent {
    /// The next piece of text, to append.
    Delta(String),
    /// The reply finished normally. Always the last item of a successful
    /// stream. `model` is the model the endpoint reports, or the requested
    /// one if absent.
    Done { model: String },
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
    /// e.g. `https://api.openai.com/v1`. The client appends its path.
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

    /// The whole reply at once.
    fn chat(&self, req: ChatRequest) -> BoxFuture<Result<ChatResponse, LlmError>>;

    /// The reply as it is generated: zero or more `Delta`s, then exactly one
    /// `Done` on success. On failure the stream yields one `Err` and ends;
    /// text already yielded is then incomplete and must be discarded.
    /// A stream that ends without `Done` or `Err` is a bug in the client.
    ///
    /// Default: calls `chat` and yields its text as a single `Delta`.
    fn chat_stream(&self, req: ChatRequest) -> BoxStream<Result<ChatEvent, LlmError>> {
        let reply = self.chat(req);
        Box::pin(futures_util::stream::once(reply).flat_map(|result| {
            let items = match result {
                Ok(resp) => vec![
                    Ok(ChatEvent::Delta(resp.text)),
                    Ok(ChatEvent::Done { model: resp.model }),
                ],
                Err(e) => vec![Err(e)],
            };
            futures_util::stream::iter(items)
        }))
    }

    /// Model ids the endpoint offers, for the Settings model list. An empty
    /// list means the endpoint can't say; the UI then falls back to free
    /// text. Default: an empty list.
    fn list_models(&self) -> BoxFuture<Result<Vec<String>, LlmError>> {
        Box::pin(async { Ok(Vec::new()) })
    }
}

/// Collects a stream into a whole reply: the joined deltas and the model
/// from `Done`. A stream that ends without `Done` is `Truncated`.
pub async fn collect_stream(
    mut stream: BoxStream<Result<ChatEvent, LlmError>>,
) -> Result<ChatResponse, LlmError> {
    let mut text = String::new();
    while let Some(item) = stream.next().await {
        match item? {
            ChatEvent::Delta(d) => text.push_str(&d),
            ChatEvent::Done { model } => return Ok(ChatResponse { text, model }),
        }
    }
    Err(LlmError::Truncated)
}

/// The client for `protocol`. Errors with `NotConfigured` if the base URL
/// or model is empty.
pub fn connect(protocol: Protocol, config: LlmConfig) -> Result<Arc<dyn LlmClient>, LlmError> {
    Ok(match protocol {
        Protocol::OpenAi => Arc::new(crate::openai::OpenAiClient::new(config)?),
        Protocol::Anthropic => Arc::new(crate::anthropic::AnthropicClient::new(config)?),
    })
}

/// Environment overrides for one process (PLAN.md section 7.1), applied on
/// top of saved settings: `FORTNIGHT_LLM_PROTOCOL`, `FORTNIGHT_LLM_BASE_URL`,
/// `FORTNIGHT_LLM_MODEL`. Empty or unparseable values are ignored.
pub fn with_env_overrides(llm: &LlmSettings) -> LlmSettings {
    with_overrides(llm, |name| std::env::var(name).ok())
}

/// `with_env_overrides` with the environment supplied, for tests.
pub fn with_overrides(llm: &LlmSettings, var: impl Fn(&str) -> Option<String>) -> LlmSettings {
    let mut out = llm.clone();
    let get = |name: &str| var(name).filter(|v| !v.trim().is_empty());
    if let Some(p) = get("FORTNIGHT_LLM_PROTOCOL").and_then(|v| Protocol::parse(&v)) {
        out.protocol = p;
    }
    if let Some(url) = get("FORTNIGHT_LLM_BASE_URL") {
        out.base_url = url.trim().to_string();
    }
    if let Some(model) = get("FORTNIGHT_LLM_MODEL") {
        out.model = model.trim().to_string();
    }
    out
}

/// `FORTNIGHT_LLM_API_KEY`, if set and non-empty: a key for this process
/// only, never persisted. Takes precedence over the key file.
pub fn env_api_key() -> Option<SecretString> {
    std::env::var("FORTNIGHT_LLM_API_KEY")
        .ok()
        .filter(|k| !k.trim().is_empty())
        .map(|k| SecretString::from(k.trim().to_string()))
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

    /// Like `chat`, but yields the reply in word-sized pieces.
    fn chat_stream(&self, req: ChatRequest) -> BoxStream<Result<ChatEvent, LlmError>> {
        let reply = self.chat(req);
        Box::pin(futures_util::stream::once(reply).flat_map(|result| {
            let items = match result {
                Ok(resp) => {
                    let mut items: Vec<_> = resp
                        .text
                        .split_inclusive(' ')
                        .map(|w| Ok(ChatEvent::Delta(w.to_string())))
                        .collect();
                    items.push(Ok(ChatEvent::Done { model: resp.model }));
                    items
                }
                Err(e) => vec![Err(e)],
            };
            futures_util::stream::iter(items)
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn mock_stream_collects_to_the_reply() {
        let mock = MockClient::new("one two three");
        let reply = collect_stream(mock.chat_stream(ChatRequest::test()))
            .await
            .unwrap();
        assert_eq!(reply.text, "one two three");
        assert_eq!(reply.model, "mock");
        mock.push_reply(Err(LlmError::RateLimited));
        assert_eq!(
            collect_stream(mock.chat_stream(ChatRequest::test())).await,
            Err(LlmError::RateLimited)
        );
    }

    #[test]
    fn overrides_apply_non_empty_values() {
        let base = LlmSettings {
            protocol: Protocol::OpenAi,
            base_url: "http://saved/v1".into(),
            model: "saved".into(),
            keyless: false,
        };
        let out = with_overrides(&base, |name| match name {
            "FORTNIGHT_LLM_PROTOCOL" => Some("Anthropic".into()),
            "FORTNIGHT_LLM_MODEL" => Some("  ".into()),
            _ => None,
        });
        assert_eq!(out.protocol, Protocol::Anthropic);
        assert_eq!(out.base_url, "http://saved/v1");
        assert_eq!(out.model, "saved");
    }

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
