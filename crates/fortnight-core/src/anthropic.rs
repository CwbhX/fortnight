//! Anthropic Messages client. Owner: agent D (round 2).
//!
//! `POST {base_url}/v1/messages` with `x-api-key` (only when a key is set,
//! marked sensitive), `anthropic-version: 2023-06-01` and
//! `content-type: application/json` (PLAN.md sections 2.3 and 7.2). A base
//! URL that already ends in `/v1` is accepted: the path is never doubled.
//! Sends only `model`, `max_tokens`, `system`, `messages` and `stream`: no
//! temperature or other sampling parameters.
//!
//! `stop_reason` is checked before content: `refusal` → `Refused`,
//! `max_tokens` → `Truncated`. Streaming parses server-sent events; a plain
//! JSON reply to a streaming request is handled like `chat`.
//!
//! Retries up to three attempts with exponential backoff on connection
//! errors, timeouts, 429, 5xx and 529 (and the equivalent SSE `error`
//! events), only before any text has been yielded; never on other 4xx.
//! Each attempt is bounded by the timeout; when streaming, the timeout
//! bounds the wait for response headers and each gap between chunks.
//!
//! Skipped for the prototype: `Retry-After` is not honoured, and the model
//! list reads one page of up to 1000 models.
//! Requires a tokio runtime (reqwest does too).

use crate::llm::{
    BoxFuture, BoxStream, ChatEvent, ChatRequest, ChatResponse, LlmClient, LlmConfig, LlmError,
};
use eventsource_stream::{EventStreamError, Eventsource};
use futures_util::StreamExt;
use reqwest::header::{CONTENT_TYPE, HeaderValue};
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use std::collections::VecDeque;
use std::fmt;
use std::time::Duration;

const MAX_ATTEMPTS: u32 = 3;
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const DEFAULT_BACKOFF: Duration = Duration::from_millis(500);
const MAX_ERROR_BODY_CHARS: usize = 500;
const MAX_TOKENS: u32 = 4096;
const ANTHROPIC_VERSION: &str = "2023-06-01";

pub struct AnthropicClient {
    config: LlmConfig,
    http: reqwest::Client,
    timeout: Duration,
    backoff: Duration,
}

impl fmt::Debug for AnthropicClient {
    /// Shows the host, never the full base URL (it may carry credentials)
    /// or the key.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AnthropicClient")
            .field("host", &self.config.endpoint_host())
            .field("model", &self.config.model)
            .field("has_key", &self.config.api_key.is_some())
            .field("timeout", &self.timeout)
            .field("backoff", &self.backoff)
            .finish()
    }
}

