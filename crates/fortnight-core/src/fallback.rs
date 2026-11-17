//! A client that tries one endpoint and falls back to another.
//!
//! Used for recommended tasks: the optional decision model
//! (`Settings::decision_llm`) is tried first, and the normal endpoint
//! answers instead whenever the decision model fails before it has sent
//! any text: not running, unreachable, model not pulled, key rejected,
//! rate limited, or cut off before its first word. An error after text has
//! started is final, as with any stream: that text can't be taken back.
//!
//! Make one `FallbackClient` per request when the caller needs to know
//! which endpoint answered (`used_fallback`), e.g. to record the host.

use crate::domain::LlmSettings;
use crate::llm::{
    BoxFuture, BoxStream, ChatEvent, ChatRequest, ChatResponse, LlmClient, LlmConfig, LlmError,
};
use futures_util::StreamExt;
use secrecy::SecretString;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// The config for the decision endpoint. It gets `key` (the main
/// endpoint's API key) only when it is on the same host as `main`, so the
/// key never goes to a provider it wasn't saved for.
pub fn decision_config(
    decision: &LlmSettings,
    main: &LlmSettings,
    key: Option<SecretString>,
) -> LlmConfig {
    let config = |llm: &LlmSettings, api_key| LlmConfig {
        base_url: llm.base_url.trim().to_string(),
        model: llm.model.trim().to_string(),
        api_key,
    };
    let main_host = config(main, None).endpoint_host();
    let mut out = config(decision, None);
    if main_host.is_some() && out.endpoint_host() == main_host {
        out.api_key = key;
    }
    out
}

pub struct FallbackClient {
    primary: Arc<dyn LlmClient>,
    fallback: Arc<dyn LlmClient>,
    used: Arc<AtomicBool>,
}

impl std::fmt::Debug for FallbackClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FallbackClient")
            .field("primary", &self.primary.model())
            .field("fallback", &self.fallback.model())
            .field("used_fallback", &self.used_fallback())
            .finish()
    }
}

