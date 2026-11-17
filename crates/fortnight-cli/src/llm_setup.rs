//! Choosing the endpoint and the API key (PLAN.md 3.3 and 7.1).
//!
//! Endpoint precedence: flags, then `FORTNIGHT_LLM_*` environment variables,
//! then the saved settings. Key precedence: `FORTNIGHT_LLM_API_KEY`, then
//! the key file (password from `--password-stdin` or a terminal prompt),
//! otherwise no key. The key is never printed, logged or saved.
//!
//! The saved decision model (`Settings::decision_llm`, recommended tasks
//! only) has its own optional key: `FORTNIGHT_DECISION_API_KEY`, then
//! `llm-key-decision`, opened with the same password in the same prompt.
//! Without one it borrows the main key on the main endpoint's host.

use crate::args::LlmArgs;
use crate::error::{CliError, CliResult};
use fortnight_core::domain::LlmSettings;
use fortnight_core::fallback;
use fortnight_core::llm::{self, LlmClient, LlmConfig};
use fortnight_core::secure;
use fortnight_core::vault::{self, VaultError};
use secrecy::SecretString;
use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// A client and where it points (for `Report::endpoint_host`).
#[derive(Clone)]
pub struct LlmHandle {
    pub client: Arc<dyn LlmClient>,
    pub endpoint_host: Option<String>,
    /// The decision model for recommended tasks, and its host, if one is
    /// saved (`Settings::decision_llm`). `client` is its fallback.
    pub decision: Option<(Arc<dyn LlmClient>, Option<String>)>,
}

/// Saved settings, then the environment (`var`), then the flags.
pub fn effective_settings(
    saved: &LlmSettings,
    var: impl Fn(&str) -> Option<String>,
    flags: &LlmArgs,
) -> LlmSettings {
    let mut out = llm::with_overrides(saved, var);
    if let Some(p) = flags.protocol {
        out.protocol = p;
    }
    if let Some(url) = flags.base_url.as_deref().filter(|v| !v.trim().is_empty()) {
        out.base_url = url.trim().to_string();
    }
    if let Some(model) = flags.model.as_deref().filter(|v| !v.trim().is_empty()) {
        out.model = model.trim().to_string();
    }
    out
}

/// The API key for this run, if there is one.
///
/// `env_key` is `FORTNIGHT_LLM_API_KEY`. If the key file exists, the password
/// is one line of `stdin` when `password_stdin` is set, otherwise whatever
/// `prompt` returns (the terminal prompt in the real program).
pub fn resolve_key(
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

/// Which saved keys a run needs: a local endpoint needs none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyNeeds {
    pub main: bool,
    pub decision: bool,
}

/// The main key and the decision model's key for this run, like
/// `resolve_key`: an environment key (`env` = `FORTNIGHT_LLM_API_KEY`,
/// `FORTNIGHT_DECISION_API_KEY`) wins; otherwise each needed key file that
/// exists is opened with one password, asked for once. Both files share
/// that password (the app keeps them so). It fails only if the password
/// opens none of them.
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
    match (open(main_path), open(decision_path)) {
        (Err(e), Err(_) | Ok(None)) | (Ok(None), Err(e)) => Err(match e {
            VaultError::WrongPassword => CliError::failure("wrong password"),
            e => CliError::failure(e.to_string()),
        }),
        (main, decision) => Ok((
            env_main.or(main.ok().flatten()),
            env_decision.or(decision.ok().flatten()),
        )),
    }
}

/// One line of `stdin` with `password_stdin`, otherwise `prompt`'s answer.
fn read_password(
    password_stdin: bool,
    stdin: &mut dyn BufRead,
    prompt: &mut dyn FnMut() -> std::io::Result<String>,
) -> CliResult<SecretString> {
    if password_stdin {
        let mut line = String::new();
        stdin.read_line(&mut line)?;
        // Only the line ending goes; a password may end in a space.
        while line.ends_with(['\n', '\r']) {
            line.pop();
        }
        Ok(SecretString::from(line))
    } else {
        let line = prompt().map_err(|e| {
            CliError::failure(format!(
                "cannot read the key password: {e}. Use --password-stdin or set FORTNIGHT_LLM_API_KEY."
            ))
        })?;
        Ok(SecretString::from(line))
    }
}

