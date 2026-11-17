//! Ollama decision models (Jev-style, Ollama 0.35+) for recommended tasks.
//!
//! A decision model answers questions with choices and probabilities, not
//! text: `POST {root}/v1/systemone` with a `state` (the context) and
//! `questions` (`choice` between 2 to 26 named options, `noul` yes/no, or
//! `score`). `DecisionClient` is an `LlmClient` so it slots in wherever a
//! chat model would (fallback, streaming, CLI), but it only understands
//! the recommended-tasks request (`report::prepare_plan`): it reads the
//! plan facts from between `<facts>` and `</facts>`, offers the open tasks
//! as one `choice` question ("which first?"), ranks them by probability
//! and replies with the same JSON a chat model would
//! (`report::parse_recommendations`). The reasons are written here from
//! the facts ("overdue 2 days", "due Thursday"); the model gives none.
//!
//! The Settings Test button's request (`ChatRequest::test`) sends one
//! `noul` question and replies "OK". Any other request (a weekly summary)
//! is refused with `Malformed`, so a fallback can take it.
//!
//! Skipped for the prototype: retries (the fallback to the main endpoint
//! covers failures), the custom recommendations prompt (a decision model
//! gets the built-in rule as its question), and `score` questions.

use crate::llm::{BoxFuture, ChatRequest, ChatResponse, LlmClient, LlmConfig, LlmError};
use secrecy::ExposeSecret;
use serde::Deserialize;
use serde_json::{Value, json};
use std::time::Duration;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// The most options a `choice` question takes.
const MAX_OPTIONS: usize = 26;
/// How many tasks to recommend.
const PICKS: usize = 3;

pub struct DecisionClient {
    config: LlmConfig,
    http: reqwest::Client,
    timeout: Duration,
}

impl std::fmt::Debug for DecisionClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DecisionClient")
            .field("config", &self.config)
            .field("timeout", &self.timeout)
            .finish_non_exhaustive()
    }
}

impl DecisionClient {
    /// Errors with `NotConfigured` if base URL or model is empty.
    pub fn new(config: LlmConfig) -> Result<Self, LlmError> {
        if config.base_url.trim().is_empty() || config.model.trim().is_empty() {
            return Err(LlmError::NotConfigured);
        }
        crate::secure::check_key_transport(&config)?;
        let http = crate::secure::client_builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .build()
            .map_err(|e| LlmError::Connection(crate::openai::error_chain(&e)))?;
        let timeout = if crate::secure::is_local_endpoint(&config.base_url) {
            crate::openai::LOCAL_TIMEOUT
        } else {
            DEFAULT_TIMEOUT
        };
        Ok(DecisionClient {
            config,
            http,
            timeout,
        })
    }

    /// Time allowed for each request (default 120 s, 600 s locally).
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

impl LlmClient for DecisionClient {
    fn model(&self) -> &str {
        &self.config.model
    }

