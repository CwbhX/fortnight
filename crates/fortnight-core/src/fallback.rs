//! A client that tries one endpoint and falls back to another.
//!
//! Used for recommended tasks: the optional decision model
//! (`Settings::decision_llm`) is tried first, and the normal endpoint
//! answers instead whenever the decision model fails before it has sent
//! any text: not running, unreachable, model not pulled, key rejected,
//! rate limited, or cut off before its first word. An error after text has
//! started is final, as with any stream: that text can't be taken back.
//!
//! The primary also gives way when it sends nothing for
//! `PRIMARY_FIRST_ITEM_DEADLINE`: a server that accepts the connection but
//! hangs would otherwise hold the request for its whole retry budget (up to
//! half an hour for a local one).
//!
//! Make one `FallbackClient` per request when the caller needs to know
//! which endpoint answered (`used_fallback`), e.g. to record the host;
//! `PlanRoute` does that bookkeeping for recommended tasks.

use crate::domain::LlmSettings;
use crate::llm::{
    BoxFuture, BoxStream, ChatEvent, ChatRequest, ChatResponse, LlmClient, LlmConfig, LlmError,
};
use futures_util::StreamExt;
use secrecy::SecretString;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// How long the primary may take to send its first item (text, the end of
/// the reply, or an error), or to answer a `chat`, before the fallback is
/// asked instead. Generous, so a slow but working model isn't abandoned.
pub const PRIMARY_FIRST_ITEM_DEADLINE: Duration = Duration::from_secs(120);

/// The config for the decision endpoint. A keyless one gets no key. It
/// uses its own key (`decision_key`, from `vault::decision_key_path`) when
/// there is one. Otherwise it borrows `main_key` only when it is on the
/// same host as `main`, so a key never goes to a provider it wasn't saved
/// for.
pub fn decision_config(
    decision: &LlmSettings,
    main: &LlmSettings,
    main_key: Option<SecretString>,
    decision_key: Option<SecretString>,
) -> LlmConfig {
    let config = |llm: &LlmSettings, api_key| LlmConfig::from_settings(llm, api_key);
    if decision.keyless {
        return config(decision, None);
    }
    if decision_key.is_some() {
        return config(decision, decision_key);
    }
    let main_host = config(main, None).endpoint_host();
    let mut out = config(decision, None);
    if main_host.is_some() && out.endpoint_host() == main_host {
        out.api_key = main_key;
    }
    out
}

