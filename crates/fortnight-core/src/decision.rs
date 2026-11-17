//! Ollama decision models (Jev-style, Ollama 0.35+) for recommended tasks.
//!
//! A decision model answers questions with choices and probabilities, not
//! text: `POST {root}/v1/systemone` with a `state` (the context) and
//! `questions` (`choice` between 2 to 26 named options, `noul` yes/no, or
//! `score`). `DecisionClient` is an `LlmClient` so it slots in wherever a
//! chat model would (fallback, streaming, CLI), but it only understands
//! the recommended-tasks request (`report::prepare_plan`): it reads the
//! plan facts from between `<facts>` and `</facts>`, offers the open tasks
//! on the Today list (planned for today, or carried over from an earlier
//! day) and the overdue ones (due on an earlier day and not
//! done, wherever they sit; never other days' tasks) as one `choice`
//! question ("which first?"), ranks them by probability
//! and replies with the same JSON a chat model would
//! (`report::parse_recommendations`). The reasons are written here from
//! the facts ("overdue 2 days", "high priority, due Thursday"); the model
//! gives none. Priority counts twice: each option starts with it ("High
//! priority; …"), and the ranking multiplies the probabilities by 1, 2 or
//! 4 for low, medium and high (`priority_weight`). Being overdue counts
//! too: up to twice as much the longer it is late (`overdue_weight`).
//!
//! The Settings Test button's request (`ChatRequest::test`) sends one
//! `noul` question and replies "OK". Any other request (a weekly summary)
//! is refused with `Malformed`, so a fallback can take it.
//!
//! `suggest_priority` asks which priority a new task deserves from its
//! title and notes: one `choice` between low, medium and high (the task
//! dialog's three levels, stored as 0, 1 and 2).
//!
//! Skipped for the prototype: retries (the fallback to the main endpoint
//! covers failures), the custom recommendations prompt (a decision model
//! gets the built-in rule as its question), and `score` questions.

use crate::domain::Priority;
use crate::llm::{BoxFuture, ChatRequest, ChatResponse, LlmClient, LlmConfig, LlmError};
use secrecy::ExposeSecret;
use serde::Deserialize;
use serde_json::{Value, json};
use std::time::Duration;

/// Decision models answer in milliseconds; this leaves room for loading
/// the model, not the ten minutes a local chat model may get, so a hung
/// server soon gives way to the fallback.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);
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
        Ok(DecisionClient {
            config,
            http,
            timeout: DEFAULT_TIMEOUT,
        })
    }

    /// Time allowed for each request (default 60 s).
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    fn request(&self) -> reqwest::RequestBuilder {
        let url = format!(
            "{}/v1/systemone",
            crate::ollama::root_url(&self.config.base_url)
        );
        let builder = self.http.post(url).timeout(self.timeout);
        match &self.config.api_key {
            Some(key) => builder.bearer_auth(key.expose_secret()),
            None => builder,
        }
    }

    /// The priority a task with this title and notes deserves: 0 low,
    /// 1 medium, 2 high (the most likely of the three).
    pub fn suggest_priority(
        &self,
        title: &str,
        notes: &str,
    ) -> BoxFuture<Result<Priority, LlmError>> {
        let model = self.config.model.trim().to_string();
        let mut state = format!("Task: {}", title.trim());
        if !notes.trim().is_empty() {
            state.push_str(&format!("\nNotes: {}", notes.trim()));
        }
        let body = json!({
            "model": model,
            "state": state,
            "questions": {
                "priority": {
                    "type": "choice",
                    "instructions": "How important and urgent is this task?",
                    "criteria": {
                        "low": "Can wait: no deadline pressure, little consequence if it slips.",
                        "medium": "Normal work that should get done soon.",
                        "high": "Urgent or important: a near deadline, a blocker, people waiting, money, health or safety.",
                    }
                }
            }
        });
        let builder = self.request();
        let key = self.config.api_key.clone();
        Box::pin(async move {
            let reply = ask(builder, body, &model)
                .await
                .map_err(|e| crate::secure::redact(e, key.as_ref()))?;
            let answer = reply.answers.get("priority");
            let best = answer
                .and_then(|a| a.get("probabilities"))
                .and_then(Value::as_object)
                .and_then(|p| {
                    p.iter()
                        .filter_map(|(k, v)| v.as_f64().map(|v| (k.as_str(), v)))
                        .max_by(|a, b| a.1.total_cmp(&b.1))
                        .map(|(k, _)| k.to_string())
                })
                .or_else(|| {
                    answer
                        .and_then(|a| a.get("choice"))
                        .and_then(Value::as_str)
                        .map(str::to_string)
                });
            match best.as_deref() {
                Some("low") => Ok(0),
                Some("medium") => Ok(1),
                Some("high") => Ok(2),
                _ => Err(LlmError::Malformed("no priority in the answer".into())),
            }
        })
    }
}

