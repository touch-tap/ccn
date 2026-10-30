//! `ccn upgrade`: replaces this binary with the latest release, or says how to update when a
//! package manager owns it (Homebrew, npm, scoop, winget). Also the once-a-day check behind it.
//!
//! Wiring for the SessionStart hook (hook.rs): call `update::daily_check(&mut self.cfg)` once per
//! SessionStart, after the config is loaded. It returns within ~100 ms and never touches the
//! network itself. `None` means nothing to say. At most once a day it saves `update_checked_at`
//! and spawns `ccn upgrade --quiet --background`, fully detached from the hook and the terminal,
//! which installs a newer release in place when ccn owns its binary. When something else owns it
//! the background run records `update_available` instead, and `daily_check` then returns
//! `Some({"systemMessage": "Claude Code Notifier: ccn X is available: <command>"})` once a week,
//! for the hook to return as its output. `ccn config set auto_update off` or
//! `CCN_NO_AUTO_UPDATE=1` turns all of this off.
//!
//! Where the latest version comes from: `GET {server}/api/cli/latest`, falling back to the GitHub
//! releases API. The assets are the ones .github/workflows/release.yml publishes under the
//! release's `downloadBase`: `ccn-<triple>.tar.gz` (Unix) or `ccn-<triple>.zip` (Windows), plus
//! `SHA256SUMS`. The download is checked against SHA256SUMS before anything is written, the new
//! binary is written beside the old one and must run before it is renamed over it, so a hook
//! running mid-update never sees a half-written file.

use anyhow::{Context, Result, bail};
use rust_i18n::t;
use serde_json::{Value, json};
use std::cmp::Ordering;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use crate::config::{self, Config, VERSION};
use crate::logger::log;
use crate::{hookcfg, telemetry};

/// How often the SessionStart hook starts a background check.
const CHECK_EVERY: i64 = 24 * 60 * 60;
/// How often Claude Code is told about a version a package manager has to install.
const ANNOUNCE_EVERY: i64 = 7 * 24 * 60 * 60;
/// `ccn upgrade --check` exits with this when a newer release exists.
const EXIT_UPDATE_AVAILABLE: i32 = 10;
const GITHUB_LATEST: &str = "https://api.github.com/repos/touch-tap/ccn/releases/latest";
const RELEASES_BASE: &str = "https://github.com/touch-tap/ccn/releases/download";
/// Release archives are about 2 MB; anything near this is not one of ours.
const MAX_DOWNLOAD: u64 = 64 * 1024 * 1024;

pub struct Mode {
    /// Only report whether a newer release exists
    pub check: bool,
    /// Print nothing on success
    pub quiet: bool,
    /// Started by `daily_check`: never prints, logs to ccn.log, records what it cannot install
    pub background: bool,
}

struct Latest {
    version: String,
    download_base: String,
    /// The asset names the release carries, when the source lists them
    assets: Option<Vec<String>>,
}

