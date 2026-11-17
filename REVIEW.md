# Fortnight — code review and fix plan (17 Nov 2026)

A full read of the workspace at commit `a433428` (25k lines of Rust, CI, docs). Baseline: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` and `cargo test` all pass (366 tests).

Line numbers refer to `a433428` and drift as fixes land; function names are the stable reference.

## Outcome

Everything in the plan below was carried out and is on `main`, except the items under "Left for the owner" and "Deferred". `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` and `cargo test` pass (441 tests, up from 366); Today, Week, Month and Settings were compared by screenshot before and after each wave.

What differs from the plan as written:

- **L11**: the HTTP client is shared per thread, not per process (a process-wide one leaked pooled connections between test runtimes). Same effect in the app and CLI.
- **L16**: `report.rs` is split into `report/{history,summary,plan,recommend,format}.rs`, but `summary_facts` was not cut into one helper per list (the lists share intermediate state).
- **S12**: `get_report` was dropped; its only caller was the dead export path removed in wave 1.
- **M3**: done conservatively. Messages that only change what a view keeps (typing, drag bookkeeping, timers, stream chunks) skip the refresh; one `invalidate()` clears every cache on any reload, save, stored report or minute tick.
- **M15**: `main.rs` (2,500 → 890 lines) now sits over `shell/{reports,keys,edits,recommend,settings_panel,shortcuts,refresh,suggest,dialog,dev}.rs`, with `Keys`, `Reports`, `LaterDays`, `UndoStack` and `Suggestions` as their own structs.
- **Extra finding, fixed**: `LlmConfig::endpoint_host` had the same hand-rolled URL parsing as S3, so `https://evil.example\@api.openai.com/v1` as the Recommendations endpoint would have been lent the main API key.

A regression review of the merged result (three fresh reviewers over the whole diff) found no high-severity problems. What it found, all fixed: the new fallback deadline (L5) cut off a local reasoning model that was still thinking, so a decision model on this machine now gets the local timeout; an abandoned primary request stayed open; a closed pipe cut `done`/`run-due` short; naming the saved base URL again dropped its keyless flag; the locked last working day looked unticked; a cancelled unlock could leave its error behind; backoff counted from the start of an attempt.

## Verdict

The layering is sound: `fortnight-core` holds all logic behind a small `Store` trait and one `LlmClient` trait, views return `Action`s and never touch the store or the network, and the SQLite layer, vault crypto and date maths are careful and well tested. The problems are the ones a fast multi-agent build leaves behind:

1. **`main.rs` absorbed everything else** (2,500 lines, ~45 fields on `Ready`): report scheduling, key handling, undo, recommendations and dev hooks. Report freshness is the most fragile part and has a real bug (M1).
2. **Everything is recomputed from SQLite after almost every message**, including each streamed chunk and each keystroke (M3), and the views rescan all task history every frame (V3, W12, V14).
3. **Three HTTP clients were written separately and drifted**, so copied helpers disagree on which errors are retried (L1) and on 404s (L4).
4. **Features were removed in the UI but not in the code**: the Week tab's plan card, the report editor/export, side-work log actions, notifications, unlock-at-launch.
5. **The CLI predates keyless endpoints and the decision model** and mishandles keys in three ways (C1–C3).
6. **Compound writes are not atomic** because the `Store` trait is one transaction per call (S1).

## Findings

Severity: **high / med / low** are bugs; **perf**, **dead**, **debt** as named.

### App shell — `main.rs`, `top_bar.rs`, `theme.rs`

