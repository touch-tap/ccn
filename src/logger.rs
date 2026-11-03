//! A small append-only log under the ccn directory. Hooks have no terminal, so this is the only
//! place their failures can go. Kept under 1 MB.

use std::fs::{self, OpenOptions};
use std::io::Write;

use crate::config;

const MAX_BYTES: u64 = 1024 * 1024;

pub fn log(message: &str) {
    // Unit tests run without CCN_HOME: they must not write to the real ccn directory
    if cfg!(test) && std::env::var_os("CCN_HOME").is_none() {
        return;
    }
    let path = config::log_path();
    if let Some(parent) = path.parent() {
        let _ = config::ensure_dir(parent);
    }
    if let Ok(meta) = fs::metadata(&path) {
        if meta.len() > MAX_BYTES {
            let _ = fs::remove_file(&path);
        }
    }
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    // The log names projects and errors: this user's only, like the rest of the ccn directory
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    if let Ok(mut file) = options.open(&path) {
        let stamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
        let _ = writeln!(file, "{stamp} [{}] {message}", std::process::id());
    }
}
