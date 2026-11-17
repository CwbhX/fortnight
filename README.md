<img src="assets/icon.svg" width="64" height="64" alt="">

# Fortnight

Fortnight is a portable todo app. It keeps your tasks and their history in a SQLite file on your machine. Once a week it sends that history to an LLM endpoint you choose, which writes a summary of your week and recommends what to work on next. Nothing else leaves your machine. It is built and tested on macOS. [PLAN.md](PLAN.md) is the full design.

Created in [T3 Code](https://t3.codes).

## Quick start

1. Install Rust from [rustup.rs](https://rustup.rs). On macOS, also run `xcode-select --install`. On Windows, also install the [Visual Studio Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/) with the "Desktop development with C++" workload.
2. Get the code and build it:

   ```
   git clone https://github.com/CwbhX/fortnight.git
   cd fortnight
   cargo build --release --locked
   ```

   The first build downloads the pinned Rust version and every dependency, and takes a few minutes.
3. Start the app:

   ```
   ./target/release/Fortnight
   ```

   On Windows, run `target\release\Fortnight.exe`. On macOS you can run `cargo xtask bundle` instead, which makes `target/bundle/Fortnight.app`.

Your data goes in a `fortnight-data` folder beside the program, here `target/release/fortnight-data`. To keep it somewhere else, start the app with `--data-dir <folder>`. To set up the LLM, see [First run](#first-run).

## What it does

- Three tabs show the same tasks around one selected date. Today lists overdue tasks, the day's tasks and a collapsed No date inbox. Week has a column per working day with a load bar, and the report panel below. Month is a calendar; click a day or a week to open it. `‹` and `›` move by a day, a week or a month, and Today jumps back.
- One quick-add bar is always visible. `Write Q4 report fri #work ~2h !` creates a task due Friday in project `work`, estimated at two hours, with priority 1. Dates can be `fri`, `tomorrow`, `nov 20` or `+3d`. Estimates can be `~2h`, `~30m` or `~1h30m`. `!`, `!!` and `!!!` set the priority.
- A task has notes, an estimate, a priority, a planned day, and a status of to do, in progress, done or dropped. A private task is never sent to the LLM. Log lines record work that isn't a task, such as "helped Sam debug the deploy".
- Every change to a task is recorded. The weekly summary uses that history to say what you finished, what progressed, which deadlines moved and what you dropped.
- Recommended tasks compare estimated work with your focus hours per day. Rust does all the counting; the model picks the order. They show as numbered badges in the Week tab and as a list under it.
- The Week tab keeps both up to date on its own: when you open it, anything changed since the last update is sent once. You can copy, edit and export the summary. Both system prompts can be changed in Settings.
- From Friday at 16:00 the summary is also kept up to date in the background, so it is ready when you need it. Fortnight checks at launch and every minute. A dot on the Week tab says when a background update is ready.
- The endpoint can be any OpenAI-compatible server, such as Ollama at `http://localhost:11434/v1`, or the Anthropic Messages API at `https://api.anthropic.com`. Settings has a Test connection button and a model list.
- The API key is stored encrypted under a password. Click the lock icon in the top bar to unlock it into memory; click again to lock, which wipes it and stops any request that holds it. The app works while locked; only the LLM features wait.
- Cmd+Z or Ctrl+Z undoes the last add, status change, edit or delete.
- Tasks, history and reports export to Markdown or JSON.

### Keyboard

Single keys work when no text field has focus. Press Escape to leave a text field.

| Key | Action |
|---|---|
| `1` `2` `3` | Today, Week, Month |
| `←` `→` | Previous or next day, week or month |
| `T` | Jump to today |
| `N` or `Ctrl/Cmd+N` | Focus the quick-add bar |
| `↑` `↓` | Move the selection in Today |
| `Space` | Mark the selected task done or not done |
| `Enter` | Open the selected task for editing |
| `Ctrl/Cmd+,` | Settings |
| `Ctrl/Cmd+Z` | Undo the last change |

## Build and run

You need Rust from [rustup](https://rustup.rs). `rust-toolchain.toml` pins the compiler version, and rustup installs it on first build.

| Platform | Also needs |
|---|---|
| macOS | The Xcode command-line tools: `xcode-select --install` |
| Windows | The MSVC build tools. The C runtime is linked statically, so the built program needs nothing installed. |
| Linux | A C compiler. SQLite is built from bundled source. The program loads the window system and graphics libraries at run time, and every desktop install has them. |

```
cargo build --release --locked
./target/release/Fortnight        # the app, Fortnight.exe on Windows
./target/release/fortnight-cli    # the command-line tool
```

`cargo run --release -p fortnight-app` builds and starts the app in one step.

### Fortnight.app on macOS

```
cargo xtask bundle              # for this Mac's architecture
cargo xtask bundle --universal  # for Apple Silicon and Intel
```

This builds the app and wraps it in `target/bundle/Fortnight.app`, so it starts without a terminal window. It replaces any existing bundle. `--universal` needs both Rust targets, which `rustup target add aarch64-apple-darwin x86_64-apple-darwin` installs. The icon is made from `assets/icon.svg` with tools that come with macOS. If that fails, the bundle has no icon. You can move `Fortnight.app` anywhere; keep the data folder beside it.

## Where your data lives

Everything is in one folder, `fortnight-data`, beside the program:

```
anywhere/
├── Fortnight            or Fortnight.exe, or Fortnight.app
└── fortnight-data/
    ├── fortnight.db     tasks, history, reports and settings, in a standard SQLite file
    ├── llm-key          your API key, encrypted, if you set one
    └── fortnight.lock   present while the app has the folder open
```

Fortnight picks the folder in this order:

1. `--data-dir <path>`.
2. The `FORTNIGHT_DATA_DIR` environment variable, if it isn't empty.
3. `fortnight-data` beside the program. For `Fortnight.app`, the folder goes beside the `.app`, because writing inside a bundle would break its signature.

Fortnight writes nothing outside this folder. To move your data, copy the folder. A folder from an earlier version opens with nothing lost; Fortnight upgrades the database in place the first time.

The app creates `fortnight.lock` while it runs and removes it on exit. If the folder is in a synced directory such as Dropbox or iCloud, open it on one machine at a time. The app warns if it finds a lock from another machine. The CLI never takes the lock, so it can run while the app is open.

`llm-key` holds your API key, sealed with XChaCha20-Poly1305 under a key that Argon2id derives from your password. Someone who copies the data folder can't read the key. While the app is unlocked the key is in memory, where other software running as you could read it. `fortnight.db` is not encrypted. A forgotten password can't be recovered, so remove the key in Settings and enter it again. If your endpoint needs no key, as a local Ollama doesn't, Fortnight never creates the file and never asks for a password.

Fortnight sends the key only to the endpoint you configured. It refuses to send it over plain `http` to another machine, and it doesn't follow redirects.

## First run

1. Start the app and open Settings with the gear or `Cmd+,`.
2. Choose the protocol and enter the base URL and model. Load models lists what the endpoint offers.
3. If the endpoint needs one, enter the API key and press Save key, then choose a password. The password encrypts the key on disk; unlock it with the lock icon.
4. Press Test connection.

## Environment variables

| Variable | Meaning |
|---|---|
| `FORTNIGHT_DATA_DIR` | The data folder. `--data-dir` takes precedence. |
| `FORTNIGHT_LLM_PROTOCOL` | `openai` or `anthropic`. |
| `FORTNIGHT_LLM_BASE_URL` | The endpoint, such as `http://localhost:11434/v1`. |
| `FORTNIGHT_LLM_MODEL` | The model name. |
| `FORTNIGHT_LLM_API_KEY` | A key for this process only. Fortnight never writes it to disk. |

The `FORTNIGHT_LLM_*` variables apply to the CLI. Its flags take precedence over them, and they take precedence over the saved settings. The app uses its saved settings only.

The app also reads these for development: `FORTNIGHT_PAGE=today|week|month|settings` opens a tab, `FORTNIGHT_THEME=light|dark` overrides the system theme, and `FORTNIGHT_SCREENSHOT=<file>` saves a screenshot of the window as raw RGBA after two seconds, then exits.

## Command line

`fortnight-cli` works on the same data folder without a window. `--data-dir <path>` can go anywhere on the line.

```
fortnight-cli add <words...>                    # quick-add syntax: fri, nov 20, +3d, #project, ~2h, !
fortnight-cli list [--all] [--today] [--week]   # open tasks; --all adds done and dropped ones
fortnight-cli done <id>... [--undo]             # mark tasks done, or reopen them
fortnight-cli log [--task <id>] <words...>      # add a log line, on a task or on its own
fortnight-cli summary [--week this|last|YYYY-MM-DD] [--facts]   # the weekly summary, streamed
fortnight-cli plan [--week this|last|YYYY-MM-DD] [--facts]      # recommended tasks
fortnight-cli run-due                           # generate whatever is due, then exit
fortnight-cli export [--format md|json] [--out <path>]           # everything, to stdout or a file
```

- `summary` covers the latest week whose summary is due, and `plan` covers this week, unless you pass `--week`.
- `--facts` prints the facts and never calls the LLM.
- `summary`, `plan` and `run-due` take `--protocol openai|anthropic`, `--base-url`, `--model` and `--password-stdin`.
- For the key, the CLI uses `FORTNIGHT_LLM_API_KEY` if it is set. Otherwise, if a key file exists, it reads the password from `--password-stdin` or asks for it.
- The CLI stores each report it generates, so the app shows it too.
- Exit code 0 means success. 1 means a report failed, a task wasn't found, or a file couldn't be read or written. 2 means bad arguments, or no endpoint configured for `summary` or `plan`. `run-due` with no endpoint generates nothing and exits 0.

## Development

```
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

## Not included

- Registering with the OS scheduler, such as launchd, systemd or Task Scheduler. It would be the only thing Fortnight writes outside its folder, and a scheduled job can't type the password for an encrypted key. You can run `fortnight-cli run-due` from your own scheduler against an endpoint that needs no key. Otherwise reports are made the next time you open and unlock the app.
- Code signing. A build you make yourself isn't flagged, but a downloaded archive shows a one-time Gatekeeper or SmartScreen warning on each machine.
- Notifications (parked).