impl FallbackClient {
    pub fn new(primary: Arc<dyn LlmClient>, fallback: Arc<dyn LlmClient>) -> Self {
        FallbackClient {
            primary,
            fallback,
            used: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Whether a request so far was answered (or failed) by the fallback.
    pub fn used_fallback(&self) -> bool {
        self.used.load(Ordering::SeqCst)
    }

    /// The model the fallback sends requests with.
    pub fn fallback_model(&self) -> &str {
        self.fallback.model()
    }
}

/// Where a `chat_stream` is up to.
enum Phase {
    /// Reading the primary; `started` once it has yielded text.
    Primary {
        stream: BoxStream<Result<ChatEvent, LlmError>>,
        started: bool,
        req: ChatRequest,
    },
    Fallback(BoxStream<Result<ChatEvent, LlmError>>),
}

impl LlmClient for FallbackClient {
    /// The primary's model: the one requests go to first.
    fn model(&self) -> &str {
        self.primary.model()
    }

    fn chat(&self, req: ChatRequest) -> BoxFuture<Result<ChatResponse, LlmError>> {
        let primary = self.primary.chat(req.clone());
        let fallback = self.fallback.clone();
        let used = self.used.clone();
        Box::pin(async move {
            match primary.await {
                Ok(resp) => Ok(resp),
                Err(_) => {
                    used.store(true, Ordering::SeqCst);
                    fallback.chat(req).await
                }
            }
        })
    }

    fn chat_stream(&self, req: ChatRequest) -> BoxStream<Result<ChatEvent, LlmError>> {
        let start = Phase::Primary {
            stream: self.primary.chat_stream(req.clone()),
            started: false,
            req,
        };
        let fallback = self.fallback.clone();
        let used = self.used.clone();
        Box::pin(futures_util::stream::unfold(start, move |phase| {
            let fallback = fallback.clone();
            let used = used.clone();
            async move {
                match phase {
                    Phase::Primary {
                        mut stream,
                        started,
                        req,
                    } => match stream.next().await? {
                        Err(_) if !started => {
                            used.store(true, Ordering::SeqCst);
                            let mut stream = fallback.chat_stream(req);
                            let item = stream.next().await?;
                            Some((item, Phase::Fallback(stream)))
                        }
                        item => {
                            let started = started || matches!(item, Ok(ChatEvent::Delta(_)));
                            Some((
                                item,
                                Phase::Primary {
                                    stream,
                                    started,
                                    req,
                                },
                            ))
                        }
                    },
                    Phase::Fallback(mut stream) => {
                        let item = stream.next().await?;
                        Some((item, Phase::Fallback(stream)))
                    }
                }
            }
        }))
    }

    fn list_models(&self) -> BoxFuture<Result<Vec<String>, LlmError>> {
        self.primary.list_models()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::{MockClient, collect_stream};

    /// A client whose stream yields `items` as given.
    struct Scripted {
        items: Vec<Result<ChatEvent, LlmError>>,
    }

    impl LlmClient for Scripted {
        fn model(&self) -> &str {
            "scripted"
        }
        fn chat(&self, _req: ChatRequest) -> BoxFuture<Result<ChatResponse, LlmError>> {
            Box::pin(async { Err(LlmError::Connection("refused".into())) })
        }
        fn chat_stream(&self, _req: ChatRequest) -> BoxStream<Result<ChatEvent, LlmError>> {
            Box::pin(futures_util::stream::iter(self.items.clone()))
        }
    }

    fn down() -> Arc<dyn LlmClient> {
        Arc::new(Scripted {
            items: vec![Err(LlmError::Connection("refused".into()))],
        })
    }

    #[test]
    fn decision_gets_the_key_only_on_the_main_host() {
        use crate::domain::Protocol;
        let llm = |url: &str| LlmSettings {
            protocol: Protocol::OpenAi,
            base_url: url.into(),
            model: "m".into(),
        };
        let key = || Some(SecretString::from("sk-1"));
        let main = llm("https://api.openai.com/v1");
        let same = decision_config(&llm(" https://api.openai.com/v1 "), &main, key());
        assert!(same.api_key.is_some());
        assert_eq!(same.base_url, "https://api.openai.com/v1");
        let other = decision_config(&llm("https://api.deepseek.com/v1"), &main, key());
        assert!(other.api_key.is_none());
        let local = decision_config(&llm("http://localhost:11434/v1"), &main, key());
        assert!(local.api_key.is_none());
        let unset = decision_config(&llm(""), &llm(""), key());
        assert!(unset.api_key.is_none());
    }

    #[tokio::test]
    async fn primary_answers_when_it_works() {
        let primary = Arc::new(MockClient::new("from decision"));
        let fallback = Arc::new(MockClient::new("from main"));
        let c = FallbackClient::new(primary, fallback.clone());
        let reply = collect_stream(c.chat_stream(ChatRequest::test()))
            .await
            .unwrap();
        assert_eq!(reply.text, "from decision");
        assert!(!c.used_fallback());
        assert!(fallback.requests.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn error_before_text_switches_to_the_fallback() {
        let fallback = Arc::new(MockClient::new("from main"));
        let c = FallbackClient::new(down(), fallback.clone());
        assert_eq!(c.model(), "scripted");
        let reply = collect_stream(c.chat_stream(ChatRequest::test()))
            .await
            .unwrap();
        assert_eq!(reply.text, "from main");
        assert_eq!(reply.model, "mock");
        assert!(c.used_fallback());
        assert_eq!(c.fallback_model(), "mock");
        assert_eq!(fallback.requests.lock().unwrap().len(), 1);

        let c = FallbackClient::new(down(), fallback);
        assert_eq!(c.chat(ChatRequest::test()).await.unwrap().text, "from main");
        assert!(c.used_fallback());
    }

    #[tokio::test]
    async fn error_after_text_is_final() {
        let primary = Arc::new(Scripted {
            items: vec![Ok(ChatEvent::Delta("half".into())), Err(LlmError::Timeout)],
        });
        let fallback = Arc::new(MockClient::new("from main"));
        let c = FallbackClient::new(primary, fallback.clone());
        let items: Vec<_> = c.chat_stream(ChatRequest::test()).collect().await;
        assert_eq!(
            items,
            vec![Ok(ChatEvent::Delta("half".into())), Err(LlmError::Timeout)]
        );
        assert!(!c.used_fallback());
        assert!(fallback.requests.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn both_failing_reports_the_fallback_error() {
        let fallback = Arc::new(MockClient::new(""));
        fallback.push_reply(Err(LlmError::Unauthorized));
        let c = FallbackClient::new(down(), fallback);
        assert_eq!(
            collect_stream(c.chat_stream(ChatRequest::test())).await,
            Err(LlmError::Unauthorized)
        );
    }
}
