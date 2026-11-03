//! Anonymous setup events: where installing, pairing, licensing and updating get stuck, so it can
//! be fixed.
//!
//! Sent by the install scripts and by commands people type (`ccn pair`, `ccn activate`, the
//! pitch), by the detached background updater (`cli_updated`, `cli_update_failed`), and from one
//! hook: SessionStart posts `cli_nudge_shown` when it shows a licence nudge, at most once a day.
//! Never anything about sessions, prompts or the computer's name. Each event is a name plus a few
//! short words (the stage that failed, how ccn was installed), posted once with a short timeout
//! and no retry: 3 s, or 1 s inside a hook, which Claude Code waits on.
//! `CCN_NO_ANALYTICS=1` or `DO_NOT_TRACK=1` turns them off.

use serde_json::{Map, Value, json};
use std::time::Duration;

use crate::config::{self, Config};

fn enabled() -> bool {
    let set = |name: &str| {
        std::env::var(name)
            .map(|v| !v.is_empty() && v != "0")
            .unwrap_or(false)
    };
    !set("CCN_NO_ANALYTICS") && !set("DO_NOT_TRACK")
}

/// How this binary got here: the install script tells it, npx sets CCN_EPHEMERAL, Homebrew, npm,
/// scoop and winget from where the binary really lives.
pub fn install_method() -> &'static str {
    if let Ok(method) = std::env::var("CCN_INSTALL_METHOD") {
        match method.as_str() {
            "script" => return "script",
            "powershell" => return "powershell",
            _ => {}
        }
    }
    if std::env::var("CCN_EPHEMERAL").is_ok() {
        return "npx";
    }
    // macOS reports the path ccn was started by, so brew's `/usr/local/bin/ccn` link must be
    // resolved to its Cellar before the path can say anything; update.rs decides it the same way
    std::env::current_exe()
        .ok()
        .and_then(|exe| crate::update::method_from_path_resolved(&exe))
        .unwrap_or("other")
}

/// The longest install source kept; install.sh and install.ps1 apply the same cap.
const MAX_SOURCE_LEN: usize = 32;

/// The install scripts' CCN_INSTALL_SOURCE. The website can tag an install line with any short
/// source (web, app, link, a campaign), so this keeps the same values the scripts send:
/// lowercase letters, digits, `_` and `-`, up to 32 of them, and "other" for anything else.
/// None when it is not set.
pub fn install_source_from_env() -> Option<String> {
    let value = std::env::var("CCN_INSTALL_SOURCE").ok()?;
    clean_source(&value)
}

fn clean_source(value: &str) -> Option<String> {
    if value.is_empty() {
        return None;
    }
    let safe = value.len() <= MAX_SOURCE_LEN
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-');
    Some(if safe { value } else { "other" }.to_string())
}

/// Where this computer's install line was copied from: this run's environment first, then what
/// `ccn setup` remembered. None for Homebrew, npm and npx installs, which have no install line.
pub fn install_source(cfg: &Config) -> Option<String> {
    install_source_from_env().or_else(|| cfg.install_source.clone())
}

/// Remembers the install source on the first run that carries one, so a later `ccn pair` typed by
/// hand still reports it. Never overwrites: the first install line is the one that brought them.
pub fn remember_install_source() {
    if let Some(source) = install_source_from_env() {
        let mut cfg = Config::load();
        if cfg.install_source.is_none() {
            cfg.install_source = Some(source);
            let _ = cfg.save();
        }
    }
}

/// How long an event may take. A hook holds up Claude Code while it runs, so an event sent from
/// one gets a third of the usual time: a slow server costs the user at most a second.
fn timeout() -> Duration {
    if std::env::args().nth(1).as_deref() == Some("hook") {
        Duration::from_secs(1)
    } else {
        Duration::from_secs(3)
    }
}

pub fn event(name: &str, properties: &[(&str, &str)]) {
    if !enabled() {
        return;
    }
    let cfg = Config::load();
    let mut props = Map::new();
    for (key, value) in properties {
        props.insert((*key).to_string(), Value::from(*value));
    }
    if let Some(source) = install_source(&cfg) {
        props
            .entry("source".to_string())
            .or_insert(Value::from(source));
    }
    let custom_server = std::env::var("CCN_SERVER_URL").is_ok();
    let body = json!({
        "context": {
            "platform": "cli",
            "appVersion": config::VERSION,
            "deviceModel": format!("{}-{}", config::platform(), std::env::consts::ARCH),
            "environment": if custom_server { "development" } else { "production" },
        },
        "events": [{
            "name": name,
            "timestamp": chrono::Utc::now().timestamp(),
            "properties": props,
        }],
    });
    let url = format!("{}/api/events", cfg.server_url.trim_end_matches('/'));
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(timeout()))
        .http_status_as_error(false)
        .build()
        .new_agent();
    let _ = agent
        .post(&url)
        .header("Accept-Language", &config::locale())
        .send_json(body);
}