    fn chat(&self, req: ChatRequest) -> BoxFuture<Result<ChatResponse, LlmError>> {
        let model = self.config.model.trim().to_string();
        let url = format!(
            "{}/v1/systemone",
            crate::ollama::root_url(&self.config.base_url)
        );
        let mut builder = self.http.post(url).timeout(self.timeout);
        if let Some(key) = &self.config.api_key {
            builder = builder.bearer_auth(key.expose_secret());
        }
        let key = self.config.api_key.clone();
        Box::pin(async move {
            let result = if req == ChatRequest::test() {
                let body = json!({
                    "model": model,
                    "state": "A connection test.",
                    "questions": {
                        "ok": { "type": "noul", "instructions": "Is this a connection test?" }
                    }
                });
                ask(builder, body, &model).await.map(|reply| ChatResponse {
                    text: "OK".into(),
                    model: reply.model,
                })
            } else {
                match plan_question(&req.user, &model) {
                    None => Err(LlmError::Malformed(
                        "a decision model can only pick recommended tasks".into(),
                    )),
                    Some(Ask::Answer(text)) => Ok(ChatResponse { text, model }),
                    Some(Ask::Question { body, candidates }) => {
                        ask(builder, body, &model).await.map(|reply| ChatResponse {
                            text: plan_reply(&reply.answers, &candidates),
                            model: reply.model,
                        })
                    }
                }
            };
            result.map_err(|e| crate::secure::redact(e, key.as_ref()))
        })
    }
}

struct Reply {
    model: String,
    answers: Value,
}

async fn ask(
    builder: reqwest::RequestBuilder,
    body: Value,
    model: &str,
) -> Result<Reply, LlmError> {
    let response = builder
        .json(&body)
        .send()
        .await
        .map_err(crate::openai::transport_error)?;
    let status = response.status().as_u16();
    let text = response
        .text()
        .await
        .map_err(crate::openai::transport_error)?;
    if !(200..300).contains(&status) {
        return Err(status_error(status, &text, model));
    }
    #[derive(Deserialize)]
    struct Body {
        model: Option<String>,
        #[serde(default)]
        answers: Value,
    }
    let body: Body = serde_json::from_str(&text).map_err(|e| LlmError::Malformed(e.to_string()))?;
    Ok(Reply {
        model: body
            .model
            .filter(|m| !m.is_empty())
            .unwrap_or_else(|| model.to_string()),
        answers: body.answers,
    })
}

fn status_error(status: u16, body: &str, model: &str) -> LlmError {
    let message = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| v.get("error").and_then(Value::as_str).map(str::to_string))
        .unwrap_or_else(|| body.trim().to_string());
    match status {
        401 | 403 => LlmError::Unauthorized,
        404 if message.contains("model") => LlmError::ModelNotFound(model.to_string()),
        429 => LlmError::RateLimited,
        _ => LlmError::Http {
            status,
            body: crate::openai::truncate_chars(&message, 500),
        },
    }
}

/// What to do for a recommended-tasks request.
enum Ask {
    /// Nothing to ask (fewer than two tasks): this is the reply.
    Answer(String),
    Question {
        body: Value,
        candidates: Vec<Candidate>,
    },
}

/// One task offered as an option.
struct Candidate {
    id: i64,
    /// The option key: `t<id>`.
    key: String,
    /// What the model reads about it.
    description: String,
    /// The reason shown with it if picked.
    why: String,
}

/// The systemone request for the plan facts in `user`, or `None` if `user`
/// isn't a recommended-tasks request.
fn plan_question(user: &str, model: &str) -> Option<Ask> {
    let start = user.find("<facts>")? + "<facts>".len();
    let end = user.rfind("</facts>")?;
    let facts: Value = serde_json::from_str(user.get(start..end)?.trim()).ok()?;
    facts.get("due_this_week")?;

    let mut candidates: Vec<Candidate> = Vec::new();
    for list in ["overdue", "due_this_week", "due_soon", "undated"] {
        for task in facts
            .get(list)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let Some(id) = task.get("id").and_then(Value::as_i64) else {
                continue;
            };
            if candidates.len() < MAX_OPTIONS && candidates.iter().all(|c| c.id != id) {
                candidates.push(candidate(id, task, &facts));
            }
        }
    }
    if candidates.len() < 2 {
        return Some(Ask::Answer(reply_json(
            &candidates.iter().collect::<Vec<_>>(),
        )));
    }

    let today = facts.get("today").and_then(Value::as_str).unwrap_or("");
    let weekday = facts
        .get("today_weekday")
        .and_then(Value::as_str)
        .unwrap_or("");
    let mut state = format!("Today is {weekday} {today}.");
    if let Some(day) = facts
        .get("remaining_working_days")
        .and_then(Value::as_array)
        .and_then(|d| d.first())
        .filter(|d| d.get("date").and_then(Value::as_str) == Some(today))
    {
        let hours = day
            .get("capacity_hours")
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
        state.push_str(&format!(" {hours} focus hours are free today."));
    }
    state.push_str(&format!(" There are {} open tasks.", candidates.len()));

    let criteria: serde_json::Map<String, Value> = candidates
        .iter()
        .map(|c| (c.key.clone(), Value::String(c.description.clone())))
        .collect();
    let body = json!({
        "model": model,
        "state": state,
        "questions": {
            "first": {
                "type": "choice",
                "instructions": "Which task should be done first today? Overdue tasks and the nearest deadlines come first, then higher priority, then tasks planned for today, then what fits the free hours.",
                "criteria": criteria,
            }
        }
    });
    Some(Ask::Question { body, candidates })
}