/// What `ccn upgrade` can do for an installed binary.
#[derive(Debug, PartialEq)]
pub enum Action {
    /// Download the release and swap it in at this path
    Replace(PathBuf),
    /// A package manager owns the binary: tell the user to run this
    Run(&'static str),
}

// ============================================
// Commands
// ============================================

pub fn run(mode: Mode) -> Result<()> {
    match run_inner(&mode) {
        Ok(0) => Ok(()),
        Ok(code) => std::process::exit(code),
        Err(err) if mode.background => {
            log(&format!("update: {err:#}"));
            Ok(())
        }
        Err(err) => Err(err),
    }
}

fn run_inner(mode: &Mode) -> Result<i32> {
    let mut cfg = Config::load();
    let say = |text: String| {
        if mode.background {
            log(&text);
        } else if !mode.quiet {
            println!("{text}");
        }
    };

    let latest = fetch_latest(&cfg.server_url).map_err(|e| failed("check", e))?;
    if !is_newer(&latest.version, VERSION) {
        if mode.background && cfg.update_available.is_some() {
            cfg.update_available = None;
            let _ = cfg.save();
        }
        say(t!("update.latest", version = VERSION).into_owned());
        return Ok(0);
    }

    let exe = hookcfg::exe_path().map_err(|e| failed("check", e))?;
    let method = detect_method(&exe);
    let action = action_for(method, &exe);

    if mode.check {
        if !mode.quiet && !mode.background {
            println!(
                "{}",
                t!(
                    "update.available_check",
                    version = latest.version,
                    current = VERSION
                )
            );
            println!(
                "{}",
                t!("update.update_with", command = update_command(&action))
            );
        }
        return Ok(EXIT_UPDATE_AVAILABLE);
    }

    let target = match action {
        Action::Run(command) => {
            if mode.background {
                cfg.update_available = Some(latest.version.clone());
                let _ = cfg.save();
                log(&format!(
                    "ccn {} is available, installed with {method}: {command}",
                    latest.version
                ));
            } else {
                // Not a success, so --quiet still says it
                println!(
                    "{}",
                    t!(
                        "update.available_run",
                        version = latest.version,
                        command = command
                    )
                );
            }
            return Ok(0);
        }
        Action::Replace(target) => target,
    };
    if !target.exists() {
        bail!(
            "{}",
            t!(
                "update.nothing_to_update",
                path = target.display(),
                npx = if method == "npx" {
                    t!("update.npx_note")
                } else {
                    "".into()
                }
            )
        );
    }

    let asset = this_asset()
        .with_context(|| {
            t!(
                "update.no_release_build",
                os = std::env::consts::OS,
                arch = std::env::consts::ARCH
            )
            .into_owned()
        })
        .map_err(|e| failed("check", e))?;
    if let Some(assets) = &latest.assets
        && !assets.iter().any(|a| a == &asset)
    {
        return Err(failed(
            "check",
            anyhow::anyhow!(
                "{}",
                t!(
                    "update.no_build_named",
                    version = latest.version,
                    asset = asset
                )
            ),
        ));
    }

    let base = latest.download_base.trim_end_matches('/');
    let archive = download(&format!("{base}/{asset}")).map_err(|e| failed("download", e))?;
    let sums = download(&format!("{base}/SHA256SUMS")).map_err(|e| failed("download", e))?;
    let expected = checksum_for(&String::from_utf8_lossy(&sums), &asset)
        .with_context(|| t!("update.no_checksum", asset = asset).into_owned())
        .map_err(|e| failed("checksum", e))?;
    let actual = sha256_hex(&archive);
    if actual != expected {
        return Err(failed(
            "checksum",
            anyhow::anyhow!("{}", t!("update.checksum_mismatch", asset = asset)),
        ));
    }

    let binary = extract_binary(&archive).map_err(|e| failed("unpack", e))?;
    install_binary(&binary, &target).map_err(|e| failed("install", e))?;

    // The hooks point at this same path, so this only does anything if they were somewhere else
    if hookcfg::status().map(|s| s.stale_exe).unwrap_or(false)
        && let Err(err) = hookcfg::install(false)
    {
        log(&format!("update: reinstalling the hooks failed: {err:#}"));
    }

    telemetry::event(
        "cli_updated",
        &[
            ("from", VERSION),
            ("to", &latest.version),
            ("method", method),
        ],
    );
    if mode.background && cfg.update_available.is_some() {
        cfg.update_available = None;
        let _ = cfg.save();
    }
    say(t!("update.updated", from = VERSION, to = latest.version).into_owned());
    Ok(0)
}

/// Posts the failure stage and hands the error back.
fn failed(stage: &str, err: anyhow::Error) -> anyhow::Error {
    telemetry::event("cli_update_failed", &[("stage", stage)]);
    err
}

fn update_command(action: &Action) -> &'static str {
    match action {
        Action::Run(command) => command,
        Action::Replace(_) => "ccn upgrade",
    }
}

