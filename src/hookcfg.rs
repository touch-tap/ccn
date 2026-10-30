//! Installs, removes and inspects ccn's entries in Claude Code's user settings file.
//! Everything else in that file is left exactly as it was, including key order.

use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};

use crate::config;

/// Event, matcher, timeout in seconds. Blocking events get a timeout above the largest hold ceiling.
const EVENTS: &[(&str, Option<&str>, u64)] = &[
    ("SessionStart", None, 30),
    ("UserPromptSubmit", None, 10),
    ("PreToolUse", None, 3660),
    ("PermissionRequest", None, 3660),
    ("Notification", Some("permission_prompt"), 30),
    ("Stop", None, 700),
    ("SessionEnd", None, 10),
];

const MAC_APP_MARKER: &str = "ccnotifier-hook.sh";

pub struct InstallReport {
    pub path: PathBuf,
    pub exe: PathBuf,
    /// False when settings.json already had exactly these entries, so it was left untouched
    pub changed: bool,
    pub mac_app_hooks_found: bool,
    pub mac_app_hooks_removed: bool,
}

pub struct Status {
    pub path: PathBuf,
    pub installed: Vec<String>,
    pub missing: Vec<String>,
    pub stale_exe: bool,
    pub mac_app_present: bool,
}

/// The path the hooks call. It has to survive upgrades: a Homebrew Cellar path is versioned, so
/// it maps to the prefix's bin link, and a binary run from npx's cache (the npm wrapper sets
/// `CCN_EPHEMERAL=1`) is replaced by the copy `install_self` puts in the user's bin directory.
pub fn exe_path() -> Result<PathBuf> {
    if ephemeral() {
        return Ok(user_bin_dir().join(exe_name()));
    }
    let exe = std::env::current_exe().context("finding the ccn executable")?;
    let exe = exe.canonicalize().unwrap_or(exe);
    Ok(homebrew_bin(&exe).unwrap_or(exe))
}

fn ephemeral() -> bool {
    std::env::var("CCN_EPHEMERAL")
        .map(|v| v == "1")
        .unwrap_or(false)
}

fn exe_name() -> &'static str {
    if cfg!(windows) { "ccn.exe" } else { "ccn" }
}

/// Where the install scripts and `install_self` put the binary.
pub fn user_bin_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("CCN_INSTALL_DIR") {
        if !dir.is_empty() {
            return PathBuf::from(dir);
        }
    }
    #[cfg(windows)]
    {
        if let Ok(local) = std::env::var("LOCALAPPDATA") {
            if !local.is_empty() {
                return PathBuf::from(local).join("Programs").join("ccn");
            }
        }
    }
    config::home_dir().join(".local").join("bin")
}

/// `/opt/homebrew/Cellar/ccn/0.1.0/bin/ccn` → `/opt/homebrew/bin/ccn`, when that link exists.
fn homebrew_bin(exe: &Path) -> Option<PathBuf> {
    let s = exe.to_str()?;
    let (prefix, rest) = s.split_once("/Cellar/ccn/")?;
    let name = rest.rsplit('/').next()?;
    let link = Path::new(prefix).join("bin").join(name);
    link.exists().then_some(link)
}