impl LlmClient for DecisionClient {
    fn model(&self) -> &str {
        &self.config.model
    }

    fn chat(&self, req: ChatRequest) -> BoxFuture<Result<ChatResponse, LlmError>> {
        let model = self.config.model.trim().to_string();
        let builder = self.request();
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
                        ask(builder, body, &model).await.and_then(|reply| {
                            Ok(ChatResponse {
                                text: plan_reply(&reply.answers, &candidates)?,
                                model: reply.model,
                            })
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
    /// How much its priority counts in the ranking (`priority_weight`).
    weight: f64,
}

/// The user's priority is a strong signal: the model's probability is
/// multiplied by 1 (low), 2 (medium) or 4 (high) before ranking, so a lower
/// priority task needs a much likelier answer to come first.
fn priority_weight(priority: u64) -> f64 {
    match priority {
        0 => 1.0,
        1 => 2.0,
        _ => 4.0,
    }
}

/// An overdue task counts more the longer it is late: 1.25 at one day
/// overdue, up to 2 at four days or more. Not overdue: 1.
fn overdue_weight(days_until_due: Option<i64>) -> f64 {
    match days_until_due {
        Some(d) if d < 0 => 1.0 + 0.25 * (-d).min(4) as f64,
        _ => 1.0,
    }
}

/// The systemone request for the plan facts in `user`, or `None` if `user`
/// isn't a recommended-tasks request.
fn plan_question(user: &str, model: &str) -> Option<Ask> {
    let start = user.find("<facts>")? + "<facts>".len();
    let end = user.rfind("</facts>")?;
    let facts: Value = serde_json::from_str(user.get(start..end)?.trim()).ok()?;
    facts.get("due_this_week")?;

    let today = facts.get("today").and_then(Value::as_str).unwrap_or("");
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
            // Only the tasks on today's list (carried-over ones too), and
            // overdue ones until done.
            let overdue = task.get("overdue").and_then(Value::as_bool) == Some(true);
            if !on_today_list(task, today) && !overdue {
                continue;
            }
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
                "instructions": "Which task should be done first today? Weigh priority, deadlines and context together. Higher priority comes first: High before Medium before Low. Overdue tasks were not finished on the day they were due and should not slip again: the longer overdue, the sooner. Among tasks of the same priority, overdue tasks and the nearest deadlines come first, then what unblocks other work or fits the free hours.",
                "criteria": criteria,
            }
        }
    });
    Some(Ask::Question { body, candidates })
}