fn candidate(id: i64, task: &Value, facts: &Value) -> Candidate {
    let text = |k: &str| {
        task.get(k)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    let today = facts.get("today").and_then(Value::as_str).unwrap_or("");
    let days = task.get("days_until_due").and_then(Value::as_i64);
    let priority = task.get("priority").and_then(Value::as_u64).unwrap_or(0);
    let planned_today = !today.is_empty() && text("planned_for") == today;

    let due = days.map(|d| match d {
        d if d < -1 => format!("overdue {} days", -d),
        -1 => "overdue 1 day".to_string(),
        0 => "due today".to_string(),
        1 => "due tomorrow".to_string(),
        _ => format!("due {}", text("due_weekday")),
    });
    let mut parts = vec![text("title")];
    if let Some(project) = task.get("project").and_then(Value::as_str) {
        parts.push(format!("project {project}"));
    }
    if let (Some(due), Some(d)) = (&due, days) {
        parts.push(if d > 1 {
            format!("{due} (in {d} days)")
        } else {
            due.clone()
        });
    } else {
        parts.push("no deadline".into());
    }
    if planned_today {
        parts.push("planned for today".into());
    } else if let Some(day) = task.get("planned_weekday").and_then(Value::as_str) {
        parts.push(format!("planned for {day}"));
    }
    if priority > 0 {
        parts.push(format!("priority {priority} of 3"));
    }
    if let Some(minutes) = task.get("estimate_minutes").and_then(Value::as_u64) {
        parts.push(format!("about {minutes} minutes"));
    }

    let why = match (&due, days) {
        (Some(due), Some(d)) if d <= 7 => due.clone(),
        _ if planned_today => "planned for today".into(),
        _ if priority >= 2 => "high priority".into(),
        (Some(due), _) => due.clone(),
        _ => "no deadline".into(),
    };
    Candidate {
        id,
        key: format!("t{id}"),
        description: parts.join("; "),
        why,
    }
}

/// The recommended-tasks JSON from the `first` answer: the options by
/// probability, highest first (or just the chosen one).
fn plan_reply(answers: &Value, candidates: &[Candidate]) -> String {
    let first = answers.get("first");
    let mut ranked: Vec<(&Candidate, f64)> = first
        .and_then(|a| a.get("probabilities"))
        .and_then(Value::as_object)
        .map(|probs| {
            candidates
                .iter()
                .filter_map(|c| probs.get(&c.key).and_then(Value::as_f64).map(|p| (c, p)))
                .collect()
        })
        .unwrap_or_default();
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1));
    let mut picks: Vec<&Candidate> = ranked.into_iter().map(|(c, _)| c).collect();
    if picks.is_empty()
        && let Some(choice) = first.and_then(|a| a.get("choice")).and_then(Value::as_str)
    {
        picks.extend(candidates.iter().find(|c| c.key == choice));
    }
    picks.truncate(PICKS);
    reply_json(&picks)
}

