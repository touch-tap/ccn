//! The one place ccn mentions its sister apps: Notifier for Codex (computer side codex-notifier)
//! for OpenAI's Codex CLI, and Antigravity Notifier (computer side antigravity-notifier) for
//! Google Antigravity's `agy`. One of them is suggested only to someone who has that agent and
//! not its notifier already (Notifier for Codex first when both qualify, never both at once).
//! Shown at the end of every `ccn pair` (with a QR code: the phone is in their hand), as a
//! passive line in `ccn` (status) and `ccn doctor`, and at most twice ever at SessionStart,
//! whichever app is named: a desktop notification (most people never run ccn themselves, so that
//! is the one they see) plus a line in Claude Code.
//!
//! Detection is a handful of stat calls and at most two small file reads: no network, so hooks
//! stay fast. `ccn config set cross_promo off`, or `CCN_NO_NUDGE=1`, keeps it quiet.

use rust_i18n::t;
use serde_json::{Value, json};
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use crate::config::{self, Config};
use crate::{local, notify, qr};

/// Notifier for Codex's addresses. Not translated: the commands and URLs.
mod codex {
    pub const UNIX_COMMAND: &str = "curl -fsSL https://notifierforcodex.com/install.sh | sh";
    pub const WINDOWS_COMMAND: &str =
        "irm https://notifierforcodex.com/install.ps1 | iex; codex-notifier pair";
    /// The website, whose buttons take a phone to the App Store or Google Play
    pub const URL: &str = "https://notifierforcodex.com/?ref=ccn-cli";
    /// The QR code and link after `ccn pair`: /get sends a phone straight to the App Store or
    /// Google Play (and shows a computer both store buttons), tagged ref=ccn-pair
    pub const PAIR_URL: &str = "https://notifierforcodex.com/get?ref=ccn-pair";
    /// What a click on the SessionStart desktop notification opens
    pub const NOTIFICATION_URL: &str = "https://notifierforcodex.com/?ref=ccn-notification";
}

/// Antigravity Notifier's addresses. All three are the website itself, never a store link or
/// /get: its phone apps were still in store review when this was written (27 Nov 2026), and the
/// website says where the apps are as soon as they are there.
mod antigravity {
    pub const UNIX_COMMAND: &str = "curl -fsSL https://antigravitynotifier.com/install.sh | sh";
    pub const WINDOWS_COMMAND: &str =
        "irm https://antigravitynotifier.com/install.ps1 | iex; antigravity-notifier pair";
    pub const URL: &str = "https://antigravitynotifier.com/?ref=ccn-cli";
    /// The QR code and link after `ccn pair`
    pub const PAIR_URL: &str = "https://antigravitynotifier.com/?ref=ccn-pair";
    /// What a click on the SessionStart desktop notification opens
    pub const NOTIFICATION_URL: &str = "https://antigravitynotifier.com/?ref=ccn-notification";
}

const DAY: i64 = 24 * 60 * 60;
/// The first SessionStart mention waits this long after pairing
const FIRST_AFTER: i64 = 3 * DAY;
/// The second waits this long after the first; there is no third
const SECOND_AFTER: i64 = 30 * DAY;
/// SessionStart mentions one of them no more often than this, ever (both together)
const SESSION_CAP: u32 = 2;

/// A sister app that may be suggested. One line installs its CLI, sets up its agent's hooks and
/// pairs: each install script ends in `pair`, whose QR code also offers the app to a phone
/// without it. No Node needed, unlike npm.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sister {
    /// Notifier for Codex, for OpenAI's Codex CLI
    Codex,
    /// Antigravity Notifier, for Google Antigravity (`agy`)
    Antigravity,
}

impl Sister {
    pub const ALL: [Sister; 2] = [Sister::Codex, Sister::Antigravity];

