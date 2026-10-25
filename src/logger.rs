//! A small append-only log under the ccn directory. Hooks have no terminal, so this is the only
//! place their failures can go. Kept under 1 MB.

use std::fs::{self, OpenOptions};
use std::io::Write;

use crate::config;

const MAX_BYTES: u64 = 1024 * 1024;

pub fn log(message: &str) {
    let path = config::log_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(meta) = fs::metadata(&path) {
        if meta.len() > MAX_BYTES {
            let _ = fs::remove_file(&path);
        }
    }
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&path) {
        let stamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
        let _ = writeln!(file, "{stamp} [{}] {message}", std::process::id());
    }
}