pub struct FallbackClient {
    primary: Arc<dyn LlmClient>,
    fallback: Arc<dyn LlmClient>,
    used: Arc<AtomicBool>,
    /// `PRIMARY_FIRST_ITEM_DEADLINE`, shorter in tests.
    first_item_deadline: Duration,
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
            first_item_deadline: PRIMARY_FIRST_ITEM_DEADLINE,
        }
    }

    #[cfg(test)]
    fn with_first_item_deadline(mut self, deadline: Duration) -> Self {
        self.first_item_deadline = deadline;
        self
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
        let deadline = self.first_item_deadline;
        Box::pin(async move {
            match tokio::time::timeout(deadline, primary).await {
                Ok(Ok(resp)) => Ok(resp),
                // Failed, or no answer in time.
                Ok(Err(_)) | Err(_) => {
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
        let deadline = self.first_item_deadline;
        Box::pin(futures_util::stream::unfold(start, move |phase| {
            let fallback = fallback.clone();
            let used = used.clone();
            async move {
                match phase {
                    Phase::Primary {
                        mut stream,
                        started,
                        req,
                    } => {
                        // Until text starts, the primary gets a deadline; a
                        // miss counts as an error before text.
                        let item = if started {
                            stream.next().await?
                        } else {
                            tokio::time::timeout(deadline, stream.next())
                                .await
                                .unwrap_or(Some(Err(LlmError::Timeout)))?
                        };
                        match item {
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
                        }
                    }
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

/// A client and the host it talks to (`LlmConfig::endpoint_host`), for
/// recording on a report.
#[derive(Clone)]
pub struct Endpoint {
    pub client: Arc<dyn LlmClient>,
    pub host: Option<String>,
}

impl std::fmt::Debug for Endpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Endpoint")
            .field("model", &self.client.model())
            .field("host", &self.host)
            .finish()
    }
}

/// Where a recommended-tasks request goes, and afterwards who answered it.
/// With a decision model, requests go to it first and fall back to the
/// main endpoint (a `FallbackClient`); without one, to the main endpoint.
/// Make one per request: `answered_by` reads that request's outcome.
#[derive(Debug, Clone)]
pub struct PlanRoute {
    main: Endpoint,
    decision: Option<(Arc<FallbackClient>, Option<String>)>,
}

impl PlanRoute {
    /// `decision` is the decision model's endpoint, if one is configured
    /// and could be built; otherwise everything goes to `main`.
    pub fn new(main: Endpoint, decision: Option<Endpoint>) -> Self {
        let decision = decision.map(|d| {
            (
                Arc::new(FallbackClient::new(d.client, main.client.clone())),
                d.host,
            )
        });
        PlanRoute { main, decision }
    }

    /// The client to send the request to.
    pub fn client(&self) -> Arc<dyn LlmClient> {
        match &self.decision {
            Some((fc, _)) => fc.clone(),
            None => self.main.client.clone(),
        }
    }

    /// The requested model and host to record on the report: the decision
    /// model's unless the request fell back (or there is no decision
    /// model), then the main endpoint's. Call it once the request is over.
    pub fn answered_by(&self) -> (String, Option<String>) {
        match &self.decision {
            Some((fc, host)) if !fc.used_fallback() => (fc.model().to_string(), host.clone()),
            _ => (self.main.client.model().to_string(), self.main.host.clone()),
        }
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
            keyless: false,
        };
        use secrecy::ExposeSecret;
        let key = |k: &str| Some(SecretString::from(k.to_string()));
        let main = llm("https://api.openai.com/v1");
        let same = decision_config(
            &llm(" https://api.openai.com/v1 "),
            &main,
            key("sk-1"),
            None,
        );
        assert_eq!(same.api_key.unwrap().expose_secret(), "sk-1");
        assert_eq!(same.base_url, "https://api.openai.com/v1");
        let other = decision_config(
            &llm("https://api.deepseek.com/v1"),
            &main,
            key("sk-1"),
            None,
        );
        assert!(other.api_key.is_none());
        let local = decision_config(&llm("http://localhost:11434/v1"), &main, key("sk-1"), None);
        assert!(local.api_key.is_none());
        let unset = decision_config(&llm(""), &llm(""), key("sk-1"), None);
        assert!(unset.api_key.is_none());
        // Its own key wins, on any host.
        let own = decision_config(
            &llm("https://api.deepseek.com/v1"),
            &main,
            key("sk-1"),
            key("ds-2"),
        );
        assert_eq!(own.api_key.unwrap().expose_secret(), "ds-2");
        // A keyless endpoint gets no key, even its own or on the main host.
        let mut lan = llm("https://api.openai.com/v1");
        lan.keyless = true;
        assert!(
            decision_config(&lan, &main, key("sk-1"), key("ds-2"))
                .api_key
                .is_none()
        );
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

    /// A client that never answers.
    struct Hung;

    impl LlmClient for Hung {
        fn model(&self) -> &str {
            "hung"
        }
        fn chat(&self, _req: ChatRequest) -> BoxFuture<Result<ChatResponse, LlmError>> {
            Box::pin(futures_util::future::pending())
        }
        fn chat_stream(&self, _req: ChatRequest) -> BoxStream<Result<ChatEvent, LlmError>> {
            Box::pin(futures_util::stream::pending())
        }
    }

    #[tokio::test]
    async fn a_silent_primary_gives_way_at_the_deadline() {
        let fallback = Arc::new(MockClient::new("from main"));
        let short = Duration::from_millis(20);
        let c =
            FallbackClient::new(Arc::new(Hung), fallback.clone()).with_first_item_deadline(short);
        let reply = collect_stream(c.chat_stream(ChatRequest::test()))
            .await
            .unwrap();
        assert_eq!(reply.text, "from main");
        assert!(c.used_fallback());

        let c = FallbackClient::new(Arc::new(Hung), fallback).with_first_item_deadline(short);
        assert_eq!(c.chat(ChatRequest::test()).await.unwrap().text, "from main");
        assert!(c.used_fallback());
        assert_eq!(PRIMARY_FIRST_ITEM_DEADLINE, Duration::from_secs(120));
    }

    #[tokio::test]
    async fn the_deadline_does_not_cut_a_reply_that_has_started() {
        // Text, then a gap longer than the deadline, then the end.
        let primary = Arc::new(Scripted {
            items: vec![
                Ok(ChatEvent::Delta("a".into())),
                Ok(ChatEvent::Done { model: "p".into() }),
            ],
        });
        struct Slow(Arc<Scripted>);
        impl LlmClient for Slow {
            fn model(&self) -> &str {
                "slow"
            }
            fn chat(&self, req: ChatRequest) -> BoxFuture<Result<ChatResponse, LlmError>> {
                self.0.chat(req)
            }
            fn chat_stream(&self, req: ChatRequest) -> BoxStream<Result<ChatEvent, LlmError>> {
                Box::pin(self.0.chat_stream(req).then(|item| async move {
                    if matches!(item, Ok(ChatEvent::Done { .. })) {
                        tokio::time::sleep(Duration::from_millis(60)).await;
                    }
                    item
                }))
            }
        }
        let fallback = Arc::new(MockClient::new("from main"));
        let c = FallbackClient::new(Arc::new(Slow(primary)), fallback)
            .with_first_item_deadline(Duration::from_millis(20));
        let reply = collect_stream(c.chat_stream(ChatRequest::test()))
            .await
            .unwrap();
        assert_eq!(reply.text, "a");
        assert!(!c.used_fallback());
    }

    #[tokio::test]
    async fn plan_route_reports_who_answered() {
        let main = || Endpoint {
            client: Arc::new(MockClient::new("from main")),
            host: Some("main.example".into()),
        };
        // No decision model: the main endpoint, directly.
        let route = PlanRoute::new(main(), None);
        let reply = route.client().chat(ChatRequest::test()).await.unwrap();
        assert_eq!(reply.text, "from main");
        assert_eq!(
            route.answered_by(),
            ("mock".to_string(), Some("main.example".to_string()))
        );
        // The decision model answers.
        let decision = || Endpoint {
            client: Arc::new(Scripted {
                items: vec![
                    Ok(ChatEvent::Delta("picked".into())),
                    Ok(ChatEvent::Done {
                        model: "scripted".into(),
                    }),
                ],
            }),
            host: Some("localhost:11434".into()),
        };
        let route = PlanRoute::new(main(), Some(decision()));
        let reply = collect_stream(route.client().chat_stream(ChatRequest::test()))
            .await
            .unwrap();
        assert_eq!(reply.text, "picked");
        assert_eq!(
            route.answered_by(),
            ("scripted".to_string(), Some("localhost:11434".to_string()))
        );
        // It fails: the main endpoint answers and is recorded.
        let route = PlanRoute::new(
            main(),
            Some(Endpoint {
                client: down(),
                host: Some("localhost:11434".into()),
            }),
        );
        let reply = collect_stream(route.client().chat_stream(ChatRequest::test()))
            .await
            .unwrap();
        assert_eq!(reply.text, "from main");
        assert_eq!(
            route.answered_by(),
            ("mock".to_string(), Some("main.example".to_string()))
        );
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
