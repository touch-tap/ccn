//! Is anyone at the keyboard? Decides whether a blocking hook waits for the phone.
//! `None` means the platform cannot tell, which counts as away.

pub fn idle_seconds() -> Option<f64> {
    // Test hook: CCN_PRESENCE=away|present pins the answer; file:<path> reads it from a file each time
    if let Ok(value) = std::env::var("CCN_PRESENCE") {
        let value = match value.strip_prefix("file:") {
            Some(path) => std::fs::read_to_string(path).unwrap_or_default().trim().to_string(),
            None => value,
        };
        match value.as_str() {
            "away" => return Some(1e9),
            "present" => return Some(0.0),
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

pub fn detection_available() -> bool {
    idle_seconds().is_some()
}

#[cfg(target_os = "macos")]
fn macos() -> Option<f64> {
    let out = std::process::Command::new("ioreg")
        .args(["-c", "IOHIDSystem", "-d", "4", "-r", "-k", "HIDIdleTime"])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
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

#[cfg(target_os = "linux")]
fn linux() -> Option<f64> {
    let has_display = std::env::var("DISPLAY").map(|v| !v.is_empty()).unwrap_or(false)
        || std::env::var("WAYLAND_DISPLAY").map(|v| !v.is_empty()).unwrap_or(false);
    if !has_display {
        return None;
    }
    if let Ok(out) = std::process::Command::new("xprintidle").output() {
        if out.status.success() {
            if let Ok(ms) = String::from_utf8_lossy(&out.stdout).trim().parse::<u64>() {
                return Some(ms as f64 / 1000.0);
            }
        }
    }
    let out = std::process::Command::new("gdbus")
        .args([
            "call", "--session",
            "--dest", "org.gnome.Mutter.IdleMonitor",
            "--object-path", "/org/gnome/Mutter/IdleMonitor/Core",
            "--method", "org.gnome.Mutter.IdleMonitor.GetIdletime",
        ])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let digits: String = text.chars().filter(|c| c.is_ascii_digit()).collect();
    digits.parse::<u64>().ok().map(|ms| ms as f64 / 1000.0)
}

#[cfg(windows)]
fn windows() -> Option<f64> {
    use windows_sys::Win32::System::SystemInformation::GetTickCount;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
    let mut info = LASTINPUTINFO { cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32, dwTime: 0 };
    // SAFETY: the struct is correctly sized and the pointer is valid for the call.
    let ok = unsafe { GetLastInputInfo(&mut info) };
    if ok == 0 {
        return None;
    }
    // SAFETY: no arguments, no invariants.
    let now = unsafe { GetTickCount() };
    Some(now.wrapping_sub(info.dwTime) as f64 / 1000.0)
}