/// `ccn config set auto_update on|off`, kept here so commands.rs needs no change.
pub fn set_auto_update(value: &str) -> Result<()> {
    let on = match value.to_ascii_lowercase().as_str() {
        "true" | "on" | "yes" | "1" => true,
        "false" | "off" | "no" | "0" => false,
        _ => bail!("{}", t!("config.auto_update_expects")),
    };
    let mut cfg = Config::load();
    cfg.auto_update = on;
    cfg.save()?;
    println!(
        "auto_update                {} {}",
        if on { "on" } else { "off" },
        t!("config.local_only")
    );
    Ok(())
}

/// On Windows a running ccn.exe is renamed to ccn.old.exe by an update, and can only be deleted
/// once it has exited: every start tries. Nothing to do elsewhere.
pub fn remove_leftovers() {
    #[cfg(windows)]
    if let Ok(exe) = std::env::current_exe() {
        let _ = fs::remove_file(exe.with_extension("old.exe"));
    }
}

// ============================================
// The daily check, called from the SessionStart hook
// ============================================

/// What a SessionStart should do now: start a background check, mention a version that needs a
/// package manager, both or neither.
fn next_step(cfg: &Config, now: i64) -> (bool, Option<String>) {
    let check = now - cfg.update_checked_at.unwrap_or(0) >= CHECK_EVERY;
    let announce = cfg
        .update_available
        .clone()
        .filter(|v| is_newer(v, VERSION))
        .filter(|_| now - cfg.update_announced_at.unwrap_or(0) >= ANNOUNCE_EVERY);
    (check, announce)
}

fn auto_update_enabled(cfg: &Config) -> bool {
    let off = std::env::var("CCN_NO_AUTO_UPDATE")
        .map(|v| !v.is_empty() && v != "0")
        .unwrap_or(false);
    cfg.auto_update && !off
}

/// Once a day, starts `ccn upgrade --quiet --background` detached from the hook, and once a week
/// returns a `systemMessage` for a newer version that a package manager has to install. Returns
/// within ~100 ms: it never waits for the network. Called from the SessionStart hook.
pub fn daily_check(cfg: &mut Config) -> Option<Value> {
    if !auto_update_enabled(cfg) {
        return None;
    }
    let now = config::now();
    let (check, announce) = next_step(cfg, now);
    if check {
        cfg.update_checked_at = Some(now);
        // Saved before the child starts, so its own save (seconds later) is not overwritten
        match cfg.save() {
            Ok(()) => spawn_background(),
            Err(err) => log(&format!("update check skipped: {err:#}")),
        }
    }
    let version = announce?;
    let exe = hookcfg::exe_path().ok()?;
    let command = update_command(&action_for(detect_method(&exe), &exe));
    cfg.update_announced_at = Some(now);
    let _ = cfg.save();
    Some(json!({
        "systemMessage": t!("update.available_message", version = version, command = command)
    }))
}

/// Runs this same binary as `ccn upgrade --quiet --background`, with no terminal, no standard
/// streams and its own session, so neither Claude Code nor the shell waits for it or stops it.
fn spawn_background() {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let mut command = Command::new(exe);
    command
        .args(["upgrade", "--quiet", "--background"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: setsid only touches this child's own session and process group
        unsafe {
            command.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        use windows_sys::Win32::System::Threading::{CREATE_NO_WINDOW, DETACHED_PROCESS};
        command.creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS);
    }
    if let Err(err) = command.spawn() {
        log(&format!("update check could not start: {err}"));
    }
}

// ============================================
// Where the binary came from, and what that allows
// ============================================

/// "homebrew", "npm", "scoop", "winget" from the binary's path; otherwise what the install
/// scripts and npx told telemetry ("script", "powershell", "npx") or "other".
fn detect_method(exe: &Path) -> &'static str {
    method_from_path(exe).unwrap_or_else(telemetry::install_method)
}

