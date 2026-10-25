//! A desktop notification, fired and forgotten. The phone is the product; this is a courtesy.
//! Nothing here may block the hook for long, fail it, or write to stdout.

#[cfg(not(windows))]
use std::process::{Command, Stdio};

/// The AppUserModelID `ccn setup` registers on Windows, so toasts say "Claude Code Notifier".
#[cfg_attr(not(windows), allow(dead_code))]
pub const APP_ID: &str = "TouchTap.ccn";
/// Windows PowerShell's AUMID: always registered, so toasts still show before `ccn setup` has run.
#[cfg_attr(not(windows), allow(dead_code))]
pub const FALLBACK_APP_ID: &str = r"{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\WindowsPowerShell\v1.0\powershell.exe";
pub const APP_NAME: &str = "Claude Code Notifier";

/// The app icon, written to disk by `ccn setup` for the notification servers to read. macOS takes
/// the icon of the app that posts (the bundled notifier app), so the binary there goes without it.
#[cfg(not(target_os = "macos"))]
const ICON: &[u8] = include_bytes!("../assets/icon.png");

/// Where `ccn setup` puts the icon: the hicolor theme on Linux, ccn's own folder on Windows.
#[cfg(not(target_os = "macos"))]
fn icon_path() -> std::path::PathBuf {
    #[cfg(windows)]
    {
        crate::config::ccn_dir().join("icon.png")
    }
    #[cfg(not(windows))]
    {
        let data = std::env::var("XDG_DATA_HOME")
            .ok()
            .filter(|d| !d.is_empty())
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| crate::config::home_dir().join(".local").join("share"));
        data.join("icons").join("hicolor").join("256x256").join("apps").join("ccn.png")
    }
}

#[cfg(not(target_os = "macos"))]
fn write_icon() -> Option<std::path::PathBuf> {
    let path = icon_path();
    std::fs::create_dir_all(path.parent()?).ok()?;
    std::fs::write(&path, ICON).ok()?;
    Some(path)
}

/// `CCN_NOTIFY=off` silences desktop notifications (for tests).
fn disabled() -> bool {
    std::env::var("CCN_NOTIFY").is_ok_and(|v| v.eq_ignore_ascii_case("off") || v == "0")
}

pub fn show(title: &str, body: &str, sound: bool) {
    if disabled() {
        return;
    }
    let body = crate::session::SessionState::truncate(body, 200);
    #[cfg(target_os = "macos")]
    if !mac::post(title, &body, sound) {
        let script = format!(
            "display notification {} with title {}{}",
            osa_string(&body),
            osa_string(title),
            if sound { " sound name \"default\"" } else { "" }
        );
        let _ = Command::new("osascript")
            .arg("-e")
            .arg(script)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
    }
    #[cfg(target_os = "linux")]
    {
        let _ = sound;
        let mut cmd = Command::new("notify-send");
        cmd.arg("--app-name=Claude Code Notifier");
        let icon = icon_path();
        if icon.exists() {
            cmd.arg(format!("--icon={}", icon.display()));
        }
        let _ = cmd
            .arg(title)
            .arg(&body)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
    }
    #[cfg(windows)]
    {
        if let Err(err) = win::show(&toast_xml(title, &body, sound)) {
            crate::logger::log(&format!("toast failed: {err}"));
        }
    }
}

/// What `ccn setup` adds for desktop notifications: the icon, and on Windows the app identity
/// toasts are shown under. Returns a line to print when it did something.
pub fn install() -> Option<String> {
    #[cfg(windows)]
    {
        let icon = write_icon();
        win::register(icon.as_deref()).ok()?;
        Some("Registered \"Claude Code Notifier\" for Windows notifications.".to_string())
    }
    #[cfg(target_os = "linux")]
    {
        write_icon().map(|p| format!("Installed the notification icon at {}.", p.display()))
    }
    #[cfg(target_os = "macos")]
    {
        mac::install()
    }
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    {
        None
    }
}

