//! Finding a running Ollama and what each of its models can do.
//!
//! `detect` reads `GET {root}/api/tags` for the installed models, then
//! `POST {root}/api/show` for each one's `capabilities`:
//! - `decision`: a Jev-style decision model (Ollama 0.35+), used through
//!   `/v1/systemone` (`Protocol::OllamaDecision`); it can't chat.
//! - `thinking`: a reasoning model; it chats, and its thinking is dropped.
//! - `completion`: an ordinary chat model.
//!
//! Models without `completion` or `decision` (embedding models) are left
//! out. An older Ollama without `capabilities` lists every model as chat.
//! `root` is the base URL without its `/v1`.

use crate::llm::LlmError;
use futures_util::future::join_all;
use serde::Deserialize;
use std::time::Duration;

/// Ollama's OpenAI-compatible endpoint on this machine, default port.
pub const DEFAULT_BASE_URL: &str = "http://localhost:11434/v1";

/// The local Ollama's OpenAI-compatible base URL: from `OLLAMA_HOST` if
/// set (read the way Ollama reads it: `host`, `host:port` or a URL;
/// `0.0.0.0` means this machine), otherwise `DEFAULT_BASE_URL`.
pub fn local_base_url() -> String {
    from_host(std::env::var("OLLAMA_HOST").ok().as_deref())
}

fn from_host(host: Option<&str>) -> String {
    let Some(host) = host.map(str::trim).filter(|h| !h.is_empty()) else {
        return DEFAULT_BASE_URL.into();
    };
    let candidate = if host.contains("://") {
        host.to_string()
    } else if host.parse::<std::net::Ipv6Addr>().is_ok() {
        format!("http://[{host}]")
    } else if host.starts_with(':') {
        format!("http://127.0.0.1{host}")
    } else {
        format!("http://{host}")
    };
    let Ok(mut url) = reqwest::Url::parse(&candidate) else {
        return DEFAULT_BASE_URL.into();
    };
    match url.host_str() {
        Some("0.0.0.0") => {
            let _ = url.set_host(Some("127.0.0.1"));
        }
        Some("[::]") => {
            let _ = url.set_host(Some("[::1]"));
        }
        _ => {}
    }
    // Add Ollama's default port when the input did not name one explicitly
    // (reqwest drops `:80`/`:443` from `url.port()`).
    let authority = candidate
        .split_once("://")
        .map_or(candidate.as_str(), |(_, rest)| rest)
        .split('/')
        .next()
        .unwrap_or("");
    let explicit_default_port = (url.scheme() == "http" && authority.ends_with(":80"))
        || (url.scheme() == "https" && authority.ends_with(":443"));
    if url.port().is_none() && !explicit_default_port {
        let _ = url.set_port(Some(11434));
    }
    let path = url.path().trim_end_matches('/');
    let path = if path.ends_with("/v1") {
        path.to_string()
    } else {
        format!("{path}/v1")
    };
    url.set_path(&path);
    url.into()
}

/// What a model can be used for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ModelKind {
    Chat,
    Thinking,
    Decision,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OllamaModel {
    pub name: String,
    pub kind: ModelKind,
}

/// `http://host:11434/v1/` → `http://host:11434`.
pub fn root_url(base_url: &str) -> String {
    let url = base_url.trim().trim_end_matches('/');
    url.strip_suffix("/v1").unwrap_or(url).to_string()
}

/// The models of the Ollama at `base_url`, sorted by name. Fails (usually
/// `Connection`) when nothing answers there, so it doubles as "is Ollama
/// running?". `timeout` bounds each request.
pub async fn detect(base_url: &str, timeout: Duration) -> Result<Vec<OllamaModel>, LlmError> {
    let root = root_url(base_url);
    let http = crate::secure::client_builder()
        .connect_timeout(timeout)
        .timeout(timeout)
        .build()
        .map_err(|e| LlmError::Connection(crate::llm_http::error_chain(&e.without_url())))?;

    #[derive(Deserialize)]
    struct Tags {
        #[serde(default)]
        models: Vec<Tag>,
    }
    #[derive(Deserialize)]
    struct Tag {
        name: String,
    }
    let response = http
        .get(format!("{root}/api/tags"))
        .send()
        .await
        .map_err(crate::llm_http::transport_error)?;
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(crate::llm_http::transport_error)?;
    if !status.is_success() {
        return Err(LlmError::Http {
            status: status.as_u16(),
            body: crate::llm_http::truncate_chars(&body, 200),
        });
    }
    let tags: Tags = serde_json::from_str(&body).map_err(|e| LlmError::Malformed(e.to_string()))?;

    let shows = tags
        .models
        .iter()
        .map(|m| capabilities(&http, &root, &m.name));
    let caps = join_all(shows).await;
    let mut models: Vec<OllamaModel> = tags
        .models
        .into_iter()
        .zip(caps)
        .filter_map(|(m, caps)| {
            kind_of(caps.as_deref()).map(|kind| OllamaModel { name: m.name, kind })
        })
        .collect();
    models.sort_by(|a, b| a.name.cmp(&b.name));
    models.dedup_by(|a, b| a.name == b.name);
    Ok(models)
}

