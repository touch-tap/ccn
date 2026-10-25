//! A desktop notification, fired and forgotten. The phone is the product; this is a courtesy.

use std::process::{Command, Stdio};

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
        // Toast notifications arrive with the Windows installer (phase 3)
        let _ = (title, body, sound);
    }
}

#[cfg(target_os = "macos")]
fn osa_string(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}
