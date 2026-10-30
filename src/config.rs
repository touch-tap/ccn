//! Paths, the config file, the credential store and the per-machine settings mirror.

use anyhow::{Context, Result};
use rust_i18n::t;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

pub const DEFAULT_SERVER_URL: &str = "https://ccnotifier-api.touchtap.workers.dev";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const KEYRING_SERVICE: &str = "com.touchtap.ccn";
pub const KEYRING_USER: &str = "auth_token";

/// What this client tells the server it can do. The phone renders per capability.
pub const CAPABILITIES: &[&str] = &[
    "notify",
    "respond_options",
    "permission_decide",
    "stop_hold",
    "remote_settings",
    "snooze",
];

pub fn platform() -> &'static str {
    if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(windows) {
        "windows"
    } else {
        "linux"
    }
}

/// Where ccn keeps its config, sessions, credentials file and log. `$CCN_HOME` overrides.
pub fn ccn_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("CCN_HOME") {
        if !dir.is_empty() {
            return PathBuf::from(dir);
        }
    }
    #[cfg(windows)]
    {
        if let Ok(appdata) = std::env::var("APPDATA") {
            if !appdata.is_empty() {
                return PathBuf::from(appdata).join("ccn");
            }
        }
    }
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        if !xdg.is_empty() {
            return PathBuf::from(xdg).join("ccn");
        }
    }
    home_dir().join(".config").join("ccn")
}

