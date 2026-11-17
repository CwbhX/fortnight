//! Choosing the endpoint and the API key (PLAN.md 3.3 and 7.1).
//!
//! Endpoint precedence: flags, then `FORTNIGHT_LLM_*` environment variables,
//! then the saved settings. Overriding the base URL (by flag or
//! environment) assumes the new endpoint needs a key unless `--keyless` or
//! `FORTNIGHT_LLM_KEYLESS` says otherwise. Key precedence:
//! `FORTNIGHT_LLM_API_KEY`, then the key file (password from
//! `--password-stdin` or a terminal prompt), otherwise no key. The key is
//! never printed, logged or saved.
//!
//! The saved decision model (`Settings::decision_llm`, recommended tasks
//! only) has its own optional key: `FORTNIGHT_DECISION_API_KEY`, then
//! `llm-key-decision`, opened with the same password in the same prompt.
//! Without one it borrows the main key on the main endpoint's host. Only a
//! run that makes recommendations opens it.

use crate::args::LlmArgs;
use crate::error::{CliError, CliResult};
use fortnight_core::domain::{LlmSettings, Protocol};
use fortnight_core::fallback::{self, Endpoint, PlanRoute};
use fortnight_core::llm::{self, LlmConfig, LlmError};
use fortnight_core::secure;
use fortnight_core::vault::{self, VaultError};
use secrecy::SecretString;
use std::io::BufRead;
use std::path::{Path, PathBuf};
use zeroize::Zeroizing;

/// The main endpoint, and the decision model for recommended tasks if one
/// is saved (`Settings::decision_llm`) and this run makes recommendations.
#[derive(Clone)]
pub struct LlmHandle {
    pub main: Endpoint,
    pub decision: Option<Endpoint>,
}

impl LlmHandle {
    /// Where a request of this kind goes: recommendations to the decision
    /// model (falling back to the main endpoint), everything else to the
    /// main endpoint.
    pub fn route(&self, kind: fortnight_core::domain::ReportKind) -> PlanRoute {
        let decision = match kind {
            fortnight_core::domain::ReportKind::Plan => self.decision.clone(),
            fortnight_core::domain::ReportKind::Summary => None,
        };
        PlanRoute::new(self.main.clone(), decision)
    }
}

const NOT_CONFIGURED: &str = "no LLM endpoint is configured. Set it in the app's Settings, with \
     FORTNIGHT_LLM_BASE_URL and FORTNIGHT_LLM_MODEL, or with --base-url and --model.";