fn method_from_path(exe: &Path) -> Option<&'static str> {
    let path = exe.to_string_lossy().replace('\\', "/");
    let lower = path.to_ascii_lowercase();
    if path.contains("/Cellar/") || path.contains("/homebrew/") || path.contains("linuxbrew") {
        Some("homebrew")
    } else if lower.contains("/scoop/apps/ccn/") {
        Some("scoop")
    } else if lower.contains("/winget/packages/") {
        Some("winget")
    } else if path.contains("node_modules") {
        Some("npm")
    } else {
        None
    }
}

/// What to do for a binary installed by `method` at `exe`.
pub fn action_for(method: &str, exe: &Path) -> Action {
    match method {
        "homebrew" => Action::Run("brew upgrade touch-tap/tap/ccn"),
        "npm" => Action::Run("npm install -g @touchtap/ccn@latest"),
        "scoop" => Action::Run("scoop update ccn"),
        "winget" => Action::Run("winget upgrade TouchTap.ccn"),
        _ => Action::Replace(exe.to_path_buf()),
    }
}

// ============================================
// Versions and assets
// ============================================

/// "v0.3.0", "0.3.0" or "0.3.0-beta.1" → (0, 3, 0, Some("beta.1")). Build metadata is ignored.
fn parse_version(text: &str) -> Option<(u64, u64, u64, Option<String>)> {
    let text = text.trim().trim_start_matches('v');
    let text = text.split_once('+').map(|(v, _)| v).unwrap_or(text);
    let (core, pre) = match text.split_once('-') {
        Some((core, pre)) => (core, Some(pre.to_string())),
        None => (text, None),
    };
    let mut parts = core.split('.').map(|p| p.parse::<u64>().ok());
    let major = parts.next()??;
    let minor = parts.next().unwrap_or(Some(0))?;
    let patch = parts.next().unwrap_or(Some(0))?;
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch, pre))
}

/// Semver order: numbers first, then a release outranks any pre-release of the same number.
fn compare_versions(a: &str, b: &str) -> Option<Ordering> {
    let (a, b) = (parse_version(a)?, parse_version(b)?);
    let order = (a.0, a.1, a.2).cmp(&(b.0, b.1, b.2));
    if order != Ordering::Equal {
        return Some(order);
    }
    Some(match (&a.3, &b.3) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        (Some(x), Some(y)) => x.cmp(y),
    })
}

/// Whether `candidate` is a newer release than `current`. Anything unparsable is not.
fn is_newer(candidate: &str, current: &str) -> bool {
    compare_versions(candidate, current) == Some(Ordering::Greater)
}

/// The release asset for an OS and processor, named as release.yml names them.
fn asset_name(os: &str, arch: &str) -> Option<String> {
    if !matches!(arch, "x86_64" | "aarch64") {
        return None;
    }
    Some(match os {
        "macos" => format!("ccn-{arch}-apple-darwin.tar.gz"),
        "linux" => format!("ccn-{arch}-unknown-linux-musl.tar.gz"),
        "windows" => format!("ccn-{arch}-pc-windows-msvc.zip"),
        _ => return None,
    })
}

fn this_asset() -> Option<String> {
    asset_name(std::env::consts::OS, std::env::consts::ARCH)
}

/// The hash for `asset` in a `sha256sum` listing (`<hex>  <name>`, or `<hex> *<name>`).
fn checksum_for(sums: &str, asset: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        let hash = parts.next()?;
        let name = parts.next()?.trim_start_matches('*');
        (name == asset && hash.len() == 64).then(|| hash.to_ascii_lowercase())
    })
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    let digest = sha2::Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

// ============================================
// Network
// ============================================

fn agent(timeout: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .http_status_as_error(false)
        .user_agent(format!("ccn/{VERSION}"))
        .build()
        .new_agent()
}

