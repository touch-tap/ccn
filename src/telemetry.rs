//! Anonymous setup events: where installing and pairing get stuck, so it can be fixed.
//!
//! Only `ccn pair` and the install scripts send these, never the hooks, and never anything about
//! sessions, prompts or the computer's name. Each event is a name plus a few short words (the
//! stage that failed, how ccn was installed), posted once with a short timeout and no retry.
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

/// The install scripts' CCN_INSTALL_SOURCE, if it is one of the known values.
pub fn install_source_from_env() -> Option<&'static str> {
    match std::env::var("CCN_INSTALL_SOURCE").ok()?.as_str() {
        "web" => Some("web"),
        "app" => Some("app"),
        "link" => Some("link"),
        "direct" => Some("direct"),
        _ => None,
    }
}

/// Where this computer's install line was copied from: this run's environment first, then what
/// `ccn setup` remembered. None for Homebrew, npm and npx installs, which have no install line.
pub fn install_source(cfg: &Config) -> Option<String> {
    install_source_from_env()
        .map(str::to_string)
        .or_else(|| cfg.install_source.clone())
}

/// Remembers the install source on the first run that carries one, so a later `ccn pair` typed by
/// hand still reports it. Never overwrites: the first install line is the one that brought them.
pub fn remember_install_source() {
    if let Some(source) = install_source_from_env() {
        let mut cfg = Config::load();
        if cfg.install_source.is_none() {
            cfg.install_source = Some(source.to_string());
            let _ = cfg.save();
        }
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
        .timeout_global(Some(Duration::from_secs(3)))
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
