//! Suggests Notifier for Codex, the same phone app for OpenAI's Codex CLI (its computer side is
//! codex-notifier), to people who also use Codex and don't have it yet. Shown at the end of every
//! `ccn pair` (with a QR code for the app: the phone is in their hand), as a passive line in `ccn`
//! (status) and `ccn doctor`, and at most twice ever at SessionStart: a desktop notification
//! (most people never run ccn themselves, so that is the one they see) plus a line in Claude Code.
//!
//! Detection is a handful of stat calls and one small file read: no network, so hooks stay fast.
//! `ccn config set cross_promo off`, or `CCN_NO_NUDGE=1`, keeps it quiet.

use rust_i18n::t;
use serde_json::{Value, json};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use crate::config::{self, Config};
use crate::{local, notify, qr};

/// Not translated: the commands and URLs. One line installs codex-notifier, sets up Codex's hooks
/// and pairs: the script ends in `codex-notifier pair`, whose QR code also offers the app to a
/// phone without it. No Node needed, unlike npm.
const UNIX_COMMAND: &str = "curl -fsSL https://notifierforcodex.com/install.sh | sh";
const WINDOWS_COMMAND: &str =
    "irm https://notifierforcodex.com/install.ps1 | iex; codex-notifier pair";
/// The website, whose buttons take a phone to the App Store or Google Play
pub const URL: &str = "https://notifierforcodex.com/?ref=ccn-cli";
/// The QR code and link after `ccn pair`: /get sends a phone straight to the App Store or Google
/// Play (and shows a computer both store buttons), tagged ref=ccn-pair
pub const PAIR_URL: &str = "https://notifierforcodex.com/get?ref=ccn-pair";
/// What a click on the SessionStart desktop notification opens
pub const NOTIFICATION_URL: &str = "https://notifierforcodex.com/?ref=ccn-notification";

const DAY: i64 = 24 * 60 * 60;
/// The first SessionStart mention waits this long after pairing
const FIRST_AFTER: i64 = 3 * DAY;
/// The second waits this long after the first; there is no third
const SECOND_AFTER: i64 = 30 * DAY;
const SESSION_CAP: u32 = 2;

/// Where Codex keeps its config. `$CODEX_HOME` overrides, as it does for Codex.
pub fn codex_dir() -> PathBuf {
    match std::env::var_os("CODEX_HOME") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => config::home_dir().join(".codex"),
    }
}

/// Where codex-notifier keeps its config, resolved the way `config::ccn_dir` resolves ccn's.
pub fn codex_notifier_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("CODEX_NOTIFIER_HOME").filter(|d| !d.is_empty()) {
        return PathBuf::from(dir);
    }
    #[cfg(windows)]
    {
        if let Some(appdata) = std::env::var_os("APPDATA").filter(|d| !d.is_empty()) {
            return PathBuf::from(appdata).join("codex-notifier");
        }
    }
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME").filter(|d| !d.is_empty()) {
        return PathBuf::from(xdg).join("codex-notifier");
    }
    config::home_dir().join(".config").join("codex-notifier")
}

/// Whether any of `names` is a file in a folder on `path_var` (PATH). On Windows npm installs a
/// `.cmd` beside the extensionless script, and native installs are `.exe`.
fn on_path(names: &[&str], path_var: Option<&OsStr>) -> bool {
    let Some(path_var) = path_var else {
        return false;
    };
    let exts: &[&str] = if cfg!(windows) {
        &["", ".exe", ".cmd"]
    } else {
        &[""]
    };
    std::env::split_paths(path_var).any(|dir| {
        !dir.as_os_str().is_empty()
            && names.iter().any(|name| {
                exts.iter()
                    .any(|ext| dir.join(format!("{name}{ext}")).is_file())
            })
    })
}

/// Codex is here: its config folder exists, or `codex` is on PATH.
fn codex_present(codex_dir: &Path, path_var: Option<&OsStr>) -> bool {
    codex_dir.is_dir() || on_path(&["codex"], path_var)
}

