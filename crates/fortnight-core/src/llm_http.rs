//! HTTP plumbing shared by the LLM clients (`openai`, `anthropic`,
//! `decision`) and the Ollama probe: timeouts, the shared reqwest
//! client, retries, and turning HTTP and transport failures into
//! `LlmError`s the same way everywhere.
//!
//! The two streaming state machines stay in their own modules: they differ
//! in how they filter reasoning and detect the end of a reply.

use crate::llm::{LlmConfig, LlmError};
use eventsource_stream::EventStreamError;
use secrecy::SecretString;
use serde::Deserialize;
use std::cell::RefCell;
use std::future::Future;
use std::time::Duration;

/// Attempts per request, counting the first.
pub(crate) const MAX_ATTEMPTS: u32 = 3;
/// Time allowed for each attempt against a remote endpoint.
pub(crate) const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);
/// For endpoints on this machine (see `secure::is_local_endpoint`): a local
/// model may need to load, and a reasoning model may think for minutes
/// before its first token.
pub(crate) const LOCAL_TIMEOUT: Duration = Duration::from_secs(600);
/// Time allowed to open a TCP (and TLS) connection.
pub(crate) const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Delay before the first retry; doubled before each later one.
pub(crate) const DEFAULT_BACKOFF: Duration = Duration::from_millis(500);
/// Error bodies are cut to this many characters (after the key is redacted).
pub(crate) const MAX_ERROR_BODY_CHARS: usize = 500;

/// The shared reqwest client, so every LLM client built on a thread shares
/// one connection pool (and TLS setup) instead of paying a new handshake
/// per request. In the app every client is built on the UI thread, and the
/// CLI runs on one thread, so each has a single client.
///
/// It is shared per thread, not per process, because a pooled connection is
/// driven by the tokio runtime that opened it: threads that each run their
/// own runtime (every `#[tokio::test]` does) would otherwise reuse each
/// other's connections, and one runtime shutting down would break another's
/// request.
///
/// It has no overall timeout: each request sets its own, and streams bound
/// their gaps with tokio. Redirects are never followed
/// (`secure::client_builder`).
pub(crate) fn shared_client() -> Result<reqwest::Client, LlmError> {
    thread_local! {
        static CLIENT: RefCell<Option<reqwest::Client>> = const { RefCell::new(None) };
    }
    CLIENT.with(|cell| {
        if let Some(client) = cell.borrow().as_ref() {
            return Ok(client.clone());
        }
        let client = crate::secure::client_builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .build()
            .map_err(|e| LlmError::Connection(error_chain(&e.without_url())))?;
        *cell.borrow_mut() = Some(client.clone());
        Ok(client)
    })
}

/// Checks `config` (base URL and model set, key transport allowed) and
/// returns the shared client and the per-attempt timeout for it.
pub(crate) fn prepare(config: &LlmConfig) -> Result<(reqwest::Client, Duration), LlmError> {
    if config.base_url.trim().is_empty() || config.model.trim().is_empty() {
        return Err(LlmError::NotConfigured);
    }
    crate::secure::check_key_transport(config)?;
    let timeout = if crate::secure::is_local_endpoint(&config.base_url) {
        LOCAL_TIMEOUT
    } else {
        DEFAULT_TIMEOUT
    };
    Ok((shared_client()?, timeout))
}

/// Adds the fields every client's `Debug` shows: the host only (the full
/// base URL may carry credentials), the model, and whether there is a key.
pub(crate) fn debug_endpoint<'d, 'a, 'b>(
    d: &'d mut std::fmt::DebugStruct<'a, 'b>,
    config: &LlmConfig,
) -> &'d mut std::fmt::DebugStruct<'a, 'b> {
    d.field("host", &config.endpoint_host())
        .field("model", &config.model)
        .field("has_key", &config.api_key.is_some())
}

/// Runs `attempt` up to `MAX_ATTEMPTS` times while it fails retryably.
pub(crate) async fn with_retries<T, F, Fut>(
    backoff: Duration,
    mut attempt: F,
) -> Result<T, LlmError>
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
pub(crate) fn backoff_delay(backoff: Duration, n: u32) -> Duration {
    backoff * 2u32.pow(n - 1)
}

/// Connection errors, timeouts, 429 and 5xx are worth another attempt.
pub(crate) fn is_retryable(e: &LlmError) -> bool {
    match e {
        LlmError::Connection(_) | LlmError::Timeout | LlmError::RateLimited => true,
        LlmError::Http { status, .. } => *status >= 500,
        _ => false,
    }
}

/// An error status (not 2xx) and its body (or error message) as an
/// `LlmError`. A 404 is `ModelNotFound` only when the body mentions the
/// model; otherwise (a wrong path, say a base URL without `/v1`) it stays
/// `Http { status: 404, .. }`. `key` is scrubbed from the body before it is
/// cut to `MAX_ERROR_BODY_CHARS`, so a key straddling the cut can't leak.
pub(crate) fn status_error(
    status: u16,
    body: &str,
    requested_model: &str,
    key: Option<&SecretString>,
) -> LlmError {
    match status {
        401 | 403 => LlmError::Unauthorized,
        404 if body.to_ascii_lowercase().contains("model") => {
            LlmError::ModelNotFound(requested_model.to_string())
        }
        429 => LlmError::RateLimited,
        code => LlmError::Http {
            status: code,
            body: error_body(body, key),
        },
    }
}

/// `body` trimmed, with the key redacted, then cut to
/// `MAX_ERROR_BODY_CHARS`.
pub(crate) fn error_body(body: &str, key: Option<&SecretString>) -> String {
    truncate_chars(
        &crate::secure::scrub(body.trim(), key),
        MAX_ERROR_BODY_CHARS,
    )
}