/// Whether an environment flag's value means yes.
fn truthy(v: &str) -> bool {
    matches!(
        v.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

/// Saved settings, then the environment (`var`), then the flags. A base
/// URL from the environment or a flag is a different endpoint, so the
/// saved `keyless` doesn't carry over; `--keyless` or
/// `FORTNIGHT_LLM_KEYLESS` marks the result keyless. A protocol other than
/// `openai` or `anthropic` in `FORTNIGHT_LLM_PROTOCOL` is a usage error.
pub fn effective_settings(
    saved: &LlmSettings,
    var: impl Fn(&str) -> Option<String>,
    flags: &LlmArgs,
) -> CliResult<LlmSettings> {
    let get = |name: &str| var(name).filter(|v| !v.trim().is_empty());
    if let Some(p) = get("FORTNIGHT_LLM_PROTOCOL") {
        crate::args::parse_protocol(&p)
            .map_err(|e| CliError::usage(format!("FORTNIGHT_LLM_PROTOCOL: {e}")))?;
    }
    let mut out = llm::with_overrides(saved, &var);
    let flag_url = flags.base_url.as_deref().filter(|v| !v.trim().is_empty());
    if get("FORTNIGHT_LLM_BASE_URL").is_some() || flag_url.is_some() {
        out.keyless = false;
    }
    if let Some(p) = flags.protocol {
        out.protocol = p;
    }
    if let Some(url) = flag_url {
        out.base_url = url.trim().to_string();
    }
    if let Some(model) = flags.model.as_deref().filter(|v| !v.trim().is_empty()) {
        out.model = model.trim().to_string();
    }
    if flags.keyless || get("FORTNIGHT_LLM_KEYLESS").is_some_and(|v| truthy(&v)) {
        out.keyless = true;
    }
    Ok(out)
}

/// Which saved keys a run needs: a local endpoint needs none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyNeeds {
    pub main: bool,
    pub decision: bool,
}

/// The main key and the decision model's key for this run. An environment
/// key (`env` = `FORTNIGHT_LLM_API_KEY`, `FORTNIGHT_DECISION_API_KEY`) wins;
/// otherwise each needed key file that exists is opened with one password,
/// asked for once: one line of `stdin` when `password_stdin` is set,
/// otherwise whatever `prompt` returns (the terminal prompt in the real
/// program). Both files share that password (the app keeps them so). It
/// fails if the main key file doesn't open, or if only the decision key
/// file was needed and it doesn't open. A decision key file that doesn't
/// open beside a main one that does is left out.
pub fn resolve_keys(
    data_dir: &Path,
    needs: KeyNeeds,
    env: (Option<SecretString>, Option<SecretString>),
    password_stdin: bool,
    stdin: &mut dyn BufRead,
    prompt: &mut dyn FnMut() -> std::io::Result<String>,
) -> CliResult<(Option<SecretString>, Option<SecretString>)> {
    let (env_main, env_decision) = env;
    let file = |needed: bool, env: &Option<SecretString>, path: PathBuf| {
        (needed && env.is_none() && path.exists()).then_some(path)
    };
    let main_path = file(needs.main, &env_main, vault::key_path(data_dir));
    let decision_path = file(
        needs.decision,
        &env_decision,
        vault::decision_key_path(data_dir),
    );
    if main_path.is_none() && decision_path.is_none() {
        return Ok((env_main, env_decision));
    }
    let password = read_password(password_stdin, stdin, prompt)?;
    let open = |p: Option<PathBuf>| p.map(|p| vault::unlock(&p, &password)).transpose();
    let unlock_error = |e: VaultError| match e {
        VaultError::WrongPassword => CliError::failure("wrong password"),
        e => CliError::failure(e.to_string()),
    };
    match (open(main_path), open(decision_path)) {
        (Err(e), _) | (Ok(None), Err(e)) => Err(unlock_error(e)),
        (Ok(main), decision) => Ok((env_main.or(main), env_decision.or(decision.ok().flatten()))),
    }
}

/// One line of `stdin` with `password_stdin`, otherwise `prompt`'s answer.
/// The plain-text buffers are wiped when dropped.
fn read_password(
    password_stdin: bool,
    stdin: &mut dyn BufRead,
    prompt: &mut dyn FnMut() -> std::io::Result<String>,
) -> CliResult<SecretString> {
    let line = if password_stdin {
        // Room up front, so reading rarely regrows (and leaves a copy).
        let mut line = Zeroizing::new(String::with_capacity(256));
        stdin.read_line(&mut line)?;
        // Only the line ending goes; a password may end in a space.
        while line.ends_with(['\n', '\r']) {
            line.pop();
        }
        line
    } else {
        Zeroizing::new(prompt().map_err(|e| {
            CliError::failure(format!(
                "cannot read the key password: {e}. Use --password-stdin or set FORTNIGHT_LLM_API_KEY."
            ))
        })?)
    };
    Ok(SecretString::from(line.as_str()))
}

/// The key in environment variable `name`, trimmed, if set and not blank.
/// For this process only, never persisted. The plain copy is wiped.
fn env_key(name: &str) -> Option<SecretString> {
    let value = Zeroizing::new(std::env::var(name).ok()?);
    let key = value.trim();
    (!key.is_empty()).then(|| SecretString::from(key))
}

/// `connect`'s error as a CLI error: "not configured" is a usage error
/// (exit 2); anything else (such as refusing to send a key over plain
/// http) is a failure with its own message.
fn connect_error(e: LlmError) -> CliError {
    match e {
        LlmError::NotConfigured => CliError::not_configured(NOT_CONFIGURED),
        e => CliError::failure(e.to_string()),
    }
}

/// The client for these settings and this key (dropped for a keyless
/// endpoint). Not configured is a usage error (exit 2), any other problem a
/// failure (exit 1).
pub fn build_client(llm: &LlmSettings, api_key: Option<SecretString>) -> CliResult<LlmHandle> {
    let config = LlmConfig::from_settings(llm, api_key);
    let host = config.endpoint_host();
    let client = llm::connect(llm.protocol, config).map_err(connect_error)?;
    Ok(LlmHandle {
        main: Endpoint { client, host },
        decision: None,
    })
}

/// Adds the saved decision model to `handle`, unless it isn't configured
/// or can't be built. `main` is the endpoint `handle` talks to. The
/// decision model uses `decision_key` if given, otherwise `main_key` only on
/// that host.
pub fn with_decision(
    mut handle: LlmHandle,
    decision: &LlmSettings,
    main: &LlmSettings,
    main_key: Option<SecretString>,
    decision_key: Option<SecretString>,
) -> LlmHandle {
    if decision.is_configured() {
        let config = fallback::decision_config(decision, main, main_key, decision_key);
        let host = config.endpoint_host();
        handle.decision = llm::connect(decision.protocol, config)
            .ok()
            .map(|client| Endpoint { client, host });
    }
    handle
}

/// What the process gives `connect_with`: the environment and the ways to
/// ask for the key password.
pub struct Process<'a> {
    /// Reads an environment variable.
    pub var: &'a dyn Fn(&str) -> Option<String>,
    /// `FORTNIGHT_LLM_API_KEY` and `FORTNIGHT_DECISION_API_KEY`.
    pub keys: (Option<SecretString>, Option<SecretString>),
    pub stdin: &'a mut dyn BufRead,
    pub prompt: &'a mut dyn FnMut() -> std::io::Result<String>,
}