/// A short reason for a failed pairing: the typed kind when ccn raised it itself (its message is
/// in the user's language), otherwise from the server's message or the transport error.
pub fn failure_reason(err: &anyhow::Error) -> &'static str {
    use crate::commands::PairFailure;
    if let Some(kind) = err.downcast_ref::<PairFailure>() {
        return match kind {
            PairFailure::MalformedCode => "malformed_code",
            PairFailure::CodeTimeout => "code_timeout",
        };
    }
    // The server's code is language-independent; the text matches below are for older servers
    if let Some(server) = crate::api::server_error(err) {
        match server.code.as_str() {
            "INVALID_CODE" => return "invalid_code",
            "RATE_LIMITED" => return "rate_limited",
            "MAX_DEVICES" => return "max_devices",
            _ => {}
        }
    }
    let text = format!("{err:#}").to_lowercase();
    if text.contains("invalid or expired pairing code") {
        "invalid_code"
    } else if text.contains("too many pairing attempts") {
        "rate_limited"
    } else if text.contains("maximum") {
        "max_devices"
    } else if text.contains("settings.json") || text.contains("hook") {
        "hooks"
    } else if text.contains("credential") || text.contains("keyring") || text.contains("keychain") {
        "credentials"
    } else if text.contains("dns")
        || text.contains("connect")
        || text.contains("timed out")
        || text.contains("timeout")
        || text.contains("tls")
    {
        "network"
    } else {
        "other"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_source_falls_back_to_the_remembered_one() {
        // The test runner doesn't set CCN_INSTALL_SOURCE, so only the config speaks
        if std::env::var("CCN_INSTALL_SOURCE").is_ok() {
            return;
        }
        let mut cfg = Config::default();
        assert_eq!(install_source(&cfg), None);
        cfg.install_source = Some("web".to_string());
        assert_eq!(install_source(&cfg).as_deref(), Some("web"));
    }

    #[test]
    fn install_sources_follow_the_install_scripts_rule() {
        let c = |s: &str| clean_source(s);
        for kept in [
            "web",
            "app",
            "link",
            "direct",
            "other",
            "reddit_2026-11",
            "a",
        ] {
            assert_eq!(c(kept).as_deref(), Some(kept));
        }
        assert_eq!(c(&"a".repeat(32)), Some("a".repeat(32)));
        for odd in ["Web", "web site", "web/app", "wéb", "x\"y", &"a".repeat(33)] {
            assert_eq!(c(odd).as_deref(), Some("other"), "{odd:?}");
        }
        assert_eq!(c(""), None);
    }

    #[test]
    fn reasons_are_recognised() {
        let r = |s: &str| failure_reason(&anyhow::anyhow!(s.to_string()));
        assert_eq!(
            r("pairing: Invalid or expired pairing code"),
            "invalid_code"
        );
        assert_eq!(
            r("pairing: Too many pairing attempts. Wait a minute and try again."),
            "rate_limited"
        );
        assert_eq!(
            failure_reason(&anyhow::anyhow!(
                crate::commands::PairFailure::MalformedCode
            )),
            "malformed_code"
        );
        assert_eq!(
            failure_reason(&anyhow::anyhow!(crate::commands::PairFailure::CodeTimeout)),
            "code_timeout"
        );
        assert_eq!(
            r("POST /api/pairing/claim-by-client: io: Connection refused"),
            "network"
        );
        assert_eq!(r("something else"), "other");
    }

    #[test]
    fn the_servers_code_wins_whatever_language_its_message_is_in() {
        let err = anyhow::Error::new(crate::api::ServerError {
            code: "INVALID_CODE".into(),
            message: "Code de jumelage invalide ou expiré".into(),
        })
        .context("pairing");
        assert_eq!(failure_reason(&err), "invalid_code");
        assert_eq!(
            format!("{err:#}"),
            "pairing: Code de jumelage invalide ou expiré"
        );
    }
}
