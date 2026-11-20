//! Suggests codex-notifier, the same notifier for OpenAI's Codex CLI, to people who also use
//! Codex and don't have it yet. Shown once at the end of `ccn pair`, as a passive line in `ccn`
//! (status) and `ccn doctor`, and at most twice ever in Claude Code at SessionStart.
//!
//! Detection is a handful of stat calls and one small file read: no network, so hooks stay fast.
//! `ccn config set cross_promo off`, or `CCN_NO_NUDGE=1`, keeps it quiet.

use rust_i18n::t;
use serde_json::{Value, json};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use crate::config::{self, Config};
use crate::local;

/// Not translated: a command and a URL
pub const COMMAND: &str = "npm install -g @touchtap/codex-notifier";
pub const URL: &str = "https://notifierforcodex.com/?ref=ccn-cli";

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

/// The suggestion, in the user's language
pub fn line() -> String {
    t!("crosspromo.line", command = COMMAND, url = URL).into_owned()
}

/// Prints the suggestion when it applies, after a blank line, and reports where it was shown.
/// For `ccn pair`, `ccn` (status) and `ccn doctor`: no cap.
pub fn print_hint(cfg: &Config, place: &str) {
    if !should_suggest(cfg) {
        return;
    }
    println!();
    println!("{}", line());
    crate::telemetry::event("cli_cross_promo_shown", &[("where", place)]);
}

/// Whether a SessionStart may mention codex-notifier: at most twice ever, the first no sooner than
/// three days after pairing (`since`), the second 30 days after the first, and never within a
/// day of the licence nudge.
pub fn session_due(
    now: i64,
    since: i64,
    count: u32,
    last_at: Option<i64>,
    licence_last_at: Option<i64>,
) -> bool {
    if licence_last_at.is_some_and(|last| now - last < DAY) {
        return false;
    }
    match count {
        0 => now - since >= FIRST_AFTER,
        c if c < SESSION_CAP => last_at.is_some_and(|last| now - last >= SECOND_AFTER),
        _ => false,
    }
}

/// The SessionStart `systemMessage`, when it is due (paired mode only). Call it only when no other
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
            cfg.licence_nudge_last_at,
        )
        || !should_suggest(cfg)
    {
        return None;
    }
    cfg.cross_promo_count += 1;
    cfg.cross_promo_last_at = Some(now);
    let _ = cfg.save();
    crate::telemetry::event("cli_cross_promo_shown", &[("where", "session")]);
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
    fn session_cadence() {
        let paired = 1_000_000_000;
        // Not before three days after pairing
        assert!(!session_due(paired + 3 * DAY - 1, paired, 0, None, None));
        assert!(session_due(paired + 3 * DAY, paired, 0, None, None));
        // The second 30 days after the first
        let first = paired + 4 * DAY;
        assert!(!session_due(first + 29 * DAY, paired, 1, Some(first), None));
        assert!(session_due(first + 30 * DAY, paired, 1, Some(first), None));
        // Never a third
        assert!(!session_due(first + 400 * DAY, paired, 2, Some(first), None));
        // Never within a day of the licence nudge
        let now = paired + 10 * DAY;
        assert!(!session_due(now, paired, 0, None, Some(now - 60)));
        assert!(session_due(now, paired, 0, None, Some(now - DAY)));
    }
}