/// Everything from settings to client. Checks the endpoint is configured
/// before asking for a password. The saved decision model is used only when
/// `uses_decision` (the run makes recommendations), and not when the flags
/// name an endpoint (then that endpoint does everything); otherwise its key
/// file isn't opened.
pub fn connect_with(
    saved: &LlmSettings,
    decision: &LlmSettings,
    flags: &LlmArgs,
    data_dir: &Path,
    uses_decision: bool,
    process: Process<'_>,
) -> CliResult<LlmHandle> {
    let flags_name_endpoint =
        flags.protocol.is_some() || flags.base_url.is_some() || flags.model.is_some();
    let decision = if uses_decision && !flags_name_endpoint {
        decision.clone()
    } else {
        LlmSettings::default()
    };
    let llm = effective_settings(saved, process.var, flags)?;
    if llm.protocol == Protocol::OllamaDecision {
        return Err(CliError::usage(
            "the main endpoint's protocol must be openai or anthropic",
        ));
    }
    if !llm.is_configured() {
        return Err(CliError::not_configured(NOT_CONFIGURED));
    }
    // A keyless or local endpoint (Ollama, LM Studio) needs no key: its
    // file isn't opened, so there is no password prompt. The environment
    // keys still apply to a local one, for a local proxy.
    let needs = KeyNeeds {
        main: secure::needs_saved_key(&llm),
        decision: decision.is_configured() && secure::needs_saved_key(&decision),
    };
    let (main_key, decision_key) = resolve_keys(
        data_dir,
        needs,
        process.keys,
        flags.password_stdin,
        process.stdin,
        process.prompt,
    )?;
    let handle = build_client(&llm, main_key.clone())?;
    Ok(with_decision(
        handle,
        &decision,
        &llm,
        main_key,
        decision_key,
    ))
}

