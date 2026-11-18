//! The subtask checklist kept in a task's notes.
//!
//! A task's notes are plain text. Lines of the form `- [ ] text` (open) or
//! `- [x] text` (done, `X` also accepted), optionally indented, are its
//! subtasks; every other line is free text. The task form edits the two
//! parts separately and writes them back with [`join_notes`]: the text, a
//! blank line, then the checklist.
//!
//! A day's notes (the Today tab's notes box) are Markdown typed by hand:
//! `**bold**`, `*italic*`, `<u>underline</u>`, and lists marked `• `,
//! `- ` or `1. `. [`to_markdown`] tidies them for the model.

/// A checklist item kept in the notes as `- [ ] text` or `- [x] text`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Subtask {
    pub done: bool,
    pub text: String,
}

/// One notes line as a subtask, if it is a checklist line.
pub fn subtask_line(line: &str) -> Option<Subtask> {
    let rest = line.trim_start().strip_prefix("- [")?;
    let (mark, text) = rest.split_at_checked(1)?;
    let text = text.strip_prefix("] ")?;
    let done = match mark {
        " " => false,
        "x" | "X" => true,
        _ => return None,
    };
    Some(Subtask {
        done,
        text: text.trim().to_string(),
    })
}

/// Splits notes into their subtasks and the remaining text (trimmed).
pub fn split_notes(notes: &str) -> (Vec<Subtask>, String) {
    let mut subtasks = Vec::new();
    let mut rest = Vec::new();
    for line in notes.lines() {
        match subtask_line(line) {
            Some(s) => subtasks.push(s),
            None => rest.push(line),
        }
    }
    (subtasks, rest.join("\n").trim().to_string())
}

/// Joins notes text and subtasks back into one notes string: the text, a
/// blank line, then the checklist.
pub fn join_notes(subtasks: &[Subtask], text: &str) -> String {
    let list: Vec<String> = subtasks
        .iter()
        .map(|s| format!("- [{}] {}", if s.done { "x" } else { " " }, s.text))
        .collect();
    let text = text.trim();
    match (text.is_empty(), list.is_empty()) {
        (_, true) => text.to_string(),
        (true, false) => list.join("\n"),
        (false, false) => format!("{text}\n\n{}", list.join("\n")),
    }
}

/// `(done, total)` subtasks in the notes.
pub fn subtask_counts(notes: &str) -> (usize, usize) {
    notes
        .lines()
        .filter_map(subtask_line)
        .fold((0, 0), |(d, t), s| (d + usize::from(s.done), t + 1))
}

/// Notes as Markdown for the model: each line keeps its indentation (so
/// lists and nested lists survive), `• ` bullets become `- `, runs of
/// spaces within a line shrink to one, and runs of blank lines to one.
/// Trimmed.
pub fn to_markdown(text: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    for line in text.lines() {
        let line = line.trim_end();
        if line.is_empty() && out.last().is_none_or(|l| l.is_empty()) {
            continue;
        }
        let body = line.trim_start();
        let indent = &line[..line.len() - body.len()];
        let body = body.split_whitespace().collect::<Vec<_>>().join(" ");
        out.push(match body.strip_prefix("• ") {
            Some(rest) => format!("{indent}- {rest}"),
            None => format!("{indent}{body}"),
        });
    }
    while out.last().is_some_and(|l| l.is_empty()) {
        out.pop();
    }
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notes_split_and_join() {
        let (subs, text) = split_notes("intro\n- [ ] a\n  - [X] b\n- [y] not one\n");
        assert_eq!(
            subs,
            vec![
                Subtask {
                    done: false,
                    text: "a".into()
                },
                Subtask {
                    done: true,
                    text: "b".into()
                },
            ]
        );
        assert_eq!(text, "intro\n- [y] not one");
        assert_eq!(join_notes(&subs, ""), "- [ ] a\n- [x] b");
        assert_eq!(join_notes(&[], " x "), "x");
        assert_eq!(join_notes(&subs, "intro"), "intro\n\n- [ ] a\n- [x] b");
        assert_eq!(subtask_counts("- [x] a\n- [ ] b\nc"), (1, 2));
    }

    #[test]
    fn odd_lines_are_text() {
        // Multi-byte mark, no space after `]`, empty notes.
        for line in ["- [é] a", "- [x]a", "- [", "-[ ] a", ""] {
            assert_eq!(subtask_line(line), None, "{line:?}");
        }
        assert_eq!(subtask_counts(""), (0, 0));
        // Split then join keeps text and checklist.
        let notes = "intro\n\n- [ ] a\n- [x] b";
        let (subs, text) = split_notes(notes);
        assert_eq!(join_notes(&subs, &text), notes);
    }

    #[test]
    fn markdown_keeps_lines_and_turns_bullets_into_dashes() {
        let notes =
            "\n**Standup**  ran long  \n\n\n• one\n  • nested\n- dash\n1. first\n•not a bullet\n\n";
        assert_eq!(
            to_markdown(notes),
            "**Standup** ran long\n\n- one\n  - nested\n- dash\n1. first\n•not a bullet"
        );
        assert_eq!(to_markdown("  \n \n"), "");
    }
}
