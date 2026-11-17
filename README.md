<img src="assets/icon.svg" width="64" height="64" alt="">

# fortnight

Fortnight is a small, portable todo app (built and tested on macOS). It keeps your tasks and their history in a local SQLite file and, once a week, uses an LLM endpoint that you supply to write a summary of what you got done and advice for the week ahead. Nothing leaves your machine except the requests to that endpoint. [PLAN.md](PLAN.md) is the full design.

Created in [T3 Code](https://t3.codes).

## What it does

- **Today / Week / Month**: three tabs over one selected date, with `‹` `›` to move by a day, week or month and **Today** to jump back. Today shows overdue tasks, the day, and a collapsed *No date* inbox. Week has a column per working day with a load bar, and hosts the report panel. Month is a calendar grid that opens a day or a week.
- **Quick-add**: one bar, always visible. `Write Q4 report fri #work ~2h !` creates a task due Friday, in project `work`, estimated at two hours, high priority. Dates: `fri`, `tomorrow`, `nov 20`, `+3d`. Estimates: `~2h`, `~30m`. `!` is priority, `#name` the project.
- **Richer tasks**: notes, estimate, priority, in-progress and dropped states, a private flag (private tasks are never sent to the LLM), and **log lines** for work that isn't a checkbox ("helped Sam debug the deploy").
- **History**: every change to a task is recorded, so the summary can say what progressed, what slipped and what was dropped.
- **Weekly summary and week-ahead advice**: built from the history and from capacity (estimated load against working hours per day), with the arithmetic done in Rust. The text streams in as it is written. Edit, copy, regenerate, export, look at earlier versions, and see exactly what will be sent. Without an LLM configured you still get the factual version.
- **Automatic generation**: the summary is made Friday 16:00 and the advice Monday 08:00. Fortnight checks at launch and every minute, and catches up if it was closed at the time. An optional notification says when one is ready.
- **Your endpoint**: OpenAI-compatible (`https://api.openai.com/v1`, Ollama at `http://localhost:11434/v1`) or the Anthropic Messages format (`https://api.anthropic.com`). Settings has a Test button and a model list.
- **Encrypted API key**: a password unlocks it at launch; it lives in memory only while the app is open, and **Lock** wipes it. The app is fully usable while locked; only the LLM features wait.
- **Undo**, **export** of tasks and reports to Markdown or JSON, a **lock file** that warns if the data folder is already open elsewhere, and a **CLI**.

### Keyboard

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

## Build and run

You need Rust via [`rustup`](https://rustup.rs). The exact toolchain is pinned by `rust-toolchain.toml` and installed automatically.

| Platform | Also needs |
|---|---|
| macOS | Xcode command-line tools (`xcode-select --install`) |
| Windows | MSVC build tools (the C runtime is linked statically, so nothing needs installing to run it) |
| Linux | A C compiler. SQLite is built from bundled source. The window system and graphics libraries are loaded at run time and exist on any desktop install |

```
cargo build --release --locked
./target/release/fortnight          # the app    (fortnight.exe on Windows)
./target/release/fortnight-cli      # the command-line tool
```

or `cargo run --release -p fortnight-app`.

### Fortnight.app (macOS)

```
cargo xtask bundle              # this machine's architecture
cargo xtask bundle --universal  # Apple Silicon and Intel
```

This builds the release binary and wraps it in `target/bundle/Fortnight.app`, so it launches without a terminal window. Any existing bundle is replaced. `--universal` needs both Rust targets (`rustup target add aarch64-apple-darwin x86_64-apple-darwin`) and joins them with `lipo`. The icon is rendered from `assets/icon.svg` with tools that ship with macOS; if that fails the bundle is made without one. Move `Fortnight.app` wherever you like, but keep the data folder beside it (below).

## Where your data lives

Everything is in one folder, `fortnight-data/`, beside the executable:

```
anywhere/
├── fortnight            (or fortnight.exe, or Fortnight.app)
└── fortnight-data/
    ├── fortnight.db     tasks, history, reports and settings (a standard SQLite file)
    ├── llm-key          your API key, encrypted (only if you set one)
    └── fortnight.lock   present while the app has the folder open
```

The folder is chosen in this order:

1. `--data-dir <path>`
2. the `FORTNIGHT_DATA_DIR` environment variable (ignored if empty)
3. `fortnight-data/` beside the executable. If the executable is inside `Fortnight.app`, the folder goes beside the `.app`, not inside it (writing inside a bundle would invalidate its signature).

Fortnight never falls back to a per-user directory, and writes nothing outside the data folder. To move your data, copy the folder. Data from earlier versions opens unchanged.

**The lock file.** The app takes `fortnight.lock` while it is open (the CLI does not, so it can run beside the app). One app at a time: if the folder is in a synced directory (Dropbox, iCloud), don't open it on two machines at once. The app warns if the lock is already there.

**The key file.** `llm-key` holds your API key sealed with XChaCha20-Poly1305 under a key stretched from your password with Argon2id. A lost or copied data folder does not expose the key. It does not protect the key while the app is unlocked (software running as you could read process memory), and `fortnight.db` itself is **not** encrypted. A forgotten password cannot be recovered: remove the key in Settings and enter it again. A local endpoint that needs no key never creates the file and never asks for a password.

## First run

1. Start the app and open **Settings** (the gear, or `Cmd+,`).
2. Choose the protocol, enter the base URL, model name, API key and a password. The password encrypts the key on disk and unlocks it at each launch.
3. Press **Test** to check the connection.

## Environment variables

Precedence, highest first: command-line flag, environment variable, saved settings.

| Variable | Meaning |
|---|---|
| `FORTNIGHT_DATA_DIR` | Data folder (below `--data-dir`) |
| `FORTNIGHT_LLM_PROTOCOL` | `openai` or `anthropic` |
| `FORTNIGHT_LLM_BASE_URL` | e.g. `http://localhost:11434/v1`, `https://api.anthropic.com` |
| `FORTNIGHT_LLM_MODEL` | whatever the endpoint serves |
| `FORTNIGHT_LLM_API_KEY` | a key for this process only; never written to disk. Otherwise the key is decrypted from `llm-key` after unlock |

Development aids for the app: `FORTNIGHT_PAGE=today|week|month|settings`, `FORTNIGHT_SKIP_UNLOCK=1`, and `FORTNIGHT_SCREENSHOT=<file>` (the app screenshots its own window, writes raw RGBA and exits).

## Command line

`fortnight-cli` works on the same data folder, with no window. Global options: `--data-dir <path>`, and `--password-stdin` to read the key password from standard input instead of asking on the terminal (only commands that call the LLM need it).

```
fortnight-cli add "Write Q4 report fri #work ~2h !"   # quick-add syntax
fortnight-cli list                                    # tasks
fortnight-cli done <id>                               # mark a task done
fortnight-cli log "helped Sam debug the deploy"       # a log line
fortnight-cli summary [--week]                        # the weekly summary
fortnight-cli plan                                    # the week-ahead advice
fortnight-cli run-due                                 # generate whatever is due, then exit
fortnight-cli export                                  # tasks and reports to Markdown or JSON
```

Run `fortnight-cli --help` for the exact arguments; the flags above are as planned and may differ in detail.

## Development

```
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

## Not included

- **Registering with the OS scheduler** (launchd, systemd, Task Scheduler). It is the one feature that would write outside the data folder, and an unattended job cannot type the password for an encrypted key. `fortnight-cli run-due` can be run from your own scheduler against a no-key (local) endpoint. Otherwise, reports are generated the next time you open and unlock the app.
- **Code signing.** Builds are unsigned. One you build yourself isn't flagged, but a downloaded prebuilt archive shows a one-time Gatekeeper or SmartScreen warning per machine.
- **Notifications** are best-effort for an uninstalled app, especially on Windows; the in-app badge is the reliable signal.