/// `connect_with` for the real program: the process environment, standard
/// input and a terminal prompt.
pub fn connect_from_settings(
    saved: &LlmSettings,
    decision: &LlmSettings,
    flags: &LlmArgs,
    data_dir: &Path,
    uses_decision: bool,
) -> CliResult<LlmHandle> {
    let stdin = std::io::stdin();
    connect_with(
        saved,
        decision,
        flags,
        data_dir,
        uses_decision,
        Process {
            var: &|n| std::env::var(n).ok(),
            keys: (
                env_key("FORTNIGHT_LLM_API_KEY"),
                env_key("FORTNIGHT_DECISION_API_KEY"),
            ),
            stdin: &mut stdin.lock(),
            prompt: &mut || rpassword::prompt_password("API key password: "),
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use fortnight_core::domain::Protocol;
    use secrecy::ExposeSecret;
    use std::path::PathBuf;

    fn temp_dir(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "fortnight-cli-{tag}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn secret(s: &str) -> SecretString {
        SecretString::from(s.to_string())
    }

    fn no_prompt() -> impl FnMut() -> std::io::Result<String> {
        || panic!("must not prompt")
    }

    /// `resolve_keys` for the main key only.
    fn resolve_key(
        data_dir: &Path,
        env_key: Option<SecretString>,
        password_stdin: bool,
        stdin: &mut dyn BufRead,
        prompt: &mut dyn FnMut() -> std::io::Result<String>,
    ) -> CliResult<Option<SecretString>> {
        let needs = KeyNeeds {
            main: true,
            decision: false,
        };
        let (key, _) = resolve_keys(
            data_dir,
            needs,
            (env_key, None),
            password_stdin,
            stdin,
            prompt,
        )?;
        Ok(key)
    }

    #[test]
    fn flags_beat_env_beat_saved() {
        let saved = LlmSettings {
            protocol: Protocol::OpenAi,
            base_url: "http://saved/v1".into(),
            model: "saved".into(),
            keyless: false,
        };
        let env = |name: &str| match name {
            "FORTNIGHT_LLM_BASE_URL" => Some("http://env/v1".to_string()),
            "FORTNIGHT_LLM_MODEL" => Some("env-model".to_string()),
            _ => None,
        };
        let none = LlmArgs::default();
        let got = effective_settings(&saved, env, &none).unwrap();
        assert_eq!(got.base_url, "http://env/v1");
        assert_eq!(got.model, "env-model");
        assert_eq!(got.protocol, Protocol::OpenAi);

        let flags = LlmArgs {
            protocol: Some(Protocol::Anthropic),
            model: Some("flag-model".into()),
            ..LlmArgs::default()
        };
        let got = effective_settings(&saved, env, &flags).unwrap();
        assert_eq!(got.protocol, Protocol::Anthropic);
        assert_eq!(got.base_url, "http://env/v1");
        assert_eq!(got.model, "flag-model");
    }

    #[test]
    fn env_key_beats_the_key_file_and_never_prompts() {
        let dir = temp_dir("envkey");
        std::fs::write(vault::key_path(&dir), b"not even a real key file").unwrap();
        let got = resolve_key(
            &dir,
            Some(secret("sk-env")),
            true,
            &mut std::io::Cursor::new(b"ignored\n".to_vec()),
            &mut no_prompt(),
        )
        .unwrap();
        assert_eq!(got.unwrap().expose_secret(), "sk-env");
    }

    #[test]
    fn no_key_file_means_no_key_and_no_prompt() {
        let dir = temp_dir("nokey");
        let got = resolve_key(
            &dir,
            None,
            false,
            &mut std::io::Cursor::new(Vec::new()),
            &mut no_prompt(),
        )
        .unwrap();
        assert!(got.is_none());
    }

    #[test]
    fn password_stdin_unlocks_the_key_file() {
        let dir = temp_dir("stdin");
        vault::save(
            &vault::key_path(&dir),
            &secret("sk-from-file"),
            &secret("correct horse"),
        )
        .unwrap();
        let got = resolve_key(
            &dir,
            None,
            true,
            &mut std::io::Cursor::new(b"correct horse\r\nsecond line\n".to_vec()),
            &mut no_prompt(),
        )
        .unwrap();
        assert_eq!(got.unwrap().expose_secret(), "sk-from-file");

        let err = resolve_key(
            &dir,
            None,
            true,
            &mut std::io::Cursor::new(b"wrong password\n".to_vec()),
            &mut no_prompt(),
        )
        .unwrap_err();
        assert_eq!(err.code, 1);
        assert!(err.message.contains("wrong password"));
    }

    #[test]
    fn prompt_is_used_without_password_stdin() {
        let dir = temp_dir("prompt");
        vault::save(
            &vault::key_path(&dir),
            &secret("sk-p"),
            &secret("hunter2hunter2"),
        )
        .unwrap();
        let mut asked = 0;
        let got = resolve_key(
            &dir,
            None,
            false,
            &mut std::io::Cursor::new(Vec::new()),
            &mut || {
                asked += 1;
                Ok("hunter2hunter2".to_string())
            },
        )
        .unwrap();
        assert_eq!(got.unwrap().expose_secret(), "sk-p");
        assert_eq!(asked, 1);

        let err = resolve_key(
            &dir,
            None,
            false,
            &mut std::io::Cursor::new(Vec::new()),
            &mut || Err(std::io::Error::other("no tty")),
        )
        .unwrap_err();
        assert!(err.message.contains("--password-stdin"));
    }

    #[test]
    fn one_password_opens_both_key_files() {
        let dir = temp_dir("both-keys");
        let pw = secret("correct horse");
        vault::save(&vault::key_path(&dir), &secret("sk-main"), &pw).unwrap();
        vault::save(&vault::decision_key_path(&dir), &secret("sk-dec"), &pw).unwrap();
        let both = KeyNeeds {
            main: true,
            decision: true,
        };
        let mut prompts = 0;
        let (main, decision) =
            resolve_keys(&dir, both, (None, None), false, &mut &b""[..], &mut || {
                prompts += 1;
                Ok("correct horse".into())
            })
            .unwrap();
        assert_eq!(prompts, 1);
        assert_eq!(main.unwrap().expose_secret(), "sk-main");
        assert_eq!(decision.unwrap().expose_secret(), "sk-dec");

        // A local main endpoint: only the decision key is opened.
        let decision_only = KeyNeeds {
            main: false,
            decision: true,
        };
        let (main, decision) = resolve_keys(
            &dir,
            decision_only,
            (None, None),
            true,
            &mut &b"correct horse\n"[..],
            &mut no_prompt(),
        )
        .unwrap();
        assert!(main.is_none());
        assert_eq!(decision.unwrap().expose_secret(), "sk-dec");

        // Environment keys win and need no password.
        let (main, decision) = resolve_keys(
            &dir,
            both,
            (Some(secret("env-main")), Some(secret("env-dec"))),
            false,
            &mut &b""[..],
            &mut no_prompt(),
        )
        .unwrap();
        assert_eq!(main.unwrap().expose_secret(), "env-main");
        assert_eq!(decision.unwrap().expose_secret(), "env-dec");

        let err = resolve_keys(
            &dir,
            both,
            (None, None),
            true,
            &mut &b"wrong password\n"[..],
            &mut no_prompt(),
        )
        .unwrap_err();
        assert!(err.to_string().contains("wrong password"), "{err}");
    }

    #[test]
    fn keyless_endpoint_never_gets_the_key() {
        let lan = |keyless| LlmSettings {
            protocol: Protocol::OpenAi,
            base_url: "http://192.168.1.20:11434/v1".into(),
            model: "llama3.2".into(),
            keyless,
        };
        // With the key, plain http to another machine is refused...
        assert!(build_client(&lan(false), Some(secret("sk-1"))).is_err());
        // ...but a keyless endpoint drops the key and connects.
        assert!(build_client(&lan(true), Some(secret("sk-1"))).is_ok());
    }

    #[test]
    fn unconfigured_endpoint_is_a_usage_error() {
        let err = build_client(&LlmSettings::default(), None).err().unwrap();
        assert_eq!(err.code, 2);
        let ok = build_client(
            &LlmSettings {
                protocol: Protocol::OpenAi,
                base_url: "http://user:pw@localhost:11434/v1".into(),
                model: "m".into(),
                keyless: false,
            },
            None,
        )
        .ok()
        .unwrap();
        assert_eq!(ok.main.host.as_deref(), Some("localhost:11434"));
    }

    #[test]
    fn connect_from_settings_checks_config_before_prompting() {
        // No endpoint anywhere (flags empty; the env of the test process is
        // not set for these) -> exit 2 without touching stdin or the key.
        let saved = LlmSettings::default();
        if std::env::var("FORTNIGHT_LLM_BASE_URL").is_ok() {
            return; // a developer's own environment; nothing to check
        }
        let dir = temp_dir("cfg");
        let err = connect_from_settings(
            &saved,
            &LlmSettings::default(),
            &LlmArgs::default(),
            &dir,
            false,
        )
        .err()
        .unwrap();
        assert_eq!(err.code, 2);
        assert_eq!(err.kind, crate::error::ErrorKind::NotConfigured);
    }

    fn settings(base_url: &str, keyless: bool) -> LlmSettings {
        LlmSettings {
            protocol: Protocol::OpenAi,
            base_url: base_url.into(),
            model: "m".into(),
            keyless,
        }
    }

    /// `connect_with` with no environment and no environment keys; the
    /// password, if asked for, comes from `prompt`.
    fn connect(
        saved: &LlmSettings,
        decision: &LlmSettings,
        flags: &LlmArgs,
        dir: &Path,
        uses_decision: bool,
        prompt: &mut dyn FnMut() -> std::io::Result<String>,
    ) -> CliResult<LlmHandle> {
        connect_with(
            saved,
            decision,
            flags,
            dir,
            uses_decision,
            Process {
                var: &|_| None,
                keys: (None, None),
                stdin: &mut &b""[..],
                prompt,
            },
        )
    }

    #[test]
    fn summary_never_opens_the_decision_key() {
        // Main = keyless local endpoint; decision = remote with a key file.
        let dir = temp_dir("summary-no-decision");
        vault::save(
            &vault::decision_key_path(&dir),
            &secret("sk-dec"),
            &secret("pw-pw-pw-pw"),
        )
        .unwrap();
        let main = settings("http://localhost:11434/v1", true);
        let decision = settings("https://api.example.com/v1", false);
        let flags = LlmArgs::default();
        // The summary asks for nothing and has no decision model.
        let h = connect(&main, &decision, &flags, &dir, false, &mut no_prompt()).unwrap();
        assert!(h.decision.is_none());
        // Recommendations open the decision key (one prompt).
        let mut asked = 0;
        let h = connect(&main, &decision, &flags, &dir, true, &mut || {
            asked += 1;
            Ok("pw-pw-pw-pw".into())
        })
        .unwrap();
        assert_eq!(asked, 1);
        assert_eq!(h.decision.unwrap().host.as_deref(), Some("api.example.com"));
    }

    #[test]
    fn a_main_key_file_that_does_not_open_is_fatal() {
        let dir = temp_dir("main-fails");
        // The main file is corrupt; the decision file opens.
        std::fs::write(vault::key_path(&dir), b"not a key file").unwrap();
        let pw = secret("correct horse");
        vault::save(&vault::decision_key_path(&dir), &secret("sk-dec"), &pw).unwrap();
        let both = KeyNeeds {
            main: true,
            decision: true,
        };
        let err = resolve_keys(
            &dir,
            both,
            (None, None),
            true,
            &mut &b"correct horse\n"[..],
            &mut no_prompt(),
        )
        .unwrap_err();
        assert_eq!(err.code, 1);

        // A decision file that doesn't open beside a good main one is left
        // out (the decision model then borrows the main key on its host).
        let dir = temp_dir("decision-fails");
        vault::save(&vault::key_path(&dir), &secret("sk-main"), &pw).unwrap();
        std::fs::write(vault::decision_key_path(&dir), b"not a key file").unwrap();
        let (main, decision) = resolve_keys(
            &dir,
            both,
            (None, None),
            true,
            &mut &b"correct horse\n"[..],
            &mut no_prompt(),
        )
        .unwrap();
        assert_eq!(main.unwrap().expose_secret(), "sk-main");
        assert!(decision.is_none());
    }

    #[test]
    fn overriding_the_base_url_drops_the_saved_keyless() {
        // Saved: keyless Ollama. Override to a remote endpoint by flag or
        // by environment: it needs a key again.
        let saved = settings("http://localhost:11434/v1", true);
        let flags = LlmArgs {
            base_url: Some("https://api.openai.com/v1".into()),
            ..LlmArgs::default()
        };
        let got = effective_settings(&saved, |_| None, &flags).unwrap();
        assert!(!got.keyless);
        let env = |n: &str| {
            (n == "FORTNIGHT_LLM_BASE_URL").then(|| "https://api.openai.com/v1".to_string())
        };
        let got = effective_settings(&saved, env, &LlmArgs::default()).unwrap();
        assert!(!got.keyless);
        // Only the model changed: still the same keyless endpoint.
        let model_only = LlmArgs {
            model: Some("other".into()),
            ..LlmArgs::default()
        };
        assert!(
            effective_settings(&saved, |_| None, &model_only)
                .unwrap()
                .keyless
        );
        // The key now reaches the overriding endpoint.
        let h = build_client(&got, Some(secret("sk-1"))).unwrap();
        assert_eq!(h.main.host.as_deref(), Some("api.openai.com"));
    }

    #[test]
    fn keyless_flag_and_env_mark_the_override_keyless() {
        // Saved: remote with a key file. Override to a LAN box with
        // --keyless: no prompt, and it connects over plain http.
        let dir = temp_dir("keyless-flag");
        vault::save(
            &vault::key_path(&dir),
            &secret("sk"),
            &secret("pw-pw-pw-pw"),
        )
        .unwrap();
        let saved = settings("https://api.example.com/v1", false);
        let lan = LlmArgs {
            base_url: Some("http://lan-box:11434/v1".into()),
            keyless: true,
            ..LlmArgs::default()
        };
        let none = LlmSettings::default();
        assert!(connect(&saved, &none, &lan, &dir, false, &mut no_prompt()).is_ok());

        let env = |n: &str| match n {
            "FORTNIGHT_LLM_BASE_URL" => Some("http://lan-box:11434/v1".to_string()),
            "FORTNIGHT_LLM_KEYLESS" => Some("1".to_string()),
            _ => None,
        };
        let got = effective_settings(&saved, env, &LlmArgs::default()).unwrap();
        assert!(got.keyless);
        let env_off = |n: &str| match n {
            "FORTNIGHT_LLM_KEYLESS" => Some("0".to_string()),
            _ => None,
        };
        let got = effective_settings(&saved, env_off, &LlmArgs::default()).unwrap();
        assert!(!got.keyless);
    }

    #[test]
    fn connect_errors_other_than_not_configured_are_failures() {
        // A key over plain http to another machine is refused: exit 1 with
        // the real reason, not "not configured".
        let dir = temp_dir("http-key");
        let saved = settings("http://10.0.0.5:8000/v1", false);
        let err = connect_with(
            &saved,
            &LlmSettings::default(),
            &LlmArgs::default(),
            &dir,
            false,
            Process {
                var: &|_| None,
                keys: (Some(secret("x")), None),
                stdin: &mut &b""[..],
                prompt: &mut no_prompt(),
            },
        )
        .err()
        .unwrap();
        assert_eq!(err.code, 1);
        assert_eq!(err.kind, crate::error::ErrorKind::Other);
        assert!(err.message.contains("plain http"), "{}", err.message);
    }

    #[test]
    fn the_decision_protocol_is_refused_for_the_main_endpoint() {
        let saved = settings("http://localhost:11434/v1", false);
        let env = |n: &str| (n == "FORTNIGHT_LLM_PROTOCOL").then(|| "decision".to_string());
        let err = effective_settings(&saved, env, &LlmArgs::default()).unwrap_err();
        assert_eq!(err.code, 2);
        assert!(
            err.message.contains("openai or anthropic"),
            "{}",
            err.message
        );
        let env = |n: &str| (n == "FORTNIGHT_LLM_PROTOCOL").then(|| "Anthropic".to_string());
        let got = effective_settings(&saved, env, &LlmArgs::default()).unwrap();
        assert_eq!(got.protocol, Protocol::Anthropic);
    }

    #[test]
    fn password_stdin_keeps_trailing_spaces() {
        let got = read_password(true, &mut &b"pass word \r\n"[..], &mut no_prompt()).unwrap();
        assert_eq!(got.expose_secret(), "pass word ");
    }
}