pub fn home_dir() -> PathBuf {
    directories::BaseDirs::new()
        .map(|b| b.home_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn config_path() -> PathBuf {
    ccn_dir().join("config.toml")
}

pub fn sessions_dir() -> PathBuf {
    ccn_dir().join("sessions")
}

pub fn log_path() -> PathBuf {
    ccn_dir().join("ccn.log")
}

pub fn credentials_path() -> PathBuf {
    ccn_dir().join("credentials")
}

pub fn unpaired_path() -> PathBuf {
    ccn_dir().join("unpaired.json")
}

/// Claude Code's user settings file. `$CLAUDE_CONFIG_DIR` overrides, as it does for Claude Code.
pub fn claude_settings_path() -> PathBuf {
    if let Ok(dir) = std::env::var("CLAUDE_CONFIG_DIR") {
        if !dir.is_empty() {
            return PathBuf::from(dir).join("settings.json");
        }
    }
    home_dir().join(".claude").join("settings.json")
}

// ============================================
// Per-machine settings, mirrored from the server
// ============================================

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct QuietHours {
    pub enabled: bool,
    pub start: String, // "HH:MM"
    pub end: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub notify_on_done: Option<bool>,
    pub notify_on_permission: Option<bool>,
    pub notify_on_question: Option<bool>,
    pub quiet_hours: Option<QuietHours>,
    pub hold_ceiling_seconds: Option<u64>,
    pub presence_threshold_seconds: Option<u64>,
    pub stop_hold_seconds: Option<u64>,
    pub reminder_interval_seconds: Option<u64>,
    pub sound: Option<bool>,
    /// Send nothing to the phone while the user is at the terminal running Claude (default on)
    pub quiet_when_present: Option<bool>,
}

impl Settings {
    pub fn notify_on_done(&self) -> bool {
        self.notify_on_done.unwrap_or(true)
    }
    pub fn notify_on_permission(&self) -> bool {
        self.notify_on_permission.unwrap_or(true)
    }
    pub fn notify_on_question(&self) -> bool {
        self.notify_on_question.unwrap_or(true)
    }
    /// How long a blocking hook waits for the phone, in seconds.
    pub fn hold_ceiling(&self) -> u64 {
        self.hold_ceiling_seconds.unwrap_or(600).clamp(30, 3600)
    }
    /// Idle time after which the user counts as away from the keyboard.
    pub fn presence_threshold(&self) -> f64 {
        self.presence_threshold_seconds.unwrap_or(30).clamp(5, 600) as f64
    }
    /// How long Stop waits for a free-text follow-up from the phone. 0 turns the hold off.
    pub fn stop_hold(&self) -> u64 {
        self.stop_hold_seconds.unwrap_or(120).min(600)
    }
    pub fn sound(&self) -> bool {
        self.sound.unwrap_or(true)
    }
    pub fn quiet_when_present(&self) -> bool {
        self.quiet_when_present.unwrap_or(true)
    }

    pub fn in_quiet_hours(&self, now: chrono::NaiveTime) -> bool {
        let Some(q) = &self.quiet_hours else {
            return false;
        };
        if !q.enabled {
            return false;
        }
        let (Some(start), Some(end)) = (parse_hhmm(&q.start), parse_hhmm(&q.end)) else {
            return false;
        };
        if start <= end {
            now >= start && now < end
        } else {
            now >= start || now < end
        }
    }

    /// Applies `key = value` from the command line. Keys may be snake_case or camelCase.
    pub fn set_from_str(&mut self, key: &str, value: &str) -> Result<()> {
        let key = to_snake(key);
        let as_bool = || -> Result<bool> {
            match value.to_ascii_lowercase().as_str() {
                "true" | "on" | "yes" | "1" => Ok(true),
                "false" | "off" | "no" | "0" => Ok(false),
                _ => anyhow::bail!("{}", t!("settings.expects_on_off", key = key)),
            }
        };
        let as_secs = |min: u64, max: u64| -> Result<u64> {
            let n: u64 = value
                .parse()
                .with_context(|| t!("settings.expects_seconds", key = key).into_owned())?;
            anyhow::ensure!(
                (min..=max).contains(&n),
                "{}",
                t!("settings.seconds_range", key = key, min = min, max = max)
            );
            Ok(n)
        };
        match key.as_str() {
            "notify_on_done" => self.notify_on_done = Some(as_bool()?),
            "notify_on_permission" => self.notify_on_permission = Some(as_bool()?),
            "notify_on_question" => self.notify_on_question = Some(as_bool()?),
            "sound" => self.sound = Some(as_bool()?),
            "quiet_when_present" | "quiet_at_terminal" => {
                self.quiet_when_present = Some(as_bool()?)
            }
            "hold_ceiling_seconds" | "hold_ceiling" => {
                self.hold_ceiling_seconds = Some(as_secs(30, 3600)?)
            }
            "presence_threshold_seconds" | "presence_threshold" => {
                self.presence_threshold_seconds = Some(as_secs(5, 600)?)
            }
            "stop_hold_seconds" | "stop_hold" => self.stop_hold_seconds = Some(as_secs(0, 600)?),
            "reminder_interval_seconds" | "reminder_interval" => {
                self.reminder_interval_seconds = Some(as_secs(0, 3600)?)
            }
            "quiet_hours" => {
                if matches!(
                    value.to_ascii_lowercase().as_str(),
                    "off" | "false" | "none"
                ) {
                    if let Some(q) = &mut self.quiet_hours {
                        q.enabled = false;
                    }
                } else {
                    let (start, end) = value
                        .split_once('-')
                        .with_context(|| t!("settings.quiet_hours_format").into_owned())?;
                    anyhow::ensure!(
                        parse_hhmm(start).is_some() && parse_hhmm(end).is_some(),
                        "{}",
                        t!("settings.quiet_hours_example")
                    );
                    self.quiet_hours = Some(QuietHours {
                        enabled: true,
                        start: start.to_string(),
                        end: end.to_string(),
                    });
                }
            }
            other => anyhow::bail!("{}", t!("settings.unknown", key = other)),
        }
        Ok(())
    }
}

fn parse_hhmm(s: &str) -> Option<chrono::NaiveTime> {
    chrono::NaiveTime::parse_from_str(s.trim(), "%H:%M").ok()
}

fn to_snake(key: &str) -> String {
    let mut out = String::new();
    for ch in key.chars() {
        if ch.is_ascii_uppercase() {
            out.push('_');
            out.push(ch.to_ascii_lowercase());
        } else if ch == '-' {
            out.push('_');
        } else {
            out.push(ch);
        }
    }
    out
}

// ============================================
// The config file
// ============================================

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Config {
    pub server_url: String,
    pub client_id: Option<String>,
    pub machine_name: Option<String>,
    /// Show a desktop notification as well as pushing to the phone.
    pub desktop_notifications: bool,
    /// "keychain" or "file". Unless chosen with `ccn config set credential_store`, the token goes in
    /// the system keychain on macOS (a signed binary, so upgrades don't prompt) and Windows, and in
    /// a 0600 file on Linux, where a keyring is often missing or locked on servers.
    pub credential_store: String,
    /// True once the user picked `credential_store` themselves; until then it is decided per platform
    pub credential_store_chosen: bool,
    pub settings: Settings,
    pub settings_updated_at: Option<i64>,
    pub settings_etag: Option<String>,
    pub settings_fetched_at: Option<i64>,
    pub registered_at: Option<i64>,
    /// When a SessionStart hook last asked the server about this computer's licence (once a day)
    pub licence_checked_at: Option<i64>,
    /// When the licence nudge was last shown in Claude Code (its cadence is in hook.rs)
    pub licence_nudge_last_at: Option<i64>,
    /// How many phones are paired, as of the last `pairings` call that succeeded. None until the
    /// server has been asked; Some(0) is local mode (desktop notifications only).
    pub phones: Option<u32>,
    /// When the first phone paired with this computer
    pub paired_at: Option<i64>,
    /// Claude Code has shown "paired with <phone>" once
    pub paired_message_shown: bool,
    /// Desktop notifications shown while no phone was paired
    pub local_notifications: u64,
    /// When the "run ccn pair" message was last shown in Claude Code
    pub pair_nudge_last_at: Option<i64>,
    /// The 10th local notification has asked for the message at the next SessionStart or prompt
    pub pair_nudge_due: bool,
    /// Whether to suggest pairing at all (`ccn config set pair_nudges off`; default on)
    pub pair_nudges: bool,
    /// Look for a newer ccn once a day from the SessionStart hook and install it when ccn owns its
    /// own binary (`ccn config set auto_update off`, or `CCN_NO_AUTO_UPDATE=1`, turns it off)
    pub auto_update: bool,
    /// When the daily check last started a background `ccn upgrade`
    pub update_checked_at: Option<i64>,
    /// A newer version the background check found but could not install (Homebrew, npm, scoop,
    /// winget own the binary); the SessionStart hook mentions it once a week
    pub update_available: Option<String>,
    /// When `update_available` was last mentioned in Claude Code
    pub update_announced_at: Option<i64>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            server_url: DEFAULT_SERVER_URL.to_string(),
            client_id: None,
            machine_name: None,
            desktop_notifications: true,
            credential_store: "file".to_string(),
            credential_store_chosen: false,
            settings: Settings::default(),
            settings_updated_at: None,
            settings_etag: None,
            settings_fetched_at: None,
            registered_at: None,
            licence_checked_at: None,
            licence_nudge_last_at: None,
            phones: None,
            paired_at: None,
            paired_message_shown: false,
            local_notifications: 0,
            pair_nudge_last_at: None,
            pair_nudge_due: false,
            pair_nudges: true,
            auto_update: true,
            update_checked_at: None,
            update_available: None,
            update_announced_at: None,
        }
    }
}

