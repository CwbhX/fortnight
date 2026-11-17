# Fortnight — 24-hour prototype build

> **Historical.** This records how the prototype was planned and built, in rounds of sub-agents. The app has changed a lot since. For how it works now, the code and [README.md](README.md) are the reference.

A working prototype on macOS within a day: one orchestrator, nine sub-agent runs, three steps. [PLAN.md](PLAN.md) remains the full design; this builds the smallest slice of it that does the real job.

## What the prototype does

- **Tasks**: add, complete, edit, delete; due date, project, notes. A quick-add bar that understands `fri`, `tomorrow`, `nov 20`, `+3d` and `#project`.
- **Portable data**: one `fortnight-data/` folder beside the executable.
- **Encrypted API key**: password unlock at launch, held in memory while the app is open.
- **Your endpoint**: base URL, model and key in Settings, with a Test button. OpenAI-compatible format.
- **Weekly summary**: generated from the week's tasks; editable; copy to clipboard.
- **Week-ahead advice**: at-risk deadlines and a suggested order.
- **Automatic**: both are generated when due, checked at launch and every minute, with catch-up.

## What it leaves out

Everything else in PLAN.md, to be added afterwards:
- The Anthropic format and streaming output (a spinner instead).
- The CLI, OS-scheduler registration, notifications.
- Change history, estimates and capacity maths, log lines, the private flag.
- Undo, export to file, report versions.
- Windows and Linux builds, CI, release archives. The code stays cross-platform; it just isn't built or tested there yet.

## Structure

Two crates, so that agents working on logic never wait for the GUI to compile.

```
crates/fortnight-core/src/
  domain.rs      Task, Report, Settings, WeekRef          (step 1, frozen)
  store.rs       Store trait                              (step 1, frozen)
  llm.rs         LlmClient trait, request and error types (step 1, frozen)
  sqlite.rs      agent A
  parse.rs       agent B
  week.rs        agent B      week maths, which reports are due
  vault.rs       agent C
  openai.rs      agent C
  report.rs      agent D      facts for the week, prompts, generate
  datadir.rs     agent F
crates/fortnight-app/src/
  main.rs        orchestrator
  theme.rs       agent E
  views/tasks.rs     agent E
  views/review.rs    agent G
  views/week.rs      agent H
  views/settings.rs  agent I   includes the unlock prompt
```

Three tables: `tasks`, `reports`, `settings`.

## Steps

### Step 1 — Foundation (orchestrator, about 1–2 hours)

- Workspace, every dependency, pinned toolchain.
- The three frozen files above, plus stub signatures for everything else, so the workspace compiles.
- A bare iced window with an input, a list and a Markdown block. If iced misbehaves, this is where we find out.
- A short `CLAUDE.md`: own-files-only, no new dependencies, use the iced 0.14 docs and examples.

### Step 2 — Six agents in parallel (about 2–3 hours)

| Agent | Builds | Model |
|---|---|---|
| A | SQLite store: schema, task and report operations | Opus 5.5 |
| B | Quick-add parser, week maths, which reports are due | Opus 5.5 |
| C | Encrypted key file; OpenAI-compatible client with mock-server tests | Opus 5.5 |
| D | Week facts, the two prompts, and `generate` (facts → prompt → client → store) | Opus 5.5 |
| E | Theme and the Tasks view | Opus 5.5 |
| F | Data-folder resolution; README with build steps | Sonnet 5.5 |

### Step 3 — Three agents in parallel, then wiring (about 4–5 hours)

| Agent | Builds | Model |
|---|---|---|
| G | Review view: facts, summary, edit, copy, regenerate | Opus 5.5 |
| H | This-week view with the Advice card | Opus 5.5 |
| I | Settings view and the unlock prompt | Opus 5.5 |

While those run, the orchestrator wires `main.rs`: navigation, connecting views to the store, vault and report generation, and the minute tick. Then it merges the views, runs the app, and fixes what breaks.

That is roughly 8–10 hours of build time, which leaves room inside 24 hours for things going wrong. These are estimates, not measurements.

## Rules

Four, and no more:

1. **Each agent edits only its own files.**
2. **The frozen files don't change.** An agent that thinks one is wrong stops and says so.
3. **Each agent works in its own worktree**, so nobody builds against half-written code.
4. **An agent is done when** `cargo fmt --check`, `cargo clippy -- -D warnings` and its tests pass. The orchestrator re-runs them after merging.

Views don't call the store or the LLM directly. Each returns an action such as "generate the summary", and `main.rs` carries it out. This is what lets the views be built at the same time.

## Git

Work happens on a local `prototype` branch. Nothing is pushed and `main` is untouched until you say so.

## Your part

- At the end, open Settings, enter your endpoint, model and key, and press Test. The key never needs to pass through the orchestrator.
- Look at it and say what feels wrong. Agents can make it work; whether it is elegant is your call.

---

# Round 2 — the rest of the product

Round 1 is done and merged on `prototype`. Round 2 builds nearly everything else in [PLAN.md](PLAN.md): 15 agent runs and 2 reviewers, in four steps.

## What round 2 adds

