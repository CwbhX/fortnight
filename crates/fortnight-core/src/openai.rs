//! OpenAI-compatible Chat Completions client. Owner: agent C.
//!
//! `POST {base_url}/chat/completions` with `Authorization: Bearer <key>`
//! (omitted when no key). Sends only `model`, `messages` and `stream`:
//! no temperature or other sampling parameters. Maps HTTP and body errors
//! onto `LlmError`. Retries up to three attempts with backoff on connection
//! errors, timeouts, 429 and 5xx; never on other 4xx.
//!
//! `chat` asks for the whole reply (`stream: false`). `chat_stream` asks
//! for server-sent events (`stream: true`); if the endpoint answers with a
//! plain JSON body instead, that body is parsed like `chat`'s and yielded
//! as one `Delta`. Retries happen only before the first `Delta`. The
//! timeout bounds waiting for the response and each gap between events,
//! not the whole stream.
//!
//! `list_models` reads `GET {base_url}/models`; endpoints without it
//! (404, 405, 501) give an empty list.
//!
//! Skipped for the prototype: `Retry-After` is not honoured (fixed
//! exponential backoff instead), and a streamed body served with the wrong
//! content type is not sniffed (it fails as malformed JSON).
//! Requires a tokio runtime (reqwest does too); the app runs iced with
//! its `tokio` feature.

use crate::llm::{
    BoxFuture, BoxStream, ChatEvent, ChatRequest, ChatResponse, LlmClient, LlmConfig, LlmError,
};
use eventsource_stream::{EventStreamError, Eventsource};
use futures_util::{Stream, StreamExt};
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;
use tokio::time::Instant;

const MAX_ATTEMPTS: u32 = 3;
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const DEFAULT_BACKOFF: Duration = Duration::from_millis(500);
const MAX_ERROR_BODY_CHARS: usize = 500;

pub struct OpenAiClient {
    config: LlmConfig,
    http: reqwest::Client,
    timeout: Duration,
    backoff: Duration,
}

impl std::fmt::Debug for OpenAiClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // `LlmConfig`'s Debug redacts the key (`SecretString`).
        f.debug_struct("OpenAiClient")
            .field("config", &self.config)
            .field("timeout", &self.timeout)
            .field("backoff", &self.backoff)
            .finish_non_exhaustive()
    }
}

impl OpenAiClient {
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
        Ok(OpenAiClient {
            config,
            http,
            timeout: DEFAULT_TIMEOUT,
            backoff: DEFAULT_BACKOFF,
        })
    }

    /// Time allowed for each attempt of `chat` and `list_models`; for
    /// `chat_stream`, for the response to start and for each gap between
    /// events (default 120 s).
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

    fn base(&self) -> &str {
        self.config.base_url.trim().trim_end_matches('/')
    }

    fn url(&self) -> String {
        format!("{}/chat/completions", self.base())
    }

    fn call(&self, req: ChatRequest, stream: bool) -> Call {
        Call {
            http: self.http.clone(),
            url: self.url(),
            model: self.config.model.clone(),
            api_key: self.config.api_key.clone(),
            timeout: self.timeout,
            body: serde_json::json!({
                "model": self.config.model,
                "messages": [
                    { "role": "system", "content": req.system },
                    { "role": "user", "content": req.user },
                ],
                "stream": stream,
            }),
        }
    }
}

impl LlmClient for OpenAiClient {
    fn model(&self) -> &str {
        &self.config.model
    }

    fn chat(&self, req: ChatRequest) -> BoxFuture<Result<ChatResponse, LlmError>> {
        let call = self.call(req, false);
        let backoff = self.backoff;
        let key = self.config.api_key.clone();
        Box::pin(async move {
            with_retries(backoff, || call.send())
                .await
                .map_err(|e| crate::secure::redact(e, key.as_ref()))
        })
    }

    fn chat_stream(&self, req: ChatRequest) -> BoxStream<Result<ChatEvent, LlmError>> {
        let state = StreamState {
            call: self.call(req, true),
            backoff: self.backoff,
            attempt: 1,
            yielded_text: false,
            pending: VecDeque::new(),
            phase: Phase::Start,
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
        let http = self.http.clone();
        let url = format!("{}/models", self.base());
        let api_key = self.config.api_key.clone();
        let timeout = self.timeout;
        let backoff = self.backoff;
        Box::pin(async move {
            with_retries(backoff, || {
                fetch_models(&http, &url, api_key.as_ref(), timeout)
            })
            .await
            .map_err(|e| crate::secure::redact(e, api_key.as_ref()))
        })
    }
}

/// Runs `attempt` up to `MAX_ATTEMPTS` times while it fails retryably.
async fn with_retries<T, F, Fut>(backoff: Duration, mut attempt: F) -> Result<T, LlmError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, LlmError>>,
{
    let mut n = 1;
    loop {
        match attempt().await {
            Err(e) if n < MAX_ATTEMPTS && is_retryable(&e) => {
                tokio::time::sleep(backoff_delay(backoff, n)).await;
                n += 1;
            }
            result => return result,
        }
    }
}

/// The delay after failed attempt `n` (1-based).
fn backoff_delay(backoff: Duration, n: u32) -> Duration {
    backoff * 2u32.pow(n - 1)
}

/// Everything one request needs, owned, so futures and streams are `'static`.
struct Call {
    http: reqwest::Client,
    url: String,
    model: String,
    api_key: Option<SecretString>,
    timeout: Duration,
    body: serde_json::Value,
}

/// A successful start of a streamed request.
enum Opened {
    /// The endpoint is streaming events.
    Events(SseEvents),
    /// The endpoint ignored `stream: true` and sent the whole reply.
    Whole(ChatResponse),
}

type SseEvents = Pin<
    Box<
        dyn Stream<Item = Result<eventsource_stream::Event, EventStreamError<reqwest::Error>>>
            + Send,
    >,
>;

impl Call {
    fn request(&self) -> reqwest::RequestBuilder {
        let builder = self.http.post(&self.url).json(&self.body);
        match &self.api_key {
            // `bearer_auth` marks the header sensitive.
            Some(key) => builder.bearer_auth(key.expose_secret()),
            None => builder,
        }
    }