impl Config {
    pub fn load() -> Config {
        let mut cfg = fs::read_to_string(config_path())
            .ok()
            .and_then(|text| toml::from_str::<Config>(&text).ok())
            .unwrap_or_default();
        if let Ok(url) = std::env::var("CCN_SERVER_URL") {
            if !url.is_empty() {
                cfg.server_url = url;
            }
        }
        if cfg.server_url.is_empty() {
            cfg.server_url = DEFAULT_SERVER_URL.to_string();
        }
        cfg
    }

    pub fn save(&self) -> Result<()> {
        let path = config_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
        let mut to_write = self.clone();
        if std::env::var("CCN_SERVER_URL").is_ok() {
            // an override from the environment is not persisted
            to_write.server_url =
                Config::load_file_server_url().unwrap_or_else(|| DEFAULT_SERVER_URL.to_string());
        }
        let text = toml::to_string_pretty(&to_write)?;
        fs::write(&path, text).with_context(|| format!("writing {}", path.display()))?;
        restrict_permissions(&path);
        Ok(())
    }

    fn load_file_server_url() -> Option<String> {
        fs::read_to_string(config_path())
            .ok()
            .and_then(|text| toml::from_str::<Config>(&text).ok())
            .map(|c| c.server_url)
    }

    /// This computer has credentials: it is registered with the server (not necessarily with a
    /// phone paired; see `local_mode`).
    pub fn is_paired(&self) -> bool {
        self.client_id.is_some() && load_token(self).is_some()
    }

