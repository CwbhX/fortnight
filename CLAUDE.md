# Fortnight

A Rust workspace: `crates/fortnight-core` (logic, storage, LLM clients), `crates/fortnight-app` (the iced GUI, binary `Fortnight`), `crates/fortnight-cli`, and `xtask` (bundling).

## How to work here

- Work on `main`. Commit and push to `main` whenever work is done, without asking first. Don't push other branches unless the user names one.
- Don't stop to ask for permission or confirmation. Make the sensible call, carry on, and say what you chose.

## Checks

Run from the repo root before committing:

```
export PATH="$HOME/.cargo/bin:$PATH"
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

## Notes

- iced is 0.14 and jiff is 1.0; both changed a lot, so read the source instead of relying on memory.
  - iced 0.14 source: `~/.cargo/registry/src/index.crates.io-*/iced*-0.14*/` (`iced_widget` and `iced_core` hold most of the API). The examples (`todos`, `markdown`, `editor`, `styling`, `modal`) are in the iced repository at tag `0.14.0`.
  - Other crates: `~/.cargo/registry/src/index.crates.io-*/<crate>-<version>/`, or `cargo doc -p <crate> --no-deps`.
  - jiff 1.0: `Zoned` is now `jiff::ZonedDateTime`; `civil::Weekday` has no serde impl.
- Old data folders must keep opening with nothing lost: the database migrates on open, and settings JSON may lack newer fields. `crates/fortnight-core/tests/fixtures/round1.db` is a real schema-version-1 database used by the tests.
- LLM client tests run against `wiremock` mock servers, never a real endpoint or key.

## Seeing the GUI without a screen

Run the app with `FORTNIGHT_SCREENSHOT=/path/out.raw`; it screenshots its own window after two seconds, writes `<w> <h>\n` + RGBA, and exits. Convert with `python3 -I tools/raw2png.py out.raw out.png`, then view the PNG. If the machine is busy the capture can come out half-drawn; `FORTNIGHT_SCREENSHOT_DELAY=5` waits longer.

Useful environment: `--data-dir <folder>` (use a scratch folder outside the repo, never the repo), `FORTNIGHT_PAGE=today|week|month|settings`, `FORTNIGHT_THEME=light|dark`.

Sample data: copy `crates/fortnight-core/tests/fixtures/round1.db` to `<scratch folder>/fortnight.db` (it migrates on open; tasks are around 16 Nov 2026). To see richer data, insert rows into that scratch copy with `sqlite3` (schema in `crates/fortnight-core/src/sqlite.rs`).