    async fn send(&self) -> Result<ChatResponse, LlmError> {
        let response = self
            .request()
            .timeout(self.timeout)
            .send()
            .await
            .map_err(transport_error)?;
        let status = response.status();
        let text = response.text().await.map_err(transport_error)?;
        if status.is_success() {
            parse_body(&text, &self.model)
        } else {
            Err(status_error(status.as_u16(), &text, &self.model))
        }
    }

    /// Sends the streamed request and waits for the response to start.
    /// A non-streamed (JSON) response is read whole within the timeout.
    async fn open(&self) -> Result<Opened, LlmError> {
        let deadline = Instant::now() + self.timeout;
        let response = within(deadline, self.request().send())
            .await?
            .map_err(transport_error)?;
        let status = response.status();
        if status.is_success() && is_event_stream(response.headers()) {
            return Ok(Opened::Events(Box::pin(
                response.bytes_stream().eventsource(),
            )));
        }
        let text = within(deadline, response.text())
            .await?
            .map_err(transport_error)?;
        if status.is_success() {
            parse_body(&text, &self.model).map(Opened::Whole)
        } else {
            Err(status_error(status.as_u16(), &text, &self.model))
        }
    }
}

/// `fut`, or `Timeout` if it isn't done by `deadline`.
async fn within<T>(deadline: Instant, fut: impl Future<Output = T>) -> Result<T, LlmError> {
    tokio::time::timeout_at(deadline, fut)
        .await
        .map_err(|_| LlmError::Timeout)
}

fn is_event_stream(headers: &reqwest::header::HeaderMap) -> bool {
    headers
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| {
            v.trim_start()
                .to_ascii_lowercase()
                .starts_with("text/event-stream")
        })
}

/// The state behind `chat_stream`.
struct StreamState {
    call: Call,
    backoff: Duration,
    /// The current attempt, 1-based.
    attempt: u32,
    /// Once text has been yielded, errors are final: no retry.
    yielded_text: bool,
    /// Items to yield before doing anything else.
    pending: VecDeque<Result<ChatEvent, LlmError>>,
    phase: Phase,
}

enum Phase {
    /// No request in flight; the next step sends one.
    Start,
    /// Reading events.
    Events(SseReader),
    /// `Done` or an error has been yielded (or queued).
    Ended,
}

impl StreamState {
    /// The next item, or `None` once the stream is over.
    async fn next_item(&mut self) -> Option<Result<ChatEvent, LlmError>> {
        loop {
            if let Some(item) = self.pending.pop_front() {
                return Some(item);
            }
            let result = match &mut self.phase {
                Phase::Ended => return None,
                Phase::Start => match self.call.open().await {
                    Ok(Opened::Events(events)) => {
                        self.phase = Phase::Events(SseReader::new(events));
                        continue;
                    }
                    Ok(Opened::Whole(resp)) => {
                        self.phase = Phase::Ended;
                        self.pending.push_back(Ok(ChatEvent::Delta(resp.text)));
                        self.pending
                            .push_back(Ok(ChatEvent::Done { model: resp.model }));
                        continue;
                    }
                    Err(e) => Err(e),
                },
                Phase::Events(reader) => {
                    match reader.next_item(self.call.timeout, &self.call.model).await {
                        Some(result) => result,
                        None => continue,
                    }
                }
            };
            match result {
                Ok(ChatEvent::Delta(text)) => {
                    self.yielded_text = true;
                    return Some(Ok(ChatEvent::Delta(text)));
                }
                Ok(done @ ChatEvent::Done { .. }) => {
                    self.phase = Phase::Ended;
                    return Some(Ok(done));
                }
                Err(e) if !self.yielded_text && self.attempt < MAX_ATTEMPTS && is_retryable(&e) => {
                    self.phase = Phase::Start;
                    tokio::time::sleep(backoff_delay(self.backoff, self.attempt)).await;
                    self.attempt += 1;
                }
                Err(e) => {
                    self.phase = Phase::Ended;
                    return Some(Err(e));
                }
            }
        }
    }
}

/// Turns server-sent events into `ChatEvent`s.
struct SseReader {
    events: SseEvents,
    /// The latest non-empty `model` a chunk reported.
    model: Option<String>,
    /// A chunk carried a normal `finish_reason` (e.g. "stop"), so the
    /// stream may end without `[DONE]`.
    finished: bool,
}

impl SseReader {
    fn new(events: SseEvents) -> Self {
        SseReader {
            events,
            model: None,
            finished: false,
        }
    }

    fn done(&mut self, requested_model: &str) -> ChatEvent {
        ChatEvent::Done {
            model: self
                .model
                .take()
                .unwrap_or_else(|| requested_model.to_string()),
        }
    }