fn reply_json(picks: &[&Candidate]) -> String {
    let tasks: Vec<Value> = picks
        .iter()
        .map(|c| json!({ "id": c.id, "why": c.why }))
        .collect();
    json!({ "tasks": tasks }).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Settings, Task, TaskStatus, WeekRef};
    use crate::llm::collect_stream;
    use crate::report::{parse_recommendations, prepare_plan};
    use jiff::civil::date;
    use wiremock::matchers::{body_partial_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn client(server: &MockServer) -> DecisionClient {
        DecisionClient::new(LlmConfig {
            base_url: format!("{}/v1", server.uri()),
            model: "nimble".into(),
            api_key: None,
        })
        .unwrap()
        .with_timeout(Duration::from_secs(5))
    }

    fn task(id: i64, title: &str, due: Option<jiff::civil::Date>) -> Task {
        Task {
            id,
            title: title.into(),
            status: TaskStatus::Todo,
            due_on: due,
            planned_for: due,
            position: id as f64,
            ..Task::default()
        }
    }

    /// A real recommended-tasks request for Monday 16 Nov 2026.
    fn plan_request(tasks: &[Task]) -> ChatRequest {
        let today = date(2026, 11, 16);
        prepare_plan(tasks, &[], WeekRef::new(today), today, &Settings::default()).request
    }

    #[tokio::test]
    async fn plan_asks_one_choice_and_ranks_by_probability() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/systemone"))
            .and(body_partial_json(json!({
                "model": "nimble",
                "questions": { "first": { "type": "choice" } }
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "model": "nimble:latest",
                "answers": {
                    "first": {
                        "type": "choice",
                        "choice": "t2",
                        "probabilities": { "t1": 0.1, "t2": 0.6, "t3": 0.05, "t4": 0.25 },
                        "confidence": 0.6
                    }
                },
                "usage": { "input_tokens": 300, "output_tokens": 1 }
            })))
            .expect(1)
            .mount(&server)
            .await;
        let tasks = [
            task(1, "Write report", Some(date(2026, 11, 19))),
            task(2, "Fix the build", Some(date(2026, 11, 13))),
            task(3, "Tidy inbox", None),
            task(4, "Book flights", Some(date(2026, 11, 16))),
        ];
        let reply = collect_stream(client(&server).chat_stream(plan_request(&tasks)))
            .await
            .unwrap();
        assert_eq!(reply.model, "nimble:latest");
        let rec = parse_recommendations(&reply.text).unwrap();
        let picked: Vec<(i64, &str)> = rec.tasks.iter().map(|t| (t.id, t.why.as_str())).collect();
        assert_eq!(
            picked,
            vec![(2, "overdue 3 days"), (4, "due today"), (1, "due Thursday")]
        );

        // The options describe the tasks.
        let sent: Value = server.received_requests().await.unwrap()[0]
            .body_json()
            .unwrap();
        let criteria = &sent["questions"]["first"]["criteria"];
        assert_eq!(criteria.as_object().unwrap().len(), 4);
        assert!(
            criteria["t1"]
                .as_str()
                .unwrap()
                .starts_with("Write report; due Thursday (in 3 days)"),
            "{criteria}"
        );
        assert!(
            sent["state"]
                .as_str()
                .unwrap()
                .contains("Monday 2026-11-16")
        );
    }

    #[tokio::test]
    async fn one_task_needs_no_question() {
        let server = MockServer::start().await;
        let reply = client(&server)
            .chat(plan_request(&[task(7, "Only one", None)]))
            .await
            .unwrap();
        let rec = parse_recommendations(&reply.text).unwrap();
        assert_eq!(rec.tasks.len(), 1);
        assert_eq!(rec.tasks[0].id, 7);
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_request_sends_a_noul_and_other_requests_are_refused() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/systemone"))
            .and(body_partial_json(
                json!({ "questions": { "ok": { "type": "noul" } } }),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "model": "nimble",
                "answers": { "ok": { "type": "noul", "noul": 0.97 } }
            })))
            .mount(&server)
            .await;
        let c = client(&server);
        assert_eq!(c.chat(ChatRequest::test()).await.unwrap().text, "OK");
        let summary = ChatRequest {
            system: "Write my weekly update".into(),
            user: "<facts>{}</facts>".into(),
        };
        assert!(matches!(c.chat(summary).await, Err(LlmError::Malformed(_))));
    }

    #[tokio::test]
    async fn missing_model_maps_to_model_not_found() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/systemone"))
            .respond_with(
                ResponseTemplate::new(404)
                    .set_body_json(json!({ "error": "model 'nimble' not found" })),
            )
            .mount(&server)
            .await;
        let tasks = [task(1, "a", None), task(2, "b", None)];
        assert_eq!(
            client(&server)
                .chat(plan_request(&tasks))
                .await
                .unwrap_err(),
            LlmError::ModelNotFound("nimble".into())
        );
    }
}
