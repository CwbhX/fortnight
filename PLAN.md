# Fortnight — implementation plan

> **Historical.** This is the original design, written before the prototype was built. The app has since changed in many places. For how it works now, the code and [README.md](README.md) are the reference.

A local, cross-platform todo app in Rust. It keeps your tasks on your own machine, and it uses an LLM endpoint you supply to do two things: write a weekly summary of your work for your manager, and give you feedback on how to fit the coming week's tasks into their deadlines.

The name fits the product: the app looks at a fortnight — the week behind you (review) and the week ahead (plan).

The app is **fully portable**: one self-contained executable per platform, no installer, no runtime to install, and all data in a single folder beside the executable. Copying that folder to another machine — or carrying it on a USB stick between Windows, macOS and Linux — moves everything. Section 3 defines this precisely.

Versions and activity figures below were checked against crates.io and GitHub on 2026-11-16.

## 1. Recommendation

| Area | Choice | Why |
|---|---|---|
| GUI | **iced 0.14** (pure Rust, MIT) | One language, one binary, no webview. Its state → view → message model fits a todo app, it ships a Markdown widget for the summary view, and it is proven in the COSMIC desktop. |
| Storage | **SQLite** via `rusqlite` (bundled) | One file, transactional, queryable history. The summary needs "what changed this week", which plain-text formats handle badly. |
| LLM access | **Thin hand-written client** over `reqwest`, behind a trait | The app makes one kind of call (system + user → text). Two wire formats cover almost every endpoint: OpenAI-compatible Chat Completions and the Anthropic Messages API. |
| Secrets | An encrypted key file in the data folder, unlocked with a password | The key travels with the data and is never stored in the clear. An OS keychain would tie it to one machine. |
| Dates | `jiff` 1.0 | Correct time zones, DST and week arithmetic. |
| Automation | In-app schedule with catch-up, plus a headless CLI | Works when the app is open and catches up when it wasn't, without registering anything on the host. |
| Distribution | Build from source, or plain unsigned archives | No installer, nothing written outside the data folder. Code signing is out of scope. |

The most important structural decision is independent of the GUI choice: **all logic lives in a UI-free core crate**. The GUI and the CLI are thin shells over it. That keeps the app testable and makes the GUI framework replaceable.

## 2. Research findings

### 2.1 GUI framework