    /// Reads one event. `None` means it carried nothing to yield.
    async fn next_item(
        &mut self,
        gap: Duration,
        requested_model: &str,
    ) -> Option<Result<ChatEvent, LlmError>> {
        let event = match tokio::time::timeout(gap, self.events.next()).await {
            Err(_) => return Some(Err(LlmError::Timeout)),
            Ok(None) if self.finished => return Some(Ok(self.done(requested_model))),
            Ok(None) => return Some(Err(LlmError::Truncated)),
            Ok(Some(Err(e))) => return Some(Err(sse_error(e))),
            Ok(Some(Ok(event))) => event,
        };
        let data = event.data.trim();
        if data.is_empty() {
            return None;
        }
        if data == "[DONE]" {
            return Some(Ok(self.done(requested_model)));
        }
        let chunk: StreamChunk = match serde_json::from_str(data) {
            Ok(chunk) => chunk,
            Err(e) => return Some(Err(LlmError::Malformed(e.to_string()))),
        };
        if let Some(error) = chunk.error {
            return Some(Err(error_object(&error, requested_model)));
        }
        if let Some(model) = chunk.model.filter(|m| !m.is_empty()) {
            self.model = Some(model);
        }
        // Chunks without choices (e.g. a trailing usage chunk) carry no text.
        let choice = chunk.choices.into_iter().next()?;
        match choice.finish_reason.as_deref() {
            Some("length") => return Some(Err(LlmError::Truncated)),
            Some("content_filter") => return Some(Err(LlmError::Refused)),
            Some(_) => self.finished = true,
            None => {}
        }
        let delta = choice.delta?;
        if delta.refusal.is_some_and(|r| !r.is_empty()) {
            return Some(Err(LlmError::Refused));
        }
        delta
            .content
            .filter(|c| !c.is_empty())
            .map(|c| Ok(ChatEvent::Delta(c)))
    }
}