/// A line for `ccn doctor` about desktop notifications: (all good, text).
pub fn doctor_line() -> Option<(bool, String)> {
    #[cfg(target_os = "macos")]
    {
        Some(match mac::status().as_deref() {
            None => (true, "AppleScript (this build has no notifier app)".to_string()),
            Some("authorized") => (true, format!("allowed for \"{APP_NAME}\"")),
            Some("denied") => (false, format!("turned off: System Settings > Notifications > {APP_NAME}")),
            Some(_) => (false, "not allowed yet: run `ccn setup`".to_string()),
        })
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

/// Removes what `install` added. Safe to call when nothing was installed.
pub fn uninstall() {
    #[cfg(not(target_os = "macos"))]
    let _ = std::fs::remove_file(icon_path());
    #[cfg(windows)]
    win::unregister();
}

/// The toast's XML. Not platform-gated so the escaping is tested everywhere.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn toast_xml(title: &str, body: &str, sound: bool) -> String {
    let audio = if sound {
        r#"<audio src="ms-winsoundevent:Notification.Default"/>"#
    } else {
        r#"<audio silent="true"/>"#
    };
    format!(
        r#"<toast><visual><binding template="ToastGeneric"><text>{}</text><text>{}</text></binding></visual>{audio}</toast>"#,
        xml_escape(title),
        xml_escape(body)
    )
}

/// Escapes the five XML specials and drops characters XML 1.0 cannot carry at all.
#[cfg_attr(not(windows), allow(dead_code))]
fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            '\t' | '\n' | '\r' => out.push(ch),
            c if (c as u32) < 0x20 || c == '\u{FFFE}' || c == '\u{FFFF}' => {}
            c => out.push(c),
        }
    }
    out
}

