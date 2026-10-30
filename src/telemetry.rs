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

/// How this binary got here: the install script tells it, npx sets CCN_EPHEMERAL, Homebrew's
/// binaries live under its prefix.
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
    let exe = std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    if exe.contains("/Cellar/") || exe.contains("/homebrew/") || exe.contains("linuxbrew") {
        "homebrew"
    } else if exe.contains("node_modules") {
        "npm"
    } else {
        "other"
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
