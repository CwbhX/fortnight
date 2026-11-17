//! OpenAI-compatible Chat Completions client. Owner: agent C.
//!
//! `POST {base_url}/chat/completions` with `Authorization: Bearer <key>`
//! (omitted when no key). Sends only `model` and `messages` (plus
//! `stream: false`): no temperature or other sampling parameters.
//! Maps HTTP and body errors onto `LlmError`. Retries up to three attempts
//! with backoff on connection errors, 429 and 5xx; never on other 4xx.
//!
//! Skipped for the prototype: `Retry-After` is not honoured (fixed
//! exponential backoff instead), and streaming is not supported.
//! Requires a tokio runtime (reqwest does too); the app runs iced with
//! its `tokio` feature.

use crate::llm::{BoxFuture, ChatRequest, ChatResponse, LlmClient, LlmConfig, LlmError};
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use std::time::Duration;

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

impl OpenAiClient {
    /// Errors with `NotConfigured` if base URL or model is empty.
    pub fn new(config: LlmConfig) -> Result<Self, LlmError> {
        if config.base_url.trim().is_empty() || config.model.trim().is_empty() {
            return Err(LlmError::NotConfigured);
        }
        let http = reqwest::Client::builder()
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

    fn url(&self) -> String {
        format!(
            "{}/chat/completions",
            self.config.base_url.trim().trim_end_matches('/')
        )
    }
}

impl LlmClient for OpenAiClient {
    fn model(&self) -> &str {
        &self.config.model
    }

    fn chat(&self, req: ChatRequest) -> BoxFuture<Result<ChatResponse, LlmError>> {
        let call = Call {
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
                "stream": false,
            }),
        };
        let backoff = self.backoff;
        Box::pin(async move {
            let mut attempt = 1;
            loop {
                match call.send().await {
                    Err(e) if attempt < MAX_ATTEMPTS && is_retryable(&e) => {
                        tokio::time::sleep(backoff * 2u32.pow(attempt - 1)).await;
                        attempt += 1;
                    }
                    result => return result,
                }
            }
        })
    }
}

/// Everything one request needs, owned, so the future is `'static`.
struct Call {
    http: reqwest::Client,
    url: String,
    model: String,
    api_key: Option<SecretString>,
    timeout: Duration,
    body: serde_json::Value,
}

impl Call {
    async fn send(&self) -> Result<ChatResponse, LlmError> {
        let mut builder = self
            .http
            .post(&self.url)
            .timeout(self.timeout)
            .json(&self.body);
        if let Some(key) = &self.api_key {
            // `bearer_auth` marks the header sensitive.
            builder = builder.bearer_auth(key.expose_secret());
        }
        let response = builder.send().await.map_err(transport_error)?;
        let status = response.status();
        let text = response.text().await.map_err(transport_error)?;

        match status.as_u16() {
            200..=299 => parse_body(&text, &self.model),
            401 | 403 => Err(LlmError::Unauthorized),
            404 => Err(LlmError::ModelNotFound(self.model.clone())),
            429 => Err(LlmError::RateLimited),
            code => Err(LlmError::Http {
                status: code,
                body: truncate_chars(text.trim(), MAX_ERROR_BODY_CHARS),
            }),
        }
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
}