fn get_json(url: &str, timeout: Duration) -> Result<Value> {
    let mut response = agent(timeout)
        .get(url)
        .header("Accept", "application/json")
        .call()
        .with_context(|| format!("GET {url}"))?;
    let status = response.status().as_u16();
    if status != 200 {
        bail!("GET {url}: HTTP {status}");
    }
    response
        .body_mut()
        .read_json::<Value>()
        .with_context(|| format!("GET {url}: not JSON"))
}

fn download(url: &str) -> Result<Vec<u8>> {
    let mut response = agent(Duration::from_secs(120))
        .get(url)
        .call()
        .with_context(|| format!("GET {url}"))?;
    let status = response.status().as_u16();
    if status != 200 {
        bail!("GET {url}: HTTP {status}");
    }
    response
        .body_mut()
        .with_config()
        .limit(MAX_DOWNLOAD)
        .read_to_vec()
        .with_context(|| format!("GET {url}: reading the body"))
}

/// The server first (it knows about every published release), GitHub if it cannot say.
fn fetch_latest(server_url: &str) -> Result<Latest> {
    let timeout = Duration::from_secs(10);
    let url = format!("{}/api/cli/latest", server_url.trim_end_matches('/'));
    let from_server = get_json(&url, timeout).and_then(|v| latest_from_server(&v));
    match from_server {
        Ok(latest) => Ok(latest),
        Err(server_err) => {
            log(&format!(
                "update: {server_err:#}; asking GitHub for the latest release"
            ));
            get_json(GITHUB_LATEST, timeout)
                .and_then(|v| latest_from_github(&v))
                .context("could not find the latest release")
        }
    }
}

fn latest_from_server(v: &Value) -> Result<Latest> {
    let version = v["version"]
        .as_str()
        .or_else(|| v["tag"].as_str())
        .context("the server's reply has no version")?
        .trim_start_matches('v')
        .to_string();
    let download_base = match v["downloadBase"].as_str() {
        Some(base) => base.to_string(),
        None => format!(
            "{RELEASES_BASE}/{}",
            v["tag"]
                .as_str()
                .map(str::to_string)
                .unwrap_or_else(|| format!("v{version}"))
        ),
    };
    Ok(Latest {
        version,
        download_base,
        assets: asset_list(&v["assets"], |a| a.as_str()),
    })
}

fn latest_from_github(v: &Value) -> Result<Latest> {
    let tag = v["tag_name"]
        .as_str()
        .context("GitHub's reply has no tag_name")?;
    Ok(Latest {
        version: tag.trim_start_matches('v').to_string(),
        download_base: format!("{RELEASES_BASE}/{tag}"),
        assets: asset_list(&v["assets"], |a| a["name"].as_str()),
    })
}

fn asset_list(list: &Value, name: impl Fn(&Value) -> Option<&str>) -> Option<Vec<String>> {
    let names: Vec<String> = list
        .as_array()?
        .iter()
        .filter_map(|a| name(a).map(str::to_string))
        .collect();
    (!names.is_empty()).then_some(names)
}

// ============================================
// Unpacking and installing
// ============================================

/// The `ccn` (or `ccn.exe`) inside a release archive.
#[cfg(unix)]
fn extract_binary(archive: &[u8]) -> Result<Vec<u8>> {
    use std::io::Read;
    let gz = flate2::read::GzDecoder::new(archive);
    let mut tar = tar::Archive::new(gz);
    for entry in tar.entries().context("reading the archive")? {
        let mut entry = entry.context("reading the archive")?;
        let is_ccn = entry
            .path()
            .ok()
            .and_then(|p| p.file_name().map(|n| n == "ccn"))
            .unwrap_or(false);
        if is_ccn && entry.header().entry_type().is_file() {
            let mut bytes = Vec::with_capacity(entry.size() as usize);
            entry.read_to_end(&mut bytes).context("unpacking ccn")?;
            return Ok(bytes);
        }
    }
    bail!("{}", t!("update.archive_empty"))
}