    /// Telemetry's name for it
    pub fn id(self) -> &'static str {
        match self {
            Sister::Codex => "codex",
            Sister::Antigravity => "antigravity",
        }
    }

    /// The app's own name, which every translation keeps
    #[cfg(test)]
    fn app_name(self) -> &'static str {
        match self {
            Sister::Codex => "Notifier for Codex",
            Sister::Antigravity => "Antigravity Notifier",
        }
    }

    /// The one line that sets its CLI up on this computer
    pub fn command(self) -> &'static str {
        match (self, cfg!(windows)) {
            (Sister::Codex, false) => codex::UNIX_COMMAND,
            (Sister::Codex, true) => codex::WINDOWS_COMMAND,
            (Sister::Antigravity, false) => antigravity::UNIX_COMMAND,
            (Sister::Antigravity, true) => antigravity::WINDOWS_COMMAND,
        }
    }

    /// The website
    pub fn url(self) -> &'static str {
        match self {
            Sister::Codex => codex::URL,
            Sister::Antigravity => antigravity::URL,
        }
    }

    /// The QR code and link after `ccn pair`
    pub fn pair_url(self) -> &'static str {
        match self {
            Sister::Codex => codex::PAIR_URL,
            Sister::Antigravity => antigravity::PAIR_URL,
        }
    }

    /// What a click on the SessionStart desktop notification opens
    pub fn notification_url(self) -> &'static str {
        match self {
            Sister::Codex => codex::NOTIFICATION_URL,
            Sister::Antigravity => antigravity::NOTIFICATION_URL,
        }
    }

    /// Its agent is on this computer
    pub fn agent_present(self, places: &Places) -> bool {
        match self {
            Sister::Codex => codex_present(places),
            Sister::Antigravity => antigravity_present(places),
        }
    }

    /// Its notifier is on this computer already
    pub fn notifier_present(self, places: &Places) -> bool {
        match self {
            Sister::Codex => has_codex_notifier(places),
            Sister::Antigravity => has_antigravity_notifier(places),
        }
    }

    /// Someone with its agent who doesn't have its notifier yet
    pub fn worth_suggesting(self, places: &Places) -> bool {
        self.agent_present(places) && !self.notifier_present(places)
    }

    /// The suggestion, in the user's language, the same everywhere it is shown
    pub fn line(self) -> String {
        let (command, url) = (self.command(), self.url());
        match self {
            Sister::Codex => t!("crosspromo.line", command = command, url = url),
            Sister::Antigravity => {
                t!("crosspromo_antigravity.line", command = command, url = url)
            }
        }
        .into_owned()
    }

    /// The SessionStart desktop notification's body (its title is this app's name)
    fn notification_body(self) -> String {
        match self {
            Sister::Codex => t!("crosspromo.notification_body"),
            Sister::Antigravity => t!("crosspromo_antigravity.notification_body"),
        }
        .into_owned()
    }

    /// Above the QR code after `ccn pair`
    fn pair_title(self) -> String {
        match self {
            Sister::Codex => t!("crosspromo.pair_title"),
            Sister::Antigravity => t!("crosspromo_antigravity.pair_title"),
        }
        .into_owned()
    }

    fn pair_scan(self) -> String {
        match self {
            Sister::Codex => t!("crosspromo.pair_scan"),
            Sister::Antigravity => t!("crosspromo_antigravity.pair_scan"),
        }
        .into_owned()
    }

    fn pair_steps(self) -> String {
        match self {
            Sister::Codex => t!("crosspromo.pair_steps"),
            Sister::Antigravity => t!("crosspromo_antigravity.pair_steps"),
        }
        .into_owned()
    }
}

/// Where the sister agents and their notifiers would live on this computer.
#[derive(Debug, Clone)]
pub struct Places {
    /// Codex's config folder: `$CODEX_HOME`, else `~/.codex`
    pub codex_dir: PathBuf,
    /// codex-notifier's own folder, found the way it finds it (and `config::ccn_dir` finds ccn's)
    pub codex_notifier_dir: PathBuf,
    /// The folder Antigravity shares with the Gemini CLI, found the way antigravity-notifier finds
    /// it: `$ANTIGRAVITY_NOTIFIER_GEMINI_HOME`, else `~/.gemini`
    pub gemini_dir: PathBuf,
    /// antigravity-notifier's own folder, found the way it finds it
    pub antigravity_notifier_dir: PathBuf,
    /// `$PATH`
    pub path: Option<OsString>,
}

