<img src="assets/icon.svg" width="64" height="64" alt="">

# fortnight

Fortnight is a small, portable todo app for macOS (prototype). It keeps your tasks in a local SQLite file and, once a week, uses an LLM endpoint that you supply to write a summary of what you got done and some advice for the week ahead. Nothing leaves your machine except the requests to that endpoint.

Created in [T3 Code](https://t3.codes).

## What the prototype does

- **Tasks**: add, complete, edit and delete, with due date, project and notes. The quick-add bar understands `fri`, `tomorrow`, `nov 20`, `+3d` and `#project`.
- **Weekly summary**: generated from the week's tasks, editable, copyable.
- **Week-ahead advice**: at-risk deadlines and a suggested order for the coming week.
- **Automatic generation**: the summary is made Friday 16:00 and the advice Monday 08:00. Fortnight checks at launch and every minute, and catches up if it was closed at the time.
- **Encrypted API key**: stored encrypted on disk, unlocked with a password when you start the app and held in memory only while it is open.
- **Your endpoint**: any OpenAI-compatible API, for example `https://api.openai.com/v1` or Ollama at `http://localhost:11434/v1`.

## Build and run (macOS)

You need Rust via [`rustup`](https://rustup.rs) (the exact toolchain is pinned by `rust-toolchain.toml` and installed automatically) and the Xcode command-line tools (`xcode-select --install`).

```
cargo build --release --locked
./target/release/fortnight
```

or build and run in one step:

```
cargo run --release -p fortnight-app
```

## Where your data lives

Everything is in one folder, `fortnight-data/`, beside the executable:

```
fortnight-data/
├── fortnight.db   tasks, reports and settings (a standard SQLite file)
└── llm-key        your API key, encrypted
```

The folder is chosen in this order:

1. `--data-dir <path>`
2. the `FORTNIGHT_DATA_DIR` environment variable (ignored if empty)
3. `fortnight-data/` beside the executable. If the executable is inside `Fortnight.app`, the folder goes beside the `.app`, not inside it.

Fortnight never falls back to a per-user directory. To move your data, copy the folder.

## First run

1. Start the app and open **Settings**.
2. Enter the base URL (for example `https://api.openai.com/v1`), the model name, your API key and a password. The password encrypts the key on disk and unlocks it at each launch.
3. Press **Test** to check the connection.

## Development

```
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

## Not yet included

- The Anthropic API format and streaming output (a spinner is shown instead).
- A command-line interface, registering with the OS scheduler, and notifications.
- Change history, estimates and capacity maths, log lines, the private flag.
- Undo, export to file, and report versions.
- Windows and Linux builds, CI and release archives. The code is meant to stay cross-platform, but it is not built or tested there yet.