#[cfg(windows)]
fn extract_binary(archive: &[u8]) -> Result<Vec<u8>> {
    use std::io::Read;
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(archive)).context("reading the zip")?;
    let name = (0..zip.len())
        .filter_map(|i| zip.by_index(i).ok().map(|f| f.name().to_string()))
        .find(|n| {
            Path::new(n)
                .file_name()
                .map(|f| f.eq_ignore_ascii_case("ccn.exe"))
                .unwrap_or(false)
        })
        .with_context(|| t!("update.zip_empty").into_owned())?;
    let mut file = zip.by_name(&name).context("unpacking ccn.exe")?;
    let mut bytes = Vec::with_capacity(file.size() as usize);
    file.read_to_end(&mut bytes).context("unpacking ccn.exe")?;
    Ok(bytes)
}

/// Writes `<target>.new` beside the target, checks it runs, then swaps it in. On any failure the
/// `.new` file is removed and the target is as it was.
fn install_binary(bytes: &[u8], target: &Path) -> Result<()> {
    let dir = target
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "ccn".to_string());
    let tmp = dir.join(format!("{name}.new"));
    let result = write_and_swap(bytes, &tmp, target);
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

fn write_and_swap(bytes: &[u8], tmp: &Path, target: &Path) -> Result<()> {
    {
        use std::io::Write;
        let mut file =
            fs::File::create(tmp).with_context(|| format!("cannot write to {}", tmp.display()))?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(tmp, fs::Permissions::from_mode(0o755))?;
    }
    let runs = Command::new(tmp)
        .arg("--version")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map(|o| o.status.success() && !o.stdout.is_empty())
        .unwrap_or(false);
    if !runs {
        bail!("{}", t!("update.did_not_run"));
    }
    swap(tmp, target)
}

#[cfg(unix)]
fn swap(tmp: &Path, target: &Path) -> Result<()> {
    fs::rename(tmp, target).with_context(|| format!("replacing {}", target.display()))
}