/// A model's `capabilities`, or `None` if `/api/show` didn't give them.
async fn capabilities(http: &reqwest::Client, root: &str, model: &str) -> Option<Vec<String>> {
    #[derive(Deserialize)]
    struct Show {
        capabilities: Option<Vec<String>>,
    }
    let response = http
        .post(format!("{root}/api/show"))
        .json(&serde_json::json!({ "model": model }))
        .send()
        .await
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    response.json::<Show>().await.ok()?.capabilities
}

/// `None` for models that can neither chat nor decide (embedding models).
fn kind_of(caps: Option<&[String]>) -> Option<ModelKind> {
    let Some(caps) = caps.filter(|c| !c.is_empty()) else {
        return Some(ModelKind::Chat);
    };
    let has = |c: &str| caps.iter().any(|x| x.eq_ignore_ascii_case(c));
    if has("decision") {
        Some(ModelKind::Decision)
    } else if !has("completion") {
        None
    } else if has("thinking") {
        Some(ModelKind::Thinking)
    } else {
        Some(ModelKind::Chat)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::matchers::{body_partial_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn ollama_host_is_read_like_ollama_reads_it() {
        assert_eq!(from_host(None), DEFAULT_BASE_URL);
        assert_eq!(from_host(Some(" ")), DEFAULT_BASE_URL);
        assert_eq!(from_host(Some("0.0.0.0")), "http://127.0.0.1:11434/v1");
        assert_eq!(
            from_host(Some("127.0.0.1:11435")),
            "http://127.0.0.1:11435/v1"
        );
        assert_eq!(
            from_host(Some("https://box.lan/")),
            "https://box.lan:11434/v1"
        );
        assert_eq!(from_host(Some(":11435")), "http://127.0.0.1:11435/v1");
        assert_eq!(from_host(Some("::")), "http://[::1]:11434/v1");
        assert_eq!(from_host(Some("::1")), "http://[::1]:11434/v1");
        assert_eq!(from_host(Some("[::1]:11435")), "http://[::1]:11435/v1");
        assert_eq!(
            from_host(Some("http://[::]:11435")),
            "http://[::1]:11435/v1"
        );
        assert_eq!(
            from_host(Some("http://localhost:11434/v1")),
            "http://localhost:11434/v1"
        );
        assert_eq!(
            from_host(Some("10.0.0.0:11434")),
            "http://10.0.0.0:11434/v1"
        );
    }

    #[test]
    fn root_url_drops_v1() {
        assert_eq!(
            root_url("http://localhost:11434/v1"),
            "http://localhost:11434"
        );
        assert_eq!(root_url(" http://h:1/v1/ "), "http://h:1");
        assert_eq!(root_url("http://h:1"), "http://h:1");
    }

    async fn show(server: &MockServer, model: &str, caps: serde_json::Value) {
        Mock::given(method("POST"))
            .and(path("/api/show"))
            .and(body_partial_json(json!({ "model": model })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "capabilities": caps })))
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn detect_sorts_models_by_what_they_can_do() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/tags"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "models": [
                    { "name": "qwen3:8b" },
                    { "name": "nimble:latest" },
                    { "name": "llama3.2:latest" },
                    { "name": "nomic-embed-text:latest" },
                    { "name": "old:latest" },
                ]
            })))
            .mount(&server)
            .await;
        show(
            &server,
            "qwen3:8b",
            json!(["completion", "tools", "thinking"]),
        )
        .await;
        show(&server, "nimble:latest", json!(["decision"])).await;
        show(&server, "llama3.2:latest", json!(["completion", "tools"])).await;
        show(&server, "nomic-embed-text:latest", json!(["embedding"])).await;
        // "old" has no /api/show mock: wiremock answers 404, so it counts
        // as a chat model.

        let models = detect(&format!("{}/v1", server.uri()), Duration::from_secs(5))
            .await
            .unwrap();
        let got: Vec<(&str, ModelKind)> =
            models.iter().map(|m| (m.name.as_str(), m.kind)).collect();
        assert_eq!(
            got,
            vec![
                ("llama3.2:latest", ModelKind::Chat),
                ("nimble:latest", ModelKind::Decision),
                ("old:latest", ModelKind::Chat),
                ("qwen3:8b", ModelKind::Thinking),
            ]
        );
    }

    #[tokio::test]
    async fn detect_fails_when_nothing_answers() {
        // A port nothing listens on.
        let err = detect("http://127.0.0.1:9/v1", Duration::from_secs(2))
            .await
            .unwrap_err();
        assert!(
            matches!(err, LlmError::Connection(_) | LlmError::Timeout),
            "{err:?}"
        );
    }
}