/// Whether `task` is on the Today list: planned for today, or for an
/// earlier day and carried over (`on_today_list` in the facts; older facts
/// without it fall back to `planned_for == today`).
fn on_today_list(task: &Value, today: &str) -> bool {
    match task.get("on_today_list").and_then(Value::as_bool) {
        Some(on) => on,
        None => !today.is_empty() && task.get("planned_for").and_then(Value::as_str) == Some(today),
    }
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
    let planned_today = on_today_list(task, today);

    let due = days.map(|d| match d {
        d if d < -1 => format!("overdue {} days", -d),
        -1 => "overdue 1 day".to_string(),
        0 => "due today".to_string(),
        1 => "due tomorrow".to_string(),
        _ => format!("due {}", text("due_weekday")),
    });
    // Priority first, in the app's words (0 low, 1 medium, 2 or more high).
    let level = match priority {
        0 => "Low",
        1 => "Medium",
        _ => "High",
    };
    let mut parts = vec![format!("{level} priority"), text("title")];
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
    if let Some(minutes) = task.get("estimate_minutes").and_then(Value::as_u64) {
        parts.push(format!("about {minutes} minutes"));
    }

    let deadline = match (&due, days) {
        (Some(due), Some(d)) if d <= 7 => Some(due.clone()),
        _ => None,
    };
    let why = match (priority, deadline) {
        (p, Some(deadline)) if p >= 2 => format!("high priority, {deadline}"),
        (p, None) if p >= 2 => "high priority".into(),
        (_, Some(deadline)) => deadline,
        _ if planned_today => "planned for today".into(),
        _ => "no deadline".into(),
    };
    Candidate {
        id,
        key: format!("t{id}"),
        description: parts.join("; "),
        why,
        weight: priority_weight(priority) * overdue_weight(days),
    }
}