/// Copies a binary running from npx's cache to the user's bin directory, so the hooks keep
/// working after the cache is cleared. Returns the copy's path when one was made.
pub fn install_self() -> Result<Option<PathBuf>> {
    if !ephemeral() {
        return Ok(None);
    }
    let from = std::env::current_exe().context("finding the ccn executable")?;
    let dir = user_bin_dir();
    fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let to = dir.join(exe_name());
    let tmp = dir.join(format!("{}.new", exe_name()));
    fs::copy(&from, &tmp).with_context(|| format!("copying ccn to {}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&tmp, fs::Permissions::from_mode(0o755))?;
    }
    fs::rename(&tmp, &to).with_context(|| format!("installing {}", to.display()))?;
    Ok(Some(to))
}

fn shell_quote(path: &Path) -> String {
    let s = path.to_string_lossy();
    if s.chars()
        .all(|c| c.is_ascii_alphanumeric() || "/._-+:@".contains(c))
    {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

fn hook_entry(exe: &Path, timeout: u64) -> Value {
    if cfg!(windows) {
        // Exec form: no shell needed, so it works with or without Git Bash
        json!({ "type": "command", "command": exe.to_string_lossy(), "args": ["hook"], "timeout": timeout })
    } else {
        json!({ "type": "command", "command": format!("{} hook", shell_quote(exe)), "timeout": timeout })
    }
}

fn command_of(entry: &Value) -> &str {
    entry.get("command").and_then(|c| c.as_str()).unwrap_or("")
}

pub fn is_ours(entry: &Value) -> bool {
    let command = command_of(entry);
    let args_hook = entry
        .get("args")
        .and_then(|a| a.as_array())
        .map(|a| a.len() == 1 && a[0] == "hook")
        .unwrap_or(false);
    let base = Path::new(command.trim_end_matches(" hook").trim_matches('\''))
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_default();
    let is_ccn_binary = base == "ccn" || base == "ccn.exe";
    (command.ends_with(" hook") && is_ccn_binary) || (args_hook && is_ccn_binary)
}

fn is_mac_app(entry: &Value) -> bool {
    command_of(entry).contains(MAC_APP_MARKER)
}

fn load(path: &Path) -> Result<Value> {
    if !path.exists() {
        return Ok(json!({}));
    }
    let text = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    if text.trim().is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_str(&text).with_context(|| format!("{} is not valid JSON", path.display()))
}

fn save(path: &Path, root: &Value) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    if path.exists() {
        let backup = path.with_extension("json.ccn-backup");
        if !backup.exists() {
            let _ = fs::copy(path, &backup);
        }
    }
    let text = serde_json::to_string_pretty(root)?;
    fs::write(path, format!("{text}\n")).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

/// Removes ccn's entries (and the Mac app's, when asked) from every event. Drops groups left empty.
fn strip(hooks: &mut serde_json::Map<String, Value>, remove_mac_app: bool) -> (bool, bool) {
    let mut found_mac = false;
    let mut removed_mac = false;
    for (_event, groups) in hooks.iter_mut() {
        let Some(groups) = groups.as_array_mut() else {
            continue;
        };
        for group in groups.iter_mut() {
            let Some(list) = group.get_mut("hooks").and_then(|h| h.as_array_mut()) else {
                continue;
            };
            list.retain(|entry| {
                if is_ours(entry) {
                    return false;
                }
                if is_mac_app(entry) {
                    found_mac = true;
                    if remove_mac_app {
                        removed_mac = true;
                        return false;
                    }
                }
                true
            });
        }
        groups.retain(|g| {
            g.get("hooks")
                .and_then(|h| h.as_array())
                .map(|l| !l.is_empty())
                .unwrap_or(true)
        });
    }
    hooks.retain(|_, groups| groups.as_array().map(|g| !g.is_empty()).unwrap_or(true));
    (found_mac, removed_mac)
}

pub fn install(replace_mac_app: bool) -> Result<InstallReport> {
    let path = config::claude_settings_path();
    let exe = exe_path()?;
    let before = load(&path)?;
    let mut root = before.clone();
    let obj = root
        .as_object_mut()
        .context("settings.json is not a JSON object")?;
    let hooks = obj.entry("hooks").or_insert_with(|| json!({}));
    let hooks = hooks
        .as_object_mut()
        .context("the hooks section of settings.json is not an object")?;

    let (found_mac, removed_mac) = strip(hooks, replace_mac_app);

    for (event, matcher, timeout) in EVENTS {
        let groups = hooks.entry(*event).or_insert_with(|| json!([]));
        let groups = groups
            .as_array_mut()
            .with_context(|| format!("hooks.{event} is not an array"))?;
        let mut group = json!({ "hooks": [hook_entry(&exe, *timeout)] });
        if let Some(m) = matcher {
            group["matcher"] = json!(m);
        }
        groups.push(group);
    }

    // Idempotent: a second run finds its own entries at the end of each list and writes nothing
    let changed = root != before;
    if changed {
        save(&path, &root)?;
    }
    Ok(InstallReport {
        path,
        exe,
        changed,
        mac_app_hooks_found: found_mac,
        mac_app_hooks_removed: removed_mac,
    })
}

pub fn uninstall() -> Result<PathBuf> {
    let path = config::claude_settings_path();
    if !path.exists() {
        return Ok(path);
    }
    let mut root = load(&path)?;
    if let Some(hooks) = root.get_mut("hooks").and_then(|h| h.as_object_mut()) {
        strip(hooks, false);
    }
    // An empty hooks section is almost always one ccn created, so leave the file as it was before
    let empty = root
        .get("hooks")
        .and_then(|h| h.as_object())
        .is_some_and(|h| h.is_empty());
    if empty {
        root.as_object_mut().map(|o| o.remove("hooks"));
    }
    save(&path, &root)?;
    Ok(path)
}

/// Whether settings.json already sends Claude Code's events somewhere: ccn's own entries, or the
/// Mac app's. The plugin's hooks stand aside then, so no event is sent twice.
pub fn settings_has_hooks() -> bool {
    let Ok(root) = load(&config::claude_settings_path()) else {
        return false;
    };
    let Some(hooks) = root.get("hooks").and_then(|h| h.as_object()) else {
        return false;
    };
    hooks
        .values()
        .filter_map(|g| g.as_array())
        .flatten()
        .any(|group| {
            group
                .get("hooks")
                .and_then(|h| h.as_array())
                .map(|list| list.iter().any(|entry| is_ours(entry) || is_mac_app(entry)))
                .unwrap_or(false)
        })
}

pub fn status() -> Result<Status> {
    let path = config::claude_settings_path();
    let root = load(&path)?;
    let exe = exe_path()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();
    let mut installed = Vec::new();
    let mut missing = Vec::new();
    let mut stale_exe = false;
    let mut mac_app_present = false;

    let hooks = root.get("hooks").and_then(|h| h.as_object());
    for (event, matcher, _) in EVENTS {
        let mut present = false;
        if let Some(groups) = hooks.and_then(|h| h.get(*event)).and_then(|g| g.as_array()) {
            for group in groups {
                let group_matcher = group.get("matcher").and_then(|m| m.as_str());
                let matcher_ok = match matcher {
                    Some(m) => group_matcher == Some(m),
                    None => group_matcher
                        .map(|m| m.is_empty() || m == "*")
                        .unwrap_or(true),
                };
                let Some(list) = group.get("hooks").and_then(|h| h.as_array()) else {
                    continue;
                };
                for entry in list {
                    if is_mac_app(entry) {
                        mac_app_present = true;
                    }
                    if matcher_ok && is_ours(entry) {
                        present = true;
                        if !exe.is_empty() && !command_of(entry).contains(&exe) {
                            stale_exe = true;
                        }
                    }
                }
            }
        }
        let label = match matcher {
            Some(m) => format!("{event} ({m})"),
            None => event.to_string(),
        };
        if present {
            installed.push(label);
        } else {
            missing.push(label);
        }
    }
    // The Mac app's hooks live on events we do not use too
    if let Some(hooks) = hooks {
        for (_, groups) in hooks {
            if let Some(groups) = groups.as_array() {
                for group in groups {
                    if let Some(list) = group.get("hooks").and_then(|h| h.as_array()) {
                        if list.iter().any(is_mac_app) {
                            mac_app_present = true;
                        }
                    }
                }
            }
        }
    }
    Ok(Status {
        path,
        installed,
        missing,
        stale_exe,
        mac_app_present,
    })
}

#[cfg(test)]
mod exe_tests {
    use super::*;

    #[test]
    fn installing_twice_changes_nothing_the_second_time() {
        let dir = std::env::temp_dir().join(format!("ccn-hooks-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        fs::write(&path, r#"{"permissions":{"allow":["Bash(ls)"]}}"#).unwrap();
        // The hooks must name a binary called ccn, which the test binary isn't: point them at the
        // path an npx install would copy to
        unsafe {
            std::env::set_var("CLAUDE_CONFIG_DIR", &dir);
            std::env::set_var("CCN_EPHEMERAL", "1");
            std::env::set_var("CCN_INSTALL_DIR", &dir);
        }
        let first = install(false).unwrap();
        assert!(first.changed);
        let written = fs::read_to_string(&path).unwrap();
        let second = install(false).unwrap();
        assert!(!second.changed);
        assert_eq!(fs::read_to_string(&path).unwrap(), written);
        assert!(written.contains("Bash(ls)"));
        unsafe {
            std::env::remove_var("CLAUDE_CONFIG_DIR");
            std::env::remove_var("CCN_EPHEMERAL");
            std::env::remove_var("CCN_INSTALL_DIR");
        }
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    #[cfg(unix)] // Homebrew paths are Unix paths
    fn homebrew_cellar_maps_to_the_bin_link() {
        let root = std::env::temp_dir().join(format!("ccn-brew-{}", std::process::id()));
        let cellar = root.join("Cellar/ccn/0.1.0/bin");
        fs::create_dir_all(&cellar).unwrap();
        fs::create_dir_all(root.join("bin")).unwrap();
        fs::write(cellar.join("ccn"), "").unwrap();
        fs::write(root.join("bin/ccn"), "").unwrap();
        assert_eq!(
            homebrew_bin(&cellar.join("ccn")),
            Some(root.join("bin/ccn"))
        );
        assert_eq!(homebrew_bin(&root.join("elsewhere/ccn")), None);
        fs::remove_dir_all(root).ok();
    }
}