fn env_dir(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

impl Places {
    pub fn from_env() -> Places {
        let home = config::home_dir();
        Places {
            codex_dir: env_dir("CODEX_HOME").unwrap_or_else(|| home.join(".codex")),
            codex_notifier_dir: tool_dir(&home, "CODEX_NOTIFIER_HOME", "codex-notifier"),
            gemini_dir: env_dir("ANTIGRAVITY_NOTIFIER_GEMINI_HOME")
                .unwrap_or_else(|| home.join(".gemini")),
            antigravity_notifier_dir: tool_dir(
                &home,
                "ANTIGRAVITY_NOTIFIER_HOME",
                "antigravity-notifier",
            ),
            path: std::env::var_os("PATH"),
        }
    }
}

/// A sister CLI's folder: `$<env>`, else `%APPDATA%\<name>` on Windows, else
/// `$XDG_CONFIG_HOME/<name>`, else `~/.config/<name>`. The same order as `config::ccn_dir`.
fn tool_dir(home: &Path, env: &str, name: &str) -> PathBuf {
    if let Some(dir) = env_dir(env) {
        return dir;
    }
    #[cfg(windows)]
    {
        if let Some(appdata) = env_dir("APPDATA") {
            return appdata.join(name);
        }
    }
    if let Some(xdg) = env_dir("XDG_CONFIG_HOME") {
        return xdg.join(name);
    }
    home.join(".config").join(name)
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
fn codex_present(places: &Places) -> bool {
    places.codex_dir.is_dir() || on_path(&["codex"], places.path.as_deref())
}

/// codex-notifier is here already: its config folder, its binary on PATH, or its hooks in Codex's
/// hooks.json.
fn has_codex_notifier(places: &Places) -> bool {
    if places.codex_notifier_dir.is_dir()
        || on_path(&["codex-notifier", "codexnotifier"], places.path.as_deref())
    {
        return true;
    }
    std::fs::read_to_string(places.codex_dir.join("hooks.json"))
        .is_ok_and(|text| text.contains("codex-notifier") || text.contains("codexnotifier"))
}

/// Antigravity is here: `agy` is on PATH, or `~/.gemini` holds what Antigravity keeps there
/// (`antigravity-cli`, or the `config` folder its hooks.json lives in). `~/.gemini` alone is not
/// enough: the Gemini CLI uses it too.
fn antigravity_present(places: &Places) -> bool {
    on_path(&["agy"], places.path.as_deref())
        || places.gemini_dir.join("antigravity-cli").is_dir()
        || places.gemini_dir.join("config").is_dir()
}

/// antigravity-notifier is here already: its config folder, its binary on PATH, or its hooks in
/// Antigravity's hooks.json (where its group is named after it).
fn has_antigravity_notifier(places: &Places) -> bool {
    if places.antigravity_notifier_dir.is_dir()
        || on_path(&["antigravity-notifier"], places.path.as_deref())
    {
        return true;
    }
    std::fs::read_to_string(places.gemini_dir.join("config").join("hooks.json"))
        .is_ok_and(|text| text.contains("antigravity-notifier"))
}

/// The sister app to suggest here, if any: Notifier for Codex first when both qualify, so never
/// more than one at a time.
fn worth_suggesting(places: &Places) -> Option<Sister> {
    Sister::ALL.into_iter().find(|s| s.worth_suggesting(places))
}

/// What this computer is: Codex is here (`cli`), codex-notifier is here (`notifier`).
fn codex_state_in(places: &Places) -> Value {
    json!({
        "cli": codex_present(places),
        "notifier": has_codex_notifier(places),
    })
}

/// What registration tells the server about Codex on this computer, so a phone paired with it can
/// be told about Notifier for Codex: `{"cli": bool, "notifier": bool}`. `None` (sent as null, or
/// left out) when the cross-promo is switched off here or analytics are (`CCN_NO_ANALYTICS`,
/// `DO_NOT_TRACK`): the server then treats the computer as unknown and suggests nothing.
/// Servers older than this field ignore it.
///
/// Nothing is reported about Antigravity: the server's registration (27 Nov 2026) reads `codex`
/// and has nowhere to keep anything else, so an `antigravity` field would be sent for nothing.
pub fn codex_report(cfg: &Config) -> Option<Value> {
    if !enabled(cfg) || !crate::telemetry::enabled() {
        return None;
    }
    Some(codex_state_in(&Places::from_env()))
}

/// The setting and `CCN_NO_NUDGE` allow it
pub fn enabled(cfg: &Config) -> bool {
    cfg.cross_promo && !local::no_nudge_env()
}

/// The app to suggest on this computer now, if any (ignoring the SessionStart cadence).
pub fn eligible(cfg: &Config) -> Option<Sister> {
    if enabled(cfg) {
        worth_suggesting(&Places::from_env())
    } else {
        None
    }
}

/// After every successful `ccn pair`, when it applies: the phone is in their hand, so a QR code
/// (for the app, or for Antigravity Notifier's website), its link, and the one command for this
/// computer. Reports where it was shown.
pub fn print_after_pair(cfg: &Config) {
    let Some(sister) = eligible(cfg) else {
        return;
    };
    let code = qr::render(sister.pair_url());
    println!();
    println!("{}", sister.pair_title());
    match &code {
        Some(code) => {
            println!("{}", sister.pair_scan());
            println!();
            print!("{code}");
        }
        None => println!("{}", sister.pair_steps()),
    }
    println!("  {}", sister.pair_url());
    println!("  {}", sister.command());
    let qr = if code.is_some() { "yes" } else { "no" };
    crate::telemetry::event(
        "cli_cross_promo_shown",
        &[("where", "pair"), ("qr", qr), ("app", sister.id())],
    );
}

/// Prints the suggestion when it applies, after a blank line, and reports where it was shown.
/// For `ccn` (status) and `ccn doctor`: no cap. They don't wait on the network for the event.
pub fn print_hint(cfg: &Config, place: &str) {
    let Some(sister) = eligible(cfg) else {
        return;
    };
    println!();
    println!("{}", sister.line());
    crate::telemetry::event_detached(
        "cli_cross_promo_shown",
        &[("where", place), ("app", sister.id())],
    );
}

/// Whether a SessionStart may mention a sister app: at most twice ever (both apps together), the
/// first no sooner than three days after pairing (`since`), the second 30 days after the first,
/// and never within a day of another nudge (`other_nudges`: the licence nudge, the rating prompt).
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
    if cfg.paired_at.or(cfg.cross_promo_first_seen_at).is_none() {
        // Paired before ccn recorded when: count from the first SessionStart that looked
        cfg.cross_promo_first_seen_at = Some(now);
        let _ = cfg.save();
    }
    if !enabled(cfg) {
        return None;
    }
    let sister = session_sister(cfg, now, &Places::from_env())?;
    let _ = cfg.save();
    if cfg.desktop_notifications && notify::available() {
        notify::show_link(
            notify::APP_NAME,
            &sister.notification_body(),
            sister.notification_url(),
        );
    }
    crate::telemetry::event_detached(
        "cli_cross_promo_shown",
        &[("where", "session"), ("app", sister.id())],
    );
    Some(json!({ "systemMessage": t!("crosspromo.session_message", line = sister.line()) }))
}

/// The cadence and detection behind `session_message`, without saving: the app to name when a
/// mention is due here, with the showing counted in `cfg`.
fn session_sister(cfg: &mut Config, now: i64, places: &Places) -> Option<Sister> {
    let since = cfg
        .paired_at
        .or(cfg.cross_promo_first_seen_at)
        .unwrap_or(now);
    if !session_due(
        now,
        since,
        cfg.cross_promo_count,
        cfg.cross_promo_last_at,
        &[cfg.licence_nudge_last_at, cfg.rate_prompt_shown_at],
    ) {
        return None;
    }
    let sister = worth_suggesting(places)?;
    cfg.cross_promo_count += 1;
    cfg.cross_promo_last_at = Some(now);
    Some(sister)
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// Everything under `root`, with `root/bin` (created) as the whole PATH
    fn places(root: &Path) -> Places {
        let bin = root.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        Places {
            codex_dir: root.join(".codex"),
            codex_notifier_dir: root.join("codex-notifier"),
            gemini_dir: root.join(".gemini"),
            antigravity_notifier_dir: root.join("antigravity-notifier"),
            path: Some(std::env::join_paths([bin]).unwrap()),
        }
    }

    #[test]
    fn nothing_here_suggests_nothing() {
        let root = temp_dir("none");
        let p = places(&root);
        assert_eq!(worth_suggesting(&p), None);
        assert_eq!(worth_suggesting(&Places { path: None, ..p }), None);
    }

    #[test]
    fn codex_folder_or_binary_suggests() {
        // The folder alone
        let root = temp_dir("present-dir");
        let p = places(&root);
        std::fs::create_dir_all(&p.codex_dir).unwrap();
        assert_eq!(worth_suggesting(&p), Some(Sister::Codex));
        // The binary alone
        let root = temp_dir("present-bin");
        let p = places(&root);
        assert_eq!(worth_suggesting(&p), None);
        touch(&root.join("bin").join("codex"));
        assert_eq!(worth_suggesting(&p), Some(Sister::Codex));
    }

    #[test]
    fn codex_state_reports_cli_and_notifier() {
        let root = temp_dir("state");
        let p = places(&root);
        assert_eq!(codex_state_in(&p), json!({"cli": false, "notifier": false}));
        std::fs::create_dir_all(&p.codex_dir).unwrap();
        assert_eq!(codex_state_in(&p), json!({"cli": true, "notifier": false}));
        // codex-notifier's hooks in Codex's hooks.json
        std::fs::write(
            p.codex_dir.join("hooks.json"),
            r#"{"hooks":"codex-notifier hook"}"#,
        )
        .unwrap();
        assert_eq!(codex_state_in(&p), json!({"cli": true, "notifier": true}));
        // Antigravity on the same computer changes nothing in what is reported
        touch(&root.join("bin").join("agy"));
        assert_eq!(codex_state_in(&p), json!({"cli": true, "notifier": true}));
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
        let p = places(&root);
        let bin = root.join("bin");
        std::fs::create_dir_all(&p.codex_dir).unwrap();
        assert_eq!(worth_suggesting(&p), Some(Sister::Codex));

        // Its config folder
        std::fs::create_dir_all(&p.codex_notifier_dir).unwrap();
        assert_eq!(worth_suggesting(&p), None);
        std::fs::remove_dir_all(&p.codex_notifier_dir).unwrap();

        // Its binary, under either name
        for name in ["codex-notifier", "codexnotifier"] {
            touch(&bin.join(name));
            assert_eq!(worth_suggesting(&p), None, "{name}");
            std::fs::remove_file(bin.join(name)).unwrap();
        }

        // Its hooks in Codex's hooks.json
        std::fs::write(p.codex_dir.join("hooks.json"), r#"{"hooks":{"Stop":[]}}"#).unwrap();
        assert_eq!(worth_suggesting(&p), Some(Sister::Codex));
        std::fs::write(
            p.codex_dir.join("hooks.json"),
            r#"{"hooks":{"Stop":[{"command":"/usr/local/bin/codex-notifier hook"}]}}"#,
        )
        .unwrap();
        assert_eq!(worth_suggesting(&p), None);
    }

    #[test]
    fn antigravity_without_its_notifier_suggests() {
        // `agy` on PATH
        let root = temp_dir("agy-bin");
        let p = places(&root);
        touch(&root.join("bin").join("agy"));
        assert_eq!(worth_suggesting(&p), Some(Sister::Antigravity));

        // What Antigravity keeps in ~/.gemini
        for kept in ["antigravity-cli", "config"] {
            let root = temp_dir("agy-dir");
            let p = places(&root);
            std::fs::create_dir_all(p.gemini_dir.join(kept)).unwrap();
            assert_eq!(worth_suggesting(&p), Some(Sister::Antigravity), "{kept}");
        }
    }

    #[test]
    fn the_gemini_cli_alone_is_not_antigravity() {
        // ~/.gemini is the Gemini CLI's folder too: its own files say nothing about Antigravity
        let root = temp_dir("gemini-cli");
        let p = places(&root);
        std::fs::create_dir_all(p.gemini_dir.join("tmp")).unwrap();
        std::fs::write(p.gemini_dir.join("settings.json"), "{}").unwrap();
        touch(&root.join("bin").join("gemini"));
        assert_eq!(worth_suggesting(&p), None);
    }

    #[test]
    fn antigravity_notifier_already_here_suggests_nothing() {
        let root = temp_dir("agn-already");
        let p = places(&root);
        let bin = root.join("bin");
        touch(&bin.join("agy"));
        assert_eq!(worth_suggesting(&p), Some(Sister::Antigravity));

        // Its config folder
        std::fs::create_dir_all(&p.antigravity_notifier_dir).unwrap();
        assert_eq!(worth_suggesting(&p), None);
        std::fs::remove_dir_all(&p.antigravity_notifier_dir).unwrap();

        // Its binary
        touch(&bin.join("antigravity-notifier"));
        assert_eq!(worth_suggesting(&p), None);
        std::fs::remove_file(bin.join("antigravity-notifier")).unwrap();

        // Its group in Antigravity's hooks.json; someone else's hooks there don't count
        let hooks = p.gemini_dir.join("config").join("hooks.json");
        touch(&hooks);
        std::fs::write(
            &hooks,
            r#"{"policy":{"Stop":[{"command":"/x/policy.sh"}]}}"#,
        )
        .unwrap();
        assert_eq!(worth_suggesting(&p), Some(Sister::Antigravity));
        std::fs::write(
            &hooks,
            r#"{"antigravity-notifier":{"Stop":[{"type":"command","command":"/opt/homebrew/bin/antigravity-notifier hook Stop"}]}}"#,
        )
        .unwrap();
        assert_eq!(worth_suggesting(&p), None);
    }

    #[test]
    fn codex_comes_first_then_antigravity_and_never_both() {
        // Both agents, neither notifier: Notifier for Codex
        let root = temp_dir("both");
        let p = places(&root);
        std::fs::create_dir_all(&p.codex_dir).unwrap();
        touch(&root.join("bin").join("agy"));
        assert_eq!(worth_suggesting(&p), Some(Sister::Codex));
        // codex-notifier already here: Antigravity Notifier instead
        std::fs::create_dir_all(&p.codex_notifier_dir).unwrap();
        assert_eq!(worth_suggesting(&p), Some(Sister::Antigravity));

        // The SessionStart cap is shared: two mentions in all, whichever app they named
        let paired = 1_000_000_000;
        let mut cfg = Config::default();
        cfg.paired_at = Some(paired);
        assert_eq!(session_sister(&mut cfg, paired + DAY, &p), None);
        assert_eq!(
            session_sister(&mut cfg, paired + 3 * DAY, &p),
            Some(Sister::Antigravity)
        );
        assert_eq!(cfg.cross_promo_count, 1);
        assert_eq!(cfg.cross_promo_last_at, Some(paired + 3 * DAY));
        assert_eq!(session_sister(&mut cfg, paired + 4 * DAY, &p), None);
        assert_eq!(
            session_sister(&mut cfg, paired + 33 * DAY, &p),
            Some(Sister::Antigravity)
        );
        assert_eq!(session_sister(&mut cfg, paired + 400 * DAY, &p), None);
        assert_eq!(cfg.cross_promo_count, 2);

        // Someone who has already been told about Notifier for Codex twice is not told about
        // Antigravity Notifier at SessionStart as well
        let mut cfg = Config::default();
        cfg.paired_at = Some(paired);
        cfg.cross_promo_count = 2;
        cfg.cross_promo_last_at = Some(paired + 33 * DAY);
        assert_eq!(session_sister(&mut cfg, paired + 400 * DAY, &p), None);

        // Nothing to suggest: nothing counted
        let bare = temp_dir("both-bare");
        let mut cfg = Config::default();
        cfg.paired_at = Some(paired);
        assert_eq!(
            session_sister(&mut cfg, paired + 9 * DAY, &places(&bare)),
            None
        );
        assert_eq!(cfg.cross_promo_count, 0);
    }

    /// line, notification_body, pair_title, pair_scan, pair_steps in `lang`
    fn texts(sister: Sister, lang: &str) -> [String; 5] {
        let (command, url) = (sister.command(), sister.url());
        let all = match sister {
            Sister::Codex => [
                t!(
                    "crosspromo.line",
                    locale = lang,
                    command = command,
                    url = url
                ),
                t!("crosspromo.notification_body", locale = lang),
                t!("crosspromo.pair_title", locale = lang),
                t!("crosspromo.pair_scan", locale = lang),
                t!("crosspromo.pair_steps", locale = lang),
            ],
            Sister::Antigravity => [
                t!(
                    "crosspromo_antigravity.line",
                    locale = lang,
                    command = command,
                    url = url
                ),
                t!("crosspromo_antigravity.notification_body", locale = lang),
                t!("crosspromo_antigravity.pair_title", locale = lang),
                t!("crosspromo_antigravity.pair_scan", locale = lang),
                t!("crosspromo_antigravity.pair_steps", locale = lang),
            ],
        };
        all.map(|t| t.into_owned())
    }

    #[test]
    fn every_translation_promotes_the_app_with_one_command_and_no_one_platform() {
        for sister in Sister::ALL {
            // The other sister's name never appears
            let other = match sister {
                Sister::Codex => "Antigravity",
                Sister::Antigravity => "Codex",
            };
            for lang in config::LOCALES {
                let texts = texts(sister, lang);
                let line = &texts[0];
                assert!(
                    line.contains(sister.command()) && line.contains(sister.url()),
                    "{lang}: {line}"
                );
                for text in &texts[..3] {
                    assert!(text.contains(sister.app_name()), "{lang}: {text}");
                }
                for text in &texts {
                    for word in ["Mac", other] {
                        assert!(!text.contains(word), "{lang}: {text}");
                    }
                    // A missing translation comes back as its key
                    assert!(!text.contains("crosspromo"), "{lang}: {text}");
                }
            }
        }
    }

    #[test]
    fn the_command_installs_and_pairs_in_one_line() {
        let s = Sister::Codex;
        assert!(s.command().contains("notifierforcodex.com/install."));
        assert!(!s.command().contains("npm"));
        assert_eq!(
            s.pair_url(),
            "https://notifierforcodex.com/get?ref=ccn-pair"
        );
        assert!(qr::render(s.pair_url()).is_some());
    }

    /// Antigravity Notifier's phone apps were in store review when this shipped: everything the
    /// suggestion links to is the website, which no store link or /get redirect can stand in for.
    #[test]
    fn the_antigravity_suggestion_points_at_the_website_and_never_at_a_store() {
        let s = Sister::Antigravity;
        assert_eq!(
            t!(
                "crosspromo_antigravity.line",
                locale = "en",
                command = s.command(),
                url = s.url()
            ),
            format!(
                "Antigravity detected — get Antigravity approvals and questions on your phone too, with the Antigravity Notifier app (iPhone and Android). Set up this computer with one command: {}  ·  https://antigravitynotifier.com/?ref=ccn-cli",
                s.command()
            )
        );
        assert!(s.command().contains("antigravitynotifier.com/install."));
        assert!(!s.command().contains("npm"));
        assert_eq!(s.url(), "https://antigravitynotifier.com/?ref=ccn-cli");
        assert_eq!(
            s.pair_url(),
            "https://antigravitynotifier.com/?ref=ccn-pair"
        );
        assert_eq!(
            s.notification_url(),
            "https://antigravitynotifier.com/?ref=ccn-notification"
        );
        for url in [s.url(), s.pair_url(), s.notification_url()] {
            assert!(
                url.starts_with("https://antigravitynotifier.com/?ref="),
                "{url}"
            );
            for store in ["apple.com", "google.com", "/get", "/rate"] {
                assert!(!url.contains(store), "{url}");
            }
        }
        assert!(qr::render(s.pair_url()).is_some());
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
