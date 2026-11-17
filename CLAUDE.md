# Fortnight — rules for agents

Read ORCHESTRATION.md (scope, file ownership) and the PLAN.md sections your task names.

1. **Edit only the files you own.** Your task lists them. Nothing else, including Cargo.toml, Cargo.lock and other agents' files.
2. **Frozen files never change**: `crates/fortnight-core/src/{domain,store,llm}.rs`, and the `Action` enums and `*Data` structs in `crates/fortnight-app/src/views/*.rs`. If one is wrong, stop and say so in your report instead of working around it.
3. **No new dependencies.** Use what is in the workspace `Cargo.toml`.
4. **Use the real docs, not memory.** iced is 0.14 and jiff is 1.0; both changed a lot.
   - iced 0.14 source and examples: `/private/tmp/claude-501/-Users-clementhathaway--t3-projects-fortnight/313991d6-cb8c-4c8c-82ee-a8a7380d8a4a/scratchpad/iced-0.14` (see `examples/`, especially `todos`, `markdown`, `editor`, `styling`).
   - Crate sources for everything else: `~/.cargo/registry/src/index.crates.io-*/<crate>-<version>/`. `cargo doc -p <crate> --no-deps` also works.
   - jiff 1.0 gotchas: `Zoned` is now `jiff::ZonedDateTime`; `civil::Weekday` has no serde impl.
5. **Done means** these pass from the repo root:
   ```
   export PATH="$HOME/.cargo/bin:$PATH"
   cargo fmt --check
   cargo clippy --all-targets -- -D warnings
   cargo test
   ```
6. Prototype scope: simple over thorough. Note what you skip.

Commit your work on your branch when done. Do not push.

## Seeing the GUI without a screen

Run the app with `FORTNIGHT_SCREENSHOT=/path/out.raw`; it screenshots its own window after two seconds, writes `<w> <h>\n` + RGBA, and exits. Convert with `python3 -I <scratchpad>/tools/raw2png.py out.raw out.png` (script in the scratchpad above's `tools/`), then view the PNG.