#[cfg(target_os = "macos")]
fn osa_string(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

#[cfg(target_os = "macos")]
mod mac {
    //! "Claude Code Notifier.app" (macos/notifier), embedded at build time and written to ccn's
    //! folder, so notifications carry our name and icon instead of Script Editor's. It is rewritten
    //! whenever its Info.plist differs from the embedded one, which carries the version.

    use std::path::PathBuf;
    use std::process::{Command, Stdio};

    include!(concat!(env!("OUT_DIR"), "/notifier_files.rs"));

    fn app_path() -> PathBuf {
        crate::config::ccn_dir().join(format!("{}.app", super::APP_NAME))
    }

    fn exe() -> PathBuf {
        app_path().join("Contents/MacOS/ccn-notifier")
    }

    /// Makes sure the current app is on disk. False when this build has none, or writing failed.
    fn ensure() -> bool {
        let Some((_, plist, _)) = FILES.iter().find(|(rel, _, _)| *rel == "Contents/Info.plist") else {
            return false;
        };
        if std::fs::read(app_path().join("Contents/Info.plist")).is_ok_and(|on_disk| on_disk == *plist) {
            return true;
        }
        match write() {
            Ok(()) => true,
            Err(err) => {
                crate::logger::log(&format!("writing the notifier app failed: {err}"));
                false
            }
        }
    }

    /// Writes beside the target and renames, so a hook running at the same time never sees half an app.
    fn write() -> std::io::Result<()> {
        use std::os::unix::fs::PermissionsExt;
        let app = app_path();
        let tmp = app.with_extension(format!("tmp-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        for (rel, bytes, executable) in FILES {
            let path = tmp.join(rel);
            std::fs::create_dir_all(path.parent().unwrap())?;
            std::fs::write(&path, bytes)?;
            if *executable {
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
            }
        }
        let _ = std::fs::remove_dir_all(&app);
        std::fs::rename(&tmp, &app)
    }

    /// Fire and forget. The app falls back to AppleScript itself while permission was never asked.
    pub fn post(title: &str, body: &str, sound: bool) -> bool {
        if !ensure() {
            return false;
        }
        let mut cmd = Command::new(exe());
        cmd.args(["--title", title, "--body", body]);
        if sound {
            cmd.arg("--sound");
        }
        cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().is_ok()
    }

    /// authorized, denied or not-determined; None without the app.
    pub fn status() -> Option<String> {
        if !ensure() {
            return None;
        }
        let out = Command::new(exe()).arg("--status").stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
        Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    /// Asks for permission while the user is at the terminal. macOS only shows the prompt to an app
    /// launched through Launch Services, hence `open`.
    pub fn install() -> Option<String> {
        let name = super::APP_NAME;
        match status()?.as_str() {
            "authorized" => return Some(format!("Desktop notifications are allowed for \"{name}\".")),
            "denied" => return Some(format!(
                "Desktop notifications are turned off for \"{name}\". Turn them on in System Settings > Notifications."
            )),
            _ => {}
        }
        println!("Asking macOS to allow notifications from \"{name}\"...");
        let _ = Command::new("open")
            .args(["-W", "-g"])
            .arg(app_path())
            .args(["--args", "--request-permission"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        Some(match status().as_deref() {
            Some("authorized") => format!("Desktop notifications are allowed for \"{name}\"."),
            _ => format!("Desktop notifications aren't allowed. Turn them on in System Settings > Notifications > {name}."),
        })
    }
}

#[cfg(windows)]
mod win {
    //! WinRT toasts in-process: no PowerShell child (slow to start, often blocked or flagged by
    //! endpoint security). An unpackaged app needs a registered AppUserModelID; `ccn setup`
    //! registers ours under HKCU\Software\Classes\AppUserModelId, which Windows 10 1809 and later
    //! honour without a Start-menu shortcut. Until then the toast borrows PowerShell's.

    use windows::core::HSTRING;
    use windows::Data::Xml::Dom::XmlDocument;
    use windows::UI::Notifications::{ToastNotification, ToastNotificationManager};
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegCreateKeyExW, RegDeleteTreeW, RegOpenKeyExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER,
        KEY_READ, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ,
    };

    fn key_path() -> String {
        format!(r"Software\Classes\AppUserModelId\{}", super::APP_ID)
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn registered() -> bool {
        let path = wide(&key_path());
        let mut key: HKEY = std::ptr::null_mut();
        // SAFETY: a valid NUL-terminated path and out pointer; the key is closed below.
        let status = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, path.as_ptr(), 0, KEY_READ, &mut key) };
        if status == ERROR_SUCCESS {
            unsafe { RegCloseKey(key) };
            true
        } else {
            false
        }
    }

    pub fn show(xml: &str) -> windows::core::Result<()> {
        let doc = XmlDocument::new()?;
        doc.LoadXml(&HSTRING::from(xml))?;
        let toast = ToastNotification::CreateToastNotification(&doc)?;
        let app_id = if registered() { super::APP_ID } else { super::FALLBACK_APP_ID };
        ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(app_id))?.Show(&toast)
    }

    pub fn register(icon: Option<&std::path::Path>) -> Result<(), u32> {
        let path = wide(&key_path());
        let mut key: HKEY = std::ptr::null_mut();
        // SAFETY: a valid NUL-terminated path, null class and security attributes, a valid out pointer.
        let status = unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                path.as_ptr(),
                0,
                std::ptr::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_WRITE,
                std::ptr::null(),
                &mut key,
                std::ptr::null_mut(),
            )
        };
        if status != ERROR_SUCCESS {
            return Err(status);
        }
        let mut status = set_string(key, "DisplayName", super::APP_NAME);
        if let (ERROR_SUCCESS, Some(icon)) = (status, icon) {
            status = set_string(key, "IconUri", &icon.to_string_lossy());
        }
        unsafe { RegCloseKey(key) };
        if status == ERROR_SUCCESS { Ok(()) } else { Err(status) }
    }

    fn set_string(key: HKEY, name: &str, value: &str) -> u32 {
        let name = wide(name);
        let value = wide(value);
        // SAFETY: the key is open; the data is a NUL-terminated UTF-16 string of the given byte length.
        unsafe { RegSetValueExW(key, name.as_ptr(), 0, REG_SZ, value.as_ptr() as *const u8, (value.len() * 2) as u32) }
    }

    pub fn unregister() {
        let path = wide(&key_path());
        // SAFETY: a valid NUL-terminated path.
        unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, path.as_ptr()) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toast_xml_escapes_title_and_body() {
        let xml = toast_xml("Fix <b> & \"quotes\"", "it's 1 < 2 > 0\u{7}", true);
        assert_eq!(
            xml,
            "<toast><visual><binding template=\"ToastGeneric\"><text>Fix &lt;b&gt; &amp; &quot;quotes&quot;</text>\
             <text>it&apos;s 1 &lt; 2 &gt; 0</text></binding></visual>\
             <audio src=\"ms-winsoundevent:Notification.Default\"/></toast>"
        );
    }

    #[test]
    fn toast_xml_is_silent_without_sound() {
        let xml = toast_xml("t", "line one\nline two", false);
        assert!(xml.contains("<audio silent=\"true\"/>"));
        assert!(!xml.contains("ms-winsoundevent"));
        assert!(xml.contains("line one\nline two"));
    }
}
