//! Key-safety rules shared by both LLM clients (round 2 security review).
//!
//! - Redirects are never followed: a redirect to another host would carry
//!   the prompt, and for Anthropic the `x-api-key` header, with it.
//! - The key is never sent over plain `http://` to a host that isn't this
//!   machine.
//! - Error text from the endpoint is scrubbed of the key before it reaches
//!   the UI, the database or an export.

use crate::llm::{LlmConfig, LlmError};
use secrecy::{ExposeSecret, SecretString};

/// A reqwest client builder with redirects disabled. A 3xx reply then
/// surfaces as an HTTP error.
pub(crate) fn client_builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder().redirect(reqwest::redirect::Policy::none())
}

/// Refuses a configuration that would send the key in clear text over the
/// network: a key with an `http://` base URL whose host isn't loopback.
pub(crate) fn check_key_transport(config: &LlmConfig) -> Result<(), LlmError> {
    if config.api_key.is_none() {
        return Ok(());
    }
    let url = config.base_url.trim();
    let is_http = url
        .get(..7)
        .is_some_and(|s| s.eq_ignore_ascii_case("http://"));
    if !is_http || is_local_endpoint(url) {
        return Ok(());
    }
    Err(LlmError::Connection(format!(
        "refusing to send the API key over plain http to {}; use https, or mark the \
         endpoint as needing no API key",
        url_host(url)
    )))
}

/// Whether `base_url` points at this machine (`localhost`, `127.x.x.x` or
/// `::1`), as a local Ollama or LM Studio server does. Local endpoints get
/// a longer timeout and don't need the saved key unlocked.
pub fn is_local_endpoint(base_url: &str) -> bool {
    let host = url_host(base_url.trim());
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

/// The bare host of `url`: no scheme, credentials, port, path or brackets.
fn url_host(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host_port = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    if let Some(v6) = host_port.strip_prefix('[') {
        v6.split(']').next().unwrap_or("")
    } else {
        host_port.split(':').next().unwrap_or("")
    }
}

/// Replaces every occurrence of the key in an error's text.
pub(crate) fn redact(e: LlmError, key: Option<&SecretString>) -> LlmError {
    let Some(key) = key
        .map(ExposeSecret::expose_secret)
        .filter(|k| k.len() >= 4)
    else {
        return e;
    };
    let scrub = |s: String| s.replace(key, "[redacted]");
    match e {
        LlmError::Http { status, body } => LlmError::Http {
            status,
            body: scrub(body),
        },
        LlmError::Malformed(m) => LlmError::Malformed(scrub(m)),
        LlmError::Connection(m) => LlmError::Connection(scrub(m)),
        LlmError::ModelNotFound(m) => LlmError::ModelNotFound(scrub(m)),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(url: &str, key: bool) -> LlmConfig {
        LlmConfig {
            base_url: url.into(),
            model: "m".into(),
            api_key: key.then(|| SecretString::from("sk-secret-123")),
        }
    }

    #[test]
    fn key_never_goes_over_plain_http_to_a_remote_host() {
        assert!(check_key_transport(&cfg("http://api.example.com/v1", true)).is_err());
        assert!(check_key_transport(&cfg("HTTP://10.0.0.5:8000/v1", true)).is_err());
        assert!(check_key_transport(&cfg("http://api.example.com/v1", false)).is_ok());
        assert!(check_key_transport(&cfg("https://api.example.com/v1", true)).is_ok());
        assert!(check_key_transport(&cfg("http://localhost:11434/v1", true)).is_ok());
        assert!(check_key_transport(&cfg("http://127.0.0.1:9999/v1", true)).is_ok());
        assert!(check_key_transport(&cfg("http://[::1]:8080", true)).is_ok());
        // A loopback-looking user-info part doesn't count.
        assert!(check_key_transport(&cfg("http://localhost@evil.example/v1", true)).is_err());
    }

    #[test]
    fn local_endpoints_are_loopback_hosts_only() {
        assert!(is_local_endpoint("http://localhost:11434/v1"));
        assert!(is_local_endpoint(" http://127.0.0.1:1234/v1 "));
        assert!(is_local_endpoint("http://[::1]:11434"));
        assert!(is_local_endpoint("https://LOCALHOST/v1"));
        assert!(!is_local_endpoint("https://api.openai.com/v1"));
        assert!(!is_local_endpoint("http://127.example.com/v1"));
        assert!(!is_local_endpoint("http://localhost@evil.example/v1"));
        assert!(!is_local_endpoint(""));
    }

    #[test]
    fn redact_scrubs_the_key_from_error_text() {
        let key = SecretString::from("sk-secret-123");
        let e = redact(
            LlmError::Http {
                status: 400,
                body: "bad header: Bearer sk-secret-123".into(),
            },
            Some(&key),
        );
        assert_eq!(
            e,
            LlmError::Http {
                status: 400,
                body: "bad header: Bearer [redacted]".into()
            }
        );
        assert_eq!(
            redact(LlmError::Malformed("x sk-secret-123".into()), Some(&key)),
            LlmError::Malformed("x [redacted]".into())
        );
    }

    /// A redirect to another host must not carry the key (or the prompt):
    /// neither client follows it, and the target sees no request.
    #[tokio::test]
    async fn redirects_are_not_followed() {
        use crate::domain::Protocol;
        use crate::llm::{ChatRequest, collect_stream, connect};
        use wiremock::matchers::any;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let elsewhere = MockServer::start().await;
        Mock::given(any())
            .respond_with(ResponseTemplate::new(200))
            .expect(0)
            .mount(&elsewhere)
            .await;
        let gateway = MockServer::start().await;
        Mock::given(any())
            .respond_with(
                ResponseTemplate::new(307).insert_header("location", elsewhere.uri().as_str()),
            )
            .mount(&gateway)
            .await;
        for protocol in Protocol::ALL {
            let client = connect(protocol, cfg(&format!("{}/v1", gateway.uri()), true)).unwrap();
            let err = collect_stream(client.chat_stream(ChatRequest::test()))
                .await
                .unwrap_err();
            assert!(
                matches!(err, LlmError::Http { status: 307, .. }),
                "{protocol:?}: {err:?}"
            );
        }
        // `expect(0)` on `elsewhere` is verified when it drops.
    }
}