/// codex-notifier is here already: its config folder, its binary on PATH, or its hooks in Codex's
/// hooks.json.
fn has_codex_notifier(cn_dir: &Path, codex_dir: &Path, path_var: Option<&OsStr>) -> bool {
    if cn_dir.is_dir() || on_path(&["codex-notifier", "codexnotifier"], path_var) {
        return true;
    }
    std::fs::read_to_string(codex_dir.join("hooks.json"))
        .is_ok_and(|text| text.contains("codex-notifier") || text.contains("codexnotifier"))
}

fn suggest_in(cn_dir: &Path, codex_dir: &Path, path_var: Option<&OsStr>) -> bool {
    codex_present(codex_dir, path_var) && !has_codex_notifier(cn_dir, codex_dir, path_var)
}

/// What this computer is: Codex is here (`cli`), codex-notifier is here (`notifier`).
fn codex_state_in(cn_dir: &Path, codex_dir: &Path, path_var: Option<&OsStr>) -> Value {
    json!({
        "cli": codex_present(codex_dir, path_var),
        "notifier": has_codex_notifier(cn_dir, codex_dir, path_var),
    })
}

/// What registration tells the server about Codex on this computer, so a phone paired with it can
/// be told about Notifier for Codex: `{"cli": bool, "notifier": bool}`. `None` (sent as null, or
/// left out) when the cross-promo is switched off here or analytics are (`CCN_NO_ANALYTICS`,
/// `DO_NOT_TRACK`): the server then treats the computer as unknown and suggests nothing.
/// Servers older than this field ignore it.
pub fn codex_report(cfg: &Config) -> Option<Value> {
    if !enabled(cfg) || !crate::telemetry::enabled() {
        return None;
    }
    Some(codex_state_in(
        &codex_notifier_dir(),
        &codex_dir(),
        std::env::var_os("PATH").as_deref(),
    ))
}

/// The setting and `CCN_NO_NUDGE` allow it
pub fn enabled(cfg: &Config) -> bool {
    cfg.cross_promo && !local::no_nudge_env()
}

/// Whether to suggest codex-notifier on this computer now (ignoring the SessionStart cadence).
pub fn should_suggest(cfg: &Config) -> bool {
    enabled(cfg)
        && suggest_in(
            &codex_notifier_dir(),
            &codex_dir(),
            std::env::var_os("PATH").as_deref(),
        )
}

/// The one line that sets codex-notifier up on this computer
pub fn command() -> &'static str {
    if cfg!(windows) {
        WINDOWS_COMMAND
    } else {
        UNIX_COMMAND
    }
}

/// The suggestion, in the user's language
pub fn line() -> String {
    t!("crosspromo.line", command = command(), url = URL).into_owned()
}

/// After every successful `ccn pair`, when it applies: the phone is in their hand, so a QR code
/// for the app, its link, and the one command for this computer. Reports where it was shown.
pub fn print_after_pair(cfg: &Config) {
    if !should_suggest(cfg) {
        return;
    }
    let code = qr::render(PAIR_URL);
    println!();
    println!("{}", t!("crosspromo.pair_title"));
    match &code {
        Some(code) => {
            println!("{}", t!("crosspromo.pair_scan"));
            println!();
            print!("{code}");
        }
        None => println!("{}", t!("crosspromo.pair_steps")),
    }
    println!("  {PAIR_URL}");
    println!("  {}", command());
    let qr = if code.is_some() { "yes" } else { "no" };
    crate::telemetry::event("cli_cross_promo_shown", &[("where", "pair"), ("qr", qr)]);
}

/// Prints the suggestion when it applies, after a blank line, and reports where it was shown.
/// For `ccn` (status) and `ccn doctor`: no cap. They don't wait on the network for the event.
pub fn print_hint(cfg: &Config, place: &str) {
    if !should_suggest(cfg) {
        return;
    }
    println!();
    println!("{}", line());
    crate::telemetry::event_detached("cli_cross_promo_shown", &[("where", place)]);
}

