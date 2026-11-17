# Fortnight — rules for agents

Read ORCHESTRATION.md (scope, file ownership; round 2 is the second half) and the PLAN.md sections your task names.

1. **Edit only the files you own.** Your task lists them. Nothing else, including Cargo.toml, Cargo.lock and other agents' files.
2. **Frozen files never change** (refrozen after round 2 step 1):
   - `crates/fortnight-core/src/{domain,store,llm}.rs`
   - in `crates/fortnight-app/src/views/*.rs`: the `Action` enums, the `*Data` structs, `day::LogLine`, `report_panel::ReportSlot`, and `views::NavKey` (in `views/mod.rs`).
   - Each view's `Message` enum is internal: add variants freely. Its `load`/`update`/`view` signatures stay as stubbed.
   If a frozen item is wrong, stop and say so in your report instead of working around it.
3. **Orchestrator-owned**: `main.rs`, `top_bar.rs`, `theme.rs`, `views/mod.rs`, `lib.rs`s, the workspace `Cargo.toml`. A view may add private style helpers in its own file.
4. **No new dependencies.** Use what is in the workspace `Cargo.toml` (round 2 added `clap`, `rpassword`, `eventsource-stream`, `futures-util`, `notify-rust`, `rfd`, reqwest `stream`).
5. **Use the real docs, not memory.** iced is 0.14 and jiff is 1.0; both changed a lot.
   - iced 0.14 source and examples: `/private/tmp/claude-501/-Users-clementhathaway--t3-projects-fortnight/313991d6-cb8c-4c8c-82ee-a8a7380d8a4a/scratchpad/iced-0.14` (see `examples/`, especially `todos`, `markdown`, `editor`, `styling`, `modal`).
   - Crate sources for everything else: `~/.cargo/registry/src/index.crates.io-*/<crate>-<version>/`. `cargo doc -p <crate> --no-deps` also works.
   - jiff 1.0 gotchas: `Zoned` is now `jiff::ZonedDateTime`; `civil::Weekday` has no serde impl.
6. **Existing data must survive.** Round 1 data folders (schema `user_version = 1`, settings JSON without the new fields) must open with nothing lost. `crates/fortnight-core/tests/fixtures/round1.db` is a real round 1 database.
7. **Never use a real API key.** Test LLM clients against `wiremock` mock servers.
8. **Done means** these pass from the repo root:
   ```
   export PATH="$HOME/.cargo/bin:$PATH"
   cargo fmt --check
   cargo clippy --all-targets -- -D warnings
   cargo test
   ```
9. Prototype scope: simple over thorough. Note what you skip.

Commit your work on your branch when done. Do not push.

## Seeing the GUI without a screen

Run the app with `FORTNIGHT_SCREENSHOT=/path/out.raw`; it screenshots its own window after two seconds, writes `<w> <h>\n` + RGBA, and exits. Convert with `python3 -I /private/tmp/claude-501/-Users-clementhathaway--t3-projects-fortnight/313991d6-cb8c-4c8c-82ee-a8a7380d8a4a/scratchpad/tools/raw2png.py out.raw out.png`, then view the PNG.

Useful environment: `--data-dir <folder>` (use a scratch folder outside the repo, never the repo), `FORTNIGHT_PAGE=today|week|month|settings`, `FORTNIGHT_SKIP_UNLOCK=1`, `FORTNIGHT_THEME=light|dark`.

Sample data: copy `crates/fortnight-core/tests/fixtures/round1.db` to `<scratch folder>/fortnight.db` (it migrates on open; tasks are around 16 Nov 2026). To see richer data, insert rows into that scratch copy with `sqlite3` (schema in `crates/fortnight-core/src/sqlite.rs`).