impl AnthropicClient {
    /// Errors with `NotConfigured` if base URL or model is empty.
    pub fn new(config: LlmConfig) -> Result<Self, LlmError> {
        if config.base_url.trim().is_empty() || config.model.trim().is_empty() {
            return Err(LlmError::NotConfigured);
        }
        crate::secure::check_key_transport(&config)?;
        let http = crate::secure::client_builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .build()
            .map_err(|e| LlmError::Connection(error_chain(&e.without_url())))?;
        Ok(AnthropicClient {
            config,
            http,
            timeout: DEFAULT_TIMEOUT,
            backoff: DEFAULT_BACKOFF,
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

    /// `{base}/v1/{path}`, with a trailing `/v1` or slashes on the base
    /// URL tolerated.
    fn url(&self, path: &str) -> String {
        let base = self.config.base_url.trim().trim_end_matches('/');
        let base = base.strip_suffix("/v1").unwrap_or(base);
        format!("{base}/v1/{path}")
    }

    fn call(&self, url: String, body: Option<serde_json::Value>) -> Call {
        Call {
            http: self.http.clone(),
            url,
            model: self.config.model.clone(),
            api_key: self.config.api_key.clone(),
            timeout: self.timeout,
            body,
        }
    }

    fn message_call(&self, req: ChatRequest, stream: bool) -> Call {
        let body = serde_json::json!({
            "model": self.config.model,
            "max_tokens": MAX_TOKENS,
            "system": req.system,
            "messages": [{ "role": "user", "content": req.user }],
            "stream": stream,
        });
        self.call(self.url("messages"), Some(body))
    }
}

impl LlmClient for AnthropicClient {
    fn model(&self) -> &str {
        &self.config.model
    }

    fn chat(&self, req: ChatRequest) -> BoxFuture<Result<ChatResponse, LlmError>> {
        let call = self.message_call(req, false);
        let backoff = self.backoff;
        let key = self.config.api_key.clone();
        Box::pin(async move {
            with_retries(backoff, || async {
                let (status, text) = call.send_whole().await?;
                match status_error(status, &text, &call.model) {
                    Some(e) => Err(e),
                    None => parse_message(&text, &call.model),
                }
            })
            .await
            .map_err(|e| crate::secure::redact(e, key.as_ref()))
        })
    }

    fn chat_stream(&self, req: ChatRequest) -> BoxStream<Result<ChatEvent, LlmError>> {
        let state = StreamState {
            call: self.message_call(req, true),
            backoff: self.backoff,
            phase: Phase::Connect { attempt: 1 },
        };
        let key = self.config.api_key.clone();
        Box::pin(
            futures_util::stream::unfold(state, |mut state| async move {
                let item = state.next_item().await?;
                Some((item, state))
            })
            .map(move |item| item.map_err(|e| crate::secure::redact(e, key.as_ref()))),
        )
    }

    fn list_models(&self) -> BoxFuture<Result<Vec<String>, LlmError>> {
        let call = self.call(self.url("models?limit=1000"), None);
        let backoff = self.backoff;
        let key = self.config.api_key.clone();
        Box::pin(async move {
            with_retries(backoff, || async {
                let (status, text) = call.send_whole().await?;
                if status == 404 {
                    return Ok(Vec::new());
                }
                if let Some(e) = status_error(status, &text, &call.model) {
                    return Err(e);
                }
                parse_models(&text)
            })
            .await
            .map_err(|e| crate::secure::redact(e, key.as_ref()))
        })
    }
}

/// Runs `attempt` up to `MAX_ATTEMPTS` times while it fails retryably.
async fn with_retries<T, F, Fut>(backoff: Duration, mut attempt: F) -> Result<T, LlmError>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, LlmError>>,
{
    let mut n = 1;
    loop {
        match attempt().await {
            Err(e) if n < MAX_ATTEMPTS && is_retryable(&e) => {
                tokio::time::sleep(retry_delay(backoff, n)).await;
                n += 1;
            }
            result => return result,
        }
    }
}

fn retry_delay(backoff: Duration, attempt: u32) -> Duration {
    backoff * 2u32.pow(attempt - 1)
}

/// Everything one request needs, owned, so futures and streams are
/// `'static`.
struct Call {
    http: reqwest::Client,
    url: String,
    model: String,
    api_key: Option<SecretString>,
    timeout: Duration,
    /// `Some` for a POST with this JSON body, `None` for a GET.
    body: Option<serde_json::Value>,
}

impl Call {
    fn builder(&self) -> Result<reqwest::RequestBuilder, LlmError> {
        let mut builder = match &self.body {
            // `json` also sets `content-type: application/json`.
            Some(body) => self.http.post(&self.url).json(body),
            None => self.http.get(&self.url),
        };
        builder = builder.header("anthropic-version", ANTHROPIC_VERSION);
        if let Some(key) = &self.api_key {
            // The error carries no part of the value.
            let mut value =
                HeaderValue::from_str(key.expose_secret()).map_err(|_| LlmError::Unauthorized)?;
            value.set_sensitive(true);
            builder = builder.header("x-api-key", value);
        }
        Ok(builder)
    }

    /// Sends and reads the whole body, all within the timeout.
    async fn send_whole(&self) -> Result<(u16, String), LlmError> {
        let response = self
            .builder()?
            .timeout(self.timeout)
            .send()
            .await
            .map_err(transport_error)?;
        let status = response.status().as_u16();
        let text = response.text().await.map_err(transport_error)?;
        Ok((status, text))
    }

    /// Sends a streaming request. The timeout bounds the wait for the
    /// headers (and for the whole body when it is not an event stream).
    async fn open_stream(&self) -> Result<Opened, LlmError> {
        let builder = self.builder()?;
        let response = tokio::time::timeout(self.timeout, builder.send())
            .await
            .map_err(|_| LlmError::Timeout)?
            .map_err(transport_error)?;
        let status = response.status().as_u16();
        let is_json = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.contains("json"));
        if !(200..=299).contains(&status) || is_json {
            let text = tokio::time::timeout(self.timeout, response.text())
                .await
                .map_err(|_| LlmError::Timeout)?
                .map_err(transport_error)?;
            if let Some(e) = status_error(status, &text, &self.model) {
                return Err(e);
            }
            return parse_message(&text, &self.model).map(Opened::Whole);
        }
        Ok(Opened::Events(Box::pin(
            response.bytes_stream().eventsource(),
        )))
    }
}

type SseStream = BoxStream<Result<eventsource_stream::Event, EventStreamError<reqwest::Error>>>;

enum Opened {
    Events(SseStream),
    /// The endpoint answered with plain JSON.
    Whole(ChatResponse),
}

struct StreamState {
    call: Call,
    backoff: Duration,
    phase: Phase,
}

enum Phase {
    Connect {
        attempt: u32,
    },
    Events {
        events: SseStream,
        model: String,
        attempt: u32,
        yielded_text: bool,
    },
    Queued(VecDeque<Result<ChatEvent, LlmError>>),
    Finished,
}

/// What one SSE event means for the stream.
enum Step {
    Skip,
    Model(String),
    Delta(String),
    Done,
    Fail(LlmError),
}

impl StreamState {
    /// The next item, or `None` once the stream has ended.
    async fn next_item(&mut self) -> Option<Result<ChatEvent, LlmError>> {
        loop {
            match std::mem::replace(&mut self.phase, Phase::Finished) {
                Phase::Finished => return None,
                Phase::Queued(mut queue) => {
                    let item = queue.pop_front();
                    if !queue.is_empty() {
                        self.phase = Phase::Queued(queue);
                    }
                    return item;
                }
                Phase::Connect { attempt } => match self.call.open_stream().await {
                    Ok(Opened::Events(events)) => {
                        self.phase = Phase::Events {
                            events,
                            model: self.call.model.clone(),
                            attempt,
                            yielded_text: false,
                        };
                    }
                    Ok(Opened::Whole(resp)) => {
                        let mut queue = VecDeque::new();
                        if !resp.text.is_empty() {
                            queue.push_back(Ok(ChatEvent::Delta(resp.text)));
                        }
                        queue.push_back(Ok(ChatEvent::Done { model: resp.model }));
                        self.phase = Phase::Queued(queue);
                    }
                    Err(e) => {
                        if !self.retry(attempt, &e).await {
                            return Some(Err(e));
                        }
                    }
                },
                Phase::Events {
                    mut events,
                    mut model,
                    attempt,
                    yielded_text,
                } => {
                    let step = match tokio::time::timeout(self.call.timeout, events.next()).await {
                        Err(_) => Step::Fail(LlmError::Timeout),
                        Ok(None) => Step::Fail(LlmError::Truncated),
                        Ok(Some(Err(e))) => Step::Fail(sse_error(e)),
                        Ok(Some(Ok(event))) => interpret(&event, &self.call.model),
                    };
                    match step {
                        Step::Done => return Some(Ok(ChatEvent::Done { model })),
                        Step::Fail(e) => {
                            if yielded_text || !self.retry(attempt, &e).await {
                                return Some(Err(e));
                            }
                            continue;
                        }
                        Step::Delta(d) => {
                            self.phase = Phase::Events {
                                events,
                                model,
                                attempt,
                                yielded_text: true,
                            };
                            return Some(Ok(ChatEvent::Delta(d)));
                        }
                        Step::Model(m) => model = m,
                        Step::Skip => {}
                    }
                    self.phase = Phase::Events {
                        events,
                        model,
                        attempt,
                        yielded_text,
                    };
                }
            }
        }
    }

    /// If `e` after `attempt` is worth retrying, waits and schedules a new
    /// connection; returns whether it did.
    async fn retry(&mut self, attempt: u32, e: &LlmError) -> bool {
        if attempt >= MAX_ATTEMPTS || !is_retryable(e) {
            return false;
        }
        tokio::time::sleep(retry_delay(self.backoff, attempt)).await;
        self.phase = Phase::Connect {
            attempt: attempt + 1,
        };
        true
    }
}

/// Reads one Messages API stream event.
fn interpret(event: &eventsource_stream::Event, requested_model: &str) -> Step {
    let data: serde_json::Value = match serde_json::from_str(&event.data) {
        Ok(v) => v,
        Err(e) => {
            return match event.event.as_str() {
                "message_start" | "content_block_delta" | "message_delta" | "error" => Step::Fail(
                    LlmError::Malformed(format!("bad {} event: {e}", event.event)),
                ),
                _ => Step::Skip,
            };
        }
    };
    // The event name is authoritative; fall back to the data's `type`.
    let kind = match event.event.as_str() {
        "" | "message" => data["type"].as_str().unwrap_or_default(),
        name => name,
    };
    match kind {
        "message_start" => match data["message"]["model"].as_str() {
            Some(m) if !m.is_empty() => Step::Model(m.to_string()),
            _ => Step::Skip,
        },
        "content_block_delta" => match (
            data["delta"]["type"].as_str(),
            data["delta"]["text"].as_str(),
        ) {
            (Some("text_delta"), Some(text)) if !text.is_empty() => Step::Delta(text.to_string()),
            _ => Step::Skip,
        },
        "message_delta" => match stop_reason_error(data["delta"]["stop_reason"].as_str()) {
            Some(e) => Step::Fail(e),
            None => Step::Skip,
        },
        "message_stop" => Step::Done,
        "error" => Step::Fail(api_error(
            data["error"]["type"].as_str().unwrap_or_default(),
            data["error"]["message"].as_str().unwrap_or_default(),
            requested_model,
        )),
        _ => Step::Skip,
    }
}

fn stop_reason_error(reason: Option<&str>) -> Option<LlmError> {
    match reason {
        Some("refusal") => Some(LlmError::Refused),
        Some("max_tokens") => Some(LlmError::Truncated),
        _ => None,
    }
}

/// Maps an Anthropic `error.type` (from an SSE `error` event) onto the
/// error the matching HTTP status would give.
fn api_error(kind: &str, message: &str, model: &str) -> LlmError {
    let http = |status: u16| LlmError::Http {
        status,
        body: truncate_chars(message.trim(), MAX_ERROR_BODY_CHARS),
    };
    match kind {
        "overloaded_error" => http(529),
        "api_error" => http(500),
        "timeout_error" => http(504),
        "rate_limit_error" => LlmError::RateLimited,
        "authentication_error" | "permission_error" => LlmError::Unauthorized,
        "not_found_error" => LlmError::ModelNotFound(model.to_string()),
        "invalid_request_error" => http(400),
        "billing_error" => http(402),
        "request_too_large" => http(413),
        _ => LlmError::Malformed(format!(
            "unknown error event {kind}: {}",
            truncate_chars(message.trim(), MAX_ERROR_BODY_CHARS)
        )),
    }
}

/// The error for a non-2xx status, or `None` for success.
fn status_error(status: u16, body: &str, model: &str) -> Option<LlmError> {
    match status {
        200..=299 => None,
        401 | 403 => Some(LlmError::Unauthorized),
        404 => Some(LlmError::ModelNotFound(model.to_string())),
        429 => Some(LlmError::RateLimited),
        code => Some(LlmError::Http {
            status: code,
            body: truncate_chars(body.trim(), MAX_ERROR_BODY_CHARS),
        }),
    }
}

fn is_retryable(e: &LlmError) -> bool {
    match e {
        LlmError::Connection(_) | LlmError::Timeout | LlmError::RateLimited => true,
        LlmError::Http { status, .. } => *status >= 500,
        _ => false,
    }
}

fn transport_error(e: reqwest::Error) -> LlmError {
    if e.is_timeout() {
        LlmError::Timeout
    } else if e.is_decode() {
        LlmError::Malformed(error_chain(&e.without_url()))
    } else {
        // The URL is dropped in case `base_url` carries credentials. A body
        // error (connection lost mid-reply) counts as a connection error.
        LlmError::Connection(error_chain(&e.without_url()))
    }
}

fn sse_error(e: EventStreamError<reqwest::Error>) -> LlmError {
    match e {
        EventStreamError::Transport(e) => transport_error(e),
        other => LlmError::Malformed(other.to_string()),
    }
}

/// `e` and its sources, joined with ": ".
fn error_chain(e: &dyn std::error::Error) -> String {
    let mut out = e.to_string();
    let mut source = e.source();
    while let Some(s) = source {
        let text = s.to_string();
        if !out.contains(&text) {
            out.push_str(": ");
            out.push_str(&text);
        }
        source = s.source();
    }
    out
}

fn truncate_chars(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s.to_string(),
    }
}

#[derive(Deserialize)]
struct MessageBody {
    model: Option<String>,
    stop_reason: Option<String>,
    content: Option<Vec<ContentBlock>>,
}

#[derive(Deserialize)]
struct ContentBlock {
    #[serde(rename = "type")]
    kind: String,
    text: Option<String>,
}

/// A non-streaming reply: `stop_reason` first, then the text blocks.
fn parse_message(text: &str, requested_model: &str) -> Result<ChatResponse, LlmError> {
    let body: MessageBody =
        serde_json::from_str(text).map_err(|e| LlmError::Malformed(e.to_string()))?;
    if let Some(e) = stop_reason_error(body.stop_reason.as_deref()) {
        return Err(e);
    }
    let content = body
        .content
        .ok_or_else(|| LlmError::Malformed("response has no content".into()))?;
    let text = content
        .into_iter()
        .filter(|b| b.kind == "text")
        .filter_map(|b| b.text)
        .collect::<String>();
    let model = body
        .model
        .filter(|m| !m.is_empty())
        .unwrap_or_else(|| requested_model.to_string());
    Ok(ChatResponse { text, model })
}

#[derive(Deserialize)]
struct ModelList {
    data: Vec<ModelEntry>,
}

#[derive(Deserialize)]
struct ModelEntry {
    id: String,
}

fn parse_models(text: &str) -> Result<Vec<String>, LlmError> {
    let list: ModelList =
        serde_json::from_str(text).map_err(|e| LlmError::Malformed(e.to_string()))?;
    Ok(list.data.into_iter().map(|m| m.id).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::Protocol;
    use crate::llm::collect_stream;
    use serde_json::json;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const KEY: &str = "sk-ant-test-secret-key";

    fn config(base_url: &str, key: Option<&str>) -> LlmConfig {
        LlmConfig {
            base_url: base_url.into(),
            model: "claude-test".into(),
            api_key: key.map(SecretString::from),
        }
    }

    fn client(base_url: &str, key: Option<&str>) -> AnthropicClient {
        AnthropicClient::new(config(base_url, key))
            .unwrap()
            .with_backoff(Duration::from_millis(5))
            .with_timeout(Duration::from_secs(5))
    }

    fn request() -> ChatRequest {
        ChatRequest {
            system: "sys prompt".into(),
            user: "user prompt".into(),
        }
    }

    fn message(text: &str, stop_reason: &str) -> serde_json::Value {
        json!({
            "id": "msg_1",
            "type": "message",
            "role": "assistant",
            "model": "claude-test-2026-01-01",
            "content": [{ "type": "text", "text": text }],
            "stop_reason": stop_reason,
            "stop_sequence": null,
            "usage": { "input_tokens": 3, "output_tokens": 2 }
        })
    }

    /// One SSE event in wire format.
    fn sse(event: &str, data: serde_json::Value) -> String {
        format!("event: {event}\ndata: {data}\n\n")
    }

    fn start() -> String {
        sse(
            "message_start",
            json!({ "type": "message_start", "message": {
                "id": "msg_1", "type": "message", "role": "assistant",
                "model": "claude-streamed-2026", "content": [],
                "stop_reason": null, "usage": { "input_tokens": 3, "output_tokens": 1 }
            }}),
        )
    }

    fn delta(text: &str) -> String {
        sse(
            "content_block_delta",
            json!({ "type": "content_block_delta", "index": 0,
                    "delta": { "type": "text_delta", "text": text } }),
        )
    }

    fn stop_reason(reason: &str) -> String {
        sse(
            "message_delta",
            json!({ "type": "message_delta",
                    "delta": { "stop_reason": reason, "stop_sequence": null },
                    "usage": { "output_tokens": 5 } }),
        )
    }

    fn stop() -> String {
        sse("message_stop", json!({ "type": "message_stop" }))
    }

    fn error_event(kind: &str, msg: &str) -> String {
        sse(
            "error",
            json!({ "type": "error", "error": { "type": kind, "message": msg } }),
        )
    }

    fn full_stream(pieces: &[&str]) -> String {
        let mut body = start();
        body += &sse(
            "content_block_start",
            json!({ "type": "content_block_start", "index": 0,
                    "content_block": { "type": "text", "text": "" } }),
        );
        body += &sse("ping", json!({ "type": "ping" }));
        for p in pieces {
            body += &delta(p);
        }
        body += &sse(
            "content_block_stop",
            json!({ "type": "content_block_stop", "index": 0 }),
        );
        body += &stop_reason("end_turn");
        body += &stop();
        body
    }

    fn sse_response(body: String) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_raw(body, "text/event-stream")
    }

    async fn mount(server: &MockServer, template: ResponseTemplate) {
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(template)
            .mount(server)
            .await;
    }

    async fn mount_once(server: &MockServer, template: ResponseTemplate) {
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(template)
            .up_to_n_times(1)
            .mount(server)
            .await;
    }

    async fn request_count(server: &MockServer) -> usize {
        server.received_requests().await.unwrap().len()
    }

    async fn stream_items(c: &AnthropicClient) -> Vec<Result<ChatEvent, LlmError>> {
        c.chat_stream(request()).collect().await
    }

    #[test]
    fn new_requires_base_url_and_model() {
        assert!(matches!(
            AnthropicClient::new(config(" ", None)),
            Err(LlmError::NotConfigured)
        ));
        let mut cfg = config("https://api.anthropic.com", None);
        cfg.model = String::new();
        assert!(matches!(
            AnthropicClient::new(cfg),
            Err(LlmError::NotConfigured)
        ));
    }

    #[test]
    fn url_never_doubles_v1() {
        for base in [
            "https://api.anthropic.com",
            "https://api.anthropic.com/",
            "https://api.anthropic.com/v1",
            "https://api.anthropic.com/v1/",
            " https://api.anthropic.com/v1// ",
        ] {
            let c = AnthropicClient::new(config(base, None)).unwrap();
            assert_eq!(c.url("messages"), "https://api.anthropic.com/v1/messages");
        }
        let c = AnthropicClient::new(config("http://proxy/anthropic", None)).unwrap();
        assert_eq!(c.url("models"), "http://proxy/anthropic/v1/models");
    }

    #[test]
    fn debug_shows_no_key_or_credentials() {
        let c = AnthropicClient::new(config("https://user:pw@api.anthropic.com/v1", Some(KEY)))
            .unwrap();
        let debug = format!("{c:?}");
        assert!(!debug.contains(KEY), "{debug}");
        assert!(!debug.contains("pw"), "{debug}");
        assert!(debug.contains("api.anthropic.com"), "{debug}");
    }

    #[tokio::test]
    async fn chat_success_sends_headers_and_exact_body() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .and(header("x-api-key", KEY))
            .and(header("anthropic-version", "2023-06-01"))
            .and(header("content-type", "application/json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "model": "claude-test-2026-01-01",
                "content": [
                    { "type": "text", "text": "Hello, " },
                    { "type": "thinking", "thinking": "hmm" },
                    { "type": "text", "text": "world" }
                ],
                "stop_reason": "end_turn"
            })))
            .expect(1)
            .mount(&server)
            .await;

        let resp = client(&server.uri(), Some(KEY))
            .chat(request())
            .await
            .unwrap();
        assert_eq!(resp.text, "Hello, world");
        assert_eq!(resp.model, "claude-test-2026-01-01");

        let received = server.received_requests().await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&received[0].body).unwrap();
        assert_eq!(
            body,
            json!({
                "model": "claude-test",
                "max_tokens": 4096,
                "system": "sys prompt",
                "messages": [{ "role": "user", "content": "user prompt" }],
                "stream": false,
            })
        );
        assert!(body.get("temperature").is_none());
    }

    #[tokio::test]
    async fn no_key_means_no_x_api_key_and_model_falls_back() {
        let server = MockServer::start().await;
        mount(
            &server,
            ResponseTemplate::new(200).set_body_json(json!({
                "content": [{ "type": "text", "text": "OK" }],
                "stop_reason": "end_turn"
            })),
        )
        .await;
        // A base URL that already ends in /v1 is not doubled.
        let resp = client(&format!("{}/v1/", server.uri()), None)
            .chat(request())
            .await
            .unwrap();
        assert_eq!(resp.text, "OK");
        assert_eq!(resp.model, "claude-test");
        let received = server.received_requests().await.unwrap();
        assert_eq!(received[0].url.path(), "/v1/messages");
        assert!(!received[0].headers.contains_key("x-api-key"));
        assert_eq!(received[0].headers["anthropic-version"], "2023-06-01");
    }

    #[tokio::test]
    async fn stop_reason_is_checked_before_content() {
        let server = MockServer::start().await;
        mount_once(
            &server,
            ResponseTemplate::new(200).set_body_json(message("I can't", "refusal")),
        )
        .await;
        mount_once(
            &server,
            ResponseTemplate::new(200).set_body_json(message("partial", "max_tokens")),
        )
        .await;
        // Content without a list is still refused first.
        mount(
            &server,
            ResponseTemplate::new(200).set_body_json(json!({ "stop_reason": "refusal" })),
        )
        .await;
        let c = client(&server.uri(), None);
        assert_eq!(c.chat(request()).await, Err(LlmError::Refused));
        assert_eq!(c.chat(request()).await, Err(LlmError::Truncated));
        assert_eq!(c.chat(request()).await, Err(LlmError::Refused));
        assert_eq!(request_count(&server).await, 3);
    }

    #[tokio::test]
    async fn malformed_body_is_malformed_not_retried() {
        let server = MockServer::start().await;
        mount(
            &server,
            ResponseTemplate::new(200).set_body_raw("not json {", "application/json"),
        )
        .await;
        let err = client(&server.uri(), None)
            .chat(request())
            .await
            .unwrap_err();
        assert!(matches!(err, LlmError::Malformed(_)), "{err:?}");
        assert_eq!(request_count(&server).await, 1);
    }

    #[tokio::test]
    async fn stream_assembles_text_in_order_with_model_from_message_start() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .and(header("x-api-key", KEY))
            .and(header("anthropic-version", "2023-06-01"))
            .respond_with(sse_response(full_stream(&["Hel", "lo, ", "wor", "ld"])))
            .expect(1)
            .mount(&server)
            .await;
        let c = client(&server.uri(), Some(KEY));
        let items = stream_items(&c).await;
        assert_eq!(
            items,
            vec![
                Ok(ChatEvent::Delta("Hel".into())),
                Ok(ChatEvent::Delta("lo, ".into())),
                Ok(ChatEvent::Delta("wor".into())),
                Ok(ChatEvent::Delta("ld".into())),
                Ok(ChatEvent::Done {
                    model: "claude-streamed-2026".into()
                }),
            ]
        );
        let received = server.received_requests().await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&received[0].body).unwrap();
        assert_eq!(body["stream"], json!(true));
        assert_eq!(body["max_tokens"], json!(4096));
        assert!(body.get("temperature").is_none());
        assert_eq!(body.as_object().unwrap().len(), 5);
    }

    #[tokio::test]
    async fn stream_refusal_and_max_tokens_are_errors() {
        let server = MockServer::start().await;
        mount_once(
            &server,
            sse_response(start() + &delta("I ") + &stop_reason("refusal") + &stop()),
        )
        .await;
        mount_once(
            &server,
            sse_response(start() + &delta("part") + &stop_reason("max_tokens") + &stop()),
        )
        .await;
        let c = client(&server.uri(), None);
        assert_eq!(
            stream_items(&c).await,
            vec![Ok(ChatEvent::Delta("I ".into())), Err(LlmError::Refused)]
        );
        assert_eq!(
            collect_stream(c.chat_stream(request())).await,
            Err(LlmError::Truncated)
        );
        assert_eq!(request_count(&server).await, 2);
    }

    #[tokio::test]
    async fn stream_without_message_stop_is_truncated() {
        let server = MockServer::start().await;
        mount(&server, sse_response(start() + &delta("cut off"))).await;
        let c = client(&server.uri(), None);
        assert_eq!(
            stream_items(&c).await,
            vec![
                Ok(ChatEvent::Delta("cut off".into())),
                Err(LlmError::Truncated)
            ]
        );
        assert_eq!(request_count(&server).await, 1);
    }

    #[tokio::test]
    async fn sse_error_events_map_to_errors() {
        let cases = [
            ("rate_limit_error", LlmError::RateLimited, 3),
            ("authentication_error", LlmError::Unauthorized, 1),
            (
                "not_found_error",
                LlmError::ModelNotFound("claude-test".into()),
                1,
            ),
            (
                "invalid_request_error",
                LlmError::Http {
                    status: 400,
                    body: "bad thing".into(),
                },
                1,
            ),
            (
                "overloaded_error",
                LlmError::Http {
                    status: 529,
                    body: "bad thing".into(),
                },
                3,
            ),
            (
                "api_error",
                LlmError::Http {
                    status: 500,
                    body: "bad thing".into(),
                },
                3,
            ),
        ];
        for (kind, expected, attempts) in cases {
            let server = MockServer::start().await;
            mount(
                &server,
                sse_response(start() + &error_event(kind, "bad thing")),
            )
            .await;
            let c = client(&server.uri(), None);
            assert_eq!(stream_items(&c).await, vec![Err(expected)], "{kind}");
            assert_eq!(request_count(&server).await, attempts, "{kind}");
        }
    }

    #[tokio::test]
    async fn sse_overloaded_before_text_is_retried() {
        let server = MockServer::start().await;
        mount_once(
            &server,
            sse_response(start() + &error_event("overloaded_error", "Overloaded")),
        )
        .await;
        mount(&server, sse_response(full_stream(&["fine"]))).await;
        let reply = collect_stream(client(&server.uri(), None).chat_stream(request()))
            .await
            .unwrap();
        assert_eq!(reply.text, "fine");
        assert_eq!(request_count(&server).await, 2);
    }

    #[tokio::test]
    async fn sse_error_after_text_is_not_retried() {
        let server = MockServer::start().await;
        mount(
            &server,
            sse_response(start() + &delta("half") + &error_event("overloaded_error", "Overloaded")),
        )
        .await;
        let items = stream_items(&client(&server.uri(), None)).await;
        assert_eq!(items.len(), 2);
        assert_eq!(items[0], Ok(ChatEvent::Delta("half".into())));
        assert!(
            matches!(items[1], Err(LlmError::Http { status: 529, .. })),
            "{items:?}"
        );
        assert_eq!(request_count(&server).await, 1);
    }

    #[tokio::test]
    async fn stream_falls_back_to_plain_json() {
        let server = MockServer::start().await;
        mount_once(
            &server,
            ResponseTemplate::new(200).set_body_json(message("whole reply", "end_turn")),
        )
        .await;
        mount(
            &server,
            ResponseTemplate::new(200).set_body_json(message("no", "refusal")),
        )
        .await;
        let c = client(&server.uri(), None);
        assert_eq!(
            stream_items(&c).await,
            vec![
                Ok(ChatEvent::Delta("whole reply".into())),
                Ok(ChatEvent::Done {
                    model: "claude-test-2026-01-01".into()
                }),
            ]
        );
        assert_eq!(stream_items(&c).await, vec![Err(LlmError::Refused)]);
    }

    async fn chat_error_for(status: u16) -> (LlmError, usize) {
        let server = MockServer::start().await;
        mount(
            &server,
            ResponseTemplate::new(status).set_body_string("{\"error\":\"nope\"}"),
        )
        .await;
        let err = client(&server.uri(), Some(KEY))
            .chat(request())
            .await
            .unwrap_err();
        (err, request_count(&server).await)
    }

    #[tokio::test]
    async fn http_errors_map_and_retry_only_where_allowed() {
        assert_eq!(chat_error_for(401).await, (LlmError::Unauthorized, 1));
        assert_eq!(chat_error_for(403).await, (LlmError::Unauthorized, 1));
        assert_eq!(
            chat_error_for(404).await,
            (LlmError::ModelNotFound("claude-test".into()), 1)
        );
        assert_eq!(chat_error_for(429).await, (LlmError::RateLimited, 3));
        let body = "{\"error\":\"nope\"}".to_string();
        assert_eq!(
            chat_error_for(400).await,
            (
                LlmError::Http {
                    status: 400,
                    body: body.clone()
                },
                1
            )
        );
        assert_eq!(
            chat_error_for(529).await,
            (LlmError::Http { status: 529, body }, 3)
        );
    }

    #[tokio::test]
    async fn long_error_body_is_truncated() {
        let server = MockServer::start().await;
        mount(
            &server,
            ResponseTemplate::new(413).set_body_string("x".repeat(2000)),
        )
        .await;
        match client(&server.uri(), None).chat(request()).await {
            Err(LlmError::Http { status: 413, body }) => {
                assert_eq!(body.chars().count(), MAX_ERROR_BODY_CHARS + 1)
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[tokio::test]
    async fn retry_then_success_for_chat_and_stream() {
        for status in [429, 500, 529] {
            let server = MockServer::start().await;
            mount_once(&server, ResponseTemplate::new(status)).await;
            mount(
                &server,
                ResponseTemplate::new(200).set_body_json(message("after retry", "end_turn")),
            )
            .await;
            let resp = client(&server.uri(), None).chat(request()).await.unwrap();
            assert_eq!(resp.text, "after retry", "{status}");
            assert_eq!(request_count(&server).await, 2, "{status}");

            let server = MockServer::start().await;
            mount_once(&server, ResponseTemplate::new(status)).await;
            mount(&server, sse_response(full_stream(&["streamed"]))).await;
            let reply = collect_stream(client(&server.uri(), None).chat_stream(request()))
                .await
                .unwrap();
            assert_eq!(reply.text, "streamed", "{status}");
            assert_eq!(request_count(&server).await, 2, "{status}");
        }
    }

    #[tokio::test]
    async fn stream_400_is_not_retried() {
        let server = MockServer::start().await;
        mount(&server, ResponseTemplate::new(400).set_body_string("bad")).await;
        let items = stream_items(&client(&server.uri(), None)).await;
        assert_eq!(
            items,
            vec![Err(LlmError::Http {
                status: 400,
                body: "bad".into()
            })]
        );
        assert_eq!(request_count(&server).await, 1);
    }

    #[tokio::test]
    async fn slow_response_times_out_per_attempt() {
        let server = MockServer::start().await;
        mount(
            &server,
            sse_response(full_stream(&["late"])).set_delay(Duration::from_secs(5)),
        )
        .await;
        let c = client(&server.uri(), None).with_timeout(Duration::from_millis(150));
        assert_eq!(stream_items(&c).await, vec![Err(LlmError::Timeout)]);
        assert_eq!(c.chat(request()).await, Err(LlmError::Timeout));
        assert_eq!(request_count(&server).await, 6);
    }

    /// A raw HTTP server that sends `head` then stalls, to test the gap
    /// timeout (wiremock can only delay a whole response).
    fn stalling_server(head: String) -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            use std::io::{Read, Write};
            for mut conn in listener.incoming().take(3).flatten() {
                let mut buf = [0u8; 65536];
                let _ = conn.read(&mut buf);
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n{head}"
                );
                let _ = conn.write_all(response.as_bytes());
                let _ = conn.flush();
                std::thread::sleep(Duration::from_secs(3));
            }
        });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn stalled_stream_times_out_between_chunks() {
        let base = stalling_server(start() + &delta("first"));
        let c = client(&base, None).with_timeout(Duration::from_millis(300));
        assert_eq!(
            stream_items(&c).await,
            vec![Ok(ChatEvent::Delta("first".into())), Err(LlmError::Timeout)]
        );
    }

    #[tokio::test]
    async fn list_models_reads_ids_and_404_is_empty() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .and(header("x-api-key", KEY))
            .and(header("anthropic-version", "2023-06-01"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [
                    { "type": "model", "id": "claude-opus-5-5", "display_name": "Opus" },
                    { "type": "model", "id": "claude-haiku-5-5", "display_name": "Haiku" }
                ],
                "has_more": false
            })))
            .expect(1)
            .mount(&server)
            .await;
        let models = client(&format!("{}/v1", server.uri()), Some(KEY))
            .list_models()
            .await
            .unwrap();
        assert_eq!(models, vec!["claude-opus-5-5", "claude-haiku-5-5"]);

        let empty = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&empty)
            .await;
        assert_eq!(
            client(&empty.uri(), None).list_models().await,
            Ok(Vec::new())
        );
    }

    #[tokio::test]
    async fn key_never_appears_in_errors() {
        let mut errors = Vec::new();
        for status in [400, 401, 404, 429, 500, 529] {
            let server = MockServer::start().await;
            Mock::given(wiremock::matchers::any())
                .respond_with(ResponseTemplate::new(status).set_body_string("nope"))
                .mount(&server)
                .await;
            let c = client(&server.uri(), Some(KEY));
            errors.push(c.chat(request()).await.unwrap_err());
            errors.extend(stream_items(&c).await.into_iter().filter_map(Result::err));
            if let Err(e) = c.list_models().await {
                errors.push(e);
            }
        }
        let port = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            listener.local_addr().unwrap().port()
        };
        let c = client(&format!("http://127.0.0.1:{port}"), Some(KEY));
        let refused = c.chat(request()).await.unwrap_err();
        assert!(matches!(refused, LlmError::Connection(_)), "{refused:?}");
        errors.push(refused);
        errors.extend(stream_items(&c).await.into_iter().filter_map(Result::err));
        // A key that is not a valid header value fails without echoing it.
        let bad = client("http://127.0.0.1:9", Some("sk-ant-bad\nkey"));
        errors.push(bad.chat(request()).await.unwrap_err());

        assert!(errors.len() > 15, "{errors:?}");
        for e in &errors {
            let text = format!("{e} {e:?}");
            assert!(!text.contains(KEY), "{text}");
            assert!(!text.contains("sk-ant-bad"), "{text}");
        }
    }

    #[tokio::test]
    async fn connect_with_anthropic_protocol_yields_a_working_client() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .and(header("x-api-key", KEY))
            .and(header("anthropic-version", "2023-06-01"))
            .respond_with(sse_response(full_stream(&["O", "K"])))
            .expect(1)
            .mount(&server)
            .await;
        let c = crate::llm::connect(
            Protocol::Anthropic,
            config(&format!("{}/v1", server.uri()), Some(KEY)),
        )
        .unwrap();
        assert_eq!(c.model(), "claude-test");
        let reply = collect_stream(c.chat_stream(ChatRequest::test()))
            .await
            .unwrap();
        assert_eq!(reply.text, "OK");
        assert_eq!(reply.model, "claude-streamed-2026");
    }

    #[test]
    fn futures_and_streams_are_send_and_static() {
        fn assert_send_static<T: Send + 'static>(_: &T) {}
        let c = AnthropicClient::new(config("http://localhost", None)).unwrap();
        let fut = c.chat(request());
        let stream = c.chat_stream(request());
        let models = c.list_models();
        drop(c);
        assert_send_static(&fut);
        assert_send_static(&stream);
        assert_send_static(&models);
    }
}
