//! Recommended tasks: reading a `Plan` report's reply.

use crate::domain::{Task, TaskId};
use std::collections::BTreeSet;
use std::fmt::Write as _;

/// The model's recommended tasks, parsed from a `Plan` report.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Recommendation {
    /// In the order to do them.
    pub tasks: Vec<RecommendedTask>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecommendedTask {
    pub id: TaskId,
    /// A few words on why.
    pub why: String,
}

/// Unknown keys (such as `focus` and `day`, which older prompt versions
/// asked for) are ignored. `tasks` is required, so an object with other
/// keys is not mistaken for "nothing to recommend".
#[derive(serde::Deserialize)]
struct RawRecommendation {
    tasks: Vec<RawTask>,
}

#[derive(serde::Deserialize)]
struct RawTask {
    id: serde_json::Value,
    #[serde(default)]
    why: String,
}

/// Parses a `Plan` report's text: one JSON object with a `tasks` list,
/// possibly wrapped in a code fence or prose (which may contain braces of
/// its own): the first `{` where such an object starts is used, and
/// anything after it is ignored. Task ids may be numbers or numeric
/// strings; duplicates are dropped. `None` when there is no such object.
pub fn parse_recommendations(text: &str) -> Option<Recommendation> {
    let raw = text.match_indices('{').find_map(|(i, _)| {
        serde_json::Deserializer::from_str(&text[i..])
            .into_iter::<RawRecommendation>()
            .next()?
            .ok()
    })?;
    let had_raw = !raw.tasks.is_empty();
    let mut seen = BTreeSet::new();
    let tasks: Vec<RecommendedTask> = raw
        .tasks
        .into_iter()
        .filter_map(|t| {
            let id = match &t.id {
                serde_json::Value::Number(n) => n.as_i64()?,
                serde_json::Value::String(s) => s.trim().trim_start_matches('#').parse().ok()?,
                _ => return None,
            };
            seen.insert(id).then(|| RecommendedTask {
                id,
                why: t.why.trim().to_string(),
            })
        })
        .collect();
    // A non-empty list whose entries all lacked valid ids is malformed, not
    // an intentional "nothing worth doing today".
    if had_raw && tasks.is_empty() {
        return None;
    }
    Some(Recommendation { tasks })
}

/// A `Plan` report as plain Markdown, for the terminal: a numbered list of
/// the recommended tasks that still exist. Text that isn't the expected
/// JSON is returned as it is.
pub fn recommendations_markdown(text: &str, tasks: &[Task]) -> String {
    let Some(rec) = parse_recommendations(text) else {
        return text.to_string();
    };
    let mut md = String::new();
    let mut n = 0;
    for r in &rec.tasks {
        let Some(task) = tasks.iter().find(|t| t.id == r.id) else {
            continue;
        };
        n += 1;
        let _ = write!(md, "{n}. {}", task.title);
        if !r.why.is_empty() {
            let _ = write!(md, ": {}", r.why);
        }
        md.push('\n');
    }
    if n == 0 {
        md.push_str("Nothing to recommend.\n");
    }
    md
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::fixtures::*;

    #[test]
    fn recommendations_parse_leniently() {
        let text = "Here you go:\n```json\n{\"tasks\": [\
            {\"id\": 3, \"why\": \" due Thu \"},\
            {\"id\": \"7\"},\
            {\"id\": 3, \"why\": \"again\"},\
            {\"id\": null}]}\n```";
        let rec = parse_recommendations(text).unwrap();
        assert_eq!(
            rec.tasks,
            vec![
                RecommendedTask {
                    id: 3,
                    why: "due Thu".into()
                },
                RecommendedTask {
                    id: 7,
                    why: String::new()
                },
            ]
        );
        assert_eq!(parse_recommendations("## Focus\nno json"), None);
        assert_eq!(parse_recommendations("} {"), None);

        // Braces in the prose around the object don't get in the way.
        let one = |text: &str| parse_recommendations(text).map(|r| r.tasks.len());
        assert_eq!(
            one("{\"tasks\": [{\"id\": 3}]}\nI chose {3} because it is due."),
            Some(1)
        );
        assert_eq!(
            one("Looking at {the facts} first.\n{\"tasks\": [{\"id\": 3}]}"),
            Some(1)
        );
        assert_eq!(
            one("<think>maybe {3}? or {7}</think>{\"tasks\": [{\"id\": 7}]}"),
            Some(1)
        );
        assert_eq!(one("{\"tasks\": []}"), Some(0));
        // A non-empty list whose entries all lack valid ids is malformed.
        assert_eq!(one("{\"tasks\": [{\"id\": \"not-a-number\"}]}"), None);
        assert_eq!(one("{\"tasks\": [{\"id\": null}]}"), None);
        // An object with other keys is not an empty recommendation.
        assert_eq!(one("{\"recommendations\": [{\"id\": 3}]}"), None);
        assert_eq!(
            recommendations_markdown("{\"picks\": []}", &[]),
            "{\"picks\": []}"
        );

        let tasks = vec![task(3, "Write report")];
        assert_eq!(
            recommendations_markdown(text, &tasks),
            "1. Write report: due Thu\n"
        );
        assert_eq!(recommendations_markdown("plain", &tasks), "plain");
    }

    /// Plans stored by older prompt versions also have `focus` and a `day`
    /// per task; those keys are ignored.
    #[test]
    fn stored_recommendations_with_focus_and_day_still_parse() {
        let text = "{\"focus\": \"Ship the report\", \"tasks\": [\
            {\"id\": 3, \"day\": \"2026-11-18\", \"why\": \"due Thu\"},\
            {\"id\": \"7\", \"day\": \"someday\"}]}";
        let rec = parse_recommendations(text).unwrap();
        assert_eq!(
            rec.tasks,
            vec![
                RecommendedTask {
                    id: 3,
                    why: "due Thu".into()
                },
                RecommendedTask {
                    id: 7,
                    why: String::new()
                },
            ]
        );
    }
}