    /// No phone gets this computer's notifications: either it has no credentials, or the server
    /// said it has none paired. Hooks then show desktop notifications and stay off the network.
    /// Credentials with an unknown count (None) count as paired until a SessionStart asks.
    pub fn local_mode(&self) -> bool {
        !self.is_paired() || self.phones == Some(0)
    }

    /// Records the paired-phone count from a successful `pairings` call.
    pub fn record_phones(&mut self, count: usize) {
        self.phones = Some(count as u32);
        if count > 0 && self.paired_at.is_none() {
            self.paired_at = Some(now());
        }
    }

    pub fn machine_name(&self) -> String {
        self.machine_name
            .clone()
            .unwrap_or_else(default_machine_name)
    }
}

pub fn default_machine_name() -> String {
    let from_env = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .ok()
        .filter(|s| !s.is_empty());
    let from_cmd = || {
        std::process::Command::new("hostname")
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|s| !s.is_empty())
    };
    let raw = from_env
        .or_else(from_cmd)
        .unwrap_or_else(|| format!("{} computer", platform()));
    raw.trim_end_matches(".local").to_string()
}

// ============================================
// The server stopped accepting this computer
// ============================================

/// Present while the server rejects this computer's credentials. Hooks then skip the network,
/// so they stay instant until `ccn pair` deletes it.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Unpaired {
    /// When a hook first saw the rejection
    pub since: i64,
    /// When the server was last asked (a SessionStart re-checks once a day)
    pub checked_at: i64,
    /// When the desktop notification was last shown (at most once a day)
    pub notified_at: i64,
}

impl Unpaired {
    pub const RECHECK_AFTER: i64 = 24 * 60 * 60;
    pub const NOTIFY_EVERY: i64 = 24 * 60 * 60;

    pub fn new() -> Unpaired {
        let now = now();
        Unpaired {
            since: now,
            checked_at: now,
            notified_at: 0,
        }
    }

    pub fn load() -> Option<Unpaired> {
        let text = fs::read_to_string(unpaired_path()).ok()?;
        Some(serde_json::from_str(&text).unwrap_or_else(|_| Unpaired::new()))
    }

    pub fn save(&self) {
        let path = unpaired_path();
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Ok(text) = serde_json::to_string(self) {
            let _ = fs::write(path, text);
        }
    }

    pub fn clear() {
        let _ = fs::remove_file(unpaired_path());
    }
}

// ============================================
// Credentials
// ============================================

/// Where the token belongs: "keychain" or "file".
pub fn credential_store(cfg: &Config) -> &'static str {
    if cfg.credential_store_chosen {
        return if cfg.credential_store == "keychain" {
            "keychain"
        } else {
            "file"
        };
    }
    if std::env::var("CCN_CREDENTIAL_STORE")
        .map(|v| v == "file")
        .unwrap_or(false)
    {
        return "file";
    }
    if cfg!(any(target_os = "macos", windows)) {
        "keychain"
    } else {
        "file"
    }
}

pub fn load_token(cfg: &Config) -> Option<String> {
    let keychain = credential_store(cfg) == "keychain";
    if keychain {
        if let Ok(entry) = keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER) {
            // Empty is what delete_token leaves when it may not delete
            if let Ok(token) = entry.get_password().map(|t| t.trim().to_string()) {
                if !token.is_empty() {
                    return Some(token);
                }
            }
        }
    }
    let token = fs::read_to_string(credentials_path())
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())?;
    // A token from before the keychain became the default moves there the first time it is read.
    // If the keychain refuses (locked, no keyring), the file stays and keeps working.
    if keychain {
        if let Ok(entry) = keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER) {
            if entry.set_password(&token).is_ok()
                && entry.get_password().ok().as_deref() == Some(token.as_str())
            {
                let _ = fs::remove_file(credentials_path());
            }
        }
    }
    Some(token)
}

/// Stores the token and returns where it went.
pub fn store_token(cfg: &Config, token: &str) -> Result<&'static str> {
    if credential_store(cfg) == "keychain" {
        if let Ok(entry) = keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER) {
            if entry.set_password(token).is_ok() {
                let _ = fs::remove_file(credentials_path());
                return Ok("keychain");
            }
        }
    }
    let path = credentials_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, format!("{token}\n"))
        .with_context(|| format!("writing {}", path.display()))?;
    restrict_permissions(&path);
    Ok("file")
}

