//! Is anyone at the keyboard? Decides whether a blocking hook waits for the phone.
//! `None` means the platform cannot tell. A hook then does not hold (see `hold_allowed`): holding
//! blind would stall someone who is sitting right at an ssh terminal for the whole ceiling.

use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Each idle probe that runs another program gets this long. A hook polls presence every second
/// while it holds, so a probe that hangs (a D-Bus call to a session bus that never answers) must
/// not hang the hook with it.
const PROBE_TIMEOUT: Duration = Duration::from_millis(500);

pub fn idle_seconds() -> Option<f64> {
    // Test hook: CCN_PRESENCE=away|present|unknown pins the answer (unknown: as where idle time
    // can't be read); file:<path> reads it from a file each time
    if let Ok(value) = std::env::var("CCN_PRESENCE") {
        let value = match value.strip_prefix("file:") {
            Some(path) => std::fs::read_to_string(path)
                .unwrap_or_default()
                .trim()
                .to_string(),
            None => value,
        };
        match value.as_str() {
            "away" => return Some(1e9),
            "present" => return Some(0.0),
            "unknown" => return None,
            _ => {}
        }
    }
    #[cfg(target_os = "macos")]
    {
        macos()
    }
    #[cfg(target_os = "linux")]
    {
        linux()
    }
    #[cfg(windows)]
    {
        windows()
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
    {
        None
    }
}

/// True when the user touched the keyboard or mouse within `threshold` seconds.
pub fn user_present(threshold: f64) -> bool {
    idle_seconds().map(|idle| idle < threshold).unwrap_or(false)
}

/// Whether a blocking hook may hold Claude for the phone, given what presence detection said.
/// Unknown presence holds only when the user asked for it (`ccn config set hold_without_presence
/// on`, or `CCN_HOLD_WITHOUT_PRESENCE=1`): typing can't release a hold that can't see typing.
pub fn hold_allowed(idle: Option<f64>, threshold: f64, hold_without_presence: bool) -> bool {
    match idle {
        Some(idle) => idle >= threshold,
        None => hold_without_presence,
    }
}

/// Runs a probe command with a deadline, returning its stdout when it exits successfully in time.
/// Kills it otherwise. A missing program is just None.
#[cfg_attr(not(any(target_os = "macos", target_os = "linux")), allow(dead_code))]
fn run_bounded(program: &str, args: &[&str], timeout: Duration) -> Option<String> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    // Read on a thread, so a probe with more output than a pipe holds can't block on the write
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut out = String::new();
        stdout.read_to_string(&mut out).ok().map(|_| out)
    });
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let out = reader.join().ok().flatten()?;
                return status.success().then_some(out);
            }
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

/// True when the user is at the terminal Claude runs in: active within `threshold` seconds and,
/// on macOS, with that terminal the frontmost app. Someone typing in a browser is not at the
/// terminal. Where the terminal can't be identified (Linux, Windows, ssh), activity alone decides.
pub fn at_terminal(threshold: f64) -> bool {
    if let Ok(value) = std::env::var("CCN_PRESENCE") {
        if value == "present-elsewhere" {
            return false;
        }
    }
    user_present(threshold) && terminal_frontmost().unwrap_or(true)
}