- **New navigation**: no sidebar. A top bar with three tabs — Today, Week, Month — over one selected date, plus a Settings gear. The Week tab absorbs the old This-week and Review tabs. PLAN.md section 4.2 is the spec.
- **Change history**: every task change is recorded, so the summary can say what progressed, what slipped and what was dropped.
- **Richer tasks**: estimate (`~2h`), priority (`!`), in-progress and dropped states, a private flag, a planned day, and log lines for work that isn't a checkbox.
- **Capacity**: estimated load against working hours per day, in the facts and in the Week tab.
- **Streaming output** for both reports, replacing the spinner.
- **Anthropic Messages format** as a second protocol.
- **Report extras**: earlier versions of a report, "what will be sent", export to Markdown or JSON.
- **App extras**: undo for delete, keyboard shortcuts, notifications, a Lock button, a lock file for the data folder, the model list in Settings.
- **CLI**: `fortnight-cli` with `add`, `list`, `done`, `log`, `summary`, `plan`, `run-due`, `export`.
- **Cross-platform**: CI on macOS, Windows and Linux, the dependency check, a release workflow, and `cargo xtask bundle` for `Fortnight.app`.

Still left out: registering with the OS scheduler (it cannot unlock an encrypted key) and code signing.

## Step 1 — Unfreeze, extend, refreeze (orchestrator)

This is the only point in round 2 where frozen files change.

- Remove round 1's worktrees and branches, but only those fully merged into `prototype`.
- `Cargo.toml`: add the `fortnight-cli` and `xtask` crates and the new dependencies (`clap`, `rpassword`, `eventsource-stream`, `futures-util`, `notify-rust`, `rfd`, and reqwest's `stream` feature).
- `domain.rs`: `Task` gains `estimate_minutes`, `priority`, `private`, `planned_for`; add `Event` and `EventKind`; `LlmSettings` gains `protocol`; `Settings` gains hours per day and a notifications switch.
- `store.rs`: history and log lines, listing a week's events, setting status, listing all versions of a report.
- `llm.rs`: a streaming call beside the existing one.
- Views: restructure the files for the new navigation, then extend the `Action` enums and `*Data` structs for everything in step 3.
  - `views/tasks.rs` becomes `views/day.rs`.
  - `views/review.rs` becomes `views/report_panel.rs`, a component the Week tab embeds for both Advice and Summary.
  - `views/month.rs` is new.
  - `main.rs`: replace the sidebar with the top bar (tabs, period navigation, the Today button, lock and gear) and move the quick-add bar there, driven by one shared selected date.
  - `theme.rs`: add the shared styles the new views need (tab control, day column, calendar cell, icon button). A view may add private style helpers in its own file.
- Stub every new signature so the workspace compiles and round 1's tests still pass, then update `CLAUDE.md` and refreeze.

## Step 2 — Nine agents in parallel

| Agent | Builds | Owns | Model |
|---|---|---|---|
| A | Schema v2: new columns, `events` table, event writing on every change, new store methods. A v1 data folder must migrate without loss. | `core/sqlite.rs` | Opus 5.5 |
| B | Quick-add: `~2h`, `~30m`, `!` | `core/parse.rs` | Opus 5.5 |
| C | Streaming for the OpenAI-compatible client, with a fallback when an endpoint ignores it; model list | `core/openai.rs` | Opus 5.5 |
| D | Anthropic Messages client, streaming, stop reasons, mock-server tests | `core/anthropic.rs` | Opus 5.5 |
| E | Facts from history (progressed, slipped, dropped, log lines), capacity maths, private tasks excluded, updated prompts | `core/report.rs` | Opus 5.5 |
| F | Markdown and JSON export of tasks and reports | `core/export.rs` | Sonnet 5.5 |
| G | Lock file for the data folder | `core/datadir.rs` | Sonnet 5.5 |
| H | CI and release workflows, static C runtime on Windows, dependency check scripts | `.github/`, `.cargo/config.toml`, `ci/` | Sonnet 5.5 |
| I | `cargo xtask bundle`; README brought up to date | `xtask/`, `README.md` | Sonnet 5.5 |

## Step 3 — Six agents in parallel, plus wiring

| Agent | Builds | Owns | Model |
|---|---|---|---|
| J | Today tab: overdue, the day, collapsed No date; estimate, priority, private, in-progress and dropped, log lines; keyboard navigation | `app/views/day.rs` | Opus 5.5 |
| K | Week tab: a column per working day with load bars, optional weekend column, hosts the report panel | `app/views/week.rs` | Opus 5.5 |
| L | Report panel: Advice or Summary by week, streamed text, facts, edit, copy, export, versions, "what will be sent" | `app/views/report_panel.rs` | Opus 5.5 |
| M | Settings overlay: protocol, model list, hours per day, notifications, data folder path | `app/views/settings.rs` | Opus 5.5 |
| N | The CLI | `crates/fortnight-cli/` | Sonnet 5.5 |
| O | Month tab: calendar grid, three titles per day and a count, click-through to a day or a week, summary dot | `app/views/month.rs` | Opus 5.5 |

Meanwhile the orchestrator extends `main.rs`: streaming, choosing the client by protocol, undo, the shortcuts in PLAN.md section 4.4, notifications, Lock, the lock file, the export dialog.

## Step 4 — Run, review, fix

- Merge, run the app, and screenshot every view in light and dark. Fix what is broken or looks wrong.
- Open a round 1 data folder and confirm nothing is lost.
- Two fresh Opus 5.5 reviewers, in parallel, given the code and PLAN.md and no summaries:
  - **Security**: the key vault, both clients, logging. Is the key ever written, logged or left in memory after Lock?
  - **Correctness**: the migration, history, week boundaries, duplicate or missed reports.
- Fix what they find.

## Rules

Unchanged from round 1. Frozen files change only in step 1.

## Git and CI

Work continues on the local `prototype` branch. The workflows are written either way, but Windows and Linux are only actually built and tested if the branch is pushed, and that happens only if you say so.