pub fn delete_token() {
    // CCN_CREDENTIAL_STORE=file is what the tests set: they must not touch the real keychain
    let file_only = std::env::var("CCN_CREDENTIAL_STORE").is_ok_and(|v| v == "file");
    if let (false, Ok(entry)) = (
        file_only,
        keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER),
    ) {
        // On macOS only the build that created the item may delete it; a later version (same
        // signature) may still change it, so it empties the item instead
        if entry.delete_credential().is_err() {
            let _ = entry.set_password("");
        }
    }
    let _ = fs::remove_file(credentials_path());
}

#[cfg(unix)]
fn restrict_permissions(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &std::path::Path) {}

pub fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

// ============================================
// Language
// ============================================

/// The languages the server and the phone apps speak, as sent in Accept-Language.
pub const LOCALES: &[&str] = &[
    "en", "ar", "de", "es", "fr", "he", "it", "ja", "ko", "nl", "pl", "pt-BR", "ru", "tr", "vi",
    "zh-Hans", "zh-Hant",
];

/// The language to ask the server for: `CCN_LANG`, else the OS locale, mapped onto `LOCALES`.
pub fn locale() -> String {
    let raw = std::env::var("CCN_LANG")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .or_else(sys_locale::get_locale)
        .unwrap_or_default();
    normalise_locale(&raw).to_string()
}

/// Maps a BCP 47 or POSIX locale tag onto one of `LOCALES`; anything unknown is "en".
pub fn normalise_locale(raw: &str) -> &'static str {
    // en_GB.UTF-8 → en-GB; drop the encoding and modifier
    let tag = raw.split(['.', '@']).next().unwrap_or("").replace('_', "-");
    let mut parts = tag.split('-').filter(|p| !p.is_empty());
    let language = parts.next().unwrap_or("").to_ascii_lowercase();
    let rest: Vec<String> = parts.map(|p| p.to_ascii_lowercase()).collect();
    match language.as_str() {
        "zh" => {
            let traditional = rest
                .iter()
                .any(|p| matches!(p.as_str(), "hant" | "tw" | "hk" | "mo"));
            if traditional { "zh-Hant" } else { "zh-Hans" }
        }
        "pt" => "pt-BR",
        "iw" | "he" => "he",
        other => LOCALES
            .iter()
            .find(|l| l.eq_ignore_ascii_case(other))
            .copied()
            .unwrap_or("en"),
    }
}

#[cfg(test)]
mod locale_tests {
    use super::normalise_locale;

    #[test]
    fn maps_os_locales_onto_the_supported_set() {
        assert_eq!(normalise_locale("en_GB.UTF-8"), "en");
        assert_eq!(normalise_locale("en-US"), "en");
        assert_eq!(normalise_locale("de-DE"), "de");
        assert_eq!(normalise_locale("pt_PT"), "pt-BR");
        assert_eq!(normalise_locale("pt"), "pt-BR");
        assert_eq!(normalise_locale("zh-TW"), "zh-Hant");
        assert_eq!(normalise_locale("zh-Hant-HK"), "zh-Hant");
        assert_eq!(normalise_locale("zh_HK"), "zh-Hant");
        assert_eq!(normalise_locale("zh-MO"), "zh-Hant");
        assert_eq!(normalise_locale("zh-CN"), "zh-Hans");
        assert_eq!(normalise_locale("zh"), "zh-Hans");
        assert_eq!(normalise_locale("zh-Hans-SG"), "zh-Hans");
        assert_eq!(normalise_locale("iw_IL"), "he");
        assert_eq!(normalise_locale("he"), "he");
        assert_eq!(normalise_locale("ja_JP@calendar=japanese"), "ja");
        assert_eq!(normalise_locale("KO"), "ko");
        assert_eq!(normalise_locale("C"), "en");
        assert_eq!(normalise_locale("POSIX"), "en");
        assert_eq!(normalise_locale(""), "en");
        assert_eq!(normalise_locale("xx-YY"), "en");
    }
}