/// A running exe cannot be overwritten but can be renamed: the old one moves aside and is
/// deleted by `remove_leftovers` on the next start (or now, if nothing is running it).
#[cfg(windows)]
fn swap(tmp: &Path, target: &Path) -> Result<()> {
    let old = target.with_extension("old.exe");
    let _ = fs::remove_file(&old);
    if target.exists() {
        fs::rename(target, &old).with_context(|| format!("moving {} aside", target.display()))?;
    }
    if let Err(err) = fs::rename(tmp, target) {
        let _ = fs::rename(&old, target);
        return Err(err).with_context(|| format!("replacing {}", target.display()));
    }
    let _ = fs::remove_file(&old);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_as_semver() {
        assert!(is_newer("0.3.0", "0.2.0"));
        assert!(is_newer("v0.3.0", "0.2.9"));
        assert!(is_newer("1.0.0", "0.99.99"));
        assert!(is_newer("0.2.10", "0.2.9"), "numeric, not lexical");
        assert!(!is_newer("0.2.0", "0.2.0"));
        assert!(!is_newer("0.1.9", "0.2.0"));
        assert!(!is_newer("0.3.0-beta.1", "0.3.0"), "a pre-release is older");
        assert!(is_newer("0.3.0", "0.3.0-beta.1"));
        assert!(is_newer("0.3.0-beta.2", "0.3.0-beta.1"));
        assert!(
            is_newer("0.3.0+build.5", "0.2.0"),
            "build metadata is ignored"
        );
        assert!(!is_newer("latest", "0.2.0"), "garbage is never newer");
        assert!(!is_newer("0.3", "0.2.0.1"));
        assert!(is_newer("0.3", "0.2.0"), "a short version is padded");
    }

    #[test]
    fn asset_names_match_the_release_workflow() {
        assert_eq!(
            asset_name("macos", "aarch64").unwrap(),
            "ccn-aarch64-apple-darwin.tar.gz"
        );
        assert_eq!(
            asset_name("macos", "x86_64").unwrap(),
            "ccn-x86_64-apple-darwin.tar.gz"
        );
        assert_eq!(
            asset_name("linux", "x86_64").unwrap(),
            "ccn-x86_64-unknown-linux-musl.tar.gz"
        );
        assert_eq!(
            asset_name("linux", "aarch64").unwrap(),
            "ccn-aarch64-unknown-linux-musl.tar.gz"
        );
        assert_eq!(
            asset_name("windows", "x86_64").unwrap(),
            "ccn-x86_64-pc-windows-msvc.zip"
        );
        assert_eq!(
            asset_name("windows", "aarch64").unwrap(),
            "ccn-aarch64-pc-windows-msvc.zip"
        );
        assert_eq!(asset_name("linux", "riscv64"), None);
        assert_eq!(asset_name("freebsd", "x86_64"), None);
        assert!(this_asset().is_some(), "this test host has a release build");
    }

    #[test]
    fn sha256sums_lines_are_looked_up_by_name() {
        let sums = "\
0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef  ccn-aarch64-apple-darwin.tar.gz
ABCDEF0123456789abcdef0123456789abcdef0123456789abcdef0123456789 *ccn-x86_64-pc-windows-msvc.zip
deadbeef  ccn-x86_64-apple-darwin.tar.gz
";
        assert_eq!(
            checksum_for(sums, "ccn-aarch64-apple-darwin.tar.gz").as_deref(),
            Some("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef")
        );
        assert_eq!(
            checksum_for(sums, "ccn-x86_64-pc-windows-msvc.zip").as_deref(),
            Some("abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789"),
            "the binary-mode star is dropped and the hash lowercased"
        );
        assert_eq!(
            checksum_for(sums, "ccn-x86_64-apple-darwin.tar.gz"),
            None,
            "a short hash is not a sha256"
        );
        assert_eq!(
            checksum_for(sums, "ccn-aarch64-unknown-linux-musl.tar.gz"),
            None
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn install_methods_map_to_actions() {
        let p = |s: &str| PathBuf::from(s);
        assert_eq!(
            method_from_path(&p("/opt/homebrew/Cellar/ccn/0.2.0/bin/ccn")),
            Some("homebrew")
        );
        assert_eq!(
            method_from_path(&p("/home/linuxbrew/.linuxbrew/bin/ccn")),
            Some("homebrew")
        );
        assert_eq!(
            method_from_path(&p(
                "/usr/local/lib/node_modules/@touchtap/ccn-darwin-arm64/bin/ccn"
            )),
            Some("npm")
        );
        assert_eq!(
            method_from_path(&p(r"C:\Users\jack\scoop\apps\ccn\current\ccn.exe")),
            Some("scoop")
        );
        assert_eq!(
            method_from_path(&p(
                r"C:\Users\jack\AppData\Local\Microsoft\WinGet\Packages\TouchTap.ccn_x\ccn.exe"
            )),
            Some("winget")
        );
        assert_eq!(method_from_path(&p("/Users/jack/.local/bin/ccn")), None);
        assert_eq!(
            method_from_path(&p(r"C:\Users\jack\AppData\Local\Programs\ccn\ccn.exe")),
            None
        );

        let exe = p("/Users/jack/.local/bin/ccn");
        assert_eq!(
            action_for("homebrew", &exe),
            Action::Run("brew upgrade touch-tap/tap/ccn")
        );
        assert_eq!(
            action_for("npm", &exe),
            Action::Run("npm install -g @touchtap/ccn@latest")
        );
        assert_eq!(action_for("scoop", &exe), Action::Run("scoop update ccn"));
        assert_eq!(
            action_for("winget", &exe),
            Action::Run("winget upgrade TouchTap.ccn")
        );
        for method in ["script", "powershell", "npx", "other"] {
            assert_eq!(action_for(method, &exe), Action::Replace(exe.clone()));
        }
    }

    #[test]
    fn the_server_and_github_replies_both_give_a_download_base() {
        let server = json!({
            "version": "0.3.0", "tag": "v0.3.0", "publishedAt": 1761800000,
            "downloadBase": "https://github.com/touch-tap/ccn/releases/download/v0.3.0",
            "assets": ["ccn-aarch64-apple-darwin.tar.gz", "SHA256SUMS"]
        });
        let latest = latest_from_server(&server).unwrap();
        assert_eq!(latest.version, "0.3.0");
        assert_eq!(
            latest.download_base,
            "https://github.com/touch-tap/ccn/releases/download/v0.3.0"
        );
        assert_eq!(latest.assets.unwrap().len(), 2);

        let bare = latest_from_server(&json!({ "version": "0.4.0" })).unwrap();
        assert_eq!(
            bare.download_base,
            "https://github.com/touch-tap/ccn/releases/download/v0.4.0"
        );
        assert!(bare.assets.is_none());

        let github = json!({
            "tag_name": "v0.3.1",
            "assets": [{ "name": "ccn-x86_64-pc-windows-msvc.zip" }, { "name": "SHA256SUMS" }]
        });
        let latest = latest_from_github(&github).unwrap();
        assert_eq!(latest.version, "0.3.1");
        assert_eq!(
            latest.download_base,
            "https://github.com/touch-tap/ccn/releases/download/v0.3.1"
        );
        assert_eq!(
            latest.assets.unwrap(),
            vec!["ccn-x86_64-pc-windows-msvc.zip", "SHA256SUMS"]
        );
        assert!(latest_from_server(&json!({})).is_err());
    }

    #[test]
    fn the_daily_check_runs_once_a_day_and_announces_once_a_week() {
        let now = 1_800_000_000;
        let mut cfg = Config::default();
        assert_eq!(next_step(&cfg, now), (true, None), "never checked: check");

        cfg.update_checked_at = Some(now - CHECK_EVERY + 60);
        assert_eq!(next_step(&cfg, now), (false, None), "checked today");

        cfg.update_checked_at = Some(now - CHECK_EVERY - 1);
        assert_eq!(next_step(&cfg, now), (true, None), "a day has passed");

        cfg.update_checked_at = Some(now);
        cfg.update_available = Some("99.0.0".into());
        assert_eq!(
            next_step(&cfg, now),
            (false, Some("99.0.0".into())),
            "a version brew has to install is announced"
        );

        cfg.update_announced_at = Some(now - ANNOUNCE_EVERY + 60);
        assert_eq!(next_step(&cfg, now), (false, None), "announced this week");

        cfg.update_announced_at = Some(now - ANNOUNCE_EVERY - 1);
        assert_eq!(next_step(&cfg, now), (false, Some("99.0.0".into())));

        cfg.update_available = Some(VERSION.into());
        assert_eq!(
            next_step(&cfg, now),
            (false, None),
            "already on that version (installed by hand)"
        );
        cfg.update_available = Some("0.0.1".into());
        assert_eq!(
            next_step(&cfg, now),
            (false, None),
            "older is never announced"
        );
    }

    #[test]
    #[cfg(unix)]
    fn a_tar_gz_yields_the_ccn_inside_it() {
        use std::io::Write;
        let mut tar_bytes = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut tar_bytes);
            let mut header = tar::Header::new_gnu();
            header.set_size(5);
            header.set_mode(0o755);
            header.set_cksum();
            builder
                .append_data(&mut header, "ccn", &b"hello"[..])
                .unwrap();
            builder.finish().unwrap();
        }
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        gz.write_all(&tar_bytes).unwrap();
        let archive = gz.finish().unwrap();
        assert_eq!(extract_binary(&archive).unwrap(), b"hello");
        assert!(extract_binary(b"not an archive").is_err());
    }
}