/// Whether a SessionStart may mention codex-notifier: at most twice ever, the first no sooner than
/// three days after pairing (`since`), the second 30 days after the first, and never within a
/// day of another nudge (`other_nudges`: the licence nudge, the rating prompt).
pub fn session_due(
    now: i64,
    since: i64,
    count: u32,
    last_at: Option<i64>,
    other_nudges: &[Option<i64>],
) -> bool {
    if other_nudges.iter().flatten().any(|last| now - last < DAY) {
        return false;
    }
    match count {
        0 => now - since >= FIRST_AFTER,
        c if c < SESSION_CAP => last_at.is_some_and(|last| now - last >= SECOND_AFTER),
        _ => false,
    }
}

/// The SessionStart `systemMessage`, when it is due (paired mode only), and the desktop
/// notification unless desktop notifications are off. Call it only when no other
/// message is being shown this SessionStart, the licence nudge included.
pub fn session_message(cfg: &mut Config, now: i64) -> Option<Value> {
    let since = match cfg.paired_at.or(cfg.cross_promo_first_seen_at) {
        Some(at) => at,
        None => {
            // Paired before ccn recorded when: count from the first SessionStart that looked
            cfg.cross_promo_first_seen_at = Some(now);
            let _ = cfg.save();
            now
        }
    };
    if !enabled(cfg)
        || !session_due(
            now,
            since,
            cfg.cross_promo_count,
            cfg.cross_promo_last_at,
            &[cfg.licence_nudge_last_at, cfg.rate_prompt_shown_at],
        )
        || !should_suggest(cfg)
    {
        return None;
    }
    cfg.cross_promo_count += 1;
    cfg.cross_promo_last_at = Some(now);
    let _ = cfg.save();
    if cfg.desktop_notifications && notify::available() {
        notify::show_link(
            notify::APP_NAME,
            &t!("crosspromo.notification_body"),
            NOTIFICATION_URL,
        );
    }
    crate::telemetry::event_detached("cli_cross_promo_shown", &[("where", "session")]);
    Some(json!({ "systemMessage": t!("crosspromo.session_message", line = line()) }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ccn-crosspromo-{name}-{}-{}",
            std::process::id(),
            config::now()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn touch(path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"#!/bin/sh\n").unwrap();
    }

    fn path_of(dirs: &[&Path]) -> OsString {
        std::env::join_paths(dirs).unwrap()
    }

    #[test]
    fn nothing_here_suggests_nothing() {
        let root = temp_dir("none");
        let bin = root.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let path = path_of(&[&bin]);
        assert!(!suggest_in(
            &root.join("cn"),
            &root.join(".codex"),
            Some(&path)
        ));
        assert!(!suggest_in(&root.join("cn"), &root.join(".codex"), None));
    }

    #[test]
    fn codex_folder_or_binary_suggests() {
        let root = temp_dir("present");
        let codex = root.join(".codex");
        let bin = root.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let path = path_of(&[&bin]);
        // The folder alone
        std::fs::create_dir_all(&codex).unwrap();
        assert!(suggest_in(&root.join("cn"), &codex, Some(&path)));
        // The binary alone
        let other = root.join("missing-codex");
        assert!(!suggest_in(&root.join("cn"), &other, Some(&path)));
        touch(&bin.join("codex"));
        assert!(suggest_in(&root.join("cn"), &other, Some(&path)));
    }

    #[test]
    fn codex_state_reports_cli_and_notifier() {
        let root = temp_dir("state");
        let codex = root.join(".codex");
        let cn = root.join("codex-notifier");
        let bin = root.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let path = path_of(&[&bin]);
        assert_eq!(
            codex_state_in(&cn, &codex, Some(&path)),
            json!({"cli": false, "notifier": false})
        );
        std::fs::create_dir_all(&codex).unwrap();
        assert_eq!(
            codex_state_in(&cn, &codex, Some(&path)),
            json!({"cli": true, "notifier": false})
        );
        // codex-notifier's hooks in Codex's hooks.json
        std::fs::write(
            codex.join("hooks.json"),
            r#"{"hooks":"codex-notifier hook"}"#,
        )
        .unwrap();
        assert_eq!(
            codex_state_in(&cn, &codex, Some(&path)),
            json!({"cli": true, "notifier": true})
        );
    }

    #[test]
    fn codex_report_respects_the_cross_promo_setting() {
        let mut cfg = Config::default();
        cfg.cross_promo = false;
        assert!(codex_report(&cfg).is_none());
    }

    #[test]
    fn codex_notifier_already_here_suggests_nothing() {
        let root = temp_dir("already");
        let codex = root.join(".codex");
        std::fs::create_dir_all(&codex).unwrap();
        let bin = root.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let path = path_of(&[&bin]);
        let cn = root.join("codex-notifier");
        assert!(suggest_in(&cn, &codex, Some(&path)));

        // Its config folder
        std::fs::create_dir_all(&cn).unwrap();
        assert!(!suggest_in(&cn, &codex, Some(&path)));
        std::fs::remove_dir_all(&cn).unwrap();

        // Its binary, under either name
        for name in ["codex-notifier", "codexnotifier"] {
            touch(&bin.join(name));
            assert!(!suggest_in(&cn, &codex, Some(&path)), "{name}");
            std::fs::remove_file(bin.join(name)).unwrap();
        }

        // Its hooks in Codex's hooks.json
        std::fs::write(codex.join("hooks.json"), r#"{"hooks":{"Stop":[]}}"#).unwrap();
        assert!(suggest_in(&cn, &codex, Some(&path)));
        std::fs::write(
            codex.join("hooks.json"),
            r#"{"hooks":{"Stop":[{"command":"/usr/local/bin/codex-notifier hook"}]}}"#,
        )
        .unwrap();
        assert!(!suggest_in(&cn, &codex, Some(&path)));
    }

    #[test]
    fn every_translation_promotes_the_app_with_one_command_and_no_one_platform() {
        for lang in config::LOCALES {
            let line = t!(
                "crosspromo.line",
                locale = lang,
                command = command(),
                url = URL
            );
            assert!(
                line.contains(command()) && line.contains(URL),
                "{lang}: {line}"
            );
            let naming_the_app = [
                line.to_string(),
                t!("crosspromo.notification_body", locale = lang).into_owned(),
                t!("crosspromo.pair_title", locale = lang).into_owned(),
            ];
            for text in &naming_the_app {
                assert!(text.contains("Notifier for Codex"), "{lang}: {text}");
            }
            let steps = [
                t!("crosspromo.pair_scan", locale = lang).into_owned(),
                t!("crosspromo.pair_steps", locale = lang).into_owned(),
            ];
            for text in naming_the_app.iter().chain(&steps) {
                assert!(!text.contains("Mac"), "{lang}: {text}");
            }
        }
    }

    #[test]
    fn the_command_installs_and_pairs_in_one_line() {
        assert!(command().contains("notifierforcodex.com/install."));
        assert!(!command().contains("npm"));
        assert_eq!(PAIR_URL, "https://notifierforcodex.com/get?ref=ccn-pair");
        assert!(qr::render(PAIR_URL).is_some());
    }

    #[test]
    fn session_cadence() {
        let paired = 1_000_000_000;
        // Not before three days after pairing
        assert!(!session_due(paired + 3 * DAY - 1, paired, 0, None, &[]));
        assert!(session_due(paired + 3 * DAY, paired, 0, None, &[]));
        // The second 30 days after the first
        let first = paired + 4 * DAY;
        assert!(!session_due(first + 29 * DAY, paired, 1, Some(first), &[]));
        assert!(session_due(first + 30 * DAY, paired, 1, Some(first), &[]));
        // Never a third
        assert!(!session_due(first + 400 * DAY, paired, 2, Some(first), &[]));
        // Never within a day of the licence nudge or the rating prompt
        let now = paired + 10 * DAY;
        assert!(!session_due(now, paired, 0, None, &[Some(now - 60)]));
        assert!(!session_due(now, paired, 0, None, &[None, Some(now - 60)]));
        assert!(session_due(now, paired, 0, None, &[Some(now - DAY), None]));
    }
}