| ID | Sev | What | Fix |
|---|---|---|---|
| M1 | high | `freshness` compares against `latest_report`, which returns the newest row *whatever its status*. A `Failed` row carries the current digest, so after a failure the report counts as fresh: it is never retried, and the panel shows neither the error nor Retry (it needs `Failed && stale`). Same for later-day recommendations (`DayRecs` caches `[]` on error). | Judge freshness against the newest **Ready** version; newest row `Failed` ⇒ stale (edited Ready still wins). `run_due`: `missing` = no Ready version. Clear `failures`/`last_attempt` when the endpoint or key changes. Retry failed `DayRecs` after the backoff. |
| M2 | med | After midnight the app stays on yesterday: `selected` never advances, so quick-adds and notes are filed under yesterday and Overdue/Recommended hide. | Track `last_today`; on `Tick`, if the date changed and `selected == last_today`, move `selected` to today and drop past `day_recs`. |
| M3 | perf | `refresh()` runs after nearly every message: SQL for all report versions, `summary_facts` over all events (twice), JSON digest, `tasks.clone()`, for every stream chunk, 1 s `SummaryWait`, 300 ms `NotesTick` and keystroke. `maybe_suggest` clones the form text each time. | Dirty flag (default dirty; whitelist view-local messages); stream deltas patch only the slot's text; cache `recommended_today` and `(kind, week) → (empty, digest)` behind a data-version counter; skip `maybe_suggest` for timer/stream messages. |
| M4 | perf | Browsing weeks (or later days) starts a paid LLM stream per stale period and never cancels it. | Abort non-scheduled generations for weeks no longer shown; make `DayRecs` abortable and do the same. |
| M5 | low | Tab switching is spread out: `OpenWeek` leaves the badge and skips catch-up; `FocusQuickAdd` skips catch-up; the badge is set for Plan reports the Week tab no longer shows. | One `show(tab, selected)` helper; badge only for Summary. |
| M6 | low | Lock doesn't abort `DayRecs` or priority-suggestion requests, which hold the key; results still land. | Make both abortable, track the handles in `key_requests`, clear `day_recs_pending`, bump `suggest_seq`. |
| M7 | low | Stream `Delta` doesn't check the generation id; a queued chunk from an aborted stream lands in the new one. | Filter on `g.id == id`. |
| M8 | low | A client-construction error skips failure accounting, so the banner is re-raised every minute. | Record `last_attempt`/`failures` in that branch. |
| M9 | low | Load models on a keyless non-Ollama server (LM Studio) calls only `/api/tags` and fails. | Fall back to `list_models()` when `ollama::detect` errors. |
| M10 | low | Undoing an edit also reverts a later drag (restores the old `position`). | Keep the current `position` when undoing an edit. |
| M11 | low | With an unknown system appearance iced's stock theme replaces Fortnight's. | `theme_for(mode.unwrap_or(false))`. |
| M12 | dead | Stale/misattached doc comments; unlock-at-launch remnant (`unlocking`); undo `what` strings computed then dropped; unreachable match arm. | Delete. |
| M13 | dead | `top_bar.rs` doc and `quick_add_task`'s `tab` parameter describe a quick-add bar that now lives in `day.rs`. | Move to a core helper used by Today and the CLI. |
| M14 | dead/debt | `theme::row_button` unused; `priority_color` needlessly `pub`; `tint`/`mix` private, so five views re-implement them; `dialog_backdrop` lives in `main.rs`. | Delete / publish / move. |
| M15 | debt | `main.rs` holds six subsystems. | Split into `reports.rs`, `recommend.rs`, `keys.rs`, `edits.rs`, `suggest.rs`, `dev.rs` (mechanical). |
| M16 | debt | Duplicated blocks: decision→main fallback routing ×2, "key locked for endpoint" ×2, Recommendation→`day::Recommended` ×2, `run_due + update_week_reports` ×5, settings close ×5. | One helper each. |

### Views — Today, Month, report panel