/// Whether the app hosting this session is in front. None when that can't be told.
fn terminal_frontmost() -> Option<bool> {
    #[cfg(target_os = "macos")]
    {
        // Apps launched from the Dock pass their bundle id to everything they start
        let host = std::env::var("__CFBundleIdentifier")
            .ok()
            .filter(|s| !s.is_empty())?;
        let asn = run_bounded("lsappinfo", &["front"], PROBE_TIMEOUT)?;
        let asn = asn.trim();
        if asn.is_empty() {
            return None;
        }
        let info = run_bounded(
            "lsappinfo",
            &["info", "-only", "bundleid", asn],
            PROBE_TIMEOUT,
        )?;
        let front = front_bundle_id(&info)?;
        Some(front.eq_ignore_ascii_case(&host))
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

/// `bundleID="com.apple.Terminal"` or `"CFBundleIdentifier"="com.apple.Terminal"`, as lsappinfo prints it
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn front_bundle_id(text: &str) -> Option<String> {
    for marker in ["bundleID=\"", "\"CFBundleIdentifier\"=\""] {
        if let Some(start) = text.find(marker) {
            let rest = &text[start + marker.len()..];
            return rest
                .split('"')
                .next()
                .map(|s| s.to_string())
                .filter(|s| !s.is_empty());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    #[test]
    fn reads_lsappinfo() {
        let out =
            "[ NULL ]  ASN:0x0-0x1fb1fb: (in front) \n    bundleID=\"dev.warp.Warp-Stable\"\n";
        assert_eq!(
            super::front_bundle_id(out).as_deref(),
            Some("dev.warp.Warp-Stable")
        );
        assert_eq!(
            super::front_bundle_id("\"CFBundleIdentifier\"=\"com.apple.Terminal\"").as_deref(),
            Some("com.apple.Terminal")
        );
        assert_eq!(super::front_bundle_id("nothing"), None);
    }

    #[test]
    fn unknown_presence_holds_only_when_asked() {
        use super::hold_allowed;
        // Known: away holds, present does not, whatever the opt-in says
        assert!(hold_allowed(Some(120.0), 30.0, false));
        assert!(!hold_allowed(Some(3.0), 30.0, true));
        // Unknown: never by default
        assert!(!hold_allowed(None, 30.0, false));
        assert!(hold_allowed(None, 30.0, true));
    }

    #[test]
    #[cfg(unix)]
    fn a_probe_that_hangs_is_cut_short() {
        let start = std::time::Instant::now();
        assert_eq!(
            super::run_bounded("sleep", &["5"], std::time::Duration::from_millis(200)),
            None
        );
        assert!(start.elapsed() < std::time::Duration::from_secs(2));
        assert_eq!(
            super::run_bounded("echo", &["42"], super::PROBE_TIMEOUT).as_deref(),
            Some("42\n")
        );
        assert_eq!(
            super::run_bounded("ccn-no-such-program", &[], super::PROBE_TIMEOUT),
            None
        );
    }

    #[test]
    fn reads_stat_and_dbus_replies() {
        // A command name may hold spaces and parentheses; the parent pid follows the last ')'
        assert_eq!(
            super::ppid_from_stat("1234 (node (x) y) S 987 1234 1234 34816"),
            Some(987)
        );
        assert_eq!(super::ppid_from_stat("garbage"), None);
        assert_eq!(super::millis_from_reply("(uint64 4521,)\n"), Some(4521));
        assert_eq!(super::millis_from_reply("4521\n"), Some(4521));
        assert_eq!(super::millis_from_reply("Error: no such method"), None);
    }
}

#[cfg(target_os = "macos")]
fn macos() -> Option<f64> {
    let text = run_bounded(
        "ioreg",
        &["-c", "IOHIDSystem", "-d", "4", "-r", "-k", "HIDIdleTime"],
        PROBE_TIMEOUT,
    )?;
    for line in text.lines() {
        if let Some(idx) = line.find("HIDIdleTime") {
            let digits: String = line[idx..].chars().filter(|c| c.is_ascii_digit()).collect();
            if let Ok(ns) = digits.parse::<u64>() {
                return Some(ns as f64 / 1e9);
            }
        }
    }
    None
}

/// Linux: the desktop's idle time where there is one (X11 via xprintidle, GNOME's Mutter, KDE and
/// others via the freedesktop ScreenSaver interface), and otherwise the terminal's: when Claude
/// runs over ssh, in WSL or on a console, the last keypress is the last read from its terminal.
#[cfg(target_os = "linux")]
fn linux() -> Option<f64> {
    let has_display = ["DISPLAY", "WAYLAND_DISPLAY"]
        .iter()
        .any(|name| std::env::var(name).is_ok_and(|v| !v.is_empty()));
    if has_display {
        let probes: &[(&str, &[&str])] = &[
            ("xprintidle", &[]),
            (
                "gdbus",
                &[
                    "call",
                    "--session",
                    "--dest",
                    "org.gnome.Mutter.IdleMonitor",
                    "--object-path",
                    "/org/gnome/Mutter/IdleMonitor/Core",
                    "--method",
                    "org.gnome.Mutter.IdleMonitor.GetIdletime",
                ],
            ),
            (
                "gdbus",
                &[
                    "call",
                    "--session",
                    "--dest",
                    "org.freedesktop.ScreenSaver",
                    "--object-path",
                    "/org/freedesktop/ScreenSaver",
                    "--method",
                    "org.freedesktop.ScreenSaver.GetSessionIdleTime",
                ],
            ),
            (
                "qdbus",
                &[
                    "org.freedesktop.ScreenSaver",
                    "/ScreenSaver",
                    "GetSessionIdleTime",
                ],
            ),
        ];
        for (program, args) in probes {
            if let Some(ms) = run_bounded(program, args, PROBE_TIMEOUT)
                .as_deref()
                .and_then(millis_from_reply)
            {
                return Some(ms as f64 / 1000.0);
            }
        }
    }
    terminal_idle()
}

/// Milliseconds from `xprintidle` (`4521`) or a gdbus/qdbus reply (`(uint64 4521,)`, `(uint32
/// 4521,)`, `4521`). Errors carry no digits after the type, so they come out as None.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn millis_from_reply(text: &str) -> Option<u64> {
    let text = text.trim();
    let text = text
        .strip_prefix('(')
        .and_then(|t| t.strip_suffix(",)"))
        .map(|t| t.rsplit(' ').next().unwrap_or(t))
        .unwrap_or(text);
    text.parse().ok()
}

/// The parent pid from `/proc/<pid>/stat`. The command name is in parentheses and may contain
/// spaces or parentheses itself, so the fields are counted from the last `)`.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn ppid_from_stat(stat: &str) -> Option<u32> {
    let rest = &stat[stat.rfind(')')? + 1..];
    rest.split_whitespace().nth(1)?.parse().ok()
}

/// Seconds since the terminal Claude runs in was last read from, which is the last keypress (as
/// `w` reports it). The hook's own stdin is a pipe, so this walks up to the first ancestor whose
/// stdin is a terminal: Claude Code itself, or the shell around it. The kernel updates a
/// terminal's read time in 8-second steps, well inside the presence threshold.
#[cfg(target_os = "linux")]
fn terminal_idle() -> Option<f64> {
    let mut pid = std::process::id();
    for _ in 0..6 {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        pid = ppid_from_stat(&stat)?;
        if pid <= 1 {
            return None;
        }
        let Ok(target) = std::fs::read_link(format!("/proc/{pid}/fd/0")) else {
            continue;
        };
        let name = target.to_string_lossy();
        if name.starts_with("/dev/pts/") || name.starts_with("/dev/tty") {
            let read_at = std::fs::metadata(&target).ok()?.accessed().ok()?;
            let idle = std::time::SystemTime::now()
                .duration_since(read_at)
                .unwrap_or_default();
            return Some(idle.as_secs_f64());
        }
    }
    None
}

#[cfg(windows)]
fn windows() -> Option<f64> {
    use windows_sys::Win32::System::SystemInformation::GetTickCount;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
    let mut info = LASTINPUTINFO {
        cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32,
        dwTime: 0,
    };
    // SAFETY: the struct is correctly sized and the pointer is valid for the call.
    let ok = unsafe { GetLastInputInfo(&mut info) };
    if ok == 0 {
        return None;
    }
    // SAFETY: no arguments, no invariants.
    let now = unsafe { GetTickCount() };
    Some(now.wrapping_sub(info.dwTime) as f64 / 1000.0)
}