/// The recommended-tasks JSON from the `first` answer: the options by
/// probability times priority weight, highest first (or just the chosen
/// one).
fn plan_reply(answers: &Value, candidates: &[Candidate]) -> Result<String, LlmError> {
    let first = answers.get("first");
    let mut ranked: Vec<(&Candidate, f64)> = first
        .and_then(|a| a.get("probabilities"))
        .and_then(Value::as_object)
        .map(|probs| {
            candidates
                .iter()
                .filter_map(|c| {
                    let p = probs.get(&c.key).and_then(Value::as_f64)?;
                    Some((c, p * c.weight))
                })
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
    if picks.is_empty() {
        // Not a usable answer: an error, so the fallback answers instead and
        // the last good recommendations stay on screen meanwhile.
        return Err(LlmError::Malformed(
            "the decision model's answer named none of the tasks".into(),
        ));
    }
    Ok(reply_json(&picks))
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

    /// An open task planned for today (Monday 16 Nov 2026).
    fn task(id: i64, title: &str, due: Option<jiff::civil::Date>) -> Task {
        Task {
            id,
            title: title.into(),
            status: TaskStatus::Todo,
            due_on: due,
            planned_for: Some(date(2026, 11, 16)),
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
            // Planned for Wednesday: never offered.
            Task {
                planned_for: Some(date(2026, 11, 18)),
                ..task(5, "Migrate analytics", Some(date(2026, 11, 18)))
            },
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
        assert!(criteria.get("t5").is_none(), "{criteria}");
        assert!(
            criteria["t1"]
                .as_str()
                .unwrap()
                .starts_with("Low priority; Write report; due Thursday (in 3 days)"),
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
    async fn priority_outweighs_a_somewhat_likelier_answer() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/systemone"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "model": "nimble",
                "answers": { "first": {
                    "type": "choice",
                    "choice": "t1",
                    // Low 0.5 (x1), High 0.3 (x4 = 1.2), Medium 0.2 (x2 = 0.4).
                    "probabilities": { "t1": 0.5, "t2": 0.3, "t3": 0.2 }
                } }
            })))
            .mount(&server)
            .await;
        let low = task(1, "Tidy desk", None);
        let high = Task {
            priority: 2,
            ..task(2, "Fix outage", Some(date(2026, 11, 17)))
        };
        let medium = Task {
            priority: 1,
            ..task(3, "Reply to Sam", None)
        };
        let reply = client(&server)
            .chat(plan_request(&[low, high, medium]))
            .await
            .unwrap();
        let rec = parse_recommendations(&reply.text).unwrap();
        let picked: Vec<(i64, &str)> = rec.tasks.iter().map(|t| (t.id, t.why.as_str())).collect();
        assert_eq!(
            picked,
            vec![
                (2, "high priority, due tomorrow"),
                (1, "planned for today"),
                (3, "planned for today")
            ]
        );
        let sent: Value = server.received_requests().await.unwrap()[0]
            .body_json()
            .unwrap();
        let criteria = &sent["questions"]["first"]["criteria"];
        assert!(
            criteria["t2"]
                .as_str()
                .unwrap()
                .starts_with("High priority; Fix outage")
        );
        assert!(
            criteria["t3"]
                .as_str()
                .unwrap()
                .starts_with("Medium priority; ")
        );
    }

    #[tokio::test]
    async fn overdue_and_carried_over_tasks_are_offered_and_overdue_counts_more() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/systemone"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "model": "nimble",
                "answers": { "first": {
                    "type": "choice",
                    "choice": "t1",
                    // 0.5 (x1); overdue 4 days: 0.3 (x2 = 0.6); 0.2 (x1).
                    "probabilities": { "t1": 0.5, "t2": 0.3, "t3": 0.2 }
                } }
            })))
            .mount(&server)
            .await;
        let todays = task(1, "Tidy desk", None);
        // Planned for last Thursday, due then, still open.
        let late = Task {
            planned_for: Some(date(2026, 11, 12)),
            ..task(2, "Send invoice", Some(date(2026, 11, 12)))
        };
        // Planned for an earlier day, not overdue: carried over to today's
        // list, so offered too.
        let carried = Task {
            planned_for: Some(date(2026, 11, 13)),
            ..task(3, "Read paper", Some(date(2026, 11, 20)))
        };
        let reply = client(&server)
            .chat(plan_request(&[todays, late, carried]))
            .await
            .unwrap();
        let rec = parse_recommendations(&reply.text).unwrap();
        let picked: Vec<(i64, &str)> = rec.tasks.iter().map(|t| (t.id, t.why.as_str())).collect();
        assert_eq!(
            picked,
            vec![
                (2, "overdue 4 days"),
                (1, "planned for today"),
                (3, "due Friday")
            ]
        );
        let sent: Value = server.received_requests().await.unwrap()[0]
            .body_json()
            .unwrap();
        let criteria = sent["questions"]["first"]["criteria"].as_object().unwrap();
        assert_eq!(criteria.len(), 3, "{criteria:?}");
    }

    #[test]
    fn overdue_weight_grows_then_stops() {
        assert_eq!(overdue_weight(None), 1.0);
        assert_eq!(overdue_weight(Some(0)), 1.0);
        assert_eq!(overdue_weight(Some(-1)), 1.25);
        assert_eq!(overdue_weight(Some(-4)), 2.0);
        assert_eq!(overdue_weight(Some(-30)), 2.0);
    }

    #[tokio::test]
    async fn an_answer_naming_no_task_is_an_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/systemone"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "model": "nimble",
                "answers": { "first": { "type": "choice", "choice": "0",
                    "probabilities": { "0": 0.7, "1": 0.3 } } }
            })))
            .mount(&server)
            .await;
        let tasks = [task(1, "a", None), task(2, "b", None)];
        assert!(matches!(
            client(&server).chat(plan_request(&tasks)).await,
            Err(LlmError::Malformed(_))
        ));
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
    async fn suggest_priority_picks_the_most_likely_level() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/systemone"))
            .and(body_partial_json(json!({
                "state": "Task: Pay the overdue invoice\nNotes: Supplier stops service Friday",
                "questions": { "priority": { "type": "choice" } }
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "model": "nimble",
                "answers": { "priority": {
                    "type": "choice",
                    "choice": "high",
                    "probabilities": { "low": 0.05, "medium": 0.2, "high": 0.75 },
                    "confidence": 0.75
                } }
            })))
            .mount(&server)
            .await;
        let p = client(&server)
            .suggest_priority(" Pay the overdue invoice ", "Supplier stops service Friday")
            .await
            .unwrap();
        assert_eq!(p, 2);
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
