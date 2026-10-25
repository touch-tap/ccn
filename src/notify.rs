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
#[cfg_attr(not(windows), allow(dead_code))]
pub const APP_NAME: &str = "Claude Code Notifier";

pub fn show(title: &str, body: &str, sound: bool) {
    let body = crate::session::SessionState::truncate(body, 200);
    #[cfg(target_os = "macos")]
    {
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
        let _ = Command::new("notify-send")
            .arg("--app-name=Claude Code Notifier")
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

/// Registers ccn's AppUserModelID for toasts (Windows only). Returns true when it did.
pub fn register_app() -> bool {
    #[cfg(windows)]
    {
        win::register().is_ok()
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// Removes what `register_app` added. Safe to call when nothing was registered.
pub fn unregister_app() {
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

    pub fn register() -> Result<(), u32> {
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
        let name = wide("DisplayName");
        let value = wide(super::APP_NAME);
        // SAFETY: the key is open; the data is a NUL-terminated UTF-16 string of the given byte length.
        let status = unsafe {
            RegSetValueExW(key, name.as_ptr(), 0, REG_SZ, value.as_ptr() as *const u8, (value.len() * 2) as u32)
        };
        unsafe { RegCloseKey(key) };
        if status == ERROR_SUCCESS { Ok(()) } else { Err(status) }
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
