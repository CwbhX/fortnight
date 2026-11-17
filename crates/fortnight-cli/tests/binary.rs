//! End to end: the built binary, with a data folder in a temp dir.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

fn temp_dir(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "fortnight-cli-bin-{tag}-{}-{nanos}",
        std::process::id()
    ))
}

fn run(dir: &Path, args: &[&str]) -> Output {
    run_on(dir, "2026-11-16", args)
}

fn run_on(dir: &Path, today: &str, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_fortnight-cli"))
        .arg("--data-dir")
        .arg(dir)
        .args(["--today", today])
        .args(args)
        .env_remove("FORTNIGHT_DATA_DIR")
        .env_remove("FORTNIGHT_LLM_BASE_URL")
        .env_remove("FORTNIGHT_LLM_MODEL")
        .env_remove("FORTNIGHT_LLM_PROTOCOL")
        .env_remove("FORTNIGHT_LLM_API_KEY")
        .stdin(Stdio::null())
        .output()
        .expect("run fortnight-cli")
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

#[test]
fn add_then_list_then_done() {
    let dir = temp_dir("flow");
    let added = run(
        &dir,
        &["add", "write", "report", "fri", "#work", "~2h", "!"],
    );
    assert!(added.status.success(), "{}", stderr(&added));
    assert_eq!(
        stdout(&added),
        "Added 1: write report (due 2026-11-20, #work, ~2h, !)\n"
    );

    let listed = run(&dir, &["list"]);
    assert!(listed.status.success(), "{}", stderr(&listed));
    let out = stdout(&listed);
    assert!(out.contains("write report"), "{out}");
    assert!(out.contains("work") && out.contains("2026-11-20") && out.contains("2h"));

    let done = run(&dir, &["done", "1"]);
    assert!(done.status.success(), "{}", stderr(&done));
    assert_eq!(stdout(&run(&dir, &["list"])), "No tasks.\n");
    assert!(stdout(&run(&dir, &["list", "--all"])).contains("[x]"));

    let missing = run(&dir, &["done", "7"]);
    assert_eq!(missing.status.code(), Some(1));
    assert!(stderr(&missing).contains("not found"));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn facts_work_without_an_llm_and_llm_commands_exit_2_when_unconfigured() {
    let dir = temp_dir("llm");
    assert!(run(&dir, &["add", "something", "wed"]).status.success());

    let facts = run(&dir, &["plan", "--facts"]);
    assert!(facts.status.success(), "{}", stderr(&facts));
    assert!(stdout(&facts).contains("something"));

    let plan = run(&dir, &["plan"]);
    assert_eq!(plan.status.code(), Some(2));
    assert!(stderr(&plan).contains("no LLM endpoint is configured"));
    assert!(stdout(&plan).is_empty());

    // run-due with something due but no LLM: nothing fails (PLAN.md 8).
    let due = run_on(&dir, "2026-11-20", &["run-due"]);
    assert_eq!(due.status.code(), Some(0), "{}", stderr(&due));
    assert!(stdout(&due).contains("nothing was generated"));

    let bad = run(&dir, &["summary", "--week", "someday"]);
    assert_eq!(bad.status.code(), Some(2));

    let exported = run(&dir, &["export", "--format", "json"]);
    assert!(exported.status.success(), "{}", stderr(&exported));
    serde_json::from_str::<serde_json::Value>(&stdout(&exported)).expect("json");

    let _ = std::fs::remove_dir_all(&dir);
}