/// A reqwest failure as an `LlmError`: a timeout is `Timeout`, anything
/// else (refused, reset, TLS, or the body cut off mid-read, which reqwest
/// reports as "decode") is `Connection`, and so retryable. The code never
/// asks reqwest to parse JSON (it reads `.text()` and uses serde_json,
/// whose errors are `Malformed`), and without the `charset` feature
/// `.text()` decodes lossily, so no real decoding error comes from here.
/// The URL is dropped in case `base_url` carries credentials.
pub(crate) fn transport_error(e: reqwest::Error) -> LlmError {
    if e.is_timeout() {
        LlmError::Timeout
    } else {
        LlmError::Connection(error_chain(&e.without_url()))
    }
}

/// A server-sent-events failure: transport errors as `transport_error`,
/// anything else (bad UTF-8 or framing) as `Malformed`.
pub(crate) fn sse_error(e: EventStreamError<reqwest::Error>) -> LlmError {
    match e {
        EventStreamError::Transport(e) => transport_error(e),
        other => LlmError::Malformed(other.to_string()),
    }
}

/// `e` and its sources, joined with ": ".
pub(crate) fn error_chain(e: &dyn std::error::Error) -> String {
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

/// `s` cut to `max` characters, with "…" if anything was cut.
pub(crate) fn truncate_chars(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s.to_string(),
    }
}

/// The ids of a `{"data": [{"id": …}, …]}` model list (OpenAI and
/// Anthropic both answer this way), in the endpoint's order, without
/// blank ids.
pub(crate) fn parse_model_ids(text: &str) -> Result<Vec<String>, LlmError> {
    #[derive(Deserialize)]
    struct ModelList {
        data: Vec<ModelEntry>,
    }
    #[derive(Deserialize)]
    struct ModelEntry {
        id: String,
    }
    let list: ModelList =
        serde_json::from_str(text).map_err(|e| LlmError::Malformed(e.to_string()))?;
    Ok(list
        .data
        .into_iter()
        .map(|m| m.id)
        .filter(|id| !id.trim().is_empty())
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_chars_counts_characters() {
        assert_eq!(truncate_chars("abc", 5), "abc");
        assert_eq!(truncate_chars("ééééé", 2), "éé…");
    }

    #[test]
    fn a_404_is_model_not_found_only_when_it_says_so() {
        assert_eq!(
            status_error(
                404,
                r#"{"error":{"message":"The Model `x` does not exist"}}"#,
                "x",
                None
            ),
            LlmError::ModelNotFound("x".into())
        );
        assert_eq!(
            status_error(404, "404 page not found", "x", None),
            LlmError::Http {
                status: 404,
                body: "404 page not found".into()
            }
        );
        assert!(!is_retryable(&status_error(404, "nope", "x", None)));
    }

    /// The key is redacted before the body is cut, so a key that straddles
    /// the cut leaves no prefix of itself behind.
    #[test]
    fn key_straddling_the_cut_is_redacted_first() {
        let key = "sk-straddle-0123456789";
        let secret = SecretString::from(key);
        // The key starts 5 characters before the cut.
        let body = format!("{}{key} tail", "x".repeat(MAX_ERROR_BODY_CHARS - 5));
        let e = status_error(400, &body, "m", Some(&secret));
        let LlmError::Http { body, .. } = &e else {
            panic!("{e:?}")
        };
        assert!(!body.contains("sk-st"), "{body}");
        assert!(body.ends_with('…'));
        // Truncating first would have left "sk-st" in the text.
        assert!(truncate_chars(&format!("{}{key}", "x".repeat(495)), 500).contains("sk-st"));
    }

    #[test]
    fn model_ids_keep_order_and_drop_blanks() {
        assert_eq!(
            parse_model_ids(r#"{"data":[{"id":"b"},{"id":" "},{"id":"a"}]}"#),
            Ok(vec!["b".to_string(), "a".to_string()])
        );
        assert!(matches!(
            parse_model_ids("<html>"),
            Err(LlmError::Malformed(_))
        ));
    }

    #[test]
    fn prepare_checks_config_and_picks_the_timeout() {
        let cfg = |url: &str| LlmConfig {
            base_url: url.into(),
            model: "m".into(),
            api_key: None,
        };
        assert!(matches!(prepare(&cfg("")), Err(LlmError::NotConfigured)));
        let (_, local) = prepare(&cfg("http://localhost:11434/v1")).unwrap();
        assert_eq!(local, LOCAL_TIMEOUT);
        let (_, remote) = prepare(&cfg("https://api.openai.com/v1")).unwrap();
        assert_eq!(remote, DEFAULT_TIMEOUT);
    }

    /// Every client's Debug shows the host, never credentials in the base
    /// URL or the key.
    #[test]
    fn client_debug_is_host_only() {
        use crate::domain::Protocol;
        for protocol in [
            Protocol::OpenAi,
            Protocol::Anthropic,
            Protocol::OllamaDecision,
        ] {
            let config = LlmConfig {
                base_url: "https://user:hunter2@api.example.com/v1".into(),
                model: "m".into(),
                api_key: Some(SecretString::from("sk-debug-secret")),
            };
            let debug = match protocol {
                Protocol::OpenAi => format!("{:?}", crate::openai::OpenAiClient::new(config)),
                Protocol::Anthropic => {
                    format!("{:?}", crate::anthropic::AnthropicClient::new(config))
                }
                Protocol::OllamaDecision => {
                    format!("{:?}", crate::decision::DecisionClient::new(config))
                }
            };
            assert!(debug.contains("api.example.com"), "{debug}");
            assert!(!debug.contains("hunter2"), "{debug}");
            assert!(!debug.contains("user"), "{debug}");
            assert!(!debug.contains("sk-debug"), "{debug}");
        }
    }
}