| ID | Sev | What | Fix |
|---|---|---|---|
| V1 | high | Dropping a dragged Today card on itself sends it to the bottom of the list and saves that order (`reorder`: the dragged id is removed first, so the target isn't found and defaults to the end). | `Before(dragged)` and a missing target return `None`. |
| V2 | med | An inbox task (no planned day) whose deadline has passed lands in the collapsed "No date" section, never Overdue (regression from `day()` becoming planned-day only). | Check overdue before the no-day early return. |
| V3 | perf | Section placement is recomputed per task per section per frame (quadratic over all tasks ever), including on every drag move. | Compute placement once in `load`. |
| V4 | low | Rank badges use the raw recommended index and are drawn in every section, so ticking #1 leaves "2, 3" and a "1" badge in Completed. | Number only the Recommended group, from the filtered list. |
| V5 | low | `hovered` goes stale after a form closes or a drop (removed widgets never send exit). | Clear it on DropOn/Expand/Save/Close/Delete and in `load`. |
| V6 | low | Escape with the form's due calendar open discards the whole form. | Close only the calendar first. |
| V7 | low | Notes starting with whitespace are rewritten mid-typing after autosave (trim mismatch), moving the cursor. | Compare with the same trim on both sides. |
| V8 | low | Deleting from the inline form leaves a selection ring on the next card. | Clear `selected`. |
| V9 | low | Project list keeps duplicates differing only by case order. | Sort by a cached key, then dedup. |
| V11 | dead | Report panel editor and export path (`StartEdit`, `EditAction`, `SaveEdit`, `CancelEdit`, `Export`, `export_report`, `Message::Exported`) can't be reached. `DiscardEdit` is live. | Delete. |
| V12 | dead | Report panel Plan path and its Week-tab consumers can't run: `panel_kinds` returns only Summary. `week_label` never read; `body_parses` is a test counter in production. | Delete; `#[cfg(test)]`. |
| V13 | dead | `Action::AddLog`/`DeleteLog`, `Recommended::why`, unreachable branches, stale comments, ~15 needlessly `pub` items (which hide dead code from rustc). | Delete; make private. |
| V14 | perf | Month grid shapes text and filters/sorts all tasks twice for each of 42 cells on every layout. | Measure the two mark widths once; group tasks by day once. |
| V15 | debt | Copies between views: colour mixing ×5, rank badge ×2, priority clamp ×3, week-number and month names (also in core), `labelled` ×2, drag machinery ×2. | Share from `theme`/core. |
| V16 | debt | The task form (≈600 lines) lives in `day.rs` though the Week dialog uses it; the subtask-in-notes format and the "today's list" rule are business logic in the view. | `views/task_form.rs`; move the rules to core. |

### Views — Week, Settings

| ID | Sev | What | Fix |
|---|---|---|---|
| W1 | med | Typed API key and password survive closing Settings by Escape, backdrop click or Save; only the X button wipes them. | Every hide goes through one wipe. |
| W2 | med | A Test result stays on screen after the endpoint is edited, implying the new endpoint works (or still fails). | Tie the result to the tested `LlmSettings`. |
| W3 | med | The unlock prompt keeps the typed password and old error after Escape. | `UnlockView::reset()` on every close. |
| W4 | low | The Week drag target is never cleared, so the landing line shows where a drop won't happen. | `on_exit` → `Leave`. |
| W5 | low | "Unsaved changes" shows while Save is disabled ("8" → "8.0"). | Dirty also requires the parsed settings to differ. |
| W6 | low | A key-state reload under the open password pop-up wipes the key but leaves the pop-up. | Close the key form in that branch. |
| W7 | low | The Ollama default URL ignores `OLLAMA_HOST`. | Use `ollama::local_base_url()`. |
| W8 | low | Settings can save zero working days, blanking the Week grid. | Can't untick the last day. |
| W9 | dead | Week tab's Recommended-tasks code (see V12). | Delete. |
| W10 | dead | `key_message` `Ok` text nobody shows; `Form.notifications` round-trips a parked setting. | Delete. |
| W11 | dead | Stale module docs in `week.rs` / `settings.rs`. | Correct. |
| W12 | perf | `view` filters and sorts all tasks ~25× per frame, on every pointer move while dragging. | Bucket per day in `load`. |
| W13 | perf | Settings `view` clones `Settings` ~3× and builds two `Form`s per frame; the prompt is copied on every editor action. | Keep a baseline; copy only on edits. |
| W15 | debt | Load-bar maths lives in the view while core's `DayLoad` is half unused. | Move to core; the view formats. |
| W16 | debt | Tooltip bubble ×6, bold font ×3, weekday names ×3. | `theme::tip()`, `theme::FONT_BOLD`, core helpers. |
| W17 | debt | `settings.rs` does several jobs; time/hours parsing belongs in core. | `views/unlock.rs`, `settings/` submodules, helpers to `parse`. |

### Core — LLM clients, reports, decision model

| ID | Sev | What | Fix |
|---|---|---|---|
| L1 | med | reqwest 0.13 reports a connection dropped mid-body as `Decode`; both `transport_error`s map that to non-retryable `Malformed`. Anthropic streams and all `chat()` calls lose their retry. | One shared `transport_error`: timeout → `Timeout`, everything else → `Connection`. |
| L2 | med | Undated tasks are capped at 10 by age, so with a long backlog a task added for today is missing from the facts: it can't be recommended, and if it's the only one, nothing is. | Always include tasks on today's list; cap the rest. |
| L3 | med | `parse_recommendations` takes first `{` to last `}`, so any brace in surrounding prose breaks it; a wrong-key object parses as "nothing to recommend". Stored `Ready`, so never retried. | Try each `{` with a streaming deserializer; make `tasks` required; store an unparseable Plan as `Failed`. |
| L4 | low | Every 404 becomes "model not found", including a base URL missing `/v1`. | Only when the body mentions the model. |
| L5 | low | The fallback starts only after the primary's whole retry budget (up to ~30 min on a hung local model). | Time-box the primary's first item. |
| L6 | low | Two summary facts read the task's current state, not its state at week end, so past weeks change and regenerate. | `status_at_end` / `due_at_end`. |
| L7 | low | A task due in 7 days is "due Monday" on a Monday. | Weekday-only for ≤ 6 days. |
| L8 | low | Error bodies are truncated before the key is redacted; two clients' `Debug` print the full base URL (may hold credentials). | Redact first; host-only `Debug`. |
| L9 | low | Priority suggestions send a private task's title and notes. | Skip private tasks. |
| L10 | debt | ~150 lines of constants, retry, error and model-list helpers copied between `anthropic.rs` and `openai.rs` (the cause of L1/L4). | `llm_http.rs`. |
| L11 | perf | A new `reqwest::Client` (TLS config, empty pool) per request. | One process-wide client. |
| L12 | dead | `report::generate`, `request_preview`, `Recommendation::focus`, `RecommendedTask::day`, test-only `with_timeout`/`with_backoff`. | Delete / `#[cfg(test)]`. |
| L13 | dead | `llm::with_env_overrides`, `LlmError::Locked`, test-only `collect_stream`. | Delete. |
| L14 | debt | Stale doc comments in `report.rs`; the plan user message says "Plan my week" while the system prompt asks for today's top three. | Correct; bump `PROMPT_VERSION`. |
| L15 | debt | `decision.rs` re-parses the plan facts by string keys. | Deserialize into `PlanFacts`. |
| L16 | debt | `report.rs` is 3,000 lines mixing six jobs. | `report/` submodules along the existing section markers. |

### Core — storage, domain, utilities

| ID | Sev | What | Fix |
|---|---|---|---|
| S1 | med | No atomic compound writes: undo is delete-then-restore (a failure between loses the task and its history), day notes are N deletes then an add, reorder is N autocommits. | `revert_task`, `replace_logs`, `set_positions`, each one transaction. |
| S2 | med | Markdown export groups by UTC date, so evening notes land on the next day. | Pass the time zone. |
| S3 | low (security) | `url_host` disagrees with reqwest's parser on backslashes: `http://evil.example\@localhost/v1` passes the "local, plain http is fine" check but connects to `evil.example` with the key. | Parse with `reqwest::Url`. |
| S4 | low | The CLI's lock warning ignores the `.local` host handling `datadir` already has. | Publish and use `same_host`. |
| S5 | low | Two instances launched together both take the lock silently. | `create_new` first. |
| S6 | debt | Foreign keys are on before migrations run, so any future table rebuild of `tasks` fails on every existing database. | Migrate, then enable; `foreign_key_check` before commit. |
| S7 | perf | Report versions are never pruned (tens of rows per active day) and all are loaded per refresh. | Prune to the newest 20 unedited per kind and week. |
| S8 | dead | `Store::set_done`, `Report::key`, unused `Protocol` helpers, `Settings::notifications`. | Delete. |
| S9 | dead | `week::month_start`, `parse::parse_date`, `export::all_file_name`, `vault::change_password` (also a trap: it would re-encrypt one of two files sharing the password). | Delete. |
| S10 | debt | `QuickAdd::due_on` is really the planned day. | Rename to `day`. |
| S11 | debt | `format_minutes`, `month_abbr`, week-number rule duplicated in the app. | Publish from `week`/`parse`. |
| S12 | debt | The app calls SqliteStore-only `add_log_at`; no `get_report(id)`, so one export loads every report. | Add both to the trait. |
| S13–14 | debt | Stale comments (`vault.rs` Argon2 parameters, `secure.rs`); tests that can't fail or leak temp files. | Correct. |

### CLI, build, docs

| ID | Sev | What | Fix |
|---|---|---|---|
| C1 | med | `summary` and `run-due` open the decision key (and prompt for a password) though the summary never uses it; under cron that fails everything. A main key that fails to open is silently dropped. | Open the decision key only for a Plan; main-key errors are fatal. |
| C2 | med | A `--base-url` override inherits the saved `keyless` flag, dropping the supplied key. | Reset on override; add `--keyless`. |
| C3 | med | Every connect error is mapped to "usage", which `run-due` reads as "not configured" and exits 0. | Only `NotConfigured` is usage. |
| C4 | low | `run-due`/`summary`/`plan` call the model on empty facts (the app skips). | Skip when empty. |
| C5 | low | `list --today` disagrees with the Today tab (carried-over tasks missing). | Shared core rule. |
| C7 | low | Every command creates the folder and database, so a typo'd `--data-dir` "works". | Only `add` may create. |
| C8 | low | `export \| head` exits 1 with "Broken pipe". | Treat `BrokenPipe` as success. |
| C10 | low | `--protocol` accepts decision protocols for the main endpoint. | Reject. |
| C11 | low | `plan --week` stores a plan keyed to another week whose content is about today. | Remove the flag. |
| C12 | low | Password and env keys pass through unzeroed `String`s. | `Zeroizing`. |
| C13 | low | `xtask` hard-codes `target/`, ignores `CARGO_TARGET_DIR`, builds without `--locked`. | Honour both. |
| C15–16 | dead | `notify-rust` and `futures-util` unused in the app; `serde_json` test-only in the CLI; tokio features broader than needed. | Trim. |
| C17 | debt | CLI and app each implement plan-client routing, settings→config, quick-add→task and the today rule. | Core helpers. |
| C18 | dead | `llm_setup::resolve_key`. | Delete. |
| C19–21 | docs | README: 8 factual errors. `ORCHESTRATION.md`/`PLAN.md` describe a build process and UI that no longer exist. `CLAUDE.md` points at a wiped temp folder and a removed env var. | Correct; mark the two design docs historical; vendor `raw2png.py`. |
| C22 | debt | `.gitignore` misses `*.snap.new`, `.DS_Store`; `check-deps.ps1` never reaches its `exit 2`. | Fix. |

## Fix plan

Six waves. Within a wave, agents worked in separate worktrees on disjoint files; each wave was merged, checked (`fmt`, `clippy -D warnings`, `test`) and pushed before the next started.

| Wave | Agent | Owns | Findings |
|---|---|---|---|
| 1 | sweep | whole workspace, deletions and trivial dedupe only | S8 S9 S13 L12 L13 L14 (docs) C15 C16 C18 M12 M14 V11 V12 V13 W9 W10 W11 W16 (tip, bold) V15 (colour mix, priority clamp) |
| 1 | docs/build | `README.md`, `ORCHESTRATION.md`, `PLAN.md`, `.gitignore`, `ci/`, `xtask/` | C13 C19 C21 C22 |
| 2 | llm-http | `anthropic.rs` `openai.rs` `ollama.rs` `llm.rs` `fallback.rs` `secure.rs`, top of `decision.rs`, new `llm_http.rs` | L1 L4 L5 L8 L10 L11 S3, core side of C17 (plan routing, saved-key rule) |
| 2 | report | `report.rs` → `report/`, `decision.rs` from `plan_question` down | L2 L3 L6 L7 L14 L15 L16, core side of W15 |
| 2 | storage | `sqlite.rs` `store.rs` `domain.rs` `datadir.rs` `vault.rs` `export.rs` `week.rs` `parse.rs`, `tests/`, new `notes.rs` | S1 S2 S4 S5 S6 S7 S10 S11 S12 S14, core side of V16 C5 C17 W17 |
| 3 | day | `views/day.rs`, new `views/task_form.rs`, `top_bar.rs` | V1–V9 V16 M13 |
| 3 | week/month | `views/week.rs` `month.rs` `report_panel.rs`, `calendar.rs` | W4 W12 W15 V14 V15 S11 |
| 3 | settings | `views/settings.rs` → `settings/`, new `views/unlock.rs` | W1–W3 W5–W8 W13 W17 |
| 3 | cli | `crates/fortnight-cli/` | C1–C5 C7–C12 C17 |
| 4 | main | `main.rs` | M1–M11 M16 L9, adoption of S1 S12 S7 W1 W3 C17 |
| 5 | split | `main.rs` → modules | M15 |
| 6 | review | whole diff | regression check, screenshots of each tab |

## Left for the owner

Not changed, because they are product or legal choices rather than defects:

- **Estimate format differs per tab**: Today shows "1h30m", Week shows "1.5h" (`week.rs` `duration`).
- **Month sorts a day's tasks by priority first**; Week and Today use the manual order.
- **Opening another task's form discards unsaved edits** in the open one, without asking (V6, second half).
- **Project field** opens empty with the current project as placeholder (V10); `combo_box::State::with_selection` fixes it but then filters the list by that text.
- **`fortnight-cli log --task`** writes task log lines the GUI never shows (they still feed the summary). Kept as is.
- **No `LICENSE` file** although `Cargo.toml` says MIT, and release archives omit the Inter font licence.
- **`release.yml`** does a redundant third macOS build and hand-patches the bundle instead of `cargo xtask bundle --universal`. Untestable from here, so left alone.
- **`CLAUDE.md` rules** ("frozen files", "do not push") describe the original build rounds and no longer match how the repo is worked on. Only its factual errors were corrected.

- **Saved settings with no working days** (possible before this change) now block Save until a day is ticked; the "Working days" label turns red, with no message.
- **CLI changes scripts may notice**: `plan --week` is gone; every command but `add` fails on a missing data folder instead of creating one; connect errors other than "not configured" exit 1, not 2; `--protocol` accepts only `openai` or `anthropic`; `list --today` includes carried-over tasks; `summary`, `plan` and `run-due` skip the model when there is nothing to report.

## Deferred

Not worth the risk now: a shared drag module for Today and Week (two different position schemes), a generic SSE driver over both protocols, incremental event reads (`events_between(MIN, MAX)` on every reload; measure first), provider inference from Settings into core, one `labelled` form helper for Today and Settings.