/// `FORTNIGHT_DECISION_API_KEY`, if set and non-empty: the decision
/// model's key for this process only, never persisted. Takes precedence
/// over its key file.
pub fn decision_env_key() -> Option<SecretString> {
    std::env::var("FORTNIGHT_DECISION_API_KEY")
        .ok()
        .filter(|k| !k.trim().is_empty())
        .map(|k| SecretString::from(k.trim().to_string()))
}

/// The client for these settings and this key, or a configuration error
/// (exit 2) if no endpoint is set.
pub fn build_client(llm: &LlmSettings, api_key: Option<SecretString>) -> CliResult<LlmHandle> {
    let config = LlmConfig {
        base_url: llm.base_url.trim().to_string(),
        model: llm.model.trim().to_string(),
        api_key,
    };
    let endpoint_host = config.endpoint_host();
    let client = llm::connect(llm.protocol, config).map_err(|e| CliError::usage(e.to_string()))?;
    Ok(LlmHandle {
        client,
        endpoint_host,
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
            .map(|c| (c, host));
    }
    handle
}

/// Everything from settings to client, used by the real program. Checks the
/// endpoint is configured before asking for a password. The saved decision
/// model is added unless the flags name an endpoint (then that endpoint
/// does everything).
pub fn connect_from_settings(
    saved: &LlmSettings,
    decision: &LlmSettings,
    flags: &LlmArgs,
    data_dir: &Path,
) -> CliResult<LlmHandle> {
    let decision = if flags.protocol.is_none() && flags.base_url.is_none() && flags.model.is_none()
    {
        decision.clone()
    } else {
        LlmSettings::default()
    };
    let llm = effective_settings(saved, |n| std::env::var(n).ok(), flags);
    if !llm.is_configured() {
        return Err(CliError::usage(
            "no LLM endpoint is configured. Set it in the app's Settings, with \
             FORTNIGHT_LLM_BASE_URL and FORTNIGHT_LLM_MODEL, or with --base-url and --model.",
        ));
    }
    // A local endpoint (Ollama, LM Studio) needs no key: its file isn't
    // opened. The environment keys still apply, for a local proxy.
    let needs = KeyNeeds {
        main: !secure::is_local_endpoint(&llm.base_url),
        decision: decision.is_configured() && !secure::is_local_endpoint(&decision.base_url),
    };
    let stdin = std::io::stdin();
    let (main_key, decision_key) = resolve_keys(
        data_dir,
        needs,
        (llm::env_api_key(), decision_env_key()),
        flags.password_stdin,
        &mut stdin.lock(),
        &mut || rpassword::prompt_password("API key password: "),
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

    #[test]
    fn flags_beat_env_beat_saved() {
        let saved = LlmSettings {
            protocol: Protocol::OpenAi,
            base_url: "http://saved/v1".into(),
            model: "saved".into(),
        };
        let env = |name: &str| match name {
            "FORTNIGHT_LLM_BASE_URL" => Some("http://env/v1".to_string()),
            "FORTNIGHT_LLM_MODEL" => Some("env-model".to_string()),
            _ => None,
        };
        let none = LlmArgs::default();
        let got = effective_settings(&saved, env, &none);
        assert_eq!(got.base_url, "http://env/v1");
        assert_eq!(got.model, "env-model");
        assert_eq!(got.protocol, Protocol::OpenAi);

        let flags = LlmArgs {
            protocol: Some(Protocol::Anthropic),
            model: Some("flag-model".into()),
            ..LlmArgs::default()
        };
        let got = effective_settings(&saved, env, &flags);
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
    fn unconfigured_endpoint_is_a_usage_error() {
        let err = build_client(&LlmSettings::default(), None).err().unwrap();
        assert_eq!(err.code, 2);
        let ok = build_client(
            &LlmSettings {
                protocol: Protocol::OpenAi,
                base_url: "http://user:pw@localhost:11434/v1".into(),
                model: "m".into(),
            },
            None,
        )
        .ok()
        .unwrap();
        assert_eq!(ok.endpoint_host.as_deref(), Some("localhost:11434"));
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
        let err = connect_from_settings(&saved, &LlmSettings::default(), &LlmArgs::default(), &dir)
            .err()
            .unwrap();
        assert_eq!(err.code, 2);
    }
}