#[derive(Deserialize)]
struct StreamChunk {
    model: Option<String>,
    #[serde(default)]
    choices: Vec<StreamChoice>,
    error: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct StreamChoice {
    delta: Option<Delta>,
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct Delta {
    content: Option<String>,
    refusal: Option<String>,
}

/// Maps an in-stream `{"error": ...}` like the HTTP error it stands for:
/// a numeric `code` (or `status`) is used as the status; otherwise the
/// `type`/`code` text is matched, and anything else counts as a 500.
fn error_object(error: &serde_json::Value, requested_model: &str) -> LlmError {
    let message = error
        .get("message")
        .and_then(|m| m.as_str())
        .or_else(|| error.as_str())
        .map_or_else(|| error.to_string(), str::to_string);
    let numeric = ["code", "status"].iter().find_map(|field| {
        let value = error.get(field)?;
        let n = value
            .as_u64()
            .or_else(|| value.as_str()?.trim().parse().ok())?;
        u16::try_from(n).ok().filter(|s| (400..600).contains(s))
    });
    let status = numeric.unwrap_or_else(|| {
        let label = ["type", "code"]
            .iter()
            .filter_map(|field| error.get(field)?.as_str())
            .collect::<Vec<_>>()
            .join(" ")
            .to_ascii_lowercase();
        if label.contains("rate_limit") {
            429
        } else if label.contains("auth") || label.contains("api_key") {
            401
        } else if label.contains("not_found") {
            404
        } else if label.contains("invalid_request") {
            400
        } else {
            500
        }
    });
    status_error(status, &message, requested_model)
}

/// An error status (not 2xx) and its body as an `LlmError`.
fn status_error(status: u16, body: &str, requested_model: &str) -> LlmError {
    match status {
        401 | 403 => LlmError::Unauthorized,
        404 => LlmError::ModelNotFound(requested_model.to_string()),
        429 => LlmError::RateLimited,
        code => LlmError::Http {
            status: code,
            body: truncate_chars(body.trim(), MAX_ERROR_BODY_CHARS),
        },
    }
}

fn sse_error(e: EventStreamError<reqwest::Error>) -> LlmError {
    match e {
        EventStreamError::Transport(e) if e.is_timeout() => LlmError::Timeout,
        // reqwest marks every body-stream error as "decode", so anything
        // from the transport here is the connection dropping.
        EventStreamError::Transport(e) => LlmError::Connection(error_chain(&e.without_url())),
        other => LlmError::Malformed(other.to_string()),
    }
}

async fn fetch_models(
    http: &reqwest::Client,
    url: &str,
    api_key: Option<&SecretString>,
    timeout: Duration,
) -> Result<Vec<String>, LlmError> {
    let mut builder = http.get(url).timeout(timeout);
    if let Some(key) = api_key {
        builder = builder.bearer_auth(key.expose_secret());
    }
    let response = builder.send().await.map_err(transport_error)?;
    let status = response.status().as_u16();
    if matches!(status, 404 | 405 | 501) {
        return Ok(Vec::new());
    }
    let text = response.text().await.map_err(transport_error)?;
    if !(200..300).contains(&status) {
        // No model is involved; 404 was handled above.
        return Err(status_error(status, &text, ""));
    }
    #[derive(Deserialize)]
    struct ModelList {
        data: Vec<ModelEntry>,
    }
    #[derive(Deserialize)]
    struct ModelEntry {
        id: String,
    }
    let list: ModelList =
        serde_json::from_str(&text).map_err(|e| LlmError::Malformed(e.to_string()))?;
    let mut ids: Vec<String> = list
        .data
        .into_iter()
        .map(|m| m.id)
        .filter(|id| !id.trim().is_empty())
        .collect();
    ids.sort();
    ids.dedup();
    Ok(ids)
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
    } else if e.is_body() || e.is_decode() {
        LlmError::Malformed(error_chain(&e.without_url()))
    } else {
        // The URL is dropped in case `base_url` carries credentials.
        LlmError::Connection(error_chain(&e.without_url()))
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
struct CompletionBody {
    model: Option<String>,
    choices: Vec<Choice>,
}

#[derive(Deserialize)]
struct Choice {
    message: Option<Message>,
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct Message {
    content: Option<String>,
    refusal: Option<String>,
}

fn parse_body(text: &str, requested_model: &str) -> Result<ChatResponse, LlmError> {
    let body: CompletionBody =
        serde_json::from_str(text).map_err(|e| LlmError::Malformed(e.to_string()))?;
    let choice = body
        .choices
        .into_iter()
        .next()
        .ok_or_else(|| LlmError::Malformed("no choices in response".into()))?;
    let message = choice.message;
    let refused = message.as_ref().is_some_and(|m| m.refusal.is_some());
    match choice.finish_reason.as_deref() {
        Some("content_filter") => return Err(LlmError::Refused),
        _ if refused => return Err(LlmError::Refused),
        Some("length") => return Err(LlmError::Truncated),
        _ => {}
    }
    let text = message
        .and_then(|m| m.content)
        .ok_or_else(|| LlmError::Malformed("response has no message content".into()))?;
    let model = body
        .model
        .filter(|m| !m.is_empty())
        .unwrap_or_else(|| requested_model.to_string());
    Ok(ChatResponse { text, model })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::collect_stream;
    use serde_json::json;
    use wiremock::matchers::{bearer_token, body_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn config(base_url: &str, key: Option<&str>) -> LlmConfig {
        LlmConfig {
            base_url: base_url.into(),
            model: "test-model".into(),
            api_key: key.map(SecretString::from),
        }
    }

    fn client(base_url: &str, key: Option<&str>) -> OpenAiClient {
        OpenAiClient::new(config(base_url, key))
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

    fn ok_body(content: &str) -> serde_json::Value {
        json!({
            "id": "chatcmpl-1",
            "model": "test-model-2026-01-01",
            "choices": [{
                "index": 0,
                "message": { "role": "assistant", "content": content, "refusal": null },
                "finish_reason": "stop"
            }]
        })
    }

    async fn mount(server: &MockServer, template: ResponseTemplate) {
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(template)
            .mount(server)
            .await;
    }

    async fn request_count(server: &MockServer) -> usize {
        server.received_requests().await.unwrap().len()
    }

    #[test]
    fn new_requires_base_url_and_model() {
        assert!(matches!(
            OpenAiClient::new(config("", None)),
            Err(LlmError::NotConfigured)
        ));
        let mut cfg = config("http://localhost/v1", None);
        cfg.model = " ".into();
        assert!(matches!(
            OpenAiClient::new(cfg),
            Err(LlmError::NotConfigured)
        ));
        let c = OpenAiClient::new(config("http://localhost/v1/", None)).unwrap();
        assert_eq!(c.model(), "test-model");
        assert_eq!(c.url(), "http://localhost/v1/chat/completions");
    }

    #[test]
    fn truncate_chars_counts_characters() {
        assert_eq!(truncate_chars("abc", 5), "abc");
        assert_eq!(truncate_chars("ééééé", 2), "éé…");
    }

    #[tokio::test]
    async fn success_sends_bearer_and_exact_body() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .and(bearer_token("sk-test"))
            .and(body_json(json!({
                "model": "test-model",
                "messages": [
                    { "role": "system", "content": "sys prompt" },
                    { "role": "user", "content": "user prompt" },
                ],
                "stream": false,
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(ok_body("Hello")))
            .expect(1)
            .mount(&server)
            .await;

        // Trailing slash on the base URL is trimmed.
        let c = client(&format!("{}/v1/", server.uri()), Some("sk-test"));
        let resp = c.chat(request()).await.unwrap();
        assert_eq!(resp.text, "Hello");
        assert_eq!(resp.model, "test-model-2026-01-01");

        let received = server.received_requests().await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&received[0].body).unwrap();
        let keys: Vec<_> = body.as_object().unwrap().keys().cloned().collect();
        assert_eq!(keys.len(), 3, "unexpected keys: {keys:?}");
        assert!(body.get("temperature").is_none());
    }

    #[tokio::test]
    async fn no_authorization_header_without_key_and_model_falls_back() {
        let server = MockServer::start().await;
        mount(
            &server,
            ResponseTemplate::new(200).set_body_json(json!({
                "choices": [{ "message": { "content": "OK" }, "finish_reason": "stop" }]
            })),
        )
        .await;
        let resp = client(&format!("{}/v1", server.uri()), None)
            .chat(request())
            .await
            .unwrap();
        assert_eq!(resp.text, "OK");
        assert_eq!(resp.model, "test-model");
        let received = server.received_requests().await.unwrap();
        assert!(!received[0].headers.contains_key("authorization"));
    }

    async fn error_for(status: u16) -> (LlmError, usize) {
        let server = MockServer::start().await;
        mount(
            &server,
            ResponseTemplate::new(status).set_body_string("{\"error\":\"nope\"}"),
        )
        .await;
        let err = client(&format!("{}/v1", server.uri()), Some("sk-test"))
            .chat(request())
            .await
            .unwrap_err();
        (err, request_count(&server).await)
    }

    #[tokio::test]
    async fn status_401_is_unauthorized_not_retried() {
        assert_eq!(error_for(401).await, (LlmError::Unauthorized, 1));
        assert_eq!(error_for(403).await, (LlmError::Unauthorized, 1));
    }

    #[tokio::test]
    async fn status_404_is_model_not_found() {
        assert_eq!(
            error_for(404).await,
            (LlmError::ModelNotFound("test-model".into()), 1)
        );
    }

    #[tokio::test]
    async fn status_400_is_not_retried() {
        assert_eq!(
            error_for(400).await,
            (
                LlmError::Http {
                    status: 400,
                    body: "{\"error\":\"nope\"}".into()
                },
                1
            )
        );
    }

    #[tokio::test]
    async fn status_500_retried_three_times() {
        let (err, count) = error_for(500).await;
        assert!(matches!(err, LlmError::Http { status: 500, .. }), "{err:?}");
        assert_eq!(count, 3);
    }

    #[tokio::test]
    async fn status_429_exhausted_is_rate_limited() {
        assert_eq!(error_for(429).await, (LlmError::RateLimited, 3));
    }

    #[tokio::test]
    async fn status_429_then_200_succeeds() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(429))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        mount(
            &server,
            ResponseTemplate::new(200).set_body_json(ok_body("after retry")),
        )
        .await;
        let resp = client(&format!("{}/v1", server.uri()), None)
            .chat(request())
            .await
            .unwrap();
        assert_eq!(resp.text, "after retry");
        assert_eq!(request_count(&server).await, 2);
    }

    #[tokio::test]
    async fn long_error_body_is_truncated() {
        let server = MockServer::start().await;
        mount(
            &server,
            ResponseTemplate::new(418).set_body_string("x".repeat(2000)),
        )
        .await;
        let err = client(&format!("{}/v1", server.uri()), None)
            .chat(request())
            .await
            .unwrap_err();
        match err {
            LlmError::Http { status, body } => {
                assert_eq!(status, 418);
                assert_eq!(body.chars().count(), MAX_ERROR_BODY_CHARS + 1);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    async fn result_for_body(body: serde_json::Value) -> Result<ChatResponse, LlmError> {
        let server = MockServer::start().await;
        mount(&server, ResponseTemplate::new(200).set_body_json(body)).await;
        client(&format!("{}/v1", server.uri()), None)
            .chat(request())
            .await
    }

    #[tokio::test]
    async fn finish_reason_length_is_truncated() {
        let err = result_for_body(json!({
            "choices": [{ "message": { "content": "partial" }, "finish_reason": "length" }]
        }))
        .await
        .unwrap_err();
        assert_eq!(err, LlmError::Truncated);
    }

    #[tokio::test]
    async fn refusal_is_refused() {
        let err = result_for_body(json!({
            "choices": [{
                "message": { "content": null, "refusal": "I can't help with that." },
                "finish_reason": "stop"
            }]
        }))
        .await
        .unwrap_err();
        assert_eq!(err, LlmError::Refused);

        let err = result_for_body(json!({
            "choices": [{ "message": { "content": "" }, "finish_reason": "content_filter" }]
        }))
        .await
        .unwrap_err();
        assert_eq!(err, LlmError::Refused);
    }

    #[tokio::test]
    async fn malformed_json_is_malformed() {
        let server = MockServer::start().await;
        mount(
            &server,
            ResponseTemplate::new(200).set_body_raw("not json {", "application/json"),
        )
        .await;
        let err = client(&format!("{}/v1", server.uri()), None)
            .chat(request())
            .await
            .unwrap_err();
        assert!(matches!(err, LlmError::Malformed(_)), "{err:?}");
        assert_eq!(request_count(&server).await, 1);

        let err = result_for_body(json!({ "choices": [] })).await.unwrap_err();
        assert!(matches!(err, LlmError::Malformed(_)), "{err:?}");
        let err = result_for_body(json!({ "object": "list" }))
            .await
            .unwrap_err();
        assert!(matches!(err, LlmError::Malformed(_)), "{err:?}");
    }

    #[tokio::test]
    async fn connection_refused_is_connection() {
        let port = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            listener.local_addr().unwrap().port()
        };
        let err = client(&format!("http://127.0.0.1:{port}/v1"), Some("sk-secret"))
            .chat(request())
            .await
            .unwrap_err();
        match &err {
            LlmError::Connection(msg) => assert!(!msg.contains("sk-secret")),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[tokio::test]
    async fn slow_response_times_out() {
        let server = MockServer::start().await;
        mount(
            &server,
            ResponseTemplate::new(200)
                .set_body_json(ok_body("late"))
                .set_delay(Duration::from_secs(5)),
        )
        .await;
        let err = client(&format!("{}/v1", server.uri()), None)
            .with_timeout(Duration::from_millis(150))
            .chat(request())
            .await
            .unwrap_err();
        assert_eq!(err, LlmError::Timeout);
        assert_eq!(request_count(&server).await, 3);
    }

    #[test]
    fn chat_future_is_send_and_static() {
        fn assert_send_static<T: Send + 'static>(_: &T) {}
        let c = OpenAiClient::new(config("http://localhost/v1", None)).unwrap();
        let fut = c.chat(request());
        drop(c);
        assert_send_static(&fut);
    }

    // ---- chat_stream ----

    fn sse(events: &[&str]) -> ResponseTemplate {
        let body: String = events.iter().map(|e| format!("data: {e}\n\n")).collect();
        ResponseTemplate::new(200)
            .insert_header("content-type", "text/event-stream")
            .set_body_raw(body.into_bytes(), "text/event-stream")
    }

    fn chunk(content: &str) -> String {
        json!({
            "id": "chatcmpl-1",
            "object": "chat.completion.chunk",
            "model": "test-model-2026-01-01",
            "choices": [{ "index": 0, "delta": { "content": content }, "finish_reason": null }]
        })
        .to_string()
    }

    fn finish_chunk(reason: &str) -> String {
        json!({
            "model": "test-model-2026-01-01",
            "choices": [{ "index": 0, "delta": {}, "finish_reason": reason }]
        })
        .to_string()
    }

    async fn stream_items(c: &OpenAiClient) -> Vec<Result<ChatEvent, LlmError>> {
        c.chat_stream(request()).collect().await
    }

    fn delta(s: &str) -> Result<ChatEvent, LlmError> {
        Ok(ChatEvent::Delta(s.into()))
    }

    fn done(model: &str) -> Result<ChatEvent, LlmError> {
        Ok(ChatEvent::Done {
            model: model.into(),
        })
    }

    #[tokio::test]
    async fn stream_assembles_deltas_in_order_and_sends_stream_true() {
        let server = MockServer::start().await;
        let role = json!({
            "model": "test-model-2026-01-01",
            "choices": [{ "index": 0, "delta": { "role": "assistant", "content": "" }, "finish_reason": null }]
        })
        .to_string();
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .and(bearer_token("sk-test"))
            .and(body_json(json!({
                "model": "test-model",
                "messages": [
                    { "role": "system", "content": "sys prompt" },
                    { "role": "user", "content": "user prompt" },
                ],
                "stream": true,
            })))
            .respond_with(sse(&[
                &role,
                &chunk("Hel"),
                &chunk("lo, "),
                &chunk("world"),
                &finish_chunk("stop"),
                "[DONE]",
            ]))
            .expect(2)
            .mount(&server)
            .await;
        let c = client(&format!("{}/v1", server.uri()), Some("sk-test"));
        assert_eq!(
            stream_items(&c).await,
            vec![
                delta("Hel"),
                delta("lo, "),
                delta("world"),
                done("test-model-2026-01-01")
            ]
        );
        let reply = collect_stream(c.chat_stream(request())).await.unwrap();
        assert_eq!(reply.text, "Hello, world");
    }

    #[tokio::test]
    async fn stream_without_key_has_no_authorization_and_model_falls_back() {
        let server = MockServer::start().await;
        let plain = |content: &str| {
            json!({ "choices": [{ "delta": { "content": content }, "finish_reason": null }] })
                .to_string()
        };
        mount(&server, sse(&[&plain("a"), &plain("b"), "[DONE]"])).await;
        let c = client(&format!("{}/v1", server.uri()), None);
        assert_eq!(
            stream_items(&c).await,
            vec![delta("a"), delta("b"), done("test-model")]
        );
        let received = server.received_requests().await.unwrap();
        assert!(!received[0].headers.contains_key("authorization"));
    }

    #[tokio::test]
    async fn stream_takes_model_from_chunks() {
        let server = MockServer::start().await;
        let first = json!({
            "model": "served-model",
            "choices": [{ "delta": { "content": "x" }, "finish_reason": null }]
        })
        .to_string();
        let no_model = json!({
            "choices": [{ "delta": { "content": "y" }, "finish_reason": "stop" }]
        })
        .to_string();
        mount(&server, sse(&[&first, &no_model, "[DONE]"])).await;
        let c = client(&format!("{}/v1", server.uri()), None);
        assert_eq!(
            stream_items(&c).await,
            vec![delta("x"), delta("y"), done("served-model")]
        );
    }

    #[tokio::test]
    async fn stream_missing_done_is_truncated() {
        let server = MockServer::start().await;
        mount(&server, sse(&[&chunk("a"), &chunk("b")])).await;
        let c = client(&format!("{}/v1", server.uri()), None);
        assert_eq!(
            stream_items(&c).await,
            vec![delta("a"), delta("b"), Err(LlmError::Truncated)]
        );
        // Text was yielded, so no retry.
        assert_eq!(request_count(&server).await, 1);
    }

    #[tokio::test]
    async fn stream_finish_stop_without_done_is_done() {
        let server = MockServer::start().await;
        mount(&server, sse(&[&chunk("a"), &finish_chunk("stop")])).await;
        let c = client(&format!("{}/v1", server.uri()), None);
        assert_eq!(
            stream_items(&c).await,
            vec![delta("a"), done("test-model-2026-01-01")]
        );
    }

    #[tokio::test]
    async fn stream_finish_length_is_truncated_and_content_filter_refused() {
        for (reason, err) in [
            ("length", LlmError::Truncated),
            ("content_filter", LlmError::Refused),
        ] {
            let server = MockServer::start().await;
            mount(
                &server,
                sse(&[&chunk("a"), &finish_chunk(reason), "[DONE]"]),
            )
            .await;
            let c = client(&format!("{}/v1", server.uri()), None);
            assert_eq!(stream_items(&c).await, vec![delta("a"), Err(err)]);
        }
    }

    #[tokio::test]
    async fn stream_error_object_maps_like_http() {
        let cases = [
            (
                json!({ "error": { "message": "slow down", "type": "requests", "code": "rate_limit_exceeded" } }),
                LlmError::RateLimited,
            ),
            (
                json!({ "error": { "message": "bad key", "code": 401 } }),
                LlmError::Unauthorized,
            ),
            (
                json!({ "error": { "message": "bad thing", "type": "invalid_request_error" } }),
                LlmError::Http {
                    status: 400,
                    body: "bad thing".into(),
                },
            ),
        ];
        for (error, expected) in cases {
            let server = MockServer::start().await;
            mount(&server, sse(&[&chunk("a"), &error.to_string()])).await;
            let c = client(&format!("{}/v1", server.uri()), None);
            assert_eq!(stream_items(&c).await, vec![delta("a"), Err(expected)]);
        }
    }

    #[tokio::test]
    async fn stream_error_object_before_text_is_retried() {
        let server = MockServer::start().await;
        let overloaded = json!({ "error": { "message": "overloaded" } }).to_string();
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(sse(&[&overloaded]))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        mount(&server, sse(&[&chunk("ok"), "[DONE]"])).await;
        let c = client(&format!("{}/v1", server.uri()), None);
        assert_eq!(
            stream_items(&c).await,
            vec![delta("ok"), done("test-model-2026-01-01")]
        );
        assert_eq!(request_count(&server).await, 2);
    }

    #[tokio::test]
    async fn stream_malformed_chunk_is_malformed() {
        let server = MockServer::start().await;
        mount(&server, sse(&[&chunk("a"), "{not json"])).await;
        let c = client(&format!("{}/v1", server.uri()), None);
        let items = stream_items(&c).await;
        assert_eq!(items.len(), 2);
        assert!(matches!(items[1], Err(LlmError::Malformed(_))), "{items:?}");
    }

    #[tokio::test]
    async fn stream_falls_back_to_json_body() {
        let server = MockServer::start().await;
        mount(
            &server,
            ResponseTemplate::new(200).set_body_json(ok_body("whole reply")),
        )
        .await;
        let c = client(&format!("{}/v1", server.uri()), None);
        assert_eq!(
            stream_items(&c).await,
            vec![delta("whole reply"), done("test-model-2026-01-01")]
        );

        let server = MockServer::start().await;
        mount(
            &server,
            ResponseTemplate::new(200).set_body_json(json!({
                "choices": [{ "message": { "content": "cut" }, "finish_reason": "length" }]
            })),
        )
        .await;
        let c = client(&format!("{}/v1", server.uri()), None);
        assert_eq!(stream_items(&c).await, vec![Err(LlmError::Truncated)]);
    }

    async fn stream_error_for(status: u16) -> (Vec<Result<ChatEvent, LlmError>>, usize) {
        let server = MockServer::start().await;
        mount(
            &server,
            ResponseTemplate::new(status).set_body_string("{\"error\":\"nope\"}"),
        )
        .await;
        let c = client(&format!("{}/v1", server.uri()), Some("sk-test"));
        (stream_items(&c).await, request_count(&server).await)
    }

    #[tokio::test]
    async fn stream_statuses_map_and_retry() {
        assert_eq!(
            stream_error_for(401).await,
            (vec![Err(LlmError::Unauthorized)], 1)
        );
        assert_eq!(
            stream_error_for(404).await,
            (vec![Err(LlmError::ModelNotFound("test-model".into()))], 1)
        );
        assert_eq!(
            stream_error_for(429).await,
            (vec![Err(LlmError::RateLimited)], 3)
        );
        assert_eq!(
            stream_error_for(500).await,
            (
                vec![Err(LlmError::Http {
                    status: 500,
                    body: "{\"error\":\"nope\"}".into()
                })],
                3
            )
        );
    }

    #[tokio::test]
    async fn stream_retries_429_and_500_then_succeeds() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(429))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(503))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        mount(&server, sse(&[&chunk("after retry"), "[DONE]"])).await;
        let c = client(&format!("{}/v1", server.uri()), None);
        assert_eq!(
            stream_items(&c).await,
            vec![delta("after retry"), done("test-model-2026-01-01")]
        );
        assert_eq!(request_count(&server).await, 3);
    }

    #[tokio::test]
    async fn stream_slow_start_times_out_after_retries() {
        let server = MockServer::start().await;
        mount(&server, sse(&["[DONE]"]).set_delay(Duration::from_secs(5))).await;
        let c =
            client(&format!("{}/v1", server.uri()), None).with_timeout(Duration::from_millis(150));
        assert_eq!(stream_items(&c).await, vec![Err(LlmError::Timeout)]);
        assert_eq!(request_count(&server).await, 3);
    }

    #[tokio::test]
    async fn stream_connection_refused_is_connection_without_key() {
        let port = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            listener.local_addr().unwrap().port()
        };
        let c = client(&format!("http://127.0.0.1:{port}/v1"), Some("sk-secret"));
        let items = stream_items(&c).await;
        match items.as_slice() {
            [Err(LlmError::Connection(msg))] => assert!(!msg.contains("sk-secret")),
            other => panic!("unexpected {other:?}"),
        }
    }

    /// A tiny HTTP server for what wiremock can't do: it answers every
    /// connection with SSE headers, writes `chunks` (one chunked-encoding
    /// chunk each, `pace` apart), then either drops the connection or stalls.
    /// Returns the base URL and a connection counter.
    fn raw_sse_server(
        chunks: Vec<String>,
        pace: Duration,
        stall: bool,
    ) -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
        use std::io::{BufRead, BufReader, Read, Write};
        use std::sync::atomic::{AtomicUsize, Ordering};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let count = std::sync::Arc::new(AtomicUsize::new(0));
        let counter = count.clone();
        std::thread::spawn(move || {
            for conn in listener.incoming() {
                let Ok(mut conn) = conn else { return };
                counter.fetch_add(1, Ordering::SeqCst);
                let chunks = chunks.clone();
                std::thread::spawn(move || {
                    // Read the request head and body.
                    let mut reader = BufReader::new(conn.try_clone().unwrap());
                    let mut length = 0;
                    loop {
                        let mut line = String::new();
                        if reader.read_line(&mut line).unwrap_or(0) == 0 {
                            return;
                        }
                        let lower = line.to_ascii_lowercase();
                        if let Some(v) = lower.strip_prefix("content-length:") {
                            length = v.trim().parse().unwrap_or(0);
                        }
                        if line == "\r\n" {
                            break;
                        }
                    }
                    let mut body = vec![0; length];
                    let _ = reader.read_exact(&mut body);
                    let _ = conn.write_all(
                        b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\
                          transfer-encoding: chunked\r\n\r\n",
                    );
                    for c in &chunks {
                        std::thread::sleep(pace);
                        let _ = write!(conn, "{:x}\r\n{c}\r\n", c.len());
                        let _ = conn.flush();
                    }
                    if stall {
                        std::thread::sleep(Duration::from_secs(5));
                    }
                    // Dropping `conn` closes it without the final chunk.
                });
            }
        });
        (format!("http://{addr}/v1"), count)
    }

    #[tokio::test]
    async fn stream_dropped_after_text_is_connection_and_not_retried() {
        let (url, count) = raw_sse_server(
            vec![format!("data: {}\n\n", chunk("partial"))],
            Duration::ZERO,
            false,
        );
        let items = stream_items(&client(&url, None)).await;
        assert_eq!(items.len(), 2, "{items:?}");
        assert_eq!(items[0], delta("partial"));
        assert!(
            matches!(items[1], Err(LlmError::Connection(_))),
            "{items:?}"
        );
        assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn stream_dropped_before_text_is_retried() {
        let (url, count) = raw_sse_server(vec![": keep-alive\n\n".into()], Duration::ZERO, false);
        let items = stream_items(&client(&url, None)).await;
        assert!(
            matches!(items.as_slice(), [Err(LlmError::Connection(_))]),
            "{items:?}"
        );
        assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn stream_gap_between_chunks_times_out() {
        let (url, count) = raw_sse_server(
            vec![format!("data: {}\n\n", chunk("first"))],
            Duration::ZERO,
            true,
        );
        let c = client(&url, None).with_timeout(Duration::from_millis(300));
        let items = stream_items(&c).await;
        assert_eq!(items, vec![delta("first"), Err(LlmError::Timeout)]);
        assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn stream_timeout_bounds_gaps_not_the_whole_stream() {
        // Four gaps of 150 ms under a 400 ms timeout: 600 ms in all.
        let chunks = ["a", "b", "c"]
            .iter()
            .map(|t| format!("data: {}\n\n", chunk(t)))
            .chain([format!(
                "data: {}\n\ndata: [DONE]\n\n",
                finish_chunk("stop")
            )])
            .collect();
        let (url, _) = raw_sse_server(chunks, Duration::from_millis(150), false);
        let c = client(&url, None).with_timeout(Duration::from_millis(400));
        assert_eq!(
            stream_items(&c).await,
            vec![
                delta("a"),
                delta("b"),
                delta("c"),
                done("test-model-2026-01-01")
            ]
        );
    }

    #[test]
    fn stream_is_send_and_static_and_debug_hides_key() {
        fn assert_send_static<T: Send + 'static>(_: &T) {}
        let c = OpenAiClient::new(config("http://localhost/v1", Some("sk-secret"))).unwrap();
        let stream = c.chat_stream(request());
        let models = c.list_models();
        assert!(!format!("{c:?}").contains("sk-secret"));
        drop(c);
        assert_send_static(&stream);
        assert_send_static(&models);
    }

    // ---- list_models ----

    #[tokio::test]
    async fn list_models_returns_sorted_ids_with_bearer() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .and(bearer_token("sk-test"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "object": "list",
                "data": [
                    { "id": "zeta", "object": "model" },
                    { "id": "alpha", "object": "model" },
                    { "id": "mid", "object": "model" },
                ]
            })))
            .expect(1)
            .mount(&server)
            .await;
        let models = client(&format!("{}/v1/", server.uri()), Some("sk-test"))
            .list_models()
            .await
            .unwrap();
        assert_eq!(models, vec!["alpha", "mid", "zeta"]);
    }

    #[tokio::test]
    async fn list_models_without_key_sends_no_authorization() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "data": [] })))
            .mount(&server)
            .await;
        let models = client(&format!("{}/v1", server.uri()), None)
            .list_models()
            .await
            .unwrap();
        assert!(models.is_empty());
        let received = server.received_requests().await.unwrap();
        assert!(!received[0].headers.contains_key("authorization"));
    }

    async fn models_for(status: u16) -> (Result<Vec<String>, LlmError>, usize) {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(ResponseTemplate::new(status).set_body_string("nope"))
            .mount(&server)
            .await;
        let result = client(&format!("{}/v1", server.uri()), Some("sk-test"))
            .list_models()
            .await;
        (result, request_count(&server).await)
    }

    #[tokio::test]
    async fn list_models_statuses() {
        assert_eq!(models_for(404).await, (Ok(vec![]), 1));
        assert_eq!(models_for(405).await, (Ok(vec![]), 1));
        assert_eq!(models_for(501).await, (Ok(vec![]), 1));
        assert_eq!(models_for(401).await, (Err(LlmError::Unauthorized), 1));
        assert_eq!(models_for(403).await, (Err(LlmError::Unauthorized), 1));
        assert_eq!(models_for(429).await, (Err(LlmError::RateLimited), 3));
        assert_eq!(
            models_for(500).await,
            (
                Err(LlmError::Http {
                    status: 500,
                    body: "nope".into()
                }),
                3
            )
        );
    }

    #[tokio::test]
    async fn list_models_malformed_body() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(ResponseTemplate::new(200).set_body_string("<html>"))
            .mount(&server)
            .await;
        let err = client(&format!("{}/v1", server.uri()), None)
            .list_models()
            .await
            .unwrap_err();
        assert!(matches!(err, LlmError::Malformed(_)), "{err:?}");
    }
}
