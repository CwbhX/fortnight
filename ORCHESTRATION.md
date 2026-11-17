# Fortnight — 24-hour prototype build

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