| Framework | Latest stable | What it is | Verdict for this app |
|---|---|---|---|
| **iced** | 0.14.0 (2025-12-07) | Pure Rust, Elm-style, GPU-rendered | **Chosen.** See below. |
| Slint | 1.19.0 (2026-11-16) | Declarative `.slint` markup + Rust logic | **Fallback.** Stable 1.x API, live preview, built-in date picker and accessibility. UI is written in a second language, and the free license for non-GPL apps requires showing Slint attribution. |
| egui / eframe | 0.37.0 (2026-10-15) | Pure Rust, immediate mode | Easiest to write and the most downloaded pure-Rust option. Looks like a tool rather than a polished product unless heavily restyled. |
| Tauri | 2.12.6 (2026-11-16) | Rust backend + system webview | Most mature overall, and the easiest route to visual polish. **Ruled out by portability**: it depends on a webview runtime installed on the host (WebView2 on Windows, WebKitGTK on Linux). The UI is also HTML/CSS/JS rather than Rust. |
| Dioxus | 0.7.10 (0.8 in alpha) | React-like Rust; desktop uses a webview | All-Rust code with CSS styling. **Ruled out by portability** for the same webview dependency; the native renderer is experimental. |
| GPUI + gpui-component | gpui 0.2.2; gpui-component 0.10.0 | Zed's framework + Longbridge's component kit | Best-looking defaults. The ecosystem is fragmented (the official crate lags Zed's main branch; several third-party distributions exist) and gpui-component ships breaking-prone releases weekly. Too much churn for a small app. |
| Freya, Xilem, Floem, Makepad | pre-1.0 or stale | — | Too early, or not actively released. |

Why iced:

- **It matches the problem.** A todo app is a list, a few inputs and some derived views. iced's model is a state struct, an `update` function over a message enum, and a `view` function. The framework's own canonical example is a todo app.
- **It has the widgets this app needs.** `text_input`, `checkbox`, `scrollable`, `text_editor` (to edit the summary), and `markdown` (to render LLM output, including task lists). System light/dark theme detection is built in.
- **Async is first-class.** `Task::perform`, `Task::run` and `Task::sip` cover a streaming LLM call; `time::every` covers the schedule tick. With the `tokio` feature, `reqwest` works directly.
- **Everything is Rust and MIT-licensed.** Styling is plain Rust code, so a custom, restrained look is fully in our hands.
- **It is self-contained.** No webview or UI runtime is needed on the host. Its default build includes both a GPU renderer (`wgpu`) and a software renderer (`tiny-skia`) with fallback between them, so the same executable runs on machines with no usable GPU, such as VMs and remote desktops.
- **It is used in production.** System76's COSMIC desktop is built on it.

Known costs of iced, and how the plan handles them:

- **Pre-1.0 with breaking releases.** The last crates.io release is eleven months old; `master` is at `0.15.0-dev` and the 0.15 milestone is 119 of 130 issues closed, so an upgrade is likely soon. Mitigation: pin `0.14`, keep the UI crate thin, and budget a half-day for the 0.15 migration.
- **No screen-reader support in mainline.** AccessKit integration exists only in forks. If accessibility is a requirement, choose Slint instead.
- **No built-in date picker.** Dates are typed in the quick-add bar (`fri`, `tomorrow`, `nov 20`, `+3d`), which is faster anyway. `iced_aw` 0.14.1 provides a `date_picker` widget if a calendar popup is wanted later.

Milestone 0 is a one-day spike that validates this choice before any real work depends on it.

### 2.2 Storage

- **`rusqlite` 0.43** with the `bundled` feature compiles SQLite into the binary, so there is no system dependency on any platform. `rusqlite_migration` 2.7 handles schema versions.
- `sqlx` is async and heavier than a single-user local app needs. `turso` (the Rust SQLite rewrite) is still pre-1.0. `redb` is a key-value store and would mean hand-writing the queries the reports need.
- Plain-text formats (todo.txt, Markdown) are readable but make history and reporting hard. Instead, the app offers **export** to Markdown and JSON.

### 2.3 LLM client

"Pass in an API endpoint" maps to three settings: base URL, model name, and an optional API key. The OpenAI-compatible Chat Completions format is the common denominator: it is served by OpenAI, Ollama, LM Studio, llama.cpp, vLLM, OpenRouter and most hosted providers.

| Option | Latest | Assessment |
|---|---|---|
| **Hand-written over `reqwest` 0.13** | — | **Chosen.** About 200 lines per wire format. No dependency churn, full control over which parameters are sent. |
| `genai` | 0.7.5 | Native adapters for many providers behind one API. The best choice if native multi-provider support becomes a goal. Pre-1.0. |
| `async-openai` | 0.43.0 | Complete OpenAI typed client with a configurable base URL. Far more surface than one chat call needs. |
| `rig-core` | 0.46.0 | An agent framework. Overkill here. |

The app supports two protocols, selected in settings:

1. **OpenAI-compatible** (default) — `POST {base_url}/chat/completions`, `Authorization: Bearer <key>` (omitted when no key is set, as with local servers).
2. **Anthropic Messages** — `POST {base_url}/v1/messages` with `x-api-key` and `anthropic-version` headers. Default model `claude-opus-5-5`. The client must check `stop_reason` (`refusal`, `max_tokens`) before reading content.

Both send the minimum: model, messages, `stream: true`, and `max_tokens` only where the protocol requires it. **No `temperature` or other sampling parameters** — several current models reject them, and omitting them is what makes one client work across endpoints.

### 2.4 Secrets, dates, builds

- **Secrets.** The `keyring` crate (4.2) stores keys in macOS Keychain, Windows Credential Manager or the Linux Secret Service. All three are per-machine, so a key saved on one computer is missing on the next. The plan therefore keeps the key in the data folder instead (section 3.3) and does not depend on `keyring`.
- **`jiff` 1.0** for civil dates, time zones and week boundaries. Deadlines are stored as civil dates (`YYYY-MM-DD`); timestamps are stored in UTC. Nothing stored depends on the machine's time zone.
- **TLS.** `reqwest` 0.13 with its `rustls` feature, so there is no dependency on a system OpenSSL.
- **Builds.** No installer tooling is needed. `cargo-zigbuild` 0.24 builds the Linux executable against an old glibc so it runs on older distributions.

### 2.5 What a good weekly update looks like

The writing guidance that sources agree on shapes the summary prompt: keep it short and scannable, use the same sections in the same order every week, lead with three to five outcomes that matter to the reader, call out blockers explicitly, and include what comes next.

## 3. Portability

"Fully portable" is taken to mean four things. Each is a testable requirement.

### 3.1 One self-contained executable

- A single file per platform. Fonts, icons and SQLite are compiled in. There are no side files, no installer and no runtime to install.
- **Windows**: the C runtime is linked statically (`-C target-feature=+crt-static`), so no Visual C++ redistributable is needed.
- **macOS**: one universal binary covering Apple Silicon and Intel, with a conservative minimum OS version.
- **Linux**: built against glibc 2.28 with `cargo-zigbuild`. The window system and graphics libraries are loaded at run time and exist on any desktop install. A fully static musl build is not possible for the GUI, because a static binary cannot load those libraries; the CLI can be static.
- CI inspects each executable's dynamic dependencies (`otool -L`, `dumpbin /dependents`, `ldd`) and fails if anything beyond the platform baseline appears.

### 3.2 All data in one folder beside the executable

```
anywhere/
├── fortnight            (or fortnight.exe, or Fortnight.app)
└── fortnight-data/
    ├── fortnight.db     tasks, history, reports, settings
    └── llm-key          the API key, encrypted (see 3.3)
```

- Location order: `--data-dir` flag → `FORTNIGHT_DATA_DIR` → `fortnight-data/` beside the executable.
- On macOS the folder sits beside `Fortnight.app`, not inside it, because writing inside a bundle invalidates its signature.
- If that location is not writable, the app says so and asks where to put the data. It never falls back silently to a hidden per-user directory, since that would split the data across machines.
- The data path is shown in Settings.
- The database is a standard SQLite file, identical across operating systems and CPU architectures. It stores no absolute paths and no machine identifiers.
- SQLite runs in rollback-journal mode with `synchronous=FULL`, not WAL. That leaves exactly one database file at rest, works on FAT/exFAT sticks, and is safer when a drive is pulled.

### 3.3 The API key: encrypted on disk, unlocked in memory

If an API key is set, it is stored only in encrypted form, in `fortnight-data/llm-key`. There is no plaintext mode. If no key is set — a local endpoint such as Ollama needs none — the file does not exist and the app never asks for a password.

**Encryption**
- The password is stretched with Argon2id (per-file random salt) into a 256-bit key.
- The API key is sealed with XChaCha20-Poly1305 under a random nonce.
- The file holds a format version, the Argon2 parameters, the salt, the nonce and the ciphertext. The header is authenticated, so tampering with the parameters fails the unlock.
- The password itself is never stored. A wrong password is detected by the authentication check failing.
- Writes are atomic (temporary file, then rename), so a pulled drive cannot leave a half-written key.

**Unlocking**
- At launch, if a key file exists, the app asks for the password. The prompt can be skipped: the todo features work fully while locked, and only the LLM features wait.
- Once unlocked, the decrypted key is held in memory for as long as the app is open. It is not written to disk, logged, or passed to another process.
- The in-memory copy is wiped when the app quits or when you choose **Lock** in Settings (`zeroize`, wrapped in `secrecy` so it cannot be printed or serialised by accident).
- If a scheduled report comes due while the app is locked, the report waits and the view shows "Unlock to generate". Nothing is lost.

**Managing the key** (Settings)
- Set or replace the API key: asks for a password (twice when creating one).
- Change the password: decrypts and re-encrypts under the new one.
- Remove the key: deletes the file.
- A forgotten password cannot be recovered. The remedy is to remove the key and enter it again, which is acceptable because an API key is replaceable.

**Command line**
- `fortnight-cli` asks for the password on the terminal when a command needs the LLM, or reads it with `--password-stdin`.
- `FORTNIGHT_LLM_API_KEY` supplies a key for one process without touching the file. It is never persisted.

**What this protects, and what it doesn't**
- A lost or copied data folder does not expose the API key.
- While the app is unlocked, the key is in process memory. Software already running as you on that machine could read it; no desktop app can prevent that.
- Tasks and reports in `fortnight.db` are not encrypted. Encrypting the database is a separate feature and is not in this plan.

### 3.4 No traces on the host

- Nothing is written outside the data folder: no registry keys, no files in per-user config or cache directories, no autostart entries.
- Logs go to the data folder, size-capped.
- Registering with the OS scheduler is the one feature that touches the host. It is opt-in, labelled as non-portable, and reversible with one command (section 8).

### 3.5 Limits worth knowing

- **No code signing.** Builds are unsigned. An executable you build yourself is not flagged, because macOS Gatekeeper and Windows SmartScreen act on downloaded files. A downloaded prebuilt archive will show a one-time warning per machine. Building from source (section 3.6) is therefore the primary way to get the app.
- **One writer at a time.** If the folder lives in a synced directory (Dropbox, iCloud), do not open it on two machines at once. The app takes a lock file and warns if one already exists.
- **OS notifications** are best-effort for an uninstalled app, especially on Windows. The in-app badge is the reliable signal.

### 3.6 Building from source

Anyone can produce the executable on their own machine with one command:

```
cargo build --release --locked
```

- **Prerequisites**: Rust via `rustup`; on Windows the MSVC build tools; on macOS the Xcode command-line tools; on Linux a C compiler (SQLite is compiled from bundled source). No other system packages.
- `rust-toolchain.toml` pins the compiler and `Cargo.lock` is committed, so every build uses the same versions.
- The portability settings (static C runtime on Windows, release profile) live in `.cargo/config.toml` and `Cargo.toml`, so a plain `cargo build` produces the portable binary without extra flags.
- `cargo xtask bundle` wraps the macOS binary in a minimal `Fortnight.app` so it launches without a terminal window. It is a small Rust script in the repo, not an external tool.
- The README documents these steps per platform.

## 4. Product design

### 4.1 Principles

- **The app is complete without an LLM.** Every report has a deterministic, factual version computed in Rust. The LLM only turns facts into prose and advice.
- **The LLM never does arithmetic.** Days until due, weekday names, overdue flags and load versus capacity are computed in Rust and handed over as facts.
- **Nothing leaves the machine silently.** You can see exactly what will be sent, and mark tasks private.
- **Keyboard first, one window, few controls.**

### 4.2 Views

There is no sidebar. One slim bar across the top holds everything:

```
[ Today | Week | Month ]      ‹  Mon 16 – Fri 20 Nov  ›   Today                 lock  gear
[ Add a task — e.g. Write Q4 report fri #work                                           ]
```

The three tabs are three zoom levels over the same tasks and the same selected date, the way a calendar app works. `‹` `›` move by a day, a week or a month depending on the tab; **Today** jumps back. Settings and Lock are icons, not tabs.

The quick-add bar is always visible. `Write Q4 report fri #work ~2h !` creates a task due Friday, in project `work`, estimated at two hours, high priority. In the Today tab, a task typed without a date lands on the day being shown.

**Today**
- Overdue (only if there is any), then the day's tasks, then done items collapsed.
- **No date** sits collapsed at the bottom with a count. It is the inbox: tasks you haven't scheduled.
- Each row: checkbox, title, project chip, due label. Selecting a row expands it in place for notes, due date, estimate and its log.
- A **log line** records work that isn't a checkbox ("helped Sam debug the deploy"), attached to a task or free-standing.
- Nothing else is on screen. This is the view the app opens on.

**Week** (the work week)
- One column per working day, each listing that day's tasks with checkboxes, and a load bar once tasks have estimates. Today's column is marked with the accent colour.
- A weekend column appears only when Saturday or Sunday has tasks.
- Below the columns, one report panel that depends on the week:
  - the current or a future week shows **Advice** — what is at risk, a suggested order, what to cut;
  - a past week shows the **Summary** for your manager, beside the facts it was built from;
  - once the current week's summary is due, the panel offers both.
- Panel actions: Edit, Copy, Export, Regenerate, earlier versions, and "what will be sent".

This one tab replaces the earlier This-week and Review tabs: paging back through weeks is how you reach old summaries.

**Month**
- A calendar grid. Each day shows its date and up to three task titles, then `+2 more`. Done tasks are dimmed, overdue ones red, today outlined in the accent colour, weekends muted.
- Clicking a day opens it in the Today tab. Clicking a week's label opens it in the Week tab; the label carries a dot when that week has a summary.
- The grid is for seeing and navigating, not editing.

**Settings** (the gear; opens over the current view)
- LLM: protocol, base URL, model, API key, and a **Test connection** button. Key controls: set, change password, remove.
- Schedule: week start day, summary day and time (default Friday 16:00), plan day and time (default Monday 08:00). *(Later replaced: only the week start day remains, Sunday by default; the summary is due from 16:00 on the last working day, and recommendations are not scheduled.)*
- Capacity: working days and focus hours per day.
- Summary style: length, tone, and free-form extra instructions.
- Data folder path.

Left out on purpose: drag-and-drop between days, hour-by-hour time blocking, and custom views. A task is moved by editing its date.

**Where the ideas come from**
- *Things 3*: Today shows only today and what is overdue; everything else stays out of sight.
- *Sunsama*: the week as one column per day with a daily workload, and a review at the end of the week.
- *TickTick*: day, week and month as views of one calendar, switched with `1` `2` `3`, with unscheduled tasks kept to one side.
- *Todoist*: the same task list shown as a list, a week or a month rather than as separate places.

### 4.3 Visual design

- Follows the system light/dark theme. One accent colour, taken from the existing icon (`#ea580c`).
- One bundled typeface, generous spacing, no borders where whitespace will do.
- No sidebar and no toolbars: the top bar and the quick-add bar are the only chrome.

### 4.4 Keyboard

Single-letter keys apply when no text field has focus.

| Key | Action |
|---|---|
| `1` `2` `3` | Today, Week, Month |
| `←` `→` | Previous / next day, week or month |
| `T` | Jump to today |
| `N` or `Ctrl/Cmd+N` | Focus quick-add |
| `↑` `↓` | Move selection |
| `Space` | Toggle done |
| `Enter` | Expand / edit |
| `Ctrl/Cmd+,` | Settings |
| `Ctrl/Cmd+Z` | Undo last change |

## 5. Architecture

```
fortnight/
├── Cargo.toml                 workspace
├── crates/
│   ├── fortnight-core/        no UI dependencies
│   │   ├── domain/            Task, Event, Week, Report
│   │   ├── store/             SQLite repository + migrations
│   │   ├── parse/             quick-add syntax, date tokens
│   │   ├── digest/            weekly facts: summary digest, plan digest
│   │   ├── llm/               LlmClient trait, two protocol adapters, prompts
│   │   └── schedule/          which reports are due, and when
│   ├── fortnight-app/         iced GUI      → binary `fortnight`
│   └── fortnight-cli/         clap CLI      → binary `fortnight-cli`
└── assets/                    icon, font
```

The CLI is a separate binary because a Windows GUI binary has no console.

Data flow for a report:

```
SQLite ──► digest (pure Rust, deterministic) ──► prompt ──► LLM endpoint
                    │                                          │
                    └──► shown as "the facts" in the UI        └──► Markdown, stored and editable
```

Threading: database calls are synchronous and take microseconds at this scale, so they run directly in `update`. LLM calls run as async iced tasks and stream tokens back as messages. The CLI and GUI can run at once on the same machine because SQLite is opened with a busy timeout.

Both binaries resolve the data folder through one function in the core, so they always agree on where the data is.

## 6. Data model

```sql
CREATE TABLE tasks (
  id               INTEGER PRIMARY KEY,
  title            TEXT    NOT NULL,
  notes            TEXT    NOT NULL DEFAULT '',
  status           TEXT    NOT NULL CHECK (status IN ('todo','doing','done','dropped')),
  project          TEXT,
  due_on           TEXT,                 -- deadline, civil date YYYY-MM-DD
  planned_for      TEXT,                 -- day you intend to do it
  estimate_minutes INTEGER,
  priority         INTEGER NOT NULL DEFAULT 0,
  private          INTEGER NOT NULL DEFAULT 0,   -- never sent to the LLM
  position         REAL    NOT NULL,     -- manual ordering
  created_at       TEXT    NOT NULL,     -- UTC
  updated_at       TEXT    NOT NULL,
  completed_at     TEXT
);

-- Append-only history. This is what makes weekly reporting possible.
CREATE TABLE events (
  id       INTEGER PRIMARY KEY,
  task_id  INTEGER REFERENCES tasks(id),  -- NULL for free-standing log lines
  at       TEXT NOT NULL,
  kind     TEXT NOT NULL,   -- created, status_changed, due_changed, planned_changed, log, edited
  payload  TEXT NOT NULL    -- JSON: old and new values, or the log text
);

CREATE TABLE reports (
  id             INTEGER PRIMARY KEY,
  kind           TEXT NOT NULL CHECK (kind IN ('summary','plan')),
  week_start     TEXT NOT NULL,           -- civil date of the week's first day
  status         TEXT NOT NULL,           -- pending, generating, ready, failed
  digest_json    TEXT NOT NULL,           -- exact facts given to the model
  content_md     TEXT,                    -- model output
  edited_md      TEXT,                    -- your edits; shown in preference to content_md
  model          TEXT,
  endpoint_host  TEXT,
  prompt_version INTEGER NOT NULL,
  error          TEXT,
  created_at     TEXT NOT NULL
);
CREATE INDEX reports_week ON reports (kind, week_start, created_at);

CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
```

Notes:
- The events table lets the summary say "moved the deadline twice" or "worked on X, not finished" rather than only listing completed checkboxes.
- Regenerating a report inserts a new row, so earlier versions and your edits are never overwritten.
- Storing `digest_json` makes every report reproducible and auditable.
- The database lives in `fortnight-data/` beside the executable (section 3.2). The API key is kept out of it, in its own file, so the database can be shared or backed up without exposing the key.
- Settings live in this database rather than a per-user config file, so they move with the data.

## 7. LLM integration

### 7.1 Configuration

Precedence, highest first: CLI flag → environment variable → saved settings.

| Setting | Environment variable | Example |
|---|---|---|
| Protocol | `FORTNIGHT_LLM_PROTOCOL` | `openai` or `anthropic` |
| Base URL | `FORTNIGHT_LLM_BASE_URL` | `http://localhost:11434/v1` (Ollama), `https://api.anthropic.com` |
| Model | `FORTNIGHT_LLM_MODEL` | whatever the endpoint serves |
| API key | `FORTNIGHT_LLM_API_KEY` | otherwise decrypted from the key file after unlock (section 3.3) |

For OpenAI-compatible endpoints, the settings screen can call `GET {base_url}/models` to offer a model list, falling back to free text if the endpoint doesn't support it.

### 7.2 Client interface

```rust
pub struct ChatRequest { pub system: String, pub user: String }

pub enum ChatEvent { Delta(String), Done { model: String, stop: StopReason } }

pub trait LlmClient: Send + Sync {
    fn chat(&self, req: ChatRequest) -> BoxStream<'static, Result<ChatEvent, LlmError>>;
}
```

- Two implementations: `OpenAiCompatible` and `AnthropicMessages`. A `MockClient` serves tests and offline UI work.
- Streaming is parsed from server-sent events (`eventsource-stream`). If an endpoint ignores `stream: true` and returns a plain JSON body, the client handles that too.
- `LlmError` distinguishes the cases the UI must explain differently: not configured, connection refused, unauthorised, model not found, rate limited, timed out, refused by the model, truncated, malformed response.
- Retries: up to three attempts with backoff on connection errors, 429 and 5xx. Never on 4xx.

### 7.3 Weekly summary

Digest (built in Rust for the chosen week, private tasks excluded):
- Completed tasks, with project, completion day and notes.
- Tasks progressed but not finished, with their log lines.
- Free-standing log lines.
- Deadlines that moved, and tasks dropped.
- Tasks added during the week.
- Counts by project.

Prompt rules:
- Use only the facts provided. Do not invent work, metrics or outcomes.
- Write in the first person for a manager who knows the projects but not the details.
- Lead with outcomes, not activity. Group by project.
- Fixed sections, in order: **Highlights**, **Completed**, **In progress**, **Risks and blockers**, **Next week**.
- Respect the configured length and tone. Plain Markdown.

The next-week section is drawn from open tasks due in the following week, so the summary and the plan agree.

### 7.4 Weekly plan feedback

Digest (built in Rust):
- Today's date and weekday; each remaining working day with its capacity.
- Every open task with: days until due, due weekday, estimate, priority, project, how many times its deadline has moved.
- Precomputed totals: estimated hours due this week against available hours; tasks with no estimate; tasks already overdue.

The model returns:
1. **Focus** — one sentence on what this week is about.
2. **At risk** — deadlines unlikely to be met at the current load, and why.
3. **Suggested order** — a day-by-day sequence.
4. **Cut or renegotiate** — what to defer, drop, or raise with someone.
5. **Gaps** — tasks missing an estimate or a deadline that make planning unreliable.

The plan can be regenerated mid-week; it then works from the remaining days only.

Later enhancement: ask for structured output (a JSON list of `{task_id, suggested_day, reason}`) so each suggestion gets an **Apply** button that sets `planned_for`. Structured-output support varies across endpoints, so this needs a tolerant parser and stays out of the first version.

### 7.5 Privacy and safety

- **Show what will be sent**: the digest is viewable before and after every call.
- The `private` flag excludes a task from every digest.
- A local endpoint (Ollama, LM Studio) keeps everything on the machine.
- No telemetry. Logs record the endpoint host, model, timing and status — never task content or the key.
- Task text is passed as data inside a clearly delimited block, and the system prompt tells the model to treat it as content, not instructions.

## 8. Weekly automation

A single pure function decides what is due:

```rust
fn due_reports(now: Zoned, settings: &Schedule, existing: &[ReportKey]) -> Vec<ReportJob>
```

- **Summary** for week *W* is due from the configured time (default Friday 16:00 local) and remains due until one exists for *W*.
- **Plan** for week *W* is due from the configured time (default Monday 08:00) until one exists.
- *(Later: the times are no longer settings. The summary is due from 16:00 on the week's last working day, and the plan is not scheduled, since the Today tab makes it whenever it shows it.)*
- The function is evaluated at launch and once a minute while the app runs. That covers waking from sleep.
- **Catch-up**: if the app was closed at the scheduled time, the report is generated on next launch. Only the most recent missed week is generated automatically; older weeks have a manual Generate button.
- Generation is idempotent per (kind, week). A failed job is marked `failed` and retried with backoff, with a visible Retry.
- When a report is ready: a badge on the view, and optionally an OS notification (`notify-rust`).
- With no LLM configured, the deterministic facts view is produced instead and nothing fails.

This in-app schedule is the default and is fully portable: it needs nothing from the host, and catch-up means a report is never missed, only delayed until the next launch.

Unattended operation, without opening the app, is available but is **not portable**:

```
fortnight-cli run-due            # generate anything that is due, then exit   (portable)
fortnight-cli schedule install   # register with launchd / systemd user timer / Task Scheduler
fortnight-cli schedule remove    # undo it
```

`schedule install` writes one scheduler entry on the current machine, pointing at the executable's current path. It is opt-in, it is the only thing the app ever writes outside its data folder, and it breaks if the folder moves. Because the API key is encrypted and an unattended job cannot type a password, scheduled runs work only with an endpoint that needs no key (a local model). With a hosted endpoint, reports are generated the next time you open and unlock the app. A tray icon with launch-at-login is not planned, for the same reason.

Other CLI commands, all thin wrappers over the core: `add`, `list`, `done`, `log`, `summary [--week]`, `plan`, `export`.

## 9. Dependencies

| Purpose | Crate | Version |
|---|---|---|
| GUI | `iced` (features: `tokio`, `markdown`, `svg`, `advanced`) | 0.14 |
| SQLite | `rusqlite` (feature: `bundled`) | 0.43 |
| Migrations | `rusqlite_migration` | 2.7 |
| HTTP | `reqwest` (features: `rustls`, `json`, `stream`; no native TLS) | 0.13 |
| SSE parsing | `eventsource-stream` | 0.2 |
| Async runtime | `tokio` | 1 |
| Dates | `jiff` | 1.0 |
| Key encryption | `argon2` 0.6, `chacha20poly1305` 0.11 | |
| Secret handling in memory | `zeroize` 1.9, `secrecy` 0.10 | |
| Password prompt (CLI) | `rpassword` | 7.5 |
| Serialisation | `serde`, `serde_json` | 1 |
| Errors | `thiserror` | 2 |
| CLI | `clap` | 4 |
| Logging | `tracing` | 0.1 |
| Notifications | `notify-rust` | 4.19 |
| File dialogs (export) | `rfd` | 0.18 |
| Tests | `insta` 1.49, `wiremock` 0.6 | |
| Linux build | `cargo-zigbuild` (build tool, not a dependency) | 0.24 |

Every dependency must compile from source with no system library beyond the platform baseline. `keyring` and `directories` from the earlier draft are gone: both exist to use per-machine locations.

Toolchain: Rust 1.99.1 is already installed at `~/.cargo/bin`, though that directory is not on the current shell `PATH`.

## 10. Milestones

Each milestone ends with something usable.

**M0 — Spike (1 day).** Workspace skeleton. An iced window with a text input, a list with checkboxes, a rendered Markdown block, and light/dark switching.
CI builds the spike for all three platforms and runs the dynamic-dependency check from section 3.1.
*Exit:* runs on macOS; the look and the code feel right; the Windows and Linux executables start on a clean machine or VM with nothing installed, including one without a GPU. If not, switch to Slint now — nothing else depends on the choice yet.

**M1 — Core (2–3 days).** Domain types, data-folder resolution and lock file, SQLite store with migrations, event log, quick-add parser, week arithmetic.
*Exit:* core tests pass, including date parsing and week boundaries across DST and different week-start days. A data folder created on one OS opens unchanged on another.

**M2 — Tasks view (3–4 days).** Quick-add, sections, toggle, inline edit, log lines, keyboard shortcuts, undo, theme.
*Exit:* usable as a daily todo app with no LLM.

**M3 — LLM client and settings (3–4 days).** Both protocol adapters, streaming, error mapping, encrypted key file, unlock prompt and lock state, settings screen, Test connection.
*Exit:* mock-server tests pass for both protocols; Test connection works against a local Ollama and one hosted endpoint; the key file contains no plaintext, a wrong password is rejected, and the todo views work while locked.

**M4 — Review view and summary (3 days).** Summary digest, prompt, Review view with facts and streamed summary, edit, copy, export, regenerate, payload preview.
*Exit:* a real week of tasks produces a summary you would send.

**M5 — This-week view and plan feedback (2–3 days).** Plan digest with capacity maths, Advice card, day grouping.
*Exit:* advice correctly flags an overloaded week and an at-risk deadline in test fixtures.

**M6 — Automation (2 days).** `due_reports`, minute tick, catch-up, retry, notifications, `fortnight-cli run-due` and `schedule install`.
*Exit:* with the clock mocked past Friday 16:00, a summary appears without interaction; the same happens via the CLI with the app closed.

**M7 — Polish and release builds (2–3 days).** Empty states, error messages, icon, embedded font, README build instructions, `cargo xtask bundle`, and a release workflow that publishes plain unsigned archives: a Windows `.zip` (x64, ARM64), a macOS universal `.zip`, and Linux `.tar.gz` (x86_64, aarch64). No installers.
*Exit:* each archive unpacks to one executable that runs on a clean machine; a data folder carried between all three platforms on an exFAT stick works on each.

Later: structured plan suggestions with Apply buttons, recurring tasks, Markdown/JSON import.

### MVP execution plan

> The first build is the 24-hour prototype in [ORCHESTRATION.md](ORCHESTRATION.md), which is smaller than this MVP and supersedes the ordering below.

The MVP is the shortest path to the core loop: keep tasks, get a weekly summary and plan advice from your endpoint, automatically, from a portable folder with an encrypted key. About ten working days, in eight steps. Each step ends with something you can run.

**In the MVP**
- Tasks: add, complete, edit, delete; due date, project, notes; quick-add with date tokens.
- Data folder beside the executable; the full schema from section 6, so later features need no data migration.
- Encrypted API key with unlock at launch and Lock.
- One LLM protocol: OpenAI-compatible, non-streaming, with Test connection.
- Review view: facts plus generated summary; edit, copy, regenerate.
- This-week view: tasks by day plus an Advice card.
- Automatic generation at the scheduled time, with catch-up on launch.
- Runs on macOS; CI builds Windows and Linux on every push so portability doesn't rot.

**Deferred until after the MVP**
- Anthropic Messages adapter and streaming output.
- The CLI binary, OS-scheduler registration and OS notifications.
- Estimates and capacity maths, free-standing log lines, the private flag in the UI.
- Undo, the full shortcut set, export to file, report version history.
- Release archives, `cargo xtask bundle`, lock file for synced folders, headless UI tests.

| Step | Days | Work | Done when |
|---|---|---|---|
| 1. Skeleton and spike | 1 | Workspace with `fortnight-core` and `fortnight-app`; pinned toolchain; CI (fmt, clippy, test, build on three platforms); an iced window with an input, a checkbox list and a Markdown block. | The window runs on macOS and CI is green on all three. **Go/no-go on iced.** |
| 2. Core data | 2 | Data-folder resolution; SQLite schema and migrations; task operations that also write events; quick-add parser; week arithmetic. | Core tests pass; a data folder is created beside the binary and nowhere else. |
| 3. Tasks view | 2–3 | Quick-add bar; Overdue / Today / This week / Later / No date sections; toggle, inline edit, delete; system theme. | You can use it as your daily todo app. |
| 4. Key vault and LLM client | 2 | Encrypted key file; unlock prompt and Lock; settings for base URL, model and key; OpenAI-compatible call with error mapping; Test connection. | Mock-server tests pass; Test connection succeeds against your endpoint; the key file holds no plaintext. |
| 5. Weekly summary | 1 | Summary digest; prompt; Review view with facts beside the summary; edit, copy, regenerate. | A real week of your tasks yields a summary you would send. |
| 6. Plan advice | 1 | Plan digest (days until due, overdue, moved deadlines); Advice card in This week. | Advice names the at-risk deadlines in a fixture week. |
| 7. Automation | 1 | `due_reports`; check at launch and every minute; "Unlock to generate" when locked; retry on failure. | With the clock mocked past Friday 16:00, the summary appears unprompted. |
| 8. Hardening | 1 | A week of real use on your own data; fix what grates; move the folder to another location and machine; README with build steps. | The folder works after being moved, and a fresh clone builds with one command. |

Order matters in two places. Step 1 comes first because it is the only point where changing GUI framework is free. Steps 2–3 come before any LLM work because the summary is only as good as the task history behind it, and using the app for a few days produces real data to test steps 5–6 against.

After the MVP, the deferred items map onto the remaining work in M3–M7, in roughly this order: streaming, Anthropic adapter, estimates and capacity, log lines, CLI, release archives.

## 11. Testing

- **Core unit tests**: quick-add parser, date tokens, week maths, `due_reports` over a table of clock and settings cases.
- **Snapshot tests** (`insta`): digests and rendered prompts for fixture weeks. A prompt change shows up as a reviewed diff.
- **Store tests**: in-memory SQLite; migrations run from every earlier schema version.
- **Key file tests**: encrypt/decrypt round trip, wrong password, tampered header and ciphertext, truncated file, and a fixed test vector so the format cannot change by accident.
- **LLM client tests** (`wiremock`): both protocols, streaming and non-streaming, and each error class (401, 404, 429, 5xx, timeout, connection refused, truncated stream, refusal).
- **UI tests**: iced's headless `tester` feature for two flows — add and complete a task; open Review and see a summary from the mock client.
- **CI**: `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test` on macOS, Windows and Ubuntu.
- **Portability checks** in CI: the dynamic-dependency allow-list per platform; a test that runs the app with a redirected home directory and asserts that nothing is created outside the data folder; a cross-platform round trip of a fixture data folder.

Prompt quality cannot be unit-tested. Keep four or five fixture weeks (quiet, overloaded, many slips, mostly unplanned work, empty) and read the output for each when a prompt changes.

## 12. Risks

| Risk | Mitigation |
|---|---|
| iced 0.15 breaks APIs | Pin 0.14; thin UI crate; plan the migration as a discrete task. |
| iced lacks screen-reader support | Decide at M0 whether this matters; Slint is the fallback. |
| Endpoints differ in small ways | Send minimal parameters; tolerate non-streaming replies; Test connection surfaces problems early; mock-server tests per quirk. |
| The model invents accomplishments | Facts-only prompt; facts shown beside the prose; summary is always editable before it is sent anywhere. |
| Small local models give weak advice | Deterministic at-risk and load figures are shown regardless of model quality. |
| Sensitive task content sent to a third party | Payload preview, private flag, local endpoints supported. |
| API key on a lost USB stick | Always encrypted at rest with Argon2id + XChaCha20-Poly1305; keys are also revocable. |
| Weak password chosen | Argon2id slows guessing; minimum length enforced; strength hint at creation. |
| Forgotten password | No recovery by design; remove and re-enter the key. |
| Tasks on a lost stick are readable | Out of scope; database encryption noted as a possible later feature. |
| Drive removed mid-write | Rollback journal with `synchronous=FULL`; SQLite recovers on next open. |
| Folder opened on two machines via a sync service | Lock file and warning; documented as unsupported. |
| A dependency pulls in a system library | Dependency allow-list enforced in CI from M0. |
| Linux desktops without the expected graphics libraries | Software renderer fallback; glibc 2.28 baseline; tested on a minimal VM. |
| Laptop asleep at the scheduled time | Catch-up on wake and on launch. |
| Unsigned downloads trigger a one-time OS warning per machine | Build from source avoids it; signing is out of scope for now. |

## 13. Decisions for you

Defaults are in place for all of these, so none of them blocks starting.

1. **Accessibility.** If screen-reader support matters, the GUI should be Slint rather than iced. Default: iced.
2. **Unattended runs.** Default: in-app schedule with catch-up only. The OS-scheduler command exists but is opt-in and non-portable.
3. **Summary delivery.** The app stops at copy and export; it does not send email or post to chat. Default: keep it that way.
4. **Week shape.** Default: Monday start, summary Friday 16:00, plan Monday 08:00, five working days.
5. **Licence for the repo**, if it will be public. iced's MIT licence imposes no constraint.
6. **Meaning of "portable".** This plan reads it as a no-install app whose data travels with it. If you meant something else — for example phones, or running in a browser — say so, because that changes the GUI choice.

## Sources

- [iced releases](https://github.com/iced-rs/iced/releases) and [0.14.0 release notes](https://github.com/iced-rs/iced/releases/tag/0.14.0)
- [iced 0.14 API docs](https://docs.rs/iced/0.14.0/iced/)
- [Phoronix on iced 0.14](https://scour.ing/p/https://www.phoronix.com/news/Iced-0.14-Rust-GUI-LIbrary)
- [iced accessibility issue #552](https://git.tdem.in/iced-rs/iced/issues/552)
- [gpui-component / GPUI Kit](https://github.com/longbridge/gpui-component)
- [GPUI README in the Zed repository](https://github.com/zed-industries/zed/blob/main/crates/gpui/README.md)
- [Slint royalty-free licence](https://slint.dev/agreements/slint-royalty-free-license.pdf) and [Slint pricing FAQ](https://slint.dev/pricing)
- [Dioxus releases](https://github.com/DioxusLabs/dioxus/releases) and [Dioxus desktop guide](https://dioxuslabs.com/learn/0.7/guides/desktop)
- [Rust GUI comparison, August 2026](https://www.pistack.xyz/posts/2026-08-25-rust-gui-frameworks-egui-iced-slint-comparison/)
- [Thanks for All the Frames: Rust GUI Observations](https://tritium.legal/blog/desktop)
- [Are we GUI yet?](https://areweguiyet.com/)
- [genai](https://docs.rs/genai/latest/genai/), [async-openai](https://docs.rs/async-openai)
- [keyring](https://docs.rs/keyring/latest/keyring/)
- [Things 3 features](https://culturedcode.com/things/features/), [TickTick calendar guide](https://ticktick.com/resources/article/7340680464711024640/calendar-workflows-guide), [Sunsama guided planning and reviews](https://sunsama.com/features/guided-planning-and-reviews), [Todoist calendar layout](https://todoist.com/help/articles/use-the-calendar-layout-in-todoist-lPHRQTu0o)
- [Weekly Updates: Improving Communication](https://theresponsibleengineer.substack.com/p/weekly-updates-communication)
- [Hidden Powers of Weekly Reports](https://engmanage.substack.com/p/hidden-powers-of-weekly-reports)
- [How to write async status updates managers read](https://remote-work-tools.pages.dev/how-to-write-async-status-updates-that-managers-actually-read)
- Crate versions and repository activity: crates.io and GitHub APIs, queried 2026-11-16
